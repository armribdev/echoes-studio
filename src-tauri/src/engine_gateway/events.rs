//! Engine event types emitted by the gateway to the frontend.
//!
//! Two distinct categories:
//!
//! - **Project events** — persisted in the event store. These are the source of
//!   truth for all project-structural state (tracks, clips, routing, metadata).
//!   Replaying them deterministically reconstructs the project.
//!
//! - **Runtime events** — transient signals (transport, meters, latency warnings).
//!   They are NOT persisted and carry no undo/redo semantics.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::project::generate_id;

// ── Event type constants ──────────────────────────────────────────────────────

pub mod ev {
    // Project events — persisted in EventStore
    pub const TRACK_CREATED: &str = "TRACK_CREATED";
    pub const TRACK_DELETED: &str = "TRACK_DELETED";
    pub const TRACK_UPDATED: &str = "TRACK_UPDATED";
    pub const TRACK_MUTED: &str = "TRACK_MUTED";
    pub const TRACK_SOLOED: &str = "TRACK_SOLOED";
    pub const CLIP_ADDED: &str = "CLIP_ADDED";
    pub const CLIP_MOVED: &str = "CLIP_MOVED";
    pub const CLIP_REMOVED: &str = "CLIP_REMOVED";
    pub const PROJECT_NAME_UPDATED: &str = "PROJECT_NAME_UPDATED";
    pub const TEMPO_UPDATED: &str = "TEMPO_UPDATED";
    pub const PROJECT_RESET: &str = "PROJECT_RESET";
    pub const EFFECT_ADDED: &str = "EFFECT_ADDED";
    pub const EFFECT_REMOVED: &str = "EFFECT_REMOVED";
    pub const EFFECT_UPDATED: &str = "EFFECT_UPDATED";

    // Modulation — persisted
    pub const MODULATOR_ADDED: &str = "MODULATOR_ADDED";
    pub const MODULATOR_REMOVED: &str = "MODULATOR_REMOVED";
    pub const MODULATOR_UPDATED: &str = "MODULATOR_UPDATED";
    pub const MODULATION_ROUTE_ADDED: &str = "MODULATION_ROUTE_ADDED";
    pub const MODULATION_ROUTE_REMOVED: &str = "MODULATION_ROUTE_REMOVED";
    pub const MODULATION_DEPTH_CHANGED: &str = "MODULATION_DEPTH_CHANGED";

    // MIDI — persisted
    pub const MIDI_NOTES_SET: &str = "MIDI_NOTES_SET";
    pub const MIDI_CLIP_ADDED: &str = "MIDI_CLIP_ADDED";
    pub const MIDI_CLIP_REMOVED: &str = "MIDI_CLIP_REMOVED";
    pub const MIDI_CLIP_MOVED: &str = "MIDI_CLIP_MOVED";
    pub const MIDI_CLIP_NOTES_SET: &str = "MIDI_CLIP_NOTES_SET";

    // Transport runtime events — NOT persisted
    pub const PLAY_STARTED: &str = "PLAY_STARTED";
    pub const PLAY_STOPPED: &str = "PLAY_STOPPED";
    pub const POSITION_UPDATED: &str = "POSITION_UPDATED";

    // Audio feedback — NOT persisted
    pub const PEAK_METER: &str = "PEAK_METER";
    pub const CLIP_PLAYING: &str = "CLIP_PLAYING";
    pub const LATENCY_WARNING: &str = "LATENCY_WARNING";
}

// ── Project event (stored in EventStore) ─────────────────────────────────────

/// A structural project change. These events are appended to the event store
/// and can be replayed to reconstruct any past project state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEvent {
    /// Unique event ID.
    pub id: String,
    /// Event discriminant — see `ev::*` constants.
    #[serde(rename = "type")]
    pub event_type: String,
    /// Unix timestamp in milliseconds.
    pub timestamp: u64,
    /// Event-specific data (no audio samples — see AudioDataCache for that).
    pub payload: serde_json::Value,
}

impl ProjectEvent {
    pub fn new(event_type: &str, payload: serde_json::Value) -> Self {
        ProjectEvent {
            id: generate_id(),
            event_type: event_type.to_string(),
            timestamp: now_ms(),
            payload,
        }
    }
}

// ── Engine event (sent to UI via Tauri emit) ──────────────────────────────────

/// A message from the engine to the frontend. Project events are wrapped in
/// this envelope before being emitted so the frontend has full context
/// (project_id, sequence number).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineEvent {
    /// Unique ID for this emission.
    pub id: String,
    /// Project this event belongs to.
    pub project_id: String,
    /// Event discriminant — same constants as ProjectEvent / runtime ev::*.
    #[serde(rename = "type")]
    pub event_type: String,
    /// Unix timestamp in milliseconds.
    pub timestamp: u64,
    /// Full event payload (for project events: same as ProjectEvent::payload).
    pub payload: serde_json::Value,
}

impl EngineEvent {
    pub fn new(event_type: &str, project_id: &str, payload: serde_json::Value) -> Self {
        EngineEvent {
            id: generate_id(),
            project_id: project_id.to_string(),
            event_type: event_type.to_string(),
            timestamp: now_ms(),
            payload,
        }
    }

    /// Wrap a stored ProjectEvent as an EngineEvent for emission to the UI.
    pub fn from_project_event(project_event: &ProjectEvent, project_id: &str) -> Self {
        EngineEvent {
            id: generate_id(),
            project_id: project_id.to_string(),
            event_type: project_event.event_type.clone(),
            timestamp: project_event.timestamp,
            payload: project_event.payload.clone(),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
