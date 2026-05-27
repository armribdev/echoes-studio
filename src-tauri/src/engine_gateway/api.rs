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
use serde_json::json;
use std::sync::Arc;
use tauri::Emitter;

use crate::audio_clip::AudioClip;
use crate::audio_engine::graph::AudioGraphCompiler;
use crate::project::{AppClip, AppProject, AppState, AppTrack, CachedAudioData, generate_id};
use super::commands::{
    cmd, AddClipPayload, BatchCommand, CommandEnvelope, CreateTrackPayload, DeleteTrackPayload,
    MoveClipPayload, RemoveClipPayload, SeekPayload, UpdateProjectNamePayload, UpdateTempoPayload,
    UpdateTrackPayload,
};
use super::events::{ev, EngineEvent, ProjectEvent};

// ── Command result ────────────────────────────────────────────────────────────

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
        CommandResult { success: true, command_id, events, error: None }
    }
    fn err(command_id: String, msg: String) -> Self {
        CommandResult { success: false, command_id, events: vec![], error: Some(msg) }
    }
}

// ── Public Tauri commands ─────────────────────────────────────────────────────

/// Send a single typed command to the engine.
#[tauri::command]
pub fn send_command(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    command: CommandEnvelope,
) -> Result<CommandResult, String> {
    let result = process_command(&app, &state, command.clone())
        .unwrap_or_else(|e| CommandResult::err(command.id, e));
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
            process_command(&app, &state, cmd)
                .unwrap_or_else(|e| CommandResult::err(id, e))
        })
        .collect();

    // After all commands, do a single graph recompile
    recompile_if_needed(&state);

    Ok(results)
}

/// Return a full project snapshot (DTO) for initial UI hydration or after
/// undo/redo.
#[tauri::command]
pub fn get_snapshot(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let project = state.project.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(project.to_dto()).map_err(|e| e.to_string())
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

    if !moved {
        let project = state.project.lock().map_err(|e| e.to_string())?;
        return serde_json::to_value(project.to_dto()).map_err(|e| e.to_string());
    }

    rebuild_project_from_store(&state)?;
    recompile_if_needed(&state);

    let project = state.project.lock().map_err(|e| e.to_string())?;
    let dto = project.to_dto();
    let snapshot_val = serde_json::to_value(&dto).map_err(|e| e.to_string())?;

    // Notify the UI that the entire project was replaced
    let engine_event = EngineEvent::new(ev::PROJECT_RESET, "", snapshot_val.clone());
    let _ = app.emit("engine-event", &engine_event);

    Ok(snapshot_val)
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

    if !moved {
        let project = state.project.lock().map_err(|e| e.to_string())?;
        return serde_json::to_value(project.to_dto()).map_err(|e| e.to_string());
    }

    rebuild_project_from_store(&state)?;
    recompile_if_needed(&state);

    let project = state.project.lock().map_err(|e| e.to_string())?;
    let dto = project.to_dto();
    let snapshot_val = serde_json::to_value(&dto).map_err(|e| e.to_string())?;

    let engine_event = EngineEvent::new(ev::PROJECT_RESET, "", snapshot_val.clone());
    let _ = app.emit("engine-event", &engine_event);

    Ok(snapshot_val)
}

// ── Command dispatch ──────────────────────────────────────────────────────────

