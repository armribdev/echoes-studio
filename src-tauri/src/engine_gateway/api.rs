//! Gateway Tauri commands — the authoritative mutation entry point.
//!
//! All project-structural changes funnel through `send_command` or
//! `send_batch`. Each command:
//!   1. Validates its payload
//!   2. Mutates `AppProject` directly (fast path during normal use)
//!   3. Appends a `ProjectEvent` to the event store
//!   4. Triggers audio graph recompilation if structural
//!   5. Emits an `EngineEvent` to the frontend via Tauri's event system
//!
//! Undo/redo work by moving the event store head and rebuilding the project
//! from the nearest snapshot + the events after it.

use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::{Arc, MutexGuard};
use tauri::{Emitter, Manager};

use super::commands::{
    cmd, AddAutomationLanePayload, AddClipPayload, AddEffectPayload, AddMidiClipPayload,
    AddModulationRoutePayload, AddModulatorPayload, BatchCommand, CommandEnvelope,
    CreateTrackPayload, DeleteTrackPayload, MoveClipPayload, MoveMidiClipPayload, MuteTrackPayload,
    RemoveAutomationLanePayload, RemoveClipPayload, RemoveEffectPayload, RemoveMidiClipPayload,
    RemoveModulationRoutePayload, RemoveModulatorPayload, SeekPayload, SetClipMidiNotesPayload,
    SetLoopPayload, SetMidiNotesPayload, SetModulationDepthPayload, SetTrackInputPayload,
    SetTrackInstrumentPayload, SoloTrackPayload, UpdateAutomationClipPayload, UpdateEffectPayload,
    UpdateModulatorPayload, UpdateProjectNamePayload, UpdateTempoPayload, UpdateTrackPayload,
};
use super::events::{ev, EngineEvent, ProjectEvent};
use crate::audio_clip::{AudioClip, AudioClipData};
use crate::audio_engine::transport::SharedTransport;
use crate::audio_graph::nodes::track_gain::VOLUME_RANGE;
use crate::audio_graph::GraphSnapshotDto;
use crate::plugin_system::wrapper::DeviceFormat;
use crate::project::{
    generate_id, AppClip, AppMidiClip, AppProject, AppState, AppTrack, CachedAudioData,
    CompressorParams, DelayParams, EffectParams, EqParams, MidiClipDto, MidiNote, PluginParams,
    ProjectUiLayout, ReverbParams, TrackEffect,
};

/// Returned by `send_command` / `send_batch` to the UI.
///
/// `rename_all = "camelCase"` keeps the JSON keys in sync with the TypeScript
/// `CommandResult` interface (which uses camelCase).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub success: bool,
    pub command_id: String,
    /// Engine events produced by this command (serialised for the UI).
    pub events: Vec<serde_json::Value>,
    pub error: Option<String>,
}

impl CommandResult {
    fn ok(command_id: String, events: Vec<serde_json::Value>) -> Self {
        Self {
            success: true,
            command_id,
            events,
            error: None,
        }
    }
    fn err(command_id: String, msg: String) -> Self {
        Self {
            success: false,
            command_id,
            events: vec![],
            error: Some(msg),
        }
    }
}

/// UI pan is −100…+100. The audio thread stores −1…+1.
fn normalize_pan(pan: f32) -> f32 {
    (pan / 100.0).clamp(-1.0, 1.0)
}

/// Hot mix update. Writes the live atomics only — no project event, no
/// recompile — so a fader or pan drag cannot rebuild the graph.
#[tauri::command]
pub fn set_track_mix(
    state: tauri::State<'_, AppState>,
    track_id: String,
    volume: Option<f32>,
    pan: Option<f32>,
) -> Result<(), String> {
    if let Some(vol) = volume {
        let linear = (vol / 100.0).clamp(VOLUME_RANGE.0, VOLUME_RANGE.1);
        state.live_params.set_track_volume(&track_id, linear);
    }
    if let Some(pan) = pan {
        state
            .live_params
            .set_track_pan(&track_id, normalize_pan(pan));
    }
    Ok(())
}

/// Piano-roll audition. Live MIDI only — no project event, no recompile.
#[tauri::command]
pub fn preview_midi_note(
    state: tauri::State<'_, AppState>,
    track_id: String,
    pitch: u8,
    velocity: u8,
    note_on: bool,
) -> Result<(), String> {
    state.live_params.push_preview(
        &track_id,
        crate::audio_graph::midi::MidiPreviewMessage {
            note_on,
            pitch,
            velocity,
        },
    );
    Ok(())
}

/// Arm a track for hardware MIDI input (ADR-022), or disarm when `track_id`
/// is `None`. Ephemeral session state — no project event, no recompile.
#[tauri::command]
pub fn arm_track_for_midi_input(
    state: tauri::State<'_, AppState>,
    track_id: Option<String>,
) -> Result<(), String> {
    if let Ok(mut armed) = state.armed_track_id.lock() {
        *armed = track_id;
    }
    Ok(())
}

/// Start recording the armed track's hardware MIDI notes; returns the start
/// position in seconds. The frontend starts the transport itself; notes stream
/// in through `MIDI_RECORDING_NOTE_EVENT` and the clip is delivered by
/// `MIDI_RECORDING_FINISHED_EVENT`. When the transport is stopped,
/// `count_in_bars` bars of click play before it starts moving.
#[tauri::command]
pub fn start_midi_recording(
    state: tauri::State<'_, AppState>,
    count_in_bars: u32,
) -> Result<f64, String> {
    let armed = state
        .armed_track_id
        .lock()
        .ok()
        .and_then(|armed| armed.clone());
    let track_id = armed.ok_or("Aucune piste armée pour l'enregistrement")?;
    let start_seconds = state.midi_recorder.start(track_id)?;
    if !state.transport.is_playing.load(Ordering::Relaxed) {
        state
            .transport
            .start_count_in(count_in_samples(&state.transport, count_in_bars));
    }
    Ok(start_seconds)
}

/// Metronome click on the live output (ADR-026). Session state, no project event.
#[tauri::command]
pub fn set_metronome_enabled(state: tauri::State<'_, AppState>, enabled: bool) {
    state.transport.set_metronome_enabled(enabled);
}

fn count_in_samples(transport: &SharedTransport, bars: u32) -> u64 {
    let beats = bars as f64 * transport.time_signature().numerator as f64;
    (beats * 60.0 / transport.bpm() as f64 * transport.sample_rate() as f64) as u64
}

#[tauri::command]
pub fn stop_midi_recording(app: tauri::AppHandle, state: tauri::State<'_, AppState>) {
    crate::midi_recording::finish_and_emit(&app, &state.midi_recorder);
}

/// Send a single typed command to the engine.
#[tauri::command]
pub fn send_command(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    command: CommandEnvelope,
) -> Result<CommandResult, String> {
    let result = process_command(&app, &state, command.clone())
        .unwrap_or_else(|e| CommandResult::err(command.id, e));
    crate::menu_state::refresh_menu(&app, &state);
    Ok(result)
}

/// Send multiple commands as an atomic batch (single graph recompile).
#[tauri::command]
pub fn send_batch(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    batch: BatchCommand,
) -> Result<Vec<CommandResult>, String> {
    let results: Vec<CommandResult> = batch
        .commands
        .into_iter()
        .map(|cmd| {
            let id = cmd.id.clone();
            process_command(&app, &state, cmd).unwrap_or_else(|e| CommandResult::err(id, e))
        })
        .collect();

    recompile_if_needed(&app, &state);
    crate::menu_state::refresh_menu(&app, &state);

    Ok(results)
}

/// Return a full project snapshot (DTO) for initial UI hydration or after
/// undo/redo.
#[tauri::command]
pub fn get_snapshot(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    project_dto_json(&state)
}

/// Return the audio graph topology derived from the current project.
#[tauri::command]
pub fn get_graph_snapshot(state: tauri::State<'_, AppState>) -> Result<GraphSnapshotDto, String> {
    let project = lock_project(&state)?;
    Ok(GraphSnapshotDto::from_project(&project))
}

/// Undo the last project-structural event. Returns the new project snapshot.
#[tauri::command]
pub fn undo(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let moved = {
        let mut store = state.event_store.lock().map_err(|e| e.to_string())?;
        store.undo()
    };
    finish_history_step(&app, &state, moved)
}

/// Redo the next project-structural event. Returns the new project snapshot.
#[tauri::command]
pub fn redo(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let moved = {
        let mut store = state.event_store.lock().map_err(|e| e.to_string())?;
        store.redo()
    };
    finish_history_step(&app, &state, moved)
}

// --- Project lock helpers ---

fn lock_project<'a>(
    state: &'a tauri::State<'a, AppState>,
) -> Result<MutexGuard<'a, AppProject>, String> {
    state.project.lock().map_err(|e| e.to_string())
}

fn with_project_mut<'a, R, F>(state: &'a tauri::State<'a, AppState>, f: F) -> Result<R, String>
where
    F: FnOnce(&mut AppProject) -> Result<R, String>,
{
    let mut project = lock_project(state)?;
    f(&mut project)
}

fn find_track_mut<'a>(project: &'a mut AppProject, track_id: &str) -> Option<&'a mut AppTrack> {
    project.tracks.iter_mut().find(|t| t.id == track_id)
}

fn require_track_mut<'a>(
    project: &'a mut AppProject,
    track_id: &str,
) -> Result<&'a mut AppTrack, String> {
    find_track_mut(project, track_id).ok_or_else(|| format!("Track {} not found", track_id))
}

fn require_audio_clip_mut<'a>(
    track: &'a mut AppTrack,
    clip_id: &str,
) -> Result<&'a mut AppClip, String> {
    track
        .clips
        .iter_mut()
        .find(|c| c.id == clip_id)
        .ok_or_else(|| format!("Clip {clip_id} not found"))
}

fn require_effect_mut<'a>(
    track: &'a mut AppTrack,
    effect_id: &str,
) -> Result<&'a mut TrackEffect, String> {
    track
        .effects
        .iter_mut()
        .find(|e| e.id == effect_id)
        .ok_or_else(|| format!("Effect {effect_id} not found"))
}

fn require_midi_clip_mut<'a>(
    track: &'a mut AppTrack,
    clip_id: &str,
) -> Result<&'a mut AppMidiClip, String> {
    track
        .midi_clips
        .iter_mut()
        .find(|c| c.id == clip_id)
        .ok_or_else(|| format!("MidiClip {clip_id} not found"))
}

fn apply_exclusive_solo(project: &mut AppProject, track_id: &str, solo: bool) {
    if solo {
        for track in project.tracks.iter_mut() {
            track.solo = track.id == track_id;
        }
    } else if let Some(track) = find_track_mut(project, track_id) {
        track.solo = false;
    }
}

fn empty_app_track(id: String, name: String) -> AppTrack {
    AppTrack {
        id,
        name,
        volume: 80.0,
        pan: 0.0,
        muted: false,
        solo: false,
        clips: vec![],
        effects: vec![],
        modulators: vec![],
        mod_routes: vec![],
        midi_notes: vec![],
        midi_clips: vec![],
        instrument: None,
        input_source: None,
        automation_lanes: vec![],
        color: None,
    }
}

fn validate_instrument(state: &tauri::State<'_, AppState>, plugin_id: &str) -> Result<(), String> {
    let engine = state
        .audio_engine
        .lock()
        .map_err(|e| format!("audio engine lock: {e}"))?;
    let manifest = engine
        .plugin_host
        .registry()
        .get(plugin_id)
        .ok_or_else(|| format!("Plugin '{plugin_id}' introuvable"))?;
    if !manifest.is_instrument {
        return Err(format!(
            "'{}' n'est pas un instrument — ajoutez-le dans Effets",
            manifest.name
        ));
    }
    if manifest.format != DeviceFormat::Clap {
        return Err(format!(
            "Les instruments {} ne sont pas encore supportés — utilisez la version CLAP de '{}'",
            format_tag(manifest.format),
            manifest.name
        ));
    }
    Ok(())
}

fn mark_dirty(state: &tauri::State<'_, AppState>) {
    state.dirty.store(true, Ordering::Relaxed);
}

