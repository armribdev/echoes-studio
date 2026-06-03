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
use tauri::Emitter;

use super::commands::{
    cmd, AddClipPayload, AddEffectPayload, AddMidiClipPayload, AddModulationRoutePayload,
    AddModulatorPayload, BatchCommand, CommandEnvelope, CreateTrackPayload, DeleteTrackPayload,
    MoveClipPayload, MoveMidiClipPayload, MuteTrackPayload, RemoveClipPayload, RemoveEffectPayload,
    RemoveMidiClipPayload, RemoveModulationRoutePayload, RemoveModulatorPayload, SeekPayload,
    SetClipMidiNotesPayload, SetMidiNotesPayload, SetModulationDepthPayload,
    SetTrackInstrumentPayload, SoloTrackPayload, UpdateEffectPayload, UpdateModulatorPayload,
    UpdateProjectNamePayload, UpdateTempoPayload, UpdateTrackPayload,
};
use crate::plugin_system::wrapper::DeviceFormat;
use super::events::{ev, EngineEvent, ProjectEvent};
use crate::audio_clip::{AudioClip, AudioClipData};
use crate::audio_graph::GraphSnapshotDto;
use crate::project::{
    generate_id, AppClip, AppMidiClip, AppProject, AppState, AppTrack, CachedAudioData,
    CompressorParams, DelayParams, EffectParams, EqParams, MidiClipDto, MidiNote, PluginParams,
    ReverbParams, TrackEffect,
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

    recompile_if_needed(&state);
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

fn with_project_mut<'a, R, F>(
    state: &'a tauri::State<'a, AppState>,
    f: F,
) -> Result<R, String>
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
    find_track_mut(project, track_id)
        .ok_or_else(|| format!("Track {} not found", track_id))
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
    }
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
    recompile_if_needed(state);

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
        recompile_if_needed(ctx.state);
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
    recompile_if_needed(ctx.state);
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
    })
}

fn insert_cached_clip(
    state: &tauri::State<'_, AppState>,
    clip_id: &str,
    cached: Arc<CachedAudioData>,
) -> Arc<CachedAudioData> {
    state
        .audio_cache
        .insert(clip_id.to_string(), Arc::clone(&cached));
    cached
}

fn cache_audio_clip(
    state: &tauri::State<'_, AppState>,
    clip_id: &str,
    file_path: &str,
) -> Result<Arc<CachedAudioData>, String> {
    let audio = AudioClip::from_file(file_path)
        .map_err(|e| format!("Failed to load audio '{file_path}': {e}"))?;
    Ok(insert_cached_clip(
        state,
        clip_id,
        cached_audio_from_clip_data(audio.into_clip_data()),
    ))
}

