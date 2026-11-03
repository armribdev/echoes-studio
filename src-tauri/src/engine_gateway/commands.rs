//! Typed command envelope and payload shapes.
//!
//! Every user action that mutates project state is expressed as a
//! `CommandEnvelope`. The gateway routes on `command_type` and deserialises
//! the typed payload for that branch.

use serde::{Deserialize, Serialize};

use crate::arpeggiator_settings::{ArpMode, ArpRate, ArpeggiatorSettings};
use crate::sampler_kit::{PadPlayMode, SamplerPad};

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

pub mod cmd {
    pub const PLAY: &str = "PLAY";
    pub const STOP: &str = "STOP";
    pub const PAUSE: &str = "PAUSE";
    pub const SEEK: &str = "SEEK";
    pub const SET_LOOP: &str = "SET_LOOP";

    pub const CREATE_TRACK: &str = "CREATE_TRACK";
    pub const DELETE_TRACK: &str = "DELETE_TRACK";
    pub const UPDATE_TRACK: &str = "UPDATE_TRACK";
    pub const MUTE_TRACK: &str = "MUTE_TRACK";
    pub const SOLO_TRACK: &str = "SOLO_TRACK";
    pub const SET_TRACK_INSTRUMENT: &str = "SET_TRACK_INSTRUMENT";
    pub const SET_TRACK_INPUT: &str = "SET_TRACK_INPUT";
    pub const SET_SAMPLER_PAD: &str = "SET_SAMPLER_PAD";
    pub const SET_ARPEGGIATOR: &str = "SET_ARPEGGIATOR";
    pub const ADD_CLIP: &str = "ADD_CLIP";
    pub const MOVE_CLIP: &str = "MOVE_CLIP";
    pub const REMOVE_CLIP: &str = "REMOVE_CLIP";
    pub const UPDATE_PROJECT_NAME: &str = "UPDATE_PROJECT_NAME";
    pub const UPDATE_TEMPO: &str = "UPDATE_TEMPO";

    pub const ADD_MARKER: &str = "ADD_MARKER";
    pub const UPDATE_MARKER: &str = "UPDATE_MARKER";
    pub const REMOVE_MARKER: &str = "REMOVE_MARKER";
    pub const ADD_REGION: &str = "ADD_REGION";
    pub const UPDATE_REGION: &str = "UPDATE_REGION";
    pub const REMOVE_REGION: &str = "REMOVE_REGION";

    pub const ADD_EFFECT: &str = "ADD_EFFECT";
    pub const REMOVE_EFFECT: &str = "REMOVE_EFFECT";
    pub const UPDATE_EFFECT: &str = "UPDATE_EFFECT";

    pub const ADD_AUTOMATION_LANE: &str = "ADD_AUTOMATION_LANE";
    pub const REMOVE_AUTOMATION_LANE: &str = "REMOVE_AUTOMATION_LANE";
    pub const UPDATE_AUTOMATION_CLIP: &str = "UPDATE_AUTOMATION_CLIP";

    pub const ADD_MODULATOR: &str = "ADD_MODULATOR";
    pub const REMOVE_MODULATOR: &str = "REMOVE_MODULATOR";
    pub const UPDATE_MODULATOR: &str = "UPDATE_MODULATOR";
    pub const ADD_MODULATION_ROUTE: &str = "ADD_MODULATION_ROUTE";
    pub const REMOVE_MODULATION_ROUTE: &str = "REMOVE_MODULATION_ROUTE";
    pub const SET_MODULATION_DEPTH: &str = "SET_MODULATION_DEPTH";

    pub const SET_MIDI_NOTES: &str = "SET_MIDI_NOTES";
    pub const ADD_MIDI_CLIP: &str = "ADD_MIDI_CLIP";
    pub const REMOVE_MIDI_CLIP: &str = "REMOVE_MIDI_CLIP";
    pub const UNLINK_MIDI_CLIP: &str = "UNLINK_MIDI_CLIP";
    pub const MOVE_MIDI_CLIP: &str = "MOVE_MIDI_CLIP";
    pub const SET_CLIP_MIDI_NOTES: &str = "SET_CLIP_MIDI_NOTES";
    pub const SET_CLIP_MIDI_CONTROLS: &str = "SET_CLIP_MIDI_CONTROLS";
}