fn project_dto_json(state: &tauri::State<'_, AppState>) -> Result<Value, String> {
    let project = lock_project(state)?;
    serde_json::to_value(project.to_dto()).map_err(|e| e.to_string())
}

/// Shared tail for undo/redo after the event-store head has moved.
fn finish_history_step(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    moved: bool,
) -> Result<Value, String> {
    if !moved {
        return project_dto_json(state);
    }

    rebuild_project_from_store(state)?;
    recompile_if_needed(app, state);

    let snapshot_val = project_dto_json(state)?;
    let engine_event = EngineEvent::new(ev::PROJECT_RESET, "", snapshot_val.clone());
    emit_engine_event(app, &engine_event);
    mark_dirty(state);
    crate::menu_state::refresh_menu(app, state);

    Ok(snapshot_val)
}

// --- Engine event emission ---

struct ProjectCommit<'a> {
    state: &'a tauri::State<'a, AppState>,
    app: &'a tauri::AppHandle,
    project_id: &'a str,
    command_id: String,
}

fn emit_engine_event(app: &tauri::AppHandle, engine_ev: &EngineEvent) {
    let _ = app.emit("engine-event", engine_ev);
}

fn ok_single_event(command_id: String, engine_ev: &EngineEvent) -> CommandResult {
    CommandResult::ok(
        command_id,
        vec![serde_json::to_value(engine_ev).unwrap_or_default()],
    )
}

fn ok_transport_events(command_id: String, events: &[EngineEvent]) -> CommandResult {
    CommandResult::ok(
        command_id,
        events
            .iter()
            .map(|ev| serde_json::to_value(ev).unwrap_or_default())
            .collect(),
    )
}

fn emit_and_ok(
    app: &tauri::AppHandle,
    command_id: String,
    project_id: &str,
    event_type: &str,
    payload: Value,
) -> CommandResult {
    let engine_ev = EngineEvent::new(event_type, project_id, payload);
    emit_engine_event(app, &engine_ev);
    ok_single_event(command_id, &engine_ev)
}

/// Append to the event store, emit, return the engine event (no recompile).
fn snapshot_emit(
    ctx: &ProjectCommit<'_>,
    event_type: &str,
    payload: Value,
) -> Result<EngineEvent, String> {
    append_and_maybe_snapshot(ctx.state, ProjectEvent::new(event_type, payload.clone()))?;
    let engine_ev = EngineEvent::new(event_type, ctx.project_id, payload);
    emit_engine_event(ctx.app, &engine_ev);
    Ok(engine_ev)
}

/// Persist a project event, emit the matching engine event, optionally recompile.
fn commit_project_change(
    ctx: &ProjectCommit<'_>,
    event_type: &str,
    store_payload: Value,
    engine_payload: Value,
    recompile: bool,
) -> Result<CommandResult, String> {
    append_and_maybe_snapshot(ctx.state, ProjectEvent::new(event_type, store_payload))?;
    let engine_ev = EngineEvent::new(event_type, ctx.project_id, engine_payload);
    emit_engine_event(ctx.app, &engine_ev);
    if recompile {
        recompile_if_needed(ctx.app, ctx.state);
    }
    Ok(ok_single_event(ctx.command_id.clone(), &engine_ev))
}

fn commit_same_payload(
    ctx: &ProjectCommit<'_>,
    event_type: &str,
    payload: Value,
    recompile: bool,
) -> Result<CommandResult, String> {
    commit_project_change(ctx, event_type, payload.clone(), payload, recompile)
}

/// Snapshot + recompile before emit (modulator/route structural adds/removes).
fn commit_recompile_before_emit(
    ctx: &ProjectCommit<'_>,
    event_type: &str,
    payload: Value,
) -> Result<CommandResult, String> {
    append_and_maybe_snapshot(ctx.state, ProjectEvent::new(event_type, payload.clone()))?;
    recompile_if_needed(ctx.app, ctx.state);
    let engine_ev = EngineEvent::new(event_type, ctx.project_id, payload);
    emit_engine_event(ctx.app, &engine_ev);
    Ok(ok_single_event(ctx.command_id.clone(), &engine_ev))
}

fn cached_audio_from_clip_data(data: AudioClipData) -> Arc<CachedAudioData> {
    Arc::new(CachedAudioData {
        raw_samples: data.raw_samples,
        sample_rate: data.sample_rate,
        peaks_64: data.peaks_64,
        peaks_512: data.peaks_512,
        peaks_4096: data.peaks_4096,
        complete: true,
    })
}

fn cached_audio_prefix(samples: &[f32], sample_rate: u32) -> Arc<CachedAudioData> {
    Arc::new(CachedAudioData {
        raw_samples: Arc::new(samples.to_vec()),
        sample_rate,
        peaks_64: Vec::new(),
        peaks_512: Vec::new(),
        peaks_4096: Vec::new(),
        complete: false,
    })
}

/// Push an immutable PCM prefix into the path cache and refresh live clips (ADR-024).
fn publish_pcm_prefix(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    file_path: &str,
    samples: &[f32],
    sample_rate: u32,
    decoded_duration_sec: f64,
    progress: f32,
) {
    if samples.is_empty() || sample_rate == 0 {
        return;
    }
    let cached = cached_audio_prefix(samples, sample_rate);
    state
        .audio_cache
        .insert_path(file_path.to_string(), Arc::clone(&cached));
    refresh_clips_for_path(state, file_path, &cached);
    recompile_if_needed(app, state);
    let _ = app.emit(
        "audio://import-pcm-ready",
        serde_json::json!({
            "file_path": file_path,
            "decoded_duration_sec": decoded_duration_sec,
            "sample_rate": sample_rate,
            "progress": progress,
            "complete": false,
            "sample_count": samples.len(),
        }),
    );
}

fn refresh_clips_for_path(
    state: &tauri::State<'_, AppState>,
    file_path: &str,
    cached: &Arc<CachedAudioData>,
) {
    let norm = |p: &str| p.replace('\\', "/").to_lowercase();
    let target = norm(file_path);
    let Ok(mut project) = state.project.lock() else {
        return;
    };
    for track in &mut project.tracks {
        for clip in &mut track.clips {
            if norm(&clip.file_path) != target {
                continue;
            }
            clip.raw_samples = Arc::clone(&cached.raw_samples);
            clip.sample_rate = cached.sample_rate;
            if !cached.peaks_512.is_empty() {
                clip.peaks_512 = cached.peaks_512.clone();
            }
            if !cached.peaks_4096.is_empty() {
                clip.peaks_4096 = cached.peaks_4096.clone();
            }
            if !cached.peaks_64.is_empty() {
                clip.peaks_64 = cached.peaks_64.clone();
            }
            state
                .audio_cache
                .insert(clip.id.clone(), Arc::clone(cached));
        }
    }
}

fn insert_cached_clip(
    state: &tauri::State<'_, AppState>,
    clip_id: &str,
    file_path: &str,
    cached: Arc<CachedAudioData>,
) -> Arc<CachedAudioData> {
    state.audio_cache.insert_for_clip_and_path(
        clip_id.to_string(),
        file_path.to_string(),
        Arc::clone(&cached),
    );
    cached
}

fn emit_import_progress(
    app: &tauri::AppHandle,
    file_path: &str,
    update: crate::audio_clip::DecodeProgressUpdate,
) {
    let mut payload = serde_json::json!({
        "file_path": file_path,
        "progress": update.progress,
        "cached": false,
        "sample_rate": update.sample_rate,
        "decoded_duration_sec": update.decoded_duration_sec,
    });
    if let Some(peaks) = update.peaks_64 {
        payload["peaks_64"] = serde_json::json!(peaks);
    }
    if let Some(peaks) = update.peaks_512 {
        payload["peaks_512"] = serde_json::json!(peaks);
    }
    if let Some(peaks) = update.peaks_4096 {
        payload["peaks_4096"] = serde_json::json!(peaks);
    }
    if let Some(from) = update.peaks_512_from {
        payload["peaks_512_from"] = serde_json::json!(from);
        if let Some(append) = update.peaks_512_append {
            payload["peaks_512_append"] = serde_json::json!(append);
        }
    }
    if let Some(from) = update.peaks_4096_from {
        payload["peaks_4096_from"] = serde_json::json!(from);
        if let Some(append) = update.peaks_4096_append {
            payload["peaks_4096_append"] = serde_json::json!(append);
        }
    }
    let _ = app.emit("audio://import-progress", payload);
}

fn norm_import_path(p: &str) -> String {
    p.replace('\\', "/").to_lowercase()
}

/// Decode (or reuse) audio into the path cache, emitting progressive peaks + PCM prefixes.
fn ensure_path_cached(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    file_path: &str,
) -> Result<Arc<CachedAudioData>, String> {
    let cancel_key = norm_import_path(file_path);
    if let Ok(mut set) = state.import_cancels.lock() {
        set.remove(&cancel_key);
    }

    if let Some(cached) = state.audio_cache.get_by_path(file_path) {
        if cached.complete {
            // Omit peaks_64 — dense lane can blow IPC; arrange uses 512/4096.
            let _ = app.emit(
                "audio://import-progress",
                serde_json::json!({
                    "file_path": file_path,
                    "progress": 1.0,
                    "cached": true,
                    "sample_rate": cached.sample_rate,
                    "peaks_512": cached.peaks_512,
                    "peaks_4096": cached.peaks_4096,
                    "decoded_duration_sec": cached.duration_sec(),
                }),
            );
            let _ = app.emit(
                "audio://import-pcm-ready",
                serde_json::json!({
                    "file_path": file_path,
                    "decoded_duration_sec": cached.duration_sec(),
                    "sample_rate": cached.sample_rate,
                    "progress": 1.0,
                    "complete": true,
                    "sample_count": cached.raw_samples.len(),
                    "cached": true,
                }),
            );
            return Ok(cached);
        }
        // Incomplete: wait for the in-flight decode to finish (ADR-024).
        for _ in 0..15_000 {
            if state
                .import_cancels
                .lock()
                .map(|s| s.contains(&cancel_key))
                .unwrap_or(false)
            {
                return Err("Import cancelled".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            if let Some(c) = state.audio_cache.get_by_path(file_path) {
                if c.complete {
                    return Ok(c);
                }
            } else {
                break;
            }
        }
        // Orphan prefix (cancelled decode left incomplete cache) — never reuse it.
        state.audio_cache.remove_path(file_path);
    }

    let mut emit_progress = |update: crate::audio_clip::DecodeProgressUpdate| {
        emit_import_progress(app, file_path, update);
    };

    let app_for_pcm = app.clone();
    let path_for_pcm = file_path.to_string();
    let mut on_pcm_prefix = move |samples: &[f32], sample_rate: u32, dur: f64, progress: f32| {
        let st = app_for_pcm.state::<AppState>();
        // Peaks stay on the UI progress channel; PCM prefixes are for playback only.
        publish_pcm_prefix(
            &app_for_pcm,
            &st,
            &path_for_pcm,
            samples,
            sample_rate,
            dur,
            progress,
        );
    };

    let app_for_cancel = app.clone();
    let key_for_cancel = cancel_key.clone();
    let is_cancelled = move || {
        app_for_cancel
            .state::<AppState>()
            .import_cancels
            .lock()
            .map(|s| s.contains(&key_for_cancel))
            .unwrap_or(false)
    };

    let audio = AudioClip::from_file_with_progress(
        file_path,
        &mut emit_progress,
        &mut on_pcm_prefix,
        is_cancelled,
    )
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("cancelled") {
            "Import cancelled".to_string()
        } else {
            format!("Failed to load audio '{file_path}': {msg}")
        }
    })?;

    if state
        .import_cancels
        .lock()
        .map(|s| s.contains(&cancel_key))
        .unwrap_or(false)
    {
        let _ = state
            .import_cancels
            .lock()
            .map(|mut s| s.remove(&cancel_key));
        return Err("Import cancelled".into());
    }

    let data = audio.into_clip_data_with_progress(&mut emit_progress);
    let cached = cached_audio_from_clip_data(data);
    state
        .audio_cache
        .insert_path(file_path.to_string(), Arc::clone(&cached));
    refresh_clips_for_path(state, file_path, &cached);
    recompile_if_needed(app, state);
    let _ = app.emit(
        "audio://import-pcm-ready",
        serde_json::json!({
            "file_path": file_path,
            "decoded_duration_sec": cached.duration_sec(),
            "sample_rate": cached.sample_rate,
            "progress": 1.0,
            "complete": true,
            "sample_count": cached.raw_samples.len(),
        }),
    );
    let _ = state
        .import_cancels
        .lock()
        .map(|mut s| s.remove(&cancel_key));
    Ok(cached)
}