fn process_command(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    command: CommandEnvelope,
) -> Result<CommandResult, String> {
    let command_id = command.id.clone();
    let project_id = command.project_id.clone();

    match command.command_type.as_str() {
        // ── Transport (runtime, not persisted) ────────────────────────────────
        cmd::PLAY => {
            let ev = EngineEvent::new(ev::PLAY_STARTED, &project_id, json!({}));
            let _ = app.emit("engine-event", &ev);
            Ok(CommandResult::ok(command_id, vec![serde_json::to_value(&ev).unwrap_or_default()]))
        }

        cmd::STOP => {
            let ev = EngineEvent::new(ev::PLAY_STOPPED, &project_id, json!({}));
            let _ = app.emit("engine-event", &ev);
            Ok(CommandResult::ok(command_id, vec![serde_json::to_value(&ev).unwrap_or_default()]))
        }

        cmd::PAUSE => {
            let ev = EngineEvent::new(ev::PLAY_STOPPED, &project_id, json!({ "pause": true }));
            let _ = app.emit("engine-event", &ev);
            Ok(CommandResult::ok(command_id, vec![serde_json::to_value(&ev).unwrap_or_default()]))
        }

        cmd::SEEK => {
            let p: SeekPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("SEEK payload invalid: {e}"))?;
            let ev = EngineEvent::new(
                ev::POSITION_UPDATED,
                &project_id,
                json!({ "position": p.position }),
            );
            let _ = app.emit("engine-event", &ev);
            Ok(CommandResult::ok(command_id, vec![serde_json::to_value(&ev).unwrap_or_default()]))
        }

        // ── Project structural commands (event-sourced) ───────────────────────
        cmd::CREATE_TRACK => {
            let p: CreateTrackPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("CREATE_TRACK payload invalid: {e}"))?;
            let track_id = generate_id();

            // 1. Mutate project
            {
                let mut project = state.project.lock().map_err(|e| e.to_string())?;
                project.tracks.push(AppTrack {
                    id: track_id.clone(),
                    name: p.name.clone(),
                    volume: 80.0,
                    clips: vec![],
                });
            }

            // 2. Persist event
            let event_payload = json!({ "id": track_id, "name": p.name });
            let project_event = ProjectEvent::new(ev::TRACK_CREATED, event_payload.clone());
            append_and_maybe_snapshot(state, project_event)?;

            // 3. Emit to UI
            let engine_ev = EngineEvent::new(ev::TRACK_CREATED, &project_id, event_payload);
            let _ = app.emit("engine-event", &engine_ev);

            // 4. Structural change → recompile
            recompile_if_needed(state);

            Ok(CommandResult::ok(
                command_id,
                vec![serde_json::to_value(&engine_ev).unwrap_or_default()],
            ))
        }

        cmd::DELETE_TRACK => {
            let p: DeleteTrackPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("DELETE_TRACK payload invalid: {e}"))?;

            {
                let mut project = state.project.lock().map_err(|e| e.to_string())?;
                project.tracks.retain(|t| t.id != p.track_id);
            }

            let event_payload = json!({ "id": p.track_id });
            let project_event = ProjectEvent::new(ev::TRACK_DELETED, event_payload.clone());
            append_and_maybe_snapshot(state, project_event)?;

            let engine_ev = EngineEvent::new(ev::TRACK_DELETED, &project_id, event_payload);
            let _ = app.emit("engine-event", &engine_ev);
            recompile_if_needed(state);

            Ok(CommandResult::ok(
                command_id,
                vec![serde_json::to_value(&engine_ev).unwrap_or_default()],
            ))
        }

        cmd::UPDATE_TRACK => {
            let p: UpdateTrackPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("UPDATE_TRACK payload invalid: {e}"))?;

            {
                let mut project = state.project.lock().map_err(|e| e.to_string())?;
                let track = project
                    .tracks
                    .iter_mut()
                    .find(|t| t.id == p.track_id)
                    .ok_or_else(|| format!("Track {} not found", p.track_id))?;
                if let Some(name) = p.name.as_ref() {
                    track.name = name.clone();
                }
                if let Some(vol) = p.volume {
                    track.volume = vol;
                }
            }

            let event_payload = json!({
                "id": p.track_id,
                "name": p.name,
                "volume": p.volume,
            });
            let project_event = ProjectEvent::new(ev::TRACK_UPDATED, event_payload.clone());
            append_and_maybe_snapshot(state, project_event)?;

            let engine_ev = EngineEvent::new(ev::TRACK_UPDATED, &project_id, event_payload);
            let _ = app.emit("engine-event", &engine_ev);
            // Volume change → recompile for audio engine
            recompile_if_needed(state);

            Ok(CommandResult::ok(
                command_id,
                vec![serde_json::to_value(&engine_ev).unwrap_or_default()],
            ))
        }

        cmd::ADD_CLIP => {
            let p: AddClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("ADD_CLIP payload invalid: {e}"))?;

            // Load audio (may block — acceptable for non-realtime path)
            let audio = AudioClip::from_wav_file(&p.file_path)
                .map_err(|e| format!("Failed to load audio '{}': {e}", p.file_path))?;
            let data = audio.into_clip_data();
            let clip_id = generate_id();

            // Cache audio data for future undo/redo replay
            let cached = Arc::new(CachedAudioData {
                raw_samples: data.raw_samples.clone(),
                sample_rate: data.sample_rate,
                peaks_64: data.peaks_64.clone(),
                peaks_512: data.peaks_512.clone(),
                peaks_4096: data.peaks_4096.clone(),
            });
            state.audio_cache.insert(clip_id.clone(), cached);

            let clip = AppClip {
                id: clip_id.clone(),
                start: p.start,
                end: p.end,
                file_path: p.file_path.clone(),
                raw_samples: data.raw_samples,
                sample_rate: data.sample_rate,
                peaks_64: data.peaks_64,
                peaks_512: data.peaks_512,
                peaks_4096: data.peaks_4096,
            };
            let clip_dto = clip.to_dto();

            {
                let mut project = state.project.lock().map_err(|e| e.to_string())?;
                let track = project
                    .tracks
                    .iter_mut()
                    .find(|t| t.id == p.track_id)
                    .ok_or_else(|| format!("Track {} not found", p.track_id))?;
                track.clips.push(clip);
            }

            // Persist structural event (no audio data — that lives in the cache)
            let event_payload = json!({
                "track_id": p.track_id,
                "clip_id": clip_id,
                "file_path": p.file_path,
                "start": p.start,
                "end": p.end,
            });
            let project_event = ProjectEvent::new(ev::CLIP_ADDED, event_payload.clone());
            append_and_maybe_snapshot(state, project_event)?;

            // Emit full ClipDto to UI (includes samples for waveform rendering)
            let engine_payload = json!({
                "track_id": p.track_id,
                "clip": clip_dto,
            });
            let engine_ev = EngineEvent::new(ev::CLIP_ADDED, &project_id, engine_payload);
            let _ = app.emit("engine-event", &engine_ev);
            recompile_if_needed(state);

            Ok(CommandResult::ok(
                command_id,
                vec![serde_json::to_value(&engine_ev).unwrap_or_default()],
            ))
        }

        cmd::MOVE_CLIP => {
            let p: MoveClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("MOVE_CLIP payload invalid: {e}"))?;

            {
                let mut project = state.project.lock().map_err(|e| e.to_string())?;
                let track = project
                    .tracks
                    .iter_mut()
                    .find(|t| t.id == p.track_id)
                    .ok_or_else(|| format!("Track {} not found", p.track_id))?;
                let clip = track
                    .clips
                    .iter_mut()
                    .find(|c| c.id == p.clip_id)
                    .ok_or_else(|| format!("Clip {} not found", p.clip_id))?;
                clip.start = p.start;
                clip.end = p.end;
            }

            let event_payload = json!({
                "track_id": p.track_id,
                "clip_id": p.clip_id,
                "start": p.start,
                "end": p.end,
            });
            let project_event = ProjectEvent::new(ev::CLIP_MOVED, event_payload.clone());
            append_and_maybe_snapshot(state, project_event)?;

            let engine_ev = EngineEvent::new(ev::CLIP_MOVED, &project_id, event_payload);
            let _ = app.emit("engine-event", &engine_ev);
            recompile_if_needed(state);

            Ok(CommandResult::ok(
                command_id,
                vec![serde_json::to_value(&engine_ev).unwrap_or_default()],
            ))
        }

        cmd::REMOVE_CLIP => {
            let p: RemoveClipPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("REMOVE_CLIP payload invalid: {e}"))?;

            {
                let mut project = state.project.lock().map_err(|e| e.to_string())?;
                let track = project
                    .tracks
                    .iter_mut()
                    .find(|t| t.id == p.track_id)
                    .ok_or_else(|| format!("Track {} not found", p.track_id))?;
                track.clips.retain(|c| c.id != p.clip_id);
            }

            let event_payload = json!({
                "track_id": p.track_id,
                "clip_id": p.clip_id,
            });
            let project_event = ProjectEvent::new(ev::CLIP_REMOVED, event_payload.clone());
            append_and_maybe_snapshot(state, project_event)?;

            let engine_ev = EngineEvent::new(ev::CLIP_REMOVED, &project_id, event_payload);
            let _ = app.emit("engine-event", &engine_ev);
            recompile_if_needed(state);

            Ok(CommandResult::ok(
                command_id,
                vec![serde_json::to_value(&engine_ev).unwrap_or_default()],
            ))
        }

        cmd::UPDATE_PROJECT_NAME => {
            let p: UpdateProjectNamePayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("UPDATE_PROJECT_NAME payload invalid: {e}"))?;

            {
                let mut project = state.project.lock().map_err(|e| e.to_string())?;
                project.name = p.name.clone();
            }

            let event_payload = json!({ "name": p.name });
            let project_event =
                ProjectEvent::new(ev::PROJECT_NAME_UPDATED, event_payload.clone());
            append_and_maybe_snapshot(state, project_event)?;

            let engine_ev =
                EngineEvent::new(ev::PROJECT_NAME_UPDATED, &project_id, event_payload);
            let _ = app.emit("engine-event", &engine_ev);

            Ok(CommandResult::ok(
                command_id,
                vec![serde_json::to_value(&engine_ev).unwrap_or_default()],
            ))
        }

        cmd::UPDATE_TEMPO => {
            let p: UpdateTempoPayload = serde_json::from_value(command.payload)
                .map_err(|e| format!("UPDATE_TEMPO payload invalid: {e}"))?;

            {
                let mut project = state.project.lock().map_err(|e| e.to_string())?;
                project.bpm = p.bpm;
                project.numerator = p.numerator;
                project.denominator = p.denominator;
            }

            let event_payload =
                json!({ "bpm": p.bpm, "numerator": p.numerator, "denominator": p.denominator });
            let project_event = ProjectEvent::new(ev::TEMPO_UPDATED, event_payload.clone());
            append_and_maybe_snapshot(state, project_event)?;

            let engine_ev = EngineEvent::new(ev::TEMPO_UPDATED, &project_id, event_payload);
            let _ = app.emit("engine-event", &engine_ev);

            Ok(CommandResult::ok(
                command_id,
                vec![serde_json::to_value(&engine_ev).unwrap_or_default()],
            ))
        }

        other => Err(format!("Unknown command type: {other}")),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn append_and_maybe_snapshot(
    state: &tauri::State<'_, AppState>,
    event: ProjectEvent,
) -> Result<(), String> {
    let mut store = state.event_store.lock().map_err(|e| e.to_string())?;
    store.append(event);

    if store.should_snapshot() {
        // Snapshot the current project state for fast future replay
        drop(store); // release event_store lock before taking project lock
        let project_snapshot = {
            let project = state.project.lock().map_err(|e| e.to_string())?;
            // We can't move out of a MutexGuard, so we reconstruct from DTO
            // This is a deep clone — acceptable at snapshot intervals
            snapshot_project(&project)
        };
        let mut store = state.event_store.lock().map_err(|e| e.to_string())?;
        store.take_snapshot(project_snapshot);
    }

    Ok(())
}