// Each struct is deserialised from `CommandEnvelope::payload` for its matching
// command type. Keep these lean — no audio data here.

#[derive(Debug, Deserialize)]
pub struct CreateTrackPayload {
    pub name: String,
    #[serde(default)]
    pub instrument: Option<String>,
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
    pub pan: Option<f32>,
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MuteTrackPayload {
    pub track_id: String,
    pub muted: bool,
}

#[derive(Debug, Deserialize)]
pub struct SoloTrackPayload {
    pub track_id: String,
    pub solo: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetTrackInstrumentPayload {
    pub track_id: String,
    /// `None` → built-in PolySynth.  `Some(plugin_id)` → CLAP instrument (Phase 5c).
    pub instrument: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetSamplerPadPayload {
    pub track_id: String,
    pub pad: usize,
    pub patch: SamplerPadPatch,
}

/// Sampler pad settings to change (ADR-028); absent fields stay unchanged.
#[derive(Debug, Default, Deserialize)]
pub struct SamplerPadPatch {
    pub note: Option<u8>,
    /// Absent = unchanged, `null` = empty the pad.
    #[serde(default, deserialize_with = "deserialize_present")]
    pub sample_path: Option<Option<String>>,
    pub gain_db: Option<f32>,
    pub pan: Option<f32>,
    pub tune_semitones: Option<f32>,
    pub mode: Option<PadPlayMode>,
    pub attack_ms: Option<f32>,
    pub release_ms: Option<f32>,
    pub choke_group: Option<u8>,
}

impl SamplerPadPatch {
    /// The pad with this patch applied; a new sample path drops the old PCM.
    pub fn apply_to(&self, pad: &SamplerPad) -> SamplerPad {
        let mut next = pad.clone();
        if let Some(path) = &self.sample_path {
            next.sample_path = path.clone();
            next.sample = None;
        }
        next.note = self.note.unwrap_or(next.note);
        next.gain_db = self.gain_db.unwrap_or(next.gain_db);
        next.pan = self.pan.unwrap_or(next.pan);
        next.tune_semitones = self.tune_semitones.unwrap_or(next.tune_semitones);
        next.mode = self.mode.unwrap_or(next.mode);
        next.attack_ms = self.attack_ms.unwrap_or(next.attack_ms);
        next.release_ms = self.release_ms.unwrap_or(next.release_ms);
        next.choke_group = self.choke_group.unwrap_or(next.choke_group);
        next.clamped()
    }
}

#[derive(Debug, Deserialize)]
pub struct SetArpeggiatorPayload {
    pub track_id: String,
    pub patch: ArpeggiatorPatch,
}

/// Arpeggiator settings to change (ADR-030); absent fields stay unchanged.
#[derive(Debug, Default, Deserialize)]
pub struct ArpeggiatorPatch {
    pub enabled: Option<bool>,
    pub mode: Option<ArpMode>,
    pub rate: Option<ArpRate>,
    pub triplet: Option<bool>,
    pub gate_percent: Option<u8>,
    pub octaves: Option<u8>,
}

impl ArpeggiatorPatch {
    pub fn apply_to(&self, settings: &ArpeggiatorSettings) -> ArpeggiatorSettings {
        ArpeggiatorSettings {
            enabled: self.enabled.unwrap_or(settings.enabled),
            mode: self.mode.unwrap_or(settings.mode),
            rate: self.rate.unwrap_or(settings.rate),
            triplet: self.triplet.unwrap_or(settings.triplet),
            gate_percent: self.gate_percent.unwrap_or(settings.gate_percent),
            octaves: self.octaves.unwrap_or(settings.octaves),
        }
        .clamped()
    }
}

#[derive(Debug, Deserialize)]
pub struct SetTrackInputPayload {
    pub track_id: String,
    /// `None` → no plugin output feeds the track (ADR-014 multi-out).
    pub input_source: Option<crate::project::PluginOutputSource>,
}

#[derive(Debug, Deserialize)]
pub struct AddClipPayload {
    pub track_id: String,
    pub file_path: String,
    pub start: f64,
    pub end: f64,
    /// Source-file trim in seconds. `0` when omitted.
    #[serde(default)]
    pub offset: f64,
}

#[derive(Debug, Deserialize)]
pub struct MoveClipPayload {
    pub track_id: String,
    pub clip_id: String,
    pub start: f64,
    pub end: f64,
    /// Source-file trim offset in seconds. `None` on a plain move (offset
    /// preserved); `Some` on a left-edge resize.
    #[serde(default)]
    pub offset: Option<f64>,
    /// Destination track. `None` or same as `track_id` = same-track move.
    #[serde(default)]
    pub target_track_id: Option<String>,
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
    #[serde(default = "default_midi_grid_beats_cmd")]
    pub midi_grid_beats: f32,
    #[serde(default = "default_key_root_cmd")]
    pub key_root: String,
    #[serde(default = "default_key_mode_cmd")]
    pub key_mode: String,
    #[serde(default)]
    pub swing_percent: u8,
    #[serde(default = "default_midi_ppq_cmd")]
    pub midi_ppq: u16,
    #[serde(default)]
    pub midi_grid_use_triplets: bool,
}

fn default_midi_grid_beats_cmd() -> f32 {
    0.25
}

fn default_key_root_cmd() -> String {
    "C".to_string()
}

fn default_key_mode_cmd() -> String {
    "major".to_string()
}

fn default_midi_ppq_cmd() -> u16 {
    960
}

#[derive(Debug, Deserialize)]
pub struct SeekPayload {
    pub position: f64,
}

#[derive(Debug, Deserialize)]
pub struct SetLoopPayload {
    pub enabled: bool,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Deserialize)]
pub struct AddEffectPayload {
    pub track_id: String,
    pub kind: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveEffectPayload {
    pub track_id: String,
    pub effect_id: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateEffectPayload {
    pub track_id: String,
    pub effect_id: String,
    pub enabled: Option<bool>,
    pub params: Option<serde_json::Value>,
    /// Plugin effects only: run the plugin in a sandbox process (ADR-015).
    #[serde(default)]
    pub sandboxed: Option<bool>,
    /// Plugin effects only: track id sidechaining into this plugin's aux
    /// input (ADR-014 routing). Absent = unchanged, `null` = clear.
    #[serde(default, deserialize_with = "deserialize_present")]
    pub sidechain_source: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
pub struct AddModulatorPayload {
    pub track_id: String,
    /// `"lfo"` | `"macro"`
    pub kind: String,
    pub rate_hz: Option<f32>,
    pub value: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct RemoveModulatorPayload {
    pub track_id: String,
    pub modulator_id: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateModulatorPayload {
    pub track_id: String,
    pub modulator_id: String,
    /// LFO rate (Hz).
    pub rate_hz: Option<f32>,
    /// LFO shape — baked into the LFO node, so changing it recompiles the graph.
    pub waveform: Option<crate::project::LfoWaveform>,
    /// LFO tempo sync.
    pub tempo_sync: Option<bool>,
    /// LFO beats per cycle (used when tempo_sync is true).
    pub beat_division: Option<f32>,
    /// Macro value [0..1].
    pub value: Option<f32>,
    /// Envelope ADSR params (ms / normalised level).
    pub attack_ms: Option<f32>,
    pub decay_ms: Option<f32>,
    pub sustain_level: Option<f32>,
    pub release_ms: Option<f32>,
    /// AudioFollower window length (ms).
    pub window_ms: Option<f32>,
    /// AudioFollower detection mode.
    pub mode: Option<crate::project::FollowerMode>,
    /// AudioFollower tap: absent = unchanged, `null` = track post-FX output, id = after that effect.
    #[serde(default, deserialize_with = "deserialize_present")]
    pub source_effect_id: Option<Option<String>>,
}

/// Maps a present field (even `null`) to `Some`, so "absent" and "cleared" stay distinct.
fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
pub struct AddModulationRoutePayload {
    pub track_id: String,
    pub modulator_id: String,
    /// `"track_gain"` | `"effect_param"` | `"plugin_param"`
    pub target_kind: String,
    /// Required for `effect_param` / `plugin_param` (plugin placement key).
    pub effect_id: Option<String>,
    /// Parameter name (e.g. `"room_size"`), or the plugin `native_id` as text.
    pub param: String,
    pub depth: f32,
}

#[derive(Debug, Deserialize)]
pub struct RemoveModulationRoutePayload {
    pub track_id: String,
    pub route_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SetModulationDepthPayload {
    pub track_id: String,
    pub route_id: String,
    pub depth: f32,
}

#[derive(Debug, Deserialize)]
pub struct SetMidiNotesPayload {
    pub track_id: String,
    pub notes: Vec<crate::project::MidiNote>,
}

#[derive(Debug, Deserialize)]
pub struct AddMidiClipPayload {
    pub track_id: String,
    pub start: f64,
    pub end: f64,
    /// Left-trim in seconds. `0` when omitted.
    #[serde(default)]
    pub offset: f64,
    /// Seed notes (paste). Empty when omitted; ids are regenerated on insert.
    #[serde(default)]
    pub notes: Vec<crate::project::MidiNote>,
    /// Seed controls (paste, recording). Empty when omitted.
    #[serde(default)]
    pub channel_controls: Vec<crate::project::MidiControlPoint>,
    /// Plays an existing content (repeat handle) instead of a seeded one; it
    /// excludes `notes` and `channel_controls` (ADR-032).
    #[serde(default)]
    pub content_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UnlinkMidiClipPayload {
    pub track_id: String,
    pub clip_id: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveMidiClipPayload {
    pub track_id: String,
    pub clip_id: String,
}

#[derive(Debug, Deserialize)]
pub struct MoveMidiClipPayload {
    pub track_id: String,
    pub clip_id: String,
    pub start: f64,
    pub end: f64,
    /// Left-trim offset in seconds (content hidden past the left edge). `None`
    /// on a plain move (offset preserved); `Some` on a left-edge resize.
    #[serde(default)]
    pub offset: Option<f64>,
    /// Destination track. `None` or same as `track_id` = same-track move.
    #[serde(default)]
    pub target_track_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetClipMidiNotesPayload {
    pub track_id: String,
    pub clip_id: String,
    pub notes: Vec<crate::project::MidiNote>,
}

#[derive(Debug, Deserialize)]
pub struct SetClipMidiControlsPayload {
    pub track_id: String,
    pub clip_id: String,
    pub channel_controls: Vec<crate::project::MidiControlPoint>,
}

#[derive(Debug, Deserialize)]
pub struct AddAutomationLanePayload {
    pub track_id: String,
    /// `"track_gain"` | `"effect_param"` | `"plugin_param"`
    pub target_kind: String,
    pub effect_id: Option<String>,
    /// Parameter name or native_id string (for plugin_param).
    pub param: String,
    #[serde(default)]
    pub interpolation: crate::project::InterpolationMode,
}

#[derive(Debug, Deserialize)]
pub struct RemoveAutomationLanePayload {
    pub track_id: String,
    pub lane_id: String,
}

/// Replace or create one clip within an automation lane.
#[derive(Debug, Deserialize)]
pub struct UpdateAutomationClipPayload {
    pub track_id: String,
    pub lane_id: String,
    pub clip: crate::project::AutomationClip,
}

#[derive(Debug, Deserialize)]
pub struct AddMarkerPayload {
    pub position_beats: f64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMarkerPayload {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub position_beats: Option<f64>,
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RemoveMarkerPayload {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct AddRegionPayload {
    pub start_beats: f64,
    pub end_beats: f64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRegionPayload {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub start_beats: Option<f64>,
    #[serde(default)]
    pub end_beats: Option<f64>,
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RemoveRegionPayload {
    pub id: String,
}