fn cache_audio_clip(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    clip_id: &str,
    file_path: &str,
) -> Result<Arc<CachedAudioData>, String> {
    // Prefer in-flight / completed path cache (ADR-024 early ADD_CLIP).
    if let Some(cached) = state.audio_cache.get_by_path(file_path) {
        state
            .audio_cache
            .insert(clip_id.to_string(), Arc::clone(&cached));
        return Ok(cached);
    }
    let cached = ensure_path_cached(app, state, file_path)?;
    state
        .audio_cache
        .insert(clip_id.to_string(), Arc::clone(&cached));
    Ok(cached)
}

/// Async preload so the UI event loop can paint progressive waveform updates
/// while decode runs on a blocking pool thread (not inside sync `send_command`).
#[derive(serde::Serialize)]
pub struct PrepareAudioImportResult {
    pub duration_sec: f64,
    pub sample_rate: u32,
}

#[tauri::command]
pub async fn prepare_audio_import(
    app: tauri::AppHandle,
    file_path: String,
) -> Result<PrepareAudioImportResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let cached = ensure_path_cached(&app, &state, &file_path)?;
        Ok(PrepareAudioImportResult {
            duration_sec: cached.duration_sec(),
            sample_rate: cached.sample_rate,
        })
    })
    .await
    .map_err(|e| format!("Audio import task failed: {e}"))?
}

#[tauri::command]
pub fn cancel_audio_import(
    state: tauri::State<'_, AppState>,
    file_path: String,
) -> Result<(), String> {
    let key = norm_import_path(&file_path);
    if let Ok(mut set) = state.import_cancels.lock() {
        set.insert(key);
    }
    // Drop incomplete path prefixes so a later re-import cannot adopt a stub.
    state.audio_cache.remove_if_incomplete(&file_path);
    Ok(())
}

fn resolve_clip_audio(
    state: &tauri::State<'_, AppState>,
    clip_id: &str,
    file_path: &str,
) -> Option<Arc<CachedAudioData>> {
    if let Some(cached) = state.audio_cache.get(clip_id) {
        return Some(cached);
    }
    if let Some(cached) = state.audio_cache.get_by_path(file_path) {
        state
            .audio_cache
            .insert(clip_id.to_string(), Arc::clone(&cached));
        return Some(cached);
    }
    match AudioClip::from_file(file_path) {
        Ok(audio) => Some(insert_cached_clip(
            state,
            clip_id,
            file_path,
            cached_audio_from_clip_data(audio.into_clip_data()),
        )),
        Err(e) => {
            eprintln!("[audio] Clip introuvable '{file_path}': {e}");
            None
        }
    }
}

fn app_clip_from_audio(
    clip_id: String,
    start: f64,
    end: f64,
    file_path: String,
    data: &CachedAudioData,
) -> AppClip {
    AppClip {
        id: clip_id,
        start,
        end,
        offset: 0.0,
        file_path,
        raw_samples: Arc::clone(&data.raw_samples),
        sample_rate: data.sample_rate,
        peaks_64: data.peaks_64.clone(),
        peaks_512: data.peaks_512.clone(),
        peaks_4096: data.peaks_4096.clone(),
    }
}

// --- apply_events payload helpers ---

fn payload_str(payload: &Value, key: &str) -> String {
    payload[key].as_str().unwrap_or("").to_string()
}

fn payload_str_default(payload: &Value, key: &str, default: &str) -> String {
    payload[key].as_str().unwrap_or(default).to_string()
}

enum HotEffectUpdate {
    None,
    Eq(EqParams),
    Compressor(CompressorParams),
    Reverb(ReverbParams),
    Delay(DelayParams),
    /// Push normalised `(native_id, value)` pairs into the plugin's SPSC ring (ADR-007).
    Plugin(String, Vec<(u32, f32)>),
}

fn hot_effect_update_from(effect: &TrackEffect) -> HotEffectUpdate {
    if !effect.enabled {
        return HotEffectUpdate::None;
    }
    match &effect.params {
        EffectParams::Eq(eq) => HotEffectUpdate::Eq(eq.clone()),
        EffectParams::Compressor(c) => HotEffectUpdate::Compressor(c.clone()),
        EffectParams::Reverb(r) => HotEffectUpdate::Reverb(r.clone()),
        EffectParams::Delay(d) => HotEffectUpdate::Delay(d.clone()),
        EffectParams::Plugin(pp) => {
            HotEffectUpdate::Plugin(pp.plugin_id.clone(), pp.params.clone())
        }
    }
}

fn apply_hot_effect_update(
    state: &tauri::State<'_, AppState>,
    effect_id: &str,
    hot: &HotEffectUpdate,
) -> bool {
    match hot {
        HotEffectUpdate::Eq(eq) => {
            let values = crate::audio_graph::nodes::eq::eq_block_values(eq);
            state.live_params.write_effect_block(effect_id, &values)
        }
        HotEffectUpdate::Compressor(c) => {
            let values = crate::audio_graph::nodes::compressor::compressor_block_values(c);
            state.live_params.write_effect_block(effect_id, &values)
        }
        HotEffectUpdate::Reverb(r) => {
            let values = crate::audio_graph::nodes::reverb::reverb_block_values(r);
            state.live_params.write_effect_block(effect_id, &values)
        }
        HotEffectUpdate::Delay(d) => {
            let values = crate::audio_graph::nodes::delay::delay_block_values(d);
            state.live_params.write_effect_block(effect_id, &values)
        }
        HotEffectUpdate::Plugin(_device_id, params) => {
            // Address by `effect_id` (= the plugin's per-placement instance key),
            // not the plugin *type*, so duplicates on different tracks don't
            // collide (ADR-017).
            if let Ok(engine) = state.audio_engine.lock() {
                let mut all_ok = !params.is_empty();
                for (native_id, value) in params {
                    match engine
                        .plugin_host
                        .push_plugin_param(effect_id, *native_id, *value)
                    {
                        Ok(_) => {
                            // Keep the modulation baseline shadow in sync so
                            // plugin wrappers can compute `shadow + mod_delta`
                            // without querying the plugin (which is not RT-safe).
                            state
                                .live_params
                                .set_plugin_param_shadow(effect_id, *native_id, *value);
                        }
                        Err(_) => {
                            all_ok = false;
                            break;
                        }
                    }
                }
                all_ok
            } else {
                false
            }
        }
        HotEffectUpdate::None => false,
    }
}

fn patch_effect(
    effect: &mut TrackEffect,
    p: UpdateEffectPayload,
) -> Result<(HotEffectUpdate, bool), String> {
    let mut structural = false;
    if let Some(en) = p.enabled {
        if effect.enabled != en {
            structural = true;
        }
        effect.enabled = en;
    }
    if let (Some(sandboxed), EffectParams::Plugin(plugin)) = (p.sandboxed, &mut effect.params) {
        if plugin.sandboxed != sandboxed {
            structural = true;
        }
        plugin.sandboxed = sandboxed;
    }
    if let (Some(source), EffectParams::Plugin(plugin)) = (&p.sidechain_source, &mut effect.params)
    {
        if let Some(source_track_id) = source {
            if source_track_id == &p.track_id {
                return Err(
                    "Un plugin ne peut pas se sidechaîner depuis sa propre piste".to_string(),
                );
            }
        }
        if &plugin.sidechain_source != source {
            structural = true;
        }
        plugin.sidechain_source = source.clone();
    }
    let params = p.params;
    if let Some(raw) = params {
        effect.params = merge_effect_params(&effect.params, raw)?;
    }
    Ok((hot_effect_update_from(effect), structural))
}

fn build_modulator(p: &AddModulatorPayload) -> Result<crate::project::ProjectModulator, String> {
    use crate::project::{
        AudioFollowerParams, EnvelopeParams, LfoParams, MacroParams, ModulatorParams,
        ProjectModulator,
    };
    let params = match p.kind.as_str() {
        "lfo" => ModulatorParams::Lfo(LfoParams {
            rate_hz: p.rate_hz.unwrap_or(1.0),
            ..Default::default()
        }),
        "macro" => ModulatorParams::Macro(MacroParams {
            value: p.value.unwrap_or(0.0),
        }),
        "envelope" => ModulatorParams::Envelope(EnvelopeParams::default()),
        "audio_follower" => ModulatorParams::AudioFollower(AudioFollowerParams::default()),
        other => return Err(format!("Unknown modulator kind: {other}")),
    };
    Ok(ProjectModulator {
        id: generate_id(),
        params,
    })
}

fn build_modulation_route(
    p: &AddModulationRoutePayload,
) -> Result<crate::project::ProjectModulationRoute, String> {
    use crate::project::{ModulationTargetRef, ProjectModulationRoute};
    let target = match p.target_kind.as_str() {
        "track_gain" => ModulationTargetRef::TrackGain {
            track_id: p.track_id.clone(),
        },
        "effect_param" => ModulationTargetRef::EffectParam {
            track_id: p.track_id.clone(),
            effect_id: p
                .effect_id
                .clone()
                .ok_or("effect_id required for effect_param target")?,
            param: p.param.clone(),
        },
        "plugin_param" => ModulationTargetRef::PluginParam {
            effect_id: p
                .effect_id
                .clone()
                .ok_or("effect_id required for plugin_param target")?,
            native_id: p.param.parse::<u32>().map_err(|_| {
                "param must be a native_id (u32) for plugin_param target".to_string()
            })?,
        },
        other => return Err(format!("Unknown target kind: {other}")),
    };
    Ok(ProjectModulationRoute {
        id: generate_id(),
        modulator_id: p.modulator_id.clone(),
        target,
        depth: p.depth,
    })
}

fn build_automation_lane(
    p: &AddAutomationLanePayload,
) -> Result<crate::project::AutomationLane, String> {
    use crate::project::{AutomationLane, ModulationTargetRef};
    let target = match p.target_kind.as_str() {
        "track_gain" => ModulationTargetRef::TrackGain {
            track_id: p.track_id.clone(),
        },
        "effect_param" => ModulationTargetRef::EffectParam {
            track_id: p.track_id.clone(),
            effect_id: p
                .effect_id
                .clone()
                .ok_or("effect_id required for effect_param target")?,
            param: p.param.clone(),
        },
        "plugin_param" => ModulationTargetRef::PluginParam {
            effect_id: p
                .effect_id
                .clone()
                .ok_or("effect_id required for plugin_param target")?,
            native_id: p
                .param
                .parse::<u32>()
                .map_err(|_| "param must be a native_id (u32) for plugin_param".to_string())?,
        },
        other => return Err(format!("Unknown automation target kind: {other}")),
    };
    Ok(AutomationLane {
        id: generate_id(),
        target,
        clips: vec![],
        interpolation: p.interpolation.clone(),
    })
}

