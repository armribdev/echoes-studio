//! Typed command envelope and payload shapes.
//!
//! Every user action that mutates project state is expressed as a
//! `CommandEnvelope`. The gateway routes on `command_type` and deserialises
//! the typed payload for that branch.

use serde::{Deserialize, Serialize};

// ── Envelope ──────────────────────────────────────────────────────────────────

/// The outer wrapper for every command sent from the UI.
///
/// `rename_all = "camelCase"` bridges TypeScript (camelCase) ↔ Rust (snake_case).
/// The explicit `rename = "type"` on `command_type` takes precedence over
/// `rename_all`, so the JSON key stays `"type"` as the TypeScript side expects.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandEnvelope {
    /// Caller-generated UUID (used for request-response correlation).
    pub id: String,
    /// ID of the project this command targets.
    pub project_id: String,
    /// Discriminant string — see `cmd::*` constants.
    #[serde(rename = "type")]
    pub command_type: String,
    /// Unix timestamp in milliseconds (for ordering / debugging).
    pub timestamp: u64,
    /// Command-specific data; deserialized per `command_type`.
    pub payload: serde_json::Value,
}

/// Batch wrapper — groups multiple commands into a single IPC round trip
/// so they are applied atomically and emit a single recompile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCommand {
    pub commands: Vec<CommandEnvelope>,
}

// ── Command type constants ────────────────────────────────────────────────────

pub mod cmd {
    // ── Transport ─────────────────────────────────────────────────────────────
    pub const PLAY: &str = "PLAY";
    pub const STOP: &str = "STOP";
    pub const PAUSE: &str = "PAUSE";
    pub const SEEK: &str = "SEEK";

    // ── Project structure (event-sourced) ─────────────────────────────────────
    pub const CREATE_TRACK: &str = "CREATE_TRACK";
    pub const DELETE_TRACK: &str = "DELETE_TRACK";
    pub const UPDATE_TRACK: &str = "UPDATE_TRACK";
    pub const ADD_CLIP: &str = "ADD_CLIP";
    pub const MOVE_CLIP: &str = "MOVE_CLIP";
    pub const REMOVE_CLIP: &str = "REMOVE_CLIP";
    pub const UPDATE_PROJECT_NAME: &str = "UPDATE_PROJECT_NAME";
    pub const UPDATE_TEMPO: &str = "UPDATE_TEMPO";

    // ── Audio graph (structural routing) ──────────────────────────────────────
    pub const ADD_NODE: &str = "ADD_NODE";
    pub const REMOVE_NODE: &str = "REMOVE_NODE";
    pub const CONNECT_NODES: &str = "CONNECT_NODES";
}

// ── Typed payload structs ─────────────────────────────────────────────────────
//
// Each struct is deserialised from `CommandEnvelope::payload` for its matching
// command type. Keep these lean — no audio data here.

#[derive(Debug, Deserialize)]
pub struct CreateTrackPayload {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteTrackPayload {
    pub track_id: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTrackPayload {
    pub track_id: String,
    pub name: Option<String>,
    pub volume: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct AddClipPayload {
    pub track_id: String,
    pub file_path: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Deserialize)]
pub struct MoveClipPayload {
    pub track_id: String,
    pub clip_id: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Deserialize)]
pub struct RemoveClipPayload {
    pub track_id: String,
    pub clip_id: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProjectNamePayload {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTempoPayload {
    pub bpm: f32,
    pub numerator: u8,
    pub denominator: u8,
}

#[derive(Debug, Deserialize)]
pub struct SeekPayload {
    pub position: f64,
}