fn resolve_clip_audio(
    state: &tauri::State<'_, AppState>,
    clip_id: &str,
    file_path: &str,
) -> Option<Arc<CachedAudioData>> {
    if let Some(cached) = state.audio_cache.get(clip_id) {
        return Some(cached);
    }
    match AudioClip::from_file(file_path) {
        Ok(audio) => Some(insert_cached_clip(
            state,
            clip_id,
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
        HotEffectUpdate::Plugin(device_id, params) => {
            if let Ok(engine) = state.audio_engine.lock() {
                let mut all_ok = !params.is_empty();
                for (native_id, value) in params {
                    match engine
                        .plugin_host
                        .push_plugin_param(device_id, *native_id, *value)
                    {
                        Ok(_) => {}
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
    enabled: Option<bool>,
    params: Option<serde_json::Value>,
) -> Result<(HotEffectUpdate, bool), String> {
    let mut structural = false;
    if let Some(en) = enabled {
        if effect.enabled != en {
            structural = true;
        }
        effect.enabled = en;
    }
    if let Some(raw) = params {
        effect.params = merge_effect_params(&effect.params, raw)?;
    }
    Ok((hot_effect_update_from(effect), structural))
}

fn build_modulator(p: &AddModulatorPayload) -> Result<crate::project::ProjectModulator, String> {
    use crate::project::{LfoParams, ModulatorParams, ProjectModulator};
    let params = match p.kind.as_str() {
        "lfo" => ModulatorParams::Lfo(LfoParams {
            rate_hz: p.rate_hz.unwrap_or(1.0),
            ..Default::default()
        }),
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
        other => return Err(format!("Unknown target kind: {other}")),
    };
    Ok(ProjectModulationRoute {
        id: generate_id(),
        modulator_id: p.modulator_id.clone(),
        target,
        depth: p.depth,
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
            state.transport.stop();
            let ev_stop = EngineEvent::new(ev::PLAY_STOPPED, &project_id, json!({}));
            let ev_pos = EngineEvent::new(
                ev::POSITION_UPDATED,
                &project_id,
                json!({ "position": 0.0 }),
            );
            emit_engine_event(app, &ev_stop);
            emit_engine_event(app, &ev_pos);
            Ok(ok_transport_events(ctx.command_id.clone(), &[ev_stop, ev_pos]))
        }

        cmd::PAUSE => {
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

        cmd::CREATE_TRACK => {
            let p: CreateTrackPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("CREATE_TRACK payload invalid: {e}"))?;
            let track_id = generate_id();

            with_project_mut(state, |project| {
                project
                    .tracks
                    .push(empty_app_track(track_id.clone(), p.name.clone()));
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::TRACK_CREATED,
                json!({ "id": track_id, "name": p.name }),
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

            commit_same_payload(
                &ctx,
                ev::TRACK_DELETED,
                json!({ "id": p.track_id }),
                true,
            )
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
                Ok(())
            })?;

            // Hot-path: live atomics updated directly, no recompile.
            if let Some(vol) = p.volume {
                let linear = (vol / 100.0).clamp(0.0, 2.0);
                state.live_params.set_track_volume(&p.track_id, linear);
            }
            if let Some(pan) = p.pan {
                state.live_params.set_track_pan(&p.track_id, pan);
            }

            commit_same_payload(
                &ctx,
                ev::TRACK_UPDATED,
                json!({
                    "id": p.track_id,
                    "name": p.name,
                    "volume": p.volume,
                    "pan": p.pan,
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

        cmd::ADD_CLIP => {
            let p: AddClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("ADD_CLIP payload invalid: {e}"))?;

            let clip_id = generate_id();
            let cached = cache_audio_clip(state, &clip_id, &p.file_path)?;
            let clip = app_clip_from_audio(
                clip_id.clone(),
                p.start,
                p.end,
                p.file_path.clone(),
                &cached,
            );
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
                    "end": p.end,
                }),
                json!({ "track_id": p.track_id, "clip": clip_dto }),
                true,
            )
        }

        cmd::MOVE_CLIP => {
            let p: MoveClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("MOVE_CLIP payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                let track = require_track_mut(project, &p.track_id)?;
                let clip = require_audio_clip_mut(track, &p.clip_id)?;
                clip.start = p.start;
                clip.end = p.end;
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::CLIP_MOVED,
                json!({
                    "track_id": p.track_id,
                    "clip_id": p.clip_id,
                    "start": p.start,
                    "end": p.end,
                }),
                true,
            )
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

            // Keep the CPAL transport BPM in sync (used by nodes for bar-aligned
            // operations such as looping — currently informational).
            state.transport.set_bpm(p.bpm);

            with_project_mut(state, |project| {
                project.bpm = p.bpm;
                project.numerator = p.numerator;
                project.denominator = p.denominator;
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::TEMPO_UPDATED,
                json!({ "bpm": p.bpm, "numerator": p.numerator, "denominator": p.denominator }),
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
            let (updated_effect, hot, structural) = with_project_mut(state, |project| {
                let track = require_track_mut(project, &p.track_id)?;
                let effect = require_effect_mut(track, &p.effect_id)?;
                let (hot, structural) = patch_effect(effect, p.enabled, p.params)?;
                Ok((
                    serde_json::to_value(&*effect).unwrap_or_default(),
                    hot,
                    structural,
                ))
            })?;

            let event_payload =
                json!({ "track_id": p.track_id, "effect_id": p.effect_id, "effect": updated_effect });
            let engine_ev = snapshot_emit(&ctx, ev::EFFECT_UPDATED, event_payload)?;

            if !structural && !apply_hot_effect_update(state, &p.effect_id, &hot) {
                recompile_if_needed(state);
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
                track.mod_routes.retain(|r| r.modulator_id != p.modulator_id);
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
            if let Some(rate) = p.rate_hz {
                with_project_mut(state, |project| {
                    let track = require_track_mut(project, &p.track_id)?;
                    if let Some(m) = track.modulators.iter_mut().find(|x| x.id == p.modulator_id) {
                        let crate::project::ModulatorParams::Lfo(ref mut lp) = m.params;
                        lp.rate_hz = rate;
                    }
                    Ok(())
                })?;
            }
            // Hot-path: update the LFO rate atomic, no recompile.
            if let Some(rate) = p.rate_hz {
                state.live_params.set_modulator_rate(&p.modulator_id, rate);
            }
            commit_same_payload(
                &ctx,
                ev::MODULATOR_UPDATED,
                json!({
                    "track_id": p.track_id,
                    "modulator_id": p.modulator_id,
                    "rate_hz": p.rate_hz,
                }),
                false,
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
            let clip = AppMidiClip {
                id: clip_id.clone(),
                start: p.start,
                end: p.end,
                notes: vec![],
            };
            let clip_dto = MidiClipDto {
                id: clip.id.clone(),
                start: clip.start,
                end: clip.end,
                notes: vec![],
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

            with_project_mut(state, |project| {
                let track = require_track_mut(project, &p.track_id)?;
                let clip = require_midi_clip_mut(track, &p.clip_id)?;
                clip.start = p.start;
                clip.end = p.end;
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::MIDI_CLIP_MOVED,
                json!({
                    "track_id": p.track_id,
                    "clip_id": p.clip_id,
                    "start": p.start,
                    "end": p.end,
                }),
                true,
            )
        }

        cmd::SET_CLIP_MIDI_NOTES => {
            let p: SetClipMidiNotesPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("SET_CLIP_MIDI_NOTES payload invalid: {e}"))?;

            with_project_mut(state, |project| {
                let track = require_track_mut(project, &p.track_id)?;
                require_midi_clip_mut(track, &p.clip_id)?.notes = p.notes.clone();
                Ok(())
            })?;

            commit_same_payload(
                &ctx,
                ev::MIDI_CLIP_NOTES_SET,
                json!({
                    "track_id": p.track_id,
                    "clip_id": p.clip_id,
                    "notes": serde_json::to_value(&p.notes).unwrap_or_default(),
                }),
                true,
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
        clips: t.clips.iter().map(snapshot_clip).collect(),
    }
}

/// Shallow project copy for snapshotting — audio data stays in `AudioDataCache`.
fn snapshot_project(project: &AppProject) -> AppProject {
    AppProject {
        name: project.name.clone(),
        bpm: project.bpm,
        numerator: project.numerator,
        denominator: project.denominator,
        tracks: project.tracks.iter().map(snapshot_track).collect(),
        // Plugin binary state mirrors live plugin internals and is restored
        // separately (project load → compile → setState), not via the event
        // store.  Undo/redo snapshots therefore keep this empty.
        plugin_state_blobs: std::collections::HashMap::new(),
    }
}

fn plugin_id_from_effect_kind(kind: &str) -> Option<&str> {
    kind.strip_prefix("plugin:")?.splitn(3, '|').next()
}

fn format_tag(fmt: DeviceFormat) -> &'static str {
    match fmt {
        DeviceFormat::Vst3 => "VST3",
        DeviceFormat::Clap => "CLAP",
        DeviceFormat::Au => "AU",
        DeviceFormat::Internal => "interne",
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

    let rebuilt = apply_events(state, base_project, &events_to_replay)?;
    *lock_project(state)? = rebuilt;

    Ok(())
}

fn apply_track_created(project: &mut AppProject, payload: &Value) {
    let id = payload_str(payload, "id");
    if project.tracks.iter().any(|t| t.id == id) {
        return;
    }
    project.tracks.push(empty_app_track(
        id,
        payload_str_default(payload, "name", "Track"),
    ));
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
    track.instrument = payload["instrument"]
        .as_str()
        .map(|s| s.to_string());
}

fn apply_clip_added(
    state: &tauri::State<'_, AppState>,
    project: &mut AppProject,
    payload: &Value,
) {
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
    track.clips.push(app_clip_from_audio(
        clip_id,
        start,
        end,
        file_path,
        &data,
    ));
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
    if let Some(rate) = payload["rate_hz"].as_f64() {
        let crate::project::ModulatorParams::Lfo(ref mut p) = m.params;
        p.rate_hz = rate as f32;
    }
}

fn apply_modulation_route_added(project: &mut AppProject, payload: &Value) {
    let track_id = payload_str(payload, "track_id");
    let Some(raw) = payload.get("route") else {
        return;
    };
    let Ok(route) =
        serde_json::from_value::<crate::project::ProjectModulationRoute>(raw.clone())
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
    track.midi_clips.push(AppMidiClip {
        id,
        start,
        end,
        notes: vec![],
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
            ev::CLIP_ADDED => apply_clip_added(state, &mut project, &event.payload),
            ev::CLIP_MOVED => apply_clip_moved(&mut project, &event.payload),
            ev::CLIP_REMOVED => apply_clip_removed(&mut project, &event.payload),
            ev::PROJECT_NAME_UPDATED => {
                apply_project_name_updated(&mut project, &event.payload)
            }
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
            ev::MIDI_NOTES_SET => apply_midi_notes_set(&mut project, &event.payload),
            ev::MIDI_CLIP_ADDED => apply_midi_clip_added(&mut project, &event.payload),
            ev::MIDI_CLIP_REMOVED => apply_midi_clip_removed(&mut project, &event.payload),
            ev::MIDI_CLIP_MOVED => apply_midi_clip_moved(&mut project, &event.payload),
            ev::MIDI_CLIP_NOTES_SET => apply_midi_clip_notes_set(&mut project, &event.payload),
            _ => {}
        }
    }

    Ok(project)
}

/// Lock order: project → engine. Reversing causes AB-BA deadlock with the offline render path.
fn recompile_if_needed(state: &tauri::State<'_, AppState>) {
    let compile_done = if let Ok(project) = state.project.lock() {
        if let Ok(mut engine) = state.audio_engine.lock() {
            engine.update_from_project(&project);
            true
        } else {
            false
        }
    } else {
        false
    };

    if !compile_done {
        eprintln!("[audio] Recompilation ignorée : verrous non disponibles");
    }
}