fn process_command(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    command: CommandEnvelope,
) -> Result<CommandResult, String> {
    let command_id = command.id.clone();
    let project_id = command.project_id.clone();
    let ctx = ProjectCommit {
        state,
        app,
        project_id: &project_id,
        command_id,
    };

    match command.command_type.as_str() {
        cmd::PLAY => {
            state.transport.play();
            Ok(emit_and_ok(
                app,
                ctx.command_id.clone(),
                &project_id,
                ev::PLAY_STARTED,
                json!({}),
            ))
        }

        cmd::STOP => {
            crate::midi_recording::finish_and_emit(app, &state.midi_recorder);
            state.transport.stop();
            let ev_stop = EngineEvent::new(ev::PLAY_STOPPED, &project_id, json!({}));
            let ev_pos = EngineEvent::new(
                ev::POSITION_UPDATED,
                &project_id,
                json!({ "position": 0.0 }),
            );
            emit_engine_event(app, &ev_stop);
            emit_engine_event(app, &ev_pos);
            Ok(ok_transport_events(
                ctx.command_id.clone(),
                &[ev_stop, ev_pos],
            ))
        }

        cmd::PAUSE => {
            crate::midi_recording::finish_and_emit(app, &state.midi_recorder);
            state.transport.pause();
            Ok(emit_and_ok(
                app,
                ctx.command_id.clone(),
                &project_id,
                ev::PLAY_STOPPED,
                json!({ "pause": true }),
            ))
        }

        cmd::SEEK => {
            let p: SeekPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("SEEK payload invalid: {e}"))?;
            state.transport.seek(p.position);
            Ok(emit_and_ok(
                app,
                ctx.command_id.clone(),
                &project_id,
                ev::POSITION_UPDATED,
                json!({ "position": p.position }),
            ))
        }

        cmd::SET_LOOP => {
            let p: SetLoopPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("SET_LOOP payload invalid: {e}"))?;
            state.transport.set_loop(p.enabled, p.start, p.end);
            Ok(ok_transport_events(ctx.command_id, &[]))
        }

        cmd::CREATE_TRACK => {
            let p: CreateTrackPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("CREATE_TRACK payload invalid: {e}"))?;
            if let Some(ref plugin_id) = p.instrument {
                validate_instrument(state, plugin_id)?;
            }
            let track_id = generate_id();

            with_project_mut(state, |project| {
                let mut track = empty_app_track(track_id.clone(), p.name.clone());
                track.instrument = p.instrument.clone();
                project.tracks.push(track);
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::TRACK_CREATED,
                json!({ "id": track_id, "name": p.name, "instrument": p.instrument }),
                true,
            )
        }

        cmd::DELETE_TRACK => {
            let p: DeleteTrackPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("DELETE_TRACK payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                project.tracks.retain(|t| t.id != p.track_id);
                Ok(())
            })?;

            commit_same_payload(&ctx, ev::TRACK_DELETED, json!({ "id": p.track_id }), true)
        }

        cmd::UPDATE_TRACK => {
            let p: UpdateTrackPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("UPDATE_TRACK payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                let track = require_track_mut(project, &p.track_id)?;
                if let Some(name) = p.name.as_ref() {
                    track.name = name.clone();
                }
                if let Some(vol) = p.volume {
                    track.volume = vol;
                }
                if let Some(pan) = p.pan {
                    track.pan = pan;
                }
                if let Some(color) = p.color.as_ref() {
                    track.color = Some(color.clone());
                }
                Ok(())
            })?;

            // Hot-path: live atomics updated directly, no recompile.
            if let Some(vol) = p.volume {
                let linear = (vol / 100.0).clamp(VOLUME_RANGE.0, VOLUME_RANGE.1);
                state.live_params.set_track_volume(&p.track_id, linear);
            }
            if let Some(pan) = p.pan {
                state
                    .live_params
                    .set_track_pan(&p.track_id, normalize_pan(pan));
            }

            commit_same_payload(
                &ctx,
                ev::TRACK_UPDATED,
                json!({
                    "id": p.track_id,
                    "name": p.name,
                    "volume": p.volume,
                    "pan": p.pan,
                    "color": p.color,
                }),
                false,
            )
        }

        cmd::MUTE_TRACK => {
            let p: MuteTrackPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("MUTE_TRACK payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?.muted = p.muted;
                Ok(())
            })?;

            // Hot-path: flip the atomic mute flag; no recompile.
            state.live_params.set_track_muted(&p.track_id, p.muted);

            commit_same_payload(
                &ctx,
                ev::TRACK_MUTED,
                json!({ "id": p.track_id, "muted": p.muted }),
                false,
            )
        }

        cmd::SOLO_TRACK => {
            let p: SoloTrackPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("SOLO_TRACK payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                if !project.tracks.iter().any(|t| t.id == p.track_id) {
                    return Err(format!("Track {} not found", p.track_id));
                }
                apply_exclusive_solo(project, &p.track_id, p.solo);
                Ok(())
            })?;

            state
                .live_params
                .set_track_solo_exclusive(&p.track_id, p.solo);

            commit_same_payload(
                &ctx,
                ev::TRACK_SOLOED,
                json!({ "id": p.track_id, "solo": p.solo }),
                false,
            )
        }

        cmd::SET_TRACK_INSTRUMENT => {
            let p: SetTrackInstrumentPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("SET_TRACK_INSTRUMENT payload invalid: {e}"))?;

            if let Some(ref plugin_id) = p.instrument {
                validate_instrument(state, plugin_id)?;
            }

            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?.instrument = p.instrument.clone();
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::TRACK_INSTRUMENT_SET,
                json!({ "id": p.track_id, "instrument": p.instrument }),
                true,
            )
        }

        cmd::SET_TRACK_INPUT => {
            let p: SetTrackInputPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("SET_TRACK_INPUT payload invalid: {e}"))?;
            if p.input_source
                .as_ref()
                .is_some_and(|source| source.port == 0)
            {
                return Err("SET_TRACK_INPUT: port 0 is the plugin's main output".into());
            }

            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?.input_source = p.input_source.clone();
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::TRACK_INPUT_SET,
                json!({ "id": p.track_id, "input_source": p.input_source }),
                true,
            )
        }

        cmd::ADD_CLIP => {
            let p: AddClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("ADD_CLIP payload invalid: {e}"))?;

            let clip_id = generate_id();
            let cached = cache_audio_clip(ctx.app, state, &clip_id, &p.file_path)?;
            let file_duration = cached.duration_sec();
            let end = if p.end > p.start {
                p.end
            } else {
                p.start + file_duration
            };
            let mut clip =
                app_clip_from_audio(clip_id.clone(), p.start, end, p.file_path.clone(), &cached);
            let offset = p.offset.max(0.0);
            clip.offset = offset;
            let clip_dto = clip.to_dto();

            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?.clips.push(clip);
                Ok(())
            })?;

            commit_project_change(
                &ctx,
                ev::CLIP_ADDED,
                json!({
                    "track_id": p.track_id,
                    "clip_id": clip_id,
                    "file_path": p.file_path,
                    "start": p.start,
                    "end": end,
                    "offset": offset,
                }),
                json!({ "track_id": p.track_id, "clip": clip_dto }),
                true,
            )
        }

        cmd::MOVE_CLIP => {
            let p: MoveClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("MOVE_CLIP payload invalid: {e}"))?;

            let dst = p
                .target_track_id
                .as_deref()
                .filter(|t| *t != p.track_id.as_str())
                .map(str::to_owned);

            if let Some(ref dst_id) = dst {
                let (clip_dto, file_path, offset) = with_project_mut(state, |project| {
                    let src_idx = project
                        .tracks
                        .iter()
                        .position(|t| t.id == p.track_id)
                        .ok_or_else(|| format!("Track {} not found", p.track_id))?;
                    let clip_idx = project.tracks[src_idx]
                        .clips
                        .iter()
                        .position(|c| c.id == p.clip_id)
                        .ok_or_else(|| format!("Clip {} not found", p.clip_id))?;
                    let mut clip = project.tracks[src_idx].clips.remove(clip_idx);
                    clip.start = p.start;
                    clip.end = p.end;
                    clip.offset = p.offset.unwrap_or(clip.offset);
                    let offset = clip.offset;
                    let file_path = clip.file_path.clone();
                    let clip_dto = clip.to_dto();
                    let dst_idx = project
                        .tracks
                        .iter()
                        .position(|t| t.id.as_str() == dst_id)
                        .ok_or_else(|| format!("Target track {dst_id} not found"))?;
                    project.tracks[dst_idx].clips.push(clip);
                    Ok((clip_dto, file_path, offset))
                })?;

                commit_project_change(
                    &ctx,
                    ev::CLIP_MOVED_TO_TRACK,
                    json!({
                        "from_track_id": p.track_id,
                        "to_track_id": dst_id,
                        "clip_id": p.clip_id,
                        "file_path": file_path,
                        "start": p.start,
                        "end": p.end,
                        "offset": offset,
                    }),
                    json!({
                        "from_track_id": p.track_id,
                        "to_track_id": dst_id,
                        "clip": serde_json::to_value(&clip_dto).unwrap_or_default(),
                    }),
                    true,
                )
            } else {
                let offset = with_project_mut(state, |project| {
                    let track = require_track_mut(project, &p.track_id)?;
                    let clip = require_audio_clip_mut(track, &p.clip_id)?;
                    clip.start = p.start;
                    clip.end = p.end;
                    clip.offset = p.offset.unwrap_or(clip.offset);
                    Ok(clip.offset)
                })?;

                commit_same_payload(
                    &ctx,
                    ev::CLIP_MOVED,
                    json!({
                        "track_id": p.track_id,
                        "clip_id": p.clip_id,
                        "start": p.start,
                        "end": p.end,
                        "offset": offset,
                    }),
                    true,
                )
            }
        }

        cmd::REMOVE_CLIP => {
            let p: RemoveClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("REMOVE_CLIP payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?
                    .clips
                    .retain(|c| c.id != p.clip_id);
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::CLIP_REMOVED,
                json!({ "track_id": p.track_id, "clip_id": p.clip_id }),
                true,
            )
        }

        cmd::UPDATE_PROJECT_NAME => {
            let p: UpdateProjectNamePayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("UPDATE_PROJECT_NAME payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                project.name = p.name.clone();
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::PROJECT_NAME_UPDATED,
                json!({ "name": p.name }),
                false,
            )
        }

        cmd::UPDATE_TEMPO => {
            let p: UpdateTempoPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("UPDATE_TEMPO payload invalid: {e}"))?;

            state.transport.set_tempo(
                p.bpm,
                crate::audio_graph::node::TimeSignature::new(p.numerator, p.denominator),
            );

            with_project_mut(state, |project| {
                project.bpm = p.bpm;
                project.numerator = p.numerator;
                project.denominator = p.denominator;
                project.midi_grid_beats = p.midi_grid_beats;
                project.key_root = p.key_root.clone();
                project.key_mode = p.key_mode.clone();
                project.swing_percent = p.swing_percent.min(100);
                project.midi_ppq = p.midi_ppq;
                project.midi_grid_use_triplets = p.midi_grid_use_triplets;
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::TEMPO_UPDATED,
                json!({
                    "bpm": p.bpm,
                    "numerator": p.numerator,
                    "denominator": p.denominator,
                    "midi_grid_beats": p.midi_grid_beats,
                    "key_root": p.key_root,
                    "key_mode": p.key_mode,
                    "swing_percent": p.swing_percent.min(100),
                    "midi_ppq": p.midi_ppq,
                    "midi_grid_use_triplets": p.midi_grid_use_triplets,
                }),
                false,
            )
        }

        cmd::ADD_EFFECT => {
            let p: AddEffectPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("ADD_EFFECT payload invalid: {e}"))?;

            if let Some(plugin_id) = plugin_id_from_effect_kind(&p.kind) {
                let engine = state
                    .audio_engine
                    .lock()
                    .map_err(|e| format!("audio engine lock: {e}"))?;
                if let Some(manifest) = engine.plugin_host.registry().get(plugin_id) {
                    if manifest.is_instrument {
                        return Err(format!(
                            "'{}' est un instrument — choisissez-le dans la section Instrument",
                            manifest.name
                        ));
                    }
                }
            }

            let effect_id = generate_id();
            let params = default_params_for_kind(&p.kind)
                .ok_or_else(|| format!("Unknown effect kind: {}", p.kind))?;
            let effect = TrackEffect {
                id: effect_id.clone(),
                enabled: true,
                params,
            };

            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?
                    .effects
                    .push(effect.clone());
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::EFFECT_ADDED,
                json!({
                    "track_id": p.track_id,
                    "effect": serde_json::to_value(&effect).unwrap_or_default(),
                }),
                true,
            )
        }

        cmd::REMOVE_EFFECT => {
            let p: RemoveEffectPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("REMOVE_EFFECT payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?
                    .effects
                    .retain(|e| e.id != p.effect_id);
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::EFFECT_REMOVED,
                json!({ "track_id": p.track_id, "effect_id": p.effect_id }),
                true,
            )
        }

        cmd::UPDATE_EFFECT => {
            let p: UpdateEffectPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("UPDATE_EFFECT payload invalid: {e}"))?;

            // Parameter-only edits are hot-delivered (ADR-009/ADR-007); enable/disable toggles recompile.
            let (track_id, effect_id) = (p.track_id.clone(), p.effect_id.clone());
            let (updated_effect, hot, structural) = with_project_mut(state, |project| {
                let track = require_track_mut(project, &track_id)?;
                let effect = require_effect_mut(track, &effect_id)?;
                let (hot, structural) = patch_effect(effect, p)?;
                Ok((
                    serde_json::to_value(&*effect).unwrap_or_default(),
                    hot,
                    structural,
                ))
            })?;

            let event_payload =
                json!({ "track_id": track_id, "effect_id": effect_id, "effect": updated_effect });
            let engine_ev = snapshot_emit(&ctx, ev::EFFECT_UPDATED, event_payload)?;

            if structural || !apply_hot_effect_update(state, &effect_id, &hot) {
                recompile_if_needed(app, state);
            }

            Ok(ok_single_event(ctx.command_id.clone(), &engine_ev))
        }

        cmd::ADD_MODULATOR => {
            let p: AddModulatorPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("ADD_MODULATOR payload invalid: {e}"))?;

            let modulator = build_modulator(&p)?;
            let mod_value = serde_json::to_value(&modulator).unwrap_or_default();
            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?
                    .modulators
                    .push(modulator);
                Ok(())
            })?;

            commit_recompile_before_emit(
                &ctx,
                ev::MODULATOR_ADDED,
                json!({ "track_id": p.track_id, "modulator": mod_value }),
            )
        }

        cmd::REMOVE_MODULATOR => {
            let p: RemoveModulatorPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("REMOVE_MODULATOR payload invalid: {e}"))?;
            with_project_mut(state, |project| {
                let track = require_track_mut(project, &p.track_id)?;
                track.modulators.retain(|m| m.id != p.modulator_id);
                track
                    .mod_routes
                    .retain(|r| r.modulator_id != p.modulator_id);
                Ok(())
            })?;
            commit_recompile_before_emit(
                &ctx,
                ev::MODULATOR_REMOVED,
                json!({ "track_id": p.track_id, "modulator_id": p.modulator_id }),
            )
        }

        cmd::UPDATE_MODULATOR => {
            let p: UpdateModulatorPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("UPDATE_MODULATOR payload invalid: {e}"))?;
            let has_update = p.rate_hz.is_some()
                || p.waveform.is_some()
                || p.tempo_sync.is_some()
                || p.beat_division.is_some()
                || p.value.is_some()
                || p.attack_ms.is_some()
                || p.decay_ms.is_some()
                || p.sustain_level.is_some()
                || p.release_ms.is_some()
                || p.window_ms.is_some()
                || p.mode.is_some()
                || p.source_effect_id.is_some();
            // Follower params are baked into its node at compile time (no live cells).
            let mut follower_edited = false;
            if has_update {
                with_project_mut(state, |project| {
                    let track = require_track_mut(project, &p.track_id)?;
                    if let Some(Some(effect_id)) = &p.source_effect_id {
                        if !track.effects.iter().any(|e| &e.id == effect_id) {
                            return Err(format!(
                                "Effect {} not found on track {}",
                                effect_id, p.track_id
                            ));
                        }
                    }
                    if let Some(m) = track.modulators.iter_mut().find(|x| x.id == p.modulator_id) {
                        match &mut m.params {
                            crate::project::ModulatorParams::Lfo(ref mut lp) => {
                                if let Some(rate) = p.rate_hz {
                                    lp.rate_hz = rate;
                                }
                                if let Some(w) = &p.waveform {
                                    lp.waveform = w.clone();
                                }
                                if let Some(v) = p.tempo_sync {
                                    lp.tempo_sync = v;
                                }
                                if let Some(v) = p.beat_division {
                                    lp.beat_division = v;
                                }
                            }
                            crate::project::ModulatorParams::Macro(ref mut mp) => {
                                if let Some(value) = p.value {
                                    mp.value = value;
                                }
                            }
                            crate::project::ModulatorParams::Envelope(ref mut ep) => {
                                if let Some(v) = p.attack_ms {
                                    ep.attack_ms = v;
                                }
                                if let Some(v) = p.decay_ms {
                                    ep.decay_ms = v;
                                }
                                if let Some(v) = p.sustain_level {
                                    ep.sustain_level = v;
                                }
                                if let Some(v) = p.release_ms {
                                    ep.release_ms = v;
                                }
                            }
                            crate::project::ModulatorParams::AudioFollower(ref mut fp) => {
                                if let Some(v) = p.window_ms {
                                    fp.window_ms = v;
                                }
                                if let Some(v) = p.attack_ms {
                                    fp.attack_ms = v;
                                }
                                if let Some(v) = p.release_ms {
                                    fp.release_ms = v;
                                }
                                if let Some(v) = &p.mode {
                                    fp.mode = v.clone();
                                }
                                if let Some(v) = &p.source_effect_id {
                                    fp.source_effect_id = v.clone();
                                }
                                follower_edited = p.window_ms.is_some()
                                    || p.attack_ms.is_some()
                                    || p.release_ms.is_some()
                                    || p.mode.is_some()
                                    || p.source_effect_id.is_some();
                            }
                        }
                    }
                    Ok(())
                })?;
            }
            // Hot-path: update atomics, no recompile.
            if let Some(rate) = p.rate_hz {
                state.live_params.set_modulator_rate(&p.modulator_id, rate);
            }
            if let Some(v) = p.tempo_sync {
                state
                    .live_params
                    .set_modulator_tempo_sync(&p.modulator_id, v);
            }
            if let Some(v) = p.beat_division {
                state
                    .live_params
                    .set_modulator_beat_division(&p.modulator_id, v);
            }
            if let Some(value) = p.value {
                state.live_params.set_macro_value(&p.modulator_id, value);
            }
            if p.attack_ms.is_some()
                || p.decay_ms.is_some()
                || p.sustain_level.is_some()
                || p.release_ms.is_some()
            {
                state.live_params.set_envelope_params(
                    &p.modulator_id,
                    p.attack_ms,
                    p.decay_ms,
                    p.sustain_level,
                    p.release_ms,
                );
            }
            let mut event = json!({
                "track_id": p.track_id,
                "modulator_id": p.modulator_id,
                "rate_hz": p.rate_hz,
                "waveform": p.waveform,
                "tempo_sync": p.tempo_sync,
                "beat_division": p.beat_division,
                "value": p.value,
                "attack_ms": p.attack_ms,
                "decay_ms": p.decay_ms,
                "sustain_level": p.sustain_level,
                "release_ms": p.release_ms,
                "window_ms": p.window_ms,
                "mode": p.mode,
            });
            // Only present when edited: a `null` tap means "post-FX", not "unchanged".
            if let Some(source) = &p.source_effect_id {
                event["source_effect_id"] = json!(source);
            }
            commit_same_payload(
                &ctx,
                ev::MODULATOR_UPDATED,
                event,
                p.waveform.is_some() || follower_edited,
            )
        }

        cmd::ADD_MODULATION_ROUTE => {
            let p: AddModulationRoutePayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("ADD_MODULATION_ROUTE payload invalid: {e}"))?;

            let route = build_modulation_route(&p)?;
            let route_value = serde_json::to_value(&route).unwrap_or_default();
            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?
                    .mod_routes
                    .push(route);
                Ok(())
            })?;
            commit_recompile_before_emit(
                &ctx,
                ev::MODULATION_ROUTE_ADDED,
                json!({ "track_id": p.track_id, "route": route_value }),
            )
        }

        cmd::REMOVE_MODULATION_ROUTE => {
            let p: RemoveModulationRoutePayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("REMOVE_MODULATION_ROUTE payload invalid: {e}"))?;
            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?
                    .mod_routes
                    .retain(|r| r.id != p.route_id);
                Ok(())
            })?;
            commit_recompile_before_emit(
                &ctx,
                ev::MODULATION_ROUTE_REMOVED,
                json!({ "track_id": p.track_id, "route_id": p.route_id }),
            )
        }

        cmd::SET_MODULATION_DEPTH => {
            let p: SetModulationDepthPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("SET_MODULATION_DEPTH payload invalid: {e}"))?;
            with_project_mut(state, |project| {
                let track = require_track_mut(project, &p.track_id)?;
                if let Some(route) = track.mod_routes.iter_mut().find(|r| r.id == p.route_id) {
                    route.depth = p.depth;
                }
                Ok(())
            })?;
            // Hot-path: update the per-route depth atomic, no recompile.
            state.live_params.set_route_depth(&p.route_id, p.depth);
            commit_same_payload(
                &ctx,
                ev::MODULATION_DEPTH_CHANGED,
                json!({ "track_id": p.track_id, "route_id": p.route_id, "depth": p.depth }),
                false,
            )
        }

        cmd::SET_MIDI_NOTES => {
            let p: SetMidiNotesPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("SET_MIDI_NOTES payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?.midi_notes = p.notes.clone();
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::MIDI_NOTES_SET,
                json!({
                    "track_id": p.track_id,
                    "notes": serde_json::to_value(&p.notes).unwrap_or_default(),
                }),
                false,
            )
        }

        cmd::ADD_MIDI_CLIP => {
            let p: AddMidiClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("ADD_MIDI_CLIP payload invalid: {e}"))?;
            let clip_id = generate_id();
            let notes = p
                .notes
                .into_iter()
                .map(|mut note| {
                    note.id = generate_id();
                    note
                })
                .collect();
            let clip = AppMidiClip {
                id: clip_id.clone(),
                start: p.start,
                end: p.end,
                offset: p.offset.max(0.0),
                notes,
            };
            let clip_dto = MidiClipDto {
                id: clip.id.clone(),
                start: clip.start,
                end: clip.end,
                offset: clip.offset,
                notes: clip.notes.clone(),
            };

            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?
                    .midi_clips
                    .push(clip);
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::MIDI_CLIP_ADDED,
                json!({
                    "track_id": p.track_id,
                    "clip": serde_json::to_value(&clip_dto).unwrap_or_default(),
                }),
                true,
            )
        }

        cmd::REMOVE_MIDI_CLIP => {
            let p: RemoveMidiClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("REMOVE_MIDI_CLIP payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?
                    .midi_clips
                    .retain(|c| c.id != p.clip_id);
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::MIDI_CLIP_REMOVED,
                json!({ "track_id": p.track_id, "clip_id": p.clip_id }),
                true,
            )
        }

        cmd::MOVE_MIDI_CLIP => {
            let p: MoveMidiClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("MOVE_MIDI_CLIP payload invalid: {e}"))?;

            let dst = p
                .target_track_id
                .as_deref()
                .filter(|t| *t != p.track_id.as_str())
                .map(str::to_owned);

            if let Some(ref dst_id) = dst {
                let clip_dto = with_project_mut(state, |project| {
                    let src_idx = project
                        .tracks
                        .iter()
                        .position(|t| t.id == p.track_id)
                        .ok_or_else(|| format!("Track {} not found", p.track_id))?;
                    let clip_idx = project.tracks[src_idx]
                        .midi_clips
                        .iter()
                        .position(|c| c.id == p.clip_id)
                        .ok_or_else(|| format!("MidiClip {} not found", p.clip_id))?;
                    let mut clip = project.tracks[src_idx].midi_clips.remove(clip_idx);
                    clip.start = p.start;
                    clip.end = p.end;
                    clip.offset = p.offset.unwrap_or(clip.offset);
                    let clip_dto = MidiClipDto {
                        id: clip.id.clone(),
                        start: clip.start,
                        end: clip.end,
                        offset: clip.offset,
                        notes: clip.notes.clone(),
                    };
                    let dst_idx = project
                        .tracks
                        .iter()
                        .position(|t| t.id.as_str() == dst_id)
                        .ok_or_else(|| format!("Target track {dst_id} not found"))?;
                    project.tracks[dst_idx].midi_clips.push(clip);
                    Ok(clip_dto)
                })?;

                commit_same_payload(
                    &ctx,
                    ev::MIDI_CLIP_MOVED_TO_TRACK,
                    json!({
                        "from_track_id": p.track_id,
                        "to_track_id": dst_id,
                        "clip": serde_json::to_value(&clip_dto).unwrap_or_default(),
                    }),
                    true,
                )
            } else {
                let offset = with_project_mut(state, |project| {
                    let track = require_track_mut(project, &p.track_id)?;
                    let clip = require_midi_clip_mut(track, &p.clip_id)?;
                    clip.start = p.start;
                    clip.end = p.end;
                    clip.offset = p.offset.unwrap_or(clip.offset);
                    Ok(clip.offset)
                })?;

                commit_same_payload(
                    &ctx,
                    ev::MIDI_CLIP_MOVED,
                    json!({
                        "track_id": p.track_id,
                        "clip_id": p.clip_id,
                        "start": p.start,
                        "end": p.end,
                        "offset": offset,
                    }),
                    true,
                )
            }
        }

        cmd::SET_CLIP_MIDI_NOTES => {
            let p: SetClipMidiNotesPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("SET_CLIP_MIDI_NOTES payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                let track = require_track_mut(project, &p.track_id)?;
                require_midi_clip_mut(track, &p.clip_id)?.notes = p.notes.clone();
                Ok(())
            })?;

            // Note content is not a graph change. Publish into the live
            // instrument so playback keeps its plugin state. Recompile only
            // when the track has no slot yet.
            let sample_rate = state
                .audio_engine
                .lock()
                .map(|engine| engine.transport.sample_rate())
                .unwrap_or(48_000.0);
            let baked = lock_project(state).ok().and_then(|project| {
                project
                    .tracks
                    .iter()
                    .find(|t| t.id == p.track_id)
                    .map(|track| {
                        crate::audio_graph::compiler::bake_track_midi_notes(track, sample_rate)
                    })
            });
            let published = match baked {
                Some(notes) => state.live_params.publish_track_notes(&p.track_id, notes),
                None => false,
            };

            let result = commit_same_payload(
                &ctx,
                ev::MIDI_CLIP_NOTES_SET,
                json!({
                    "track_id": p.track_id,
                    "clip_id": p.clip_id,
                    "notes": serde_json::to_value(&p.notes).unwrap_or_default(),
                }),
                false,
            )?;
            if !published {
                recompile_if_needed(ctx.app, ctx.state);
            }
            Ok(result)
        }

        cmd::ADD_AUTOMATION_LANE => {
            let p: AddAutomationLanePayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("ADD_AUTOMATION_LANE payload invalid: {e}"))?;
            let lane = build_automation_lane(&p)?;
            let lane_value = serde_json::to_value(&lane).unwrap_or_default();
            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?
                    .automation_lanes
                    .push(lane);
                Ok(())
            })?;
            commit_recompile_before_emit(
                &ctx,
                ev::AUTOMATION_LANE_ADDED,
                json!({ "track_id": p.track_id, "lane": lane_value }),
            )
        }

        cmd::REMOVE_AUTOMATION_LANE => {
            let p: RemoveAutomationLanePayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("REMOVE_AUTOMATION_LANE payload invalid: {e}"))?;
            with_project_mut(state, |project| {
                require_track_mut(project, &p.track_id)?
                    .automation_lanes
                    .retain(|l| l.id != p.lane_id);
                Ok(())
            })?;
            commit_recompile_before_emit(
                &ctx,
                ev::AUTOMATION_LANE_REMOVED,
                json!({ "track_id": p.track_id, "lane_id": p.lane_id }),
            )
        }

        cmd::UPDATE_AUTOMATION_CLIP => {
            let p: UpdateAutomationClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("UPDATE_AUTOMATION_CLIP payload invalid: {e}"))?;
            let clip_value = serde_json::to_value(&p.clip).unwrap_or_default();
            with_project_mut(state, |project| {
                let track = require_track_mut(project, &p.track_id)?;
                let lane = track
                    .automation_lanes
                    .iter_mut()
                    .find(|l| l.id == p.lane_id)
                    .ok_or_else(|| format!("Automation lane {} not found", p.lane_id))?;
                if let Some(existing) = lane.clips.iter_mut().find(|c| c.id == p.clip.id) {
                    *existing = p.clip;
                } else {
                    lane.clips.push(p.clip);
                }
                Ok(())
            })?;
            commit_recompile_before_emit(
                &ctx,
                ev::AUTOMATION_CLIP_UPDATED,
                json!({ "track_id": p.track_id, "lane_id": p.lane_id, "clip": clip_value }),
            )
        }

        other => Err(format!("Unknown command type: {other}")),
    }
}

