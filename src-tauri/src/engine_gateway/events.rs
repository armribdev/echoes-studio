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
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::project::generate_id;

/// Channel count for output/input peak arrays in telemetry payloads.
pub const METER_CHANNEL_COUNT: usize = 16;

pub mod ev {
    // Project events — persisted in EventStore
    pub const TRACK_CREATED: &str = "TRACK_CREATED";
    pub const TRACK_DELETED: &str = "TRACK_DELETED";
    pub const TRACK_UPDATED: &str = "TRACK_UPDATED";
    pub const TRACK_MUTED: &str = "TRACK_MUTED";
    pub const TRACK_SOLOED: &str = "TRACK_SOLOED";
    pub const TRACK_INSTRUMENT_SET: &str = "TRACK_INSTRUMENT_SET";
    pub const TRACK_INPUT_SET: &str = "TRACK_INPUT_SET";
    pub const SAMPLER_PAD_SET: &str = "SAMPLER_PAD_SET";
    pub const ARPEGGIATOR_SET: &str = "ARPEGGIATOR_SET";
    pub const CLIP_ADDED: &str = "CLIP_ADDED";
    pub const CLIP_MOVED: &str = "CLIP_MOVED";
    pub const CLIP_MOVED_TO_TRACK: &str = "CLIP_MOVED_TO_TRACK";
    pub const CLIP_REMOVED: &str = "CLIP_REMOVED";
    pub const PROJECT_NAME_UPDATED: &str = "PROJECT_NAME_UPDATED";
    pub const TEMPO_UPDATED: &str = "TEMPO_UPDATED";
    pub const PROJECT_RESET: &str = "PROJECT_RESET";

    pub const MARKER_ADDED: &str = "MARKER_ADDED";
    pub const MARKER_UPDATED: &str = "MARKER_UPDATED";
    pub const MARKER_REMOVED: &str = "MARKER_REMOVED";
    pub const REGION_ADDED: &str = "REGION_ADDED";
    pub const REGION_UPDATED: &str = "REGION_UPDATED";
    pub const REGION_REMOVED: &str = "REGION_REMOVED";

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

    // Automation — persisted
    pub const AUTOMATION_LANE_ADDED: &str = "AUTOMATION_LANE_ADDED";
    pub const AUTOMATION_LANE_REMOVED: &str = "AUTOMATION_LANE_REMOVED";
    pub const AUTOMATION_LANE_UPDATED: &str = "AUTOMATION_LANE_UPDATED";
    pub const AUTOMATION_CLIP_UPDATED: &str = "AUTOMATION_CLIP_UPDATED";

    // MIDI — persisted
    pub const MIDI_NOTES_SET: &str = "MIDI_NOTES_SET";
    pub const MIDI_CLIP_ADDED: &str = "MIDI_CLIP_ADDED";
    pub const MIDI_CLIP_REMOVED: &str = "MIDI_CLIP_REMOVED";
    pub const MIDI_CLIP_MOVED: &str = "MIDI_CLIP_MOVED";
    pub const MIDI_CLIP_MOVED_TO_TRACK: &str = "MIDI_CLIP_MOVED_TO_TRACK";
    pub const MIDI_CLIP_UNLINKED: &str = "MIDI_CLIP_UNLINKED";
    pub const MIDI_CONTENT_NOTES_SET: &str = "MIDI_CONTENT_NOTES_SET";
    pub const MIDI_CONTENT_CONTROLS_SET: &str = "MIDI_CONTENT_CONTROLS_SET";

    // Transport runtime events — NOT persisted
    pub const PLAY_STARTED: &str = "PLAY_STARTED";
    pub const PLAY_STOPPED: &str = "PLAY_STOPPED";
    pub const POSITION_UPDATED: &str = "POSITION_UPDATED";
    pub const LEVELS_UPDATED: &str = "LEVELS_UPDATED";

    // Audio feedback — NOT persisted. Mirrors the runtime-event filter in the frontend.
    #[allow(dead_code)]
    pub const PEAK_METER: &str = "PEAK_METER";
    #[allow(dead_code)]
    pub const CLIP_PLAYING: &str = "CLIP_PLAYING";
    #[allow(dead_code)]
    pub const LATENCY_WARNING: &str = "LATENCY_WARNING";
}

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
        Self {
            id: generate_id(),
            event_type: event_type.to_string(),
            timestamp: now_ms(),
            payload,
        }
    }
}

/// Peak levels emitted while transport is stopped (`LEVELS_UPDATED`) or piggybacked
/// on `POSITION_UPDATED` while playing.
#[derive(Debug, Clone, Serialize)]
pub struct LevelsUpdatedPayload {
    pub output: [f32; METER_CHANNEL_COUNT],
    pub input: [f32; METER_CHANNEL_COUNT],
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub tracks: HashMap<String, [f32; 2]>,
    pub xruns: u64,
}

/// Transport position plus meter telemetry (`POSITION_UPDATED`).
#[derive(Debug, Clone, Serialize)]
pub struct PositionUpdatedPayload {
    pub position: f64,
    pub output: [f32; METER_CHANNEL_COUNT],
    pub input: [f32; METER_CHANNEL_COUNT],
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub tracks: HashMap<String, [f32; 2]>,
    pub xruns: u64,
}

/// Lightweight engine-event envelope for the ~30 Hz reporter thread.
///
/// Uses `&'static str` for `id` / `project_id` so each tick does not allocate
/// envelope strings. Serializes to the same JSON shape as [`EngineEvent`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryEngineEvent<P: Serialize> {
    pub id: &'static str,
    pub project_id: &'static str,
    #[serde(rename = "type")]
    pub event_type: &'static str,
    pub timestamp: u64,
    pub payload: P,
}

/// A message from the engine to the frontend. Project events are wrapped in
/// this envelope before being emitted so the frontend has full context
/// (project_id, sequence number).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
        Self::with_payload(event_type, project_id, payload)
    }

    /// Build an engine event from any serializable payload (avoids `json!` macros).
    pub fn with_payload<P: Serialize>(event_type: &str, project_id: &str, payload: P) -> Self {
        Self {
            id: generate_id(),
            project_id: project_id.to_string(),
            event_type: event_type.to_string(),
            timestamp: now_ms(),
            payload: serde_json::to_value(payload).unwrap_or(serde_json::Value::Null),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_event_serializes_camel_case_envelope() {
        let payload = LevelsUpdatedPayload {
            output: [0.0; METER_CHANNEL_COUNT],
            input: [0.0; METER_CHANNEL_COUNT],
            tracks: HashMap::new(),
            xruns: 0,
        };
        let ev = TelemetryEngineEvent {
            id: "levels",
            project_id: "",
            event_type: ev::LEVELS_UPDATED,
            timestamp: 0,
            payload,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["id"], "levels");
        assert_eq!(json["projectId"], "");
        assert_eq!(json["type"], ev::LEVELS_UPDATED);
        assert!(json["payload"]["output"].is_array());
        assert_eq!(json["payload"]["output"].as_array().unwrap().len(), 16);
    }
}