/// Public entry point for `main.rs` to seed the event store when loading a
/// project from disk.
pub fn snapshot_project_for_store(project: &AppProject) -> AppProject {
    snapshot_project(project)
}

/// Build a lightweight AppProject copy for snapshotting.
/// Audio data is NOT cloned into the snapshot — it lives in AudioDataCache.
/// During replay, the cache is consulted to avoid disk reads.
fn snapshot_project(project: &AppProject) -> AppProject {
    AppProject {
        name: project.name.clone(),
        bpm: project.bpm,
        numerator: project.numerator,
        denominator: project.denominator,
        tracks: project
            .tracks
            .iter()
            .map(|t| crate::project::AppTrack {
                id: t.id.clone(),
                name: t.name.clone(),
                volume: t.volume,
                clips: t
                    .clips
                    .iter()
                    .map(|c| AppClip {
                        id: c.id.clone(),
                        start: c.start,
                        end: c.end,
                        file_path: c.file_path.clone(),
                        raw_samples: c.raw_samples.clone(),
                        sample_rate: c.sample_rate,
                        peaks_64: c.peaks_64.clone(),
                        peaks_512: c.peaks_512.clone(),
                        peaks_4096: c.peaks_4096.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// Rebuild `AppState::project` from the event store (snapshot + events).
/// Called after undo/redo.
pub fn rebuild_project_from_store(
    state: &tauri::State<'_, AppState>,
) -> Result<(), String> {
    // Collect what we need while holding the store lock
    let (base_project, events_to_replay): (AppProject, Vec<super::events::ProjectEvent>) = {
        let store = state.event_store.lock().map_err(|e| e.to_string())?;
        let (_snap_idx, snap_opt, events) = store.replay_plan();

        let base = match snap_opt {
            Some(p) => snapshot_project(p),
            None => AppProject::new(),
        };

        (base, events.to_vec())
    };

    // Apply events to the base project
    let rebuilt = apply_events(state, base_project, &events_to_replay)?;

    // Write back
    let mut project = state.project.lock().map_err(|e| e.to_string())?;
    *project = rebuilt;

    Ok(())
}

/// Apply a sequence of project events to a base project, using the audio cache
/// to avoid re-reading audio files.
fn apply_events(
    state: &tauri::State<'_, AppState>,
    mut project: AppProject,
    events: &[super::events::ProjectEvent],
) -> Result<AppProject, String> {
    use super::events::ev;

    for event in events {
        match event.event_type.as_str() {
            ev::TRACK_CREATED => {
                let id = event.payload["id"].as_str().unwrap_or("").to_string();
                let name =
                    event.payload["name"].as_str().unwrap_or("Track").to_string();
                // Only add if not already present (idempotent)
                if !project.tracks.iter().any(|t| t.id == id) {
                    project.tracks.push(AppTrack {
                        id,
                        name,
                        volume: 80.0,
                        clips: vec![],
                    });
                }
            }

            ev::TRACK_DELETED => {
                let id = event.payload["id"].as_str().unwrap_or("");
                project.tracks.retain(|t| t.id != id);
            }

            ev::TRACK_UPDATED => {
                let id = event.payload["id"].as_str().unwrap_or("");
                if let Some(track) = project.tracks.iter_mut().find(|t| t.id == id) {
                    if let Some(name) = event.payload["name"].as_str() {
                        track.name = name.to_string();
                    }
                    if let Some(vol) = event.payload["volume"].as_f64() {
                        track.volume = vol as f32;
                    }
                }
            }

            ev::CLIP_ADDED => {
                let track_id = event.payload["track_id"].as_str().unwrap_or("");
                let clip_id = event.payload["clip_id"].as_str().unwrap_or("").to_string();
                let file_path = event.payload["file_path"].as_str().unwrap_or("").to_string();
                let start = event.payload["start"].as_f64().unwrap_or(0.0);
                let end = event.payload["end"].as_f64().unwrap_or(0.0);

                if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id) {
                    // Skip if clip already present (idempotent replay)
                    if track.clips.iter().any(|c| c.id == clip_id) {
                        continue;
                    }

                    // Try cache first (avoids disk read on undo/redo)
                    let audio_data = if let Some(cached) = state.audio_cache.get(&clip_id) {
                        Some(cached)
                    } else {
                        // Cache miss — load from disk and cache
                        match AudioClip::from_wav_file(&file_path) {
                            Ok(audio) => {
                                let data = audio.into_clip_data();
                                let cached = Arc::new(CachedAudioData {
                                    raw_samples: data.raw_samples,
                                    sample_rate: data.sample_rate,
                                    peaks_64: data.peaks_64,
                                    peaks_512: data.peaks_512,
                                    peaks_4096: data.peaks_4096,
                                });
                                state.audio_cache.insert(clip_id.clone(), Arc::clone(&cached));
                                Some(cached)
                            }
                            Err(e) => {
                                eprintln!("Warning: could not load '{}': {e}", file_path);
                                None
                            }
                        }
                    };

                    if let Some(data) = audio_data {
                        track.clips.push(AppClip {
                            id: clip_id,
                            start,
                            end,
                            file_path,
                            raw_samples: data.raw_samples.clone(),
                            sample_rate: data.sample_rate,
                            peaks_64: data.peaks_64.clone(),
                            peaks_512: data.peaks_512.clone(),
                            peaks_4096: data.peaks_4096.clone(),
                        });
                    }
                }
            }

            ev::CLIP_MOVED => {
                let track_id = event.payload["track_id"].as_str().unwrap_or("");
                let clip_id = event.payload["clip_id"].as_str().unwrap_or("");
                let start = event.payload["start"].as_f64().unwrap_or(0.0);
                let end = event.payload["end"].as_f64().unwrap_or(0.0);

                if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id) {
                    if let Some(clip) = track.clips.iter_mut().find(|c| c.id == clip_id) {
                        clip.start = start;
                        clip.end = end;
                    }
                }
            }

            ev::CLIP_REMOVED => {
                let track_id = event.payload["track_id"].as_str().unwrap_or("");
                let clip_id = event.payload["clip_id"].as_str().unwrap_or("");
                if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id) {
                    track.clips.retain(|c| c.id != clip_id);
                }
            }

            ev::PROJECT_NAME_UPDATED => {
                if let Some(name) = event.payload["name"].as_str() {
                    project.name = name.to_string();
                }
            }

            ev::TEMPO_UPDATED => {
                if let Some(bpm) = event.payload["bpm"].as_f64() {
                    project.bpm = bpm as f32;
                }
                if let Some(n) = event.payload["numerator"].as_u64() {
                    project.numerator = n as u8;
                }
                if let Some(d) = event.payload["denominator"].as_u64() {
                    project.denominator = d as u8;
                }
            }

            _ => {} // Unknown events are ignored (forward-compat)
        }
    }

    Ok(project)
}

/// Trigger audio graph recompilation if needed.
/// Must be called outside of any Mutex locks to avoid deadlocks.
fn recompile_if_needed(state: &tauri::State<'_, AppState>) {
    let snapshot = {
        match state.project.lock() {
            Ok(project) => AudioGraphCompiler::compile(&project),
            Err(_) => return,
        }
    };

    if let Ok(mut engine) = state.audio_engine.lock() {
        engine.update_snapshot(snapshot);
    }
}