fn append_and_maybe_snapshot(
    state: &tauri::State<'_, AppState>,
    event: ProjectEvent,
) -> Result<(), String> {
    mark_dirty(state);
    let mut store = state.event_store.lock().map_err(|e| e.to_string())?;
    store.append(event);

    if store.should_snapshot() {
        drop(store); // release before acquiring project lock (lock order: project → store)
        let project_snapshot = snapshot_project(&*lock_project(state)?);
        let mut store = state.event_store.lock().map_err(|e| e.to_string())?;
        store.take_snapshot(project_snapshot);
    }

    Ok(())
}

pub fn snapshot_project_for_store(project: &AppProject) -> AppProject {
    snapshot_project(project)
}

fn snapshot_clip(c: &AppClip) -> AppClip {
    AppClip {
        id: c.id.clone(),
        start: c.start,
        end: c.end,
        offset: c.offset,
        file_path: c.file_path.clone(),
        raw_samples: c.raw_samples.clone(),
        sample_rate: c.sample_rate,
        peaks_64: c.peaks_64.clone(),
        peaks_512: c.peaks_512.clone(),
        peaks_4096: c.peaks_4096.clone(),
    }
}

fn snapshot_track(t: &AppTrack) -> AppTrack {
    AppTrack {
        id: t.id.clone(),
        name: t.name.clone(),
        volume: t.volume,
        pan: t.pan,
        muted: t.muted,
        solo: t.solo,
        effects: t.effects.clone(),
        modulators: t.modulators.clone(),
        mod_routes: t.mod_routes.clone(),
        midi_notes: t.midi_notes.clone(),
        midi_clips: t.midi_clips.clone(),
        instrument: t.instrument.clone(),
        input_source: t.input_source.clone(),
        automation_lanes: t.automation_lanes.clone(),
        clips: t.clips.iter().map(snapshot_clip).collect(),
        color: t.color.clone(),
    }
}

/// Shallow project copy for snapshotting — audio data stays in `AudioDataCache`.
fn snapshot_project(project: &AppProject) -> AppProject {
    AppProject {
        name: project.name.clone(),
        bpm: project.bpm,
        numerator: project.numerator,
        denominator: project.denominator,
        midi_grid_beats: project.midi_grid_beats,
        key_root: project.key_root.clone(),
        key_mode: project.key_mode.clone(),
        swing_percent: project.swing_percent,
        midi_ppq: project.midi_ppq,
        midi_grid_use_triplets: project.midi_grid_use_triplets,
        tracks: project.tracks.iter().map(snapshot_track).collect(),
        // Plugin binary state mirrors live plugin internals and is restored
        // separately (project load → compile → setState), not via the event
        // store.  Undo/redo snapshots therefore keep this empty.
        plugin_states: std::collections::HashMap::new(),
        ui_layout: ProjectUiLayout::default(),
    }
}

fn plugin_id_from_effect_kind(kind: &str) -> Option<&str> {
    kind.strip_prefix("plugin:")?.split('|').next()
}

fn format_tag(fmt: DeviceFormat) -> &'static str {
    match fmt {
        DeviceFormat::Vst3 => "VST3",
        DeviceFormat::Clap => "CLAP",
        #[cfg(feature = "au")]
        DeviceFormat::Au => "AU",
        DeviceFormat::Internal => "interne",
        #[cfg(feature = "wasm")]
        DeviceFormat::Wasm => "WASM",
    }
}

fn default_params_for_kind(kind: &str) -> Option<EffectParams> {
    // Plugin kind encodes identity as "plugin:<id>|<name>|<format>" so ADD_EFFECT
    // carries name and format for the UI without a separate lookup.
    if let Some(rest) = kind.strip_prefix("plugin:") {
        let mut parts = rest.splitn(3, '|');
        let plugin_id = parts.next().unwrap_or("").to_string();
        let name = parts.next().unwrap_or("Plugin").to_string();
        let format = parts.next().unwrap_or("").to_string();
        if !plugin_id.is_empty() {
            return Some(EffectParams::Plugin(PluginParams {
                plugin_id,
                name,
                format,
                params: vec![],
                sandboxed: false,
                sidechain_source: None,
            }));
        }
    }
    match kind {
        "eq" => Some(EffectParams::Eq(EqParams::default())),
        "compressor" => Some(EffectParams::Compressor(CompressorParams::default())),
        "reverb" => Some(EffectParams::Reverb(ReverbParams::default())),
        "delay" => Some(EffectParams::Delay(DelayParams::default())),
        _ => None,
    }
}

fn merge_effect_params(
    existing: &EffectParams,
    raw: serde_json::Value,
) -> Result<EffectParams, String> {
    match existing {
        EffectParams::Eq(_) => serde_json::from_value::<EqParams>(raw).map(EffectParams::Eq),
        EffectParams::Compressor(_) => {
            serde_json::from_value::<CompressorParams>(raw).map(EffectParams::Compressor)
        }
        EffectParams::Reverb(_) => {
            serde_json::from_value::<ReverbParams>(raw).map(EffectParams::Reverb)
        }
        EffectParams::Delay(_) => {
            serde_json::from_value::<DelayParams>(raw).map(EffectParams::Delay)
        }
        EffectParams::Plugin(p) => serde_json::from_value::<PluginParams>(raw).map(|updated| {
            EffectParams::Plugin(PluginParams {
                params: updated.params,
                ..p.clone()
            })
        }),
    }
    .map_err(|e| format!("Effect params invalid: {e}"))
}

/// Rebuild `AppState::project` from the event store (snapshot + events).
pub fn rebuild_project_from_store(state: &tauri::State<'_, AppState>) -> Result<(), String> {
    let (base_project, events_to_replay): (AppProject, Vec<super::events::ProjectEvent>) = {
        let store = state.event_store.lock().map_err(|e| e.to_string())?;
        let (_snap_idx, snap_opt, events) = store.replay_plan();

        let base = match snap_opt {
            Some(p) => snapshot_project(p),
            None => AppProject::new(),
        };

        (base, events.to_vec())
    };

    let ui_layout = lock_project(state)?.ui_layout.clone();
    let rebuilt = apply_events(state, base_project, &events_to_replay)?;
    let mut project = lock_project(state)?;
    *project = rebuilt;
    project.ui_layout = ui_layout;

    Ok(())
}

fn apply_track_created(project: &mut AppProject, payload: &Value) {
    let id = payload_str(payload, "id");
    if project.tracks.iter().any(|t| t.id == id) {
        return;
    }
    let mut track = empty_app_track(id, payload_str_default(payload, "name", "Track"));
    track.instrument = payload["instrument"].as_str().map(|s| s.to_string());
    project.tracks.push(track);
}

fn apply_track_deleted(project: &mut AppProject, payload: &Value) {
    let id = payload_str(payload, "id");
    project.tracks.retain(|t| t.id != id);
}

fn apply_track_updated(project: &mut AppProject, payload: &Value) {
    let id = payload_str(payload, "id");
    let Some(track) = find_track_mut(project, &id) else {
        return;
    };
    if let Some(name) = payload["name"].as_str() {
        track.name = name.to_string();
    }
    if let Some(vol) = payload["volume"].as_f64() {
        track.volume = vol as f32;
    }
    if let Some(pan) = payload["pan"].as_f64() {
        track.pan = pan as f32;
    }
    if let Some(color) = payload["color"].as_str() {
        track.color = Some(color.to_string());
    }
}

fn apply_track_muted(project: &mut AppProject, payload: &Value) {
    let id = payload_str(payload, "id");
    let Some(track) = find_track_mut(project, &id) else {
        return;
    };
    track.muted = payload["muted"].as_bool().unwrap_or(false);
}

fn apply_track_soloed(project: &mut AppProject, payload: &Value) {
    apply_exclusive_solo(
        project,
        &payload_str(payload, "id"),
        payload["solo"].as_bool().unwrap_or(false),
    );
}

fn apply_track_instrument_set(project: &mut AppProject, payload: &Value) {
    let id = payload_str(payload, "id");
    let Some(track) = find_track_mut(project, &id) else {
        return;
    };
    track.instrument = payload["instrument"].as_str().map(|s| s.to_string());
}

fn apply_track_input_set(project: &mut AppProject, payload: &Value) {
    let id = payload_str(payload, "id");
    let Some(track) = find_track_mut(project, &id) else {
        return;
    };
    track.input_source = serde_json::from_value(payload["input_source"].clone())
        .ok()
        .flatten();
}

fn apply_clip_added(state: &tauri::State<'_, AppState>, project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let clip_id = payload_str(payload, "clip_id");
    let file_path = payload_str(payload, "file_path");
    let start = payload["start"].as_f64().unwrap_or(0.0);
    let end = payload["end"].as_f64().unwrap_or(0.0);

    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    if track.clips.iter().any(|c| c.id == clip_id) {
        return;
    }
    let Some(data) = resolve_clip_audio(state, &clip_id, &file_path) else {
        return;
    };
    let mut clip = app_clip_from_audio(clip_id, start, end, file_path, &data);
    clip.offset = payload["offset"].as_f64().unwrap_or(0.0).max(0.0);
    track.clips.push(clip);
}

fn apply_clip_moved(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let clip_id = payload_str(payload, "clip_id");
    let start = payload["start"].as_f64().unwrap_or(0.0);
    let end = payload["end"].as_f64().unwrap_or(0.0);
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    let Some(clip) = track.clips.iter_mut().find(|c| c.id == clip_id) else {
        return;
    };
    clip.start = start;
    clip.end = end;
    clip.offset = payload["offset"].as_f64().unwrap_or(clip.offset);
}

fn apply_clip_moved_to_track(
    state: &tauri::State<'_, AppState>,
    project: &mut AppProject,
    payload: &Value,
) {
    let from_id = payload_str(payload, "from_track_id");
    let to_id = payload_str(payload, "to_track_id");
    let clip_id = payload_str(payload, "clip_id");
    let file_path = payload_str(payload, "file_path");
    let start = payload["start"].as_f64().unwrap_or(0.0);
    let end = payload["end"].as_f64().unwrap_or(0.0);
    let offset = payload["offset"].as_f64().unwrap_or(0.0);

    let src_idx = match project.tracks.iter().position(|t| t.id == from_id) {
        Some(i) => i,
        None => return,
    };
    let clip_idx = match project.tracks[src_idx]
        .clips
        .iter()
        .position(|c| c.id == clip_id)
    {
        Some(i) => i,
        None => {
            // Clip not in source — may need to recreate from file (cold replay after restart)
            let Some(data) = resolve_clip_audio(state, &clip_id, &file_path) else {
                return;
            };
            let clip = app_clip_from_audio(clip_id, start, end, file_path, &data);
            let mut clip = clip;
            clip.offset = offset;
            let dst_idx = match project.tracks.iter().position(|t| t.id == to_id) {
                Some(i) => i,
                None => return,
            };
            if !project.tracks[dst_idx]
                .clips
                .iter()
                .any(|c| c.id == clip.id)
            {
                project.tracks[dst_idx].clips.push(clip);
            }
            return;
        }
    };
    let mut clip = project.tracks[src_idx].clips.remove(clip_idx);
    clip.start = start;
    clip.end = end;
    clip.offset = offset;
    let dst_idx = match project.tracks.iter().position(|t| t.id == to_id) {
        Some(i) => i,
        None => return,
    };
    if !project.tracks[dst_idx]
        .clips
        .iter()
        .any(|c| c.id == clip.id)
    {
        project.tracks[dst_idx].clips.push(clip);
    }
}

fn apply_midi_clip_moved_to_track(project: &mut AppProject, payload: &Value) {
    let from_id = payload_str(payload, "from_track_id");
    let to_id = payload_str(payload, "to_track_id");
    let clip_val = &payload["clip"];
    let clip_id = clip_val["id"].as_str().unwrap_or("").to_string();

    let src_idx = match project.tracks.iter().position(|t| t.id == from_id) {
        Some(i) => i,
        None => return,
    };
    let clip_idx = match project.tracks[src_idx]
        .midi_clips
        .iter()
        .position(|c| c.id == clip_id)
    {
        Some(i) => i,
        None => return,
    };
    let mut clip = project.tracks[src_idx].midi_clips.remove(clip_idx);
    clip.start = clip_val["start"].as_f64().unwrap_or(clip.start);
    clip.end = clip_val["end"].as_f64().unwrap_or(clip.end);
    clip.offset = clip_val["offset"].as_f64().unwrap_or(clip.offset);
    let dst_idx = match project.tracks.iter().position(|t| t.id == to_id) {
        Some(i) => i,
        None => return,
    };
    if !project.tracks[dst_idx]
        .midi_clips
        .iter()
        .any(|c| c.id == clip.id)
    {
        project.tracks[dst_idx].midi_clips.push(clip);
    }
}

fn apply_clip_removed(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let clip_id = payload_str(payload, "clip_id");
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    track.clips.retain(|c| c.id != clip_id);
}

fn apply_project_name_updated(project: &mut AppProject, payload: &Value) {
    if let Some(name) = payload["name"].as_str() {
        project.name = name.to_string();
    }
}

fn apply_tempo_updated(project: &mut AppProject, payload: &Value) {
    if let Some(bpm) = payload["bpm"].as_f64() {
        project.bpm = bpm as f32;
    }
    if let Some(n) = payload["numerator"].as_u64() {
        project.numerator = n as u8;
    }
    if let Some(d) = payload["denominator"].as_u64() {
        project.denominator = d as u8;
    }
    if let Some(g) = payload["midi_grid_beats"].as_f64() {
        project.midi_grid_beats = g as f32;
    }
    if let Some(k) = payload["key_root"].as_str() {
        project.key_root = k.to_string();
    }
    if let Some(m) = payload["key_mode"].as_str() {
        project.key_mode = m.to_string();
    }
    if let Some(s) = payload["swing_percent"].as_u64() {
        project.swing_percent = (s as u8).min(100);
    }
    if let Some(p) = payload["midi_ppq"].as_u64() {
        project.midi_ppq = p as u16;
    }
    if let Some(t) = payload["midi_grid_use_triplets"].as_bool() {
        project.midi_grid_use_triplets = t;
    }
}

fn apply_effect_added(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let Some(raw_effect) = payload.get("effect") else {
        return;
    };
    let Ok(effect) = serde_json::from_value::<TrackEffect>(raw_effect.clone()) else {
        return;
    };
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    if !track.effects.iter().any(|e| e.id == effect.id) {
        track.effects.push(effect);
    }
}

fn apply_effect_removed(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let effect_id = payload_str(payload, "effect_id");
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    track.effects.retain(|e| e.id != effect_id);
}

fn apply_effect_updated(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let effect_id = payload_str(payload, "effect_id");
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    let Some(effect) = track.effects.iter_mut().find(|e| e.id == effect_id) else {
        return;
    };
    let Some(raw_effect) = payload.get("effect") else {
        return;
    };
    let Ok(updated) = serde_json::from_value::<TrackEffect>(raw_effect.clone()) else {
        return;
    };
    effect.enabled = updated.enabled;
    effect.params = updated.params;
}

fn apply_modulator_added(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let Some(raw) = payload.get("modulator") else {
        return;
    };
    let Ok(m) = serde_json::from_value::<crate::project::ProjectModulator>(raw.clone()) else {
        return;
    };
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    if !track.modulators.iter().any(|x| x.id == m.id) {
        track.modulators.push(m);
    }
}

fn apply_modulator_removed(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let mod_id = payload_str(payload, "modulator_id");
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    track.modulators.retain(|m| m.id != mod_id);
    track.mod_routes.retain(|r| r.modulator_id != mod_id);
}

fn apply_modulator_updated(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let mod_id = payload_str(payload, "modulator_id");
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    let Some(m) = track.modulators.iter_mut().find(|x| x.id == mod_id) else {
        return;
    };
    match &mut m.params {
        crate::project::ModulatorParams::Lfo(ref mut p) => {
            if let Some(rate) = payload["rate_hz"].as_f64() {
                p.rate_hz = rate as f32;
            }
            if let Ok(w) =
                serde_json::from_value::<crate::project::LfoWaveform>(payload["waveform"].clone())
            {
                p.waveform = w;
            }
            if let Some(v) = payload["tempo_sync"].as_bool() {
                p.tempo_sync = v;
            }
            if let Some(v) = payload["beat_division"].as_f64() {
                p.beat_division = v as f32;
            }
        }
        crate::project::ModulatorParams::Macro(ref mut p) => {
            if let Some(value) = payload["value"].as_f64() {
                p.value = value as f32;
            }
        }
        crate::project::ModulatorParams::Envelope(ref mut p) => {
            if let Some(v) = payload["attack_ms"].as_f64() {
                p.attack_ms = v as f32;
            }
            if let Some(v) = payload["decay_ms"].as_f64() {
                p.decay_ms = v as f32;
            }
            if let Some(v) = payload["sustain_level"].as_f64() {
                p.sustain_level = v as f32;
            }
            if let Some(v) = payload["release_ms"].as_f64() {
                p.release_ms = v as f32;
            }
        }
        crate::project::ModulatorParams::AudioFollower(ref mut p) => {
            if let Some(v) = payload["window_ms"].as_f64() {
                p.window_ms = v as f32;
            }
            if let Some(v) = payload["attack_ms"].as_f64() {
                p.attack_ms = v as f32;
            }
            if let Some(v) = payload["release_ms"].as_f64() {
                p.release_ms = v as f32;
            }
            if let Ok(mode) =
                serde_json::from_value::<crate::project::FollowerMode>(payload["mode"].clone())
            {
                p.mode = mode;
            }
            if let Some(source) = payload.get("source_effect_id") {
                p.source_effect_id = source.as_str().map(str::to_string);
            }
        }
    }
}

fn apply_modulation_route_added(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let Some(raw) = payload.get("route") else {
        return;
    };
    let Ok(route) = serde_json::from_value::<crate::project::ProjectModulationRoute>(raw.clone())
    else {
        return;
    };
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    if !track.mod_routes.iter().any(|r| r.id == route.id) {
        track.mod_routes.push(route);
    }
}

fn apply_modulation_route_removed(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let route_id = payload_str(payload, "route_id");
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    track.mod_routes.retain(|r| r.id != route_id);
}

fn apply_modulation_depth_changed(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let route_id = payload_str(payload, "route_id");
    let Some(depth) = payload["depth"].as_f64() else {
        return;
    };
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    let Some(route) = track.mod_routes.iter_mut().find(|r| r.id == route_id) else {
        return;
    };
    route.depth = depth as f32;
}

fn apply_midi_notes_set(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let Some(raw_notes) = payload.get("notes") else {
        return;
    };
    let Ok(notes) = serde_json::from_value::<Vec<MidiNote>>(raw_notes.clone()) else {
        return;
    };
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    track.midi_notes = notes;
}

fn apply_midi_clip_added(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let Some(raw_clip) = payload.get("clip") else {
        return;
    };
    let (Some(id), Some(start), Some(end)) = (
        raw_clip["id"].as_str().map(|s| s.to_string()),
        raw_clip["start"].as_f64(),
        raw_clip["end"].as_f64(),
    ) else {
        return;
    };
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    if track.midi_clips.iter().any(|c| c.id == id) {
        return;
    }
    let notes = raw_clip
        .get("notes")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    track.midi_clips.push(AppMidiClip {
        id,
        start,
        end,
        offset: raw_clip["offset"].as_f64().unwrap_or(0.0),
        notes,
    });
}

fn apply_midi_clip_removed(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let clip_id = payload_str(payload, "clip_id");
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    track.midi_clips.retain(|c| c.id != clip_id);
}

fn apply_midi_clip_moved(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let clip_id = payload_str(payload, "clip_id");
    let start = payload["start"].as_f64().unwrap_or(0.0);
    let end = payload["end"].as_f64().unwrap_or(0.0);
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    if let Ok(clip) = require_midi_clip_mut(track, &clip_id) {
        clip.start = start;
        clip.end = end;
        clip.offset = payload["offset"].as_f64().unwrap_or(clip.offset);
    }
}

fn apply_midi_clip_notes_set(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let clip_id = payload_str(payload, "clip_id");
    let Some(raw_notes) = payload.get("notes") else {
        return;
    };
    let Ok(notes) = serde_json::from_value::<Vec<MidiNote>>(raw_notes.clone()) else {
        return;
    };
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    if let Ok(clip) = require_midi_clip_mut(track, &clip_id) {
        clip.notes = notes;
    }
}

fn apply_automation_lane_added(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let Some(raw) = payload.get("lane") else {
        return;
    };
    let Ok(lane) = serde_json::from_value::<crate::project::AutomationLane>(raw.clone()) else {
        return;
    };
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    if !track.automation_lanes.iter().any(|l| l.id == lane.id) {
        track.automation_lanes.push(lane);
    }
}

fn apply_automation_lane_removed(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let lane_id = payload_str(payload, "lane_id");
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    track.automation_lanes.retain(|l| l.id != lane_id);
}

fn apply_automation_clip_updated(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let lane_id = payload_str(payload, "lane_id");
    let Some(raw) = payload.get("clip") else {
        return;
    };
    let Ok(clip) = serde_json::from_value::<crate::project::AutomationClip>(raw.clone()) else {
        return;
    };
    let Some(track) = find_track_mut(project, &track_id) else {
        return;
    };
    let Some(lane) = track.automation_lanes.iter_mut().find(|l| l.id == lane_id) else {
        return;
    };
    if let Some(existing) = lane.clips.iter_mut().find(|c| c.id == clip.id) {
        *existing = clip;
    } else {
        lane.clips.push(clip);
    }
}

fn apply_events(
    state: &tauri::State<'_, AppState>,
    mut project: AppProject,
    events: &[super::events::ProjectEvent],
) -> Result<AppProject, String> {
    use super::events::ev;

    for event in events {
        match event.event_type.as_str() {
            ev::TRACK_CREATED => apply_track_created(&mut project, &event.payload),
            ev::TRACK_DELETED => apply_track_deleted(&mut project, &event.payload),
            ev::TRACK_UPDATED => apply_track_updated(&mut project, &event.payload),
            ev::TRACK_MUTED => apply_track_muted(&mut project, &event.payload),
            ev::TRACK_SOLOED => apply_track_soloed(&mut project, &event.payload),
            ev::TRACK_INSTRUMENT_SET => apply_track_instrument_set(&mut project, &event.payload),
            ev::TRACK_INPUT_SET => apply_track_input_set(&mut project, &event.payload),
            ev::CLIP_ADDED => apply_clip_added(state, &mut project, &event.payload),
            ev::CLIP_MOVED => apply_clip_moved(&mut project, &event.payload),
            ev::CLIP_MOVED_TO_TRACK => {
                apply_clip_moved_to_track(state, &mut project, &event.payload)
            }
            ev::CLIP_REMOVED => apply_clip_removed(&mut project, &event.payload),
            ev::PROJECT_NAME_UPDATED => apply_project_name_updated(&mut project, &event.payload),
            ev::TEMPO_UPDATED => apply_tempo_updated(&mut project, &event.payload),
            ev::EFFECT_ADDED => apply_effect_added(&mut project, &event.payload),
            ev::EFFECT_REMOVED => apply_effect_removed(&mut project, &event.payload),
            ev::EFFECT_UPDATED => apply_effect_updated(&mut project, &event.payload),
            ev::MODULATOR_ADDED => apply_modulator_added(&mut project, &event.payload),
            ev::MODULATOR_REMOVED => apply_modulator_removed(&mut project, &event.payload),
            ev::MODULATOR_UPDATED => apply_modulator_updated(&mut project, &event.payload),
            ev::MODULATION_ROUTE_ADDED => {
                apply_modulation_route_added(&mut project, &event.payload)
            }
            ev::MODULATION_ROUTE_REMOVED => {
                apply_modulation_route_removed(&mut project, &event.payload)
            }
            ev::MODULATION_DEPTH_CHANGED => {
                apply_modulation_depth_changed(&mut project, &event.payload)
            }
            ev::AUTOMATION_LANE_ADDED => apply_automation_lane_added(&mut project, &event.payload),
            ev::AUTOMATION_LANE_REMOVED => {
                apply_automation_lane_removed(&mut project, &event.payload)
            }
            ev::AUTOMATION_CLIP_UPDATED => {
                apply_automation_clip_updated(&mut project, &event.payload)
            }
            ev::MIDI_NOTES_SET => apply_midi_notes_set(&mut project, &event.payload),
            ev::MIDI_CLIP_ADDED => apply_midi_clip_added(&mut project, &event.payload),
            ev::MIDI_CLIP_REMOVED => apply_midi_clip_removed(&mut project, &event.payload),
            ev::MIDI_CLIP_MOVED => apply_midi_clip_moved(&mut project, &event.payload),
            ev::MIDI_CLIP_MOVED_TO_TRACK => {
                apply_midi_clip_moved_to_track(&mut project, &event.payload)
            }
            ev::MIDI_CLIP_NOTES_SET => apply_midi_clip_notes_set(&mut project, &event.payload),
            _ => {}
        }
    }

    Ok(project)
}

/// Recompiles the audio graph, then tells the UI which plugins could not be
/// loaded.  Lock order: project → engine. Reversing causes AB-BA deadlock with
/// the offline render path.
pub fn recompile_if_needed(app: &tauri::AppHandle, state: &tauri::State<'_, AppState>) {
    let unavailable = match (state.project.lock(), state.audio_engine.lock()) {
        (Ok(project), Ok(mut engine)) => {
            engine.update_from_project(&project);
            Some(engine.plugin_host.unavailable_plugins())
        }
        _ => None,
    };

    match unavailable {
        Some(list) => {
            let _ = app.emit(PLUGIN_AVAILABILITY_EVENT, list);
        }
        None => eprintln!("[audio] Recompilation ignorée : verrous non disponibles"),
    }
}

pub const PLUGIN_AVAILABILITY_EVENT: &str = "plugin-availability";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{AppTrack, LfoParams, LfoWaveform, ModulatorParams, ProjectModulator};

    fn project_with_lfo() -> AppProject {
        let mut project = AppProject::new();
        project.tracks.push(AppTrack {
            id: "t".into(),
            name: "t".into(),
            volume: 80.0,
            pan: 0.0,
            muted: false,
            solo: false,
            clips: vec![],
            effects: vec![],
            modulators: vec![ProjectModulator {
                id: "lfo".into(),
                params: ModulatorParams::Lfo(LfoParams::default()),
            }],
            mod_routes: vec![],
            midi_notes: vec![],
            midi_clips: vec![],
            instrument: None,
            input_source: None,
            automation_lanes: vec![],
            color: None,
        });
        project
    }

    fn lfo_params(project: &AppProject) -> &LfoParams {
        match &project.tracks[0].modulators[0].params {
            ModulatorParams::Lfo(p) => p,
            other => panic!("expected an LFO, got {other:?}"),
        }
    }

    #[test]
    fn replayed_modulator_update_applies_waveform() {
        let mut project = project_with_lfo();
        apply_modulator_updated(
            &mut project,
            &json!({ "track_id": "t", "modulator_id": "lfo", "rate_hz": null, "waveform": "saw" }),
        );
        assert_eq!(lfo_params(&project).waveform, LfoWaveform::Saw);
        assert_eq!(lfo_params(&project).rate_hz, 1.0);
    }

    #[test]
    fn replayed_track_created_keeps_instrument() {
        let mut project = AppProject::new();
        apply_track_created(
            &mut project,
            &json!({ "id": "a", "name": "Vital", "instrument": "vital" }),
        );
        apply_track_created(&mut project, &json!({ "id": "b", "name": "Piste 2" }));
        assert_eq!(project.tracks[0].instrument.as_deref(), Some("vital"));
        assert_eq!(project.tracks[1].instrument, None);
    }

    #[test]
    fn replayed_midi_clip_added_keeps_notes_and_offset() {
        let mut project = project_with_lfo();
        apply_midi_clip_added(
            &mut project,
            &json!({
                "track_id": "t",
                "clip": {
                    "id": "c",
                    "start": 1.0,
                    "end": 3.0,
                    "offset": 0.25,
                    "notes": [{
                        "id": "n",
                        "pitch": 60,
                        "start": 0.0,
                        "duration": 1.0,
                        "velocity": 100
                    }]
                }
            }),
        );
        let clip = &project.tracks[0].midi_clips[0];
        assert_eq!(clip.offset, 0.25);
        assert_eq!(clip.notes.len(), 1);
        assert_eq!(clip.notes[0].pitch, 60);
        assert_eq!(clip.notes[0].id, "n");
    }

    #[test]
    fn replayed_follower_update_applies_mode_and_times() {
        let mut project = project_with_lfo();
        project.tracks[0].modulators[0].params =
            ModulatorParams::AudioFollower(crate::project::AudioFollowerParams::default());
        apply_modulator_updated(
            &mut project,
            &json!({ "track_id": "t", "modulator_id": "lfo", "mode": "peak", "window_ms": 20.0, "release_ms": null }),
        );
        let ModulatorParams::AudioFollower(p) = &project.tracks[0].modulators[0].params else {
            panic!("expected an audio follower");
        };
        assert_eq!(p.mode, crate::project::FollowerMode::Peak);
        assert_eq!(p.window_ms, 20.0);
        assert_eq!(
            p.release_ms,
            crate::project::AudioFollowerParams::default().release_ms
        );
    }

    #[test]
    fn replayed_follower_source_is_set_cleared_or_kept() {
        let mut project = project_with_lfo();
        project.tracks[0].modulators[0].params =
            ModulatorParams::AudioFollower(crate::project::AudioFollowerParams::default());
        let source = |project: &AppProject| match &project.tracks[0].modulators[0].params {
            ModulatorParams::AudioFollower(p) => p.source_effect_id.clone(),
            _ => panic!("expected an audio follower"),
        };

        apply_modulator_updated(
            &mut project,
            &json!({ "track_id": "t", "modulator_id": "lfo", "source_effect_id": "eq" }),
        );
        assert_eq!(source(&project).as_deref(), Some("eq"));

        apply_modulator_updated(
            &mut project,
            &json!({ "track_id": "t", "modulator_id": "lfo", "window_ms": 20.0 }),
        );
        assert_eq!(source(&project).as_deref(), Some("eq"));

        apply_modulator_updated(
            &mut project,
            &json!({ "track_id": "t", "modulator_id": "lfo", "source_effect_id": null }),
        );
        assert_eq!(source(&project), None);
    }

    #[test]
    fn update_modulator_payload_distinguishes_absent_and_null_source() {
        let parse = |value: Value| {
            serde_json::from_value::<UpdateModulatorPayload>(value)
                .unwrap()
                .source_effect_id
        };
        assert_eq!(parse(json!({ "track_id": "t", "modulator_id": "m" })), None);
        assert_eq!(
            parse(json!({ "track_id": "t", "modulator_id": "m", "source_effect_id": null })),
            Some(None)
        );
        assert_eq!(
            parse(json!({ "track_id": "t", "modulator_id": "m", "source_effect_id": "eq" })),
            Some(Some("eq".to_string()))
        );
    }

    #[test]
    fn replayed_rate_update_keeps_waveform() {
        let mut project = project_with_lfo();
        apply_modulator_updated(
            &mut project,
            &json!({ "track_id": "t", "modulator_id": "lfo", "waveform": "square" }),
        );
        apply_modulator_updated(
            &mut project,
            &json!({ "track_id": "t", "modulator_id": "lfo", "rate_hz": 3.0, "waveform": null }),
        );
        assert_eq!(lfo_params(&project).waveform, LfoWaveform::Square);
        assert_eq!(lfo_params(&project).rate_hz, 3.0);
    }
}
