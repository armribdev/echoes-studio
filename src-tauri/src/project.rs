use crate::audio_clip::AudioClip;
use crate::sampler_kit::{decode_pad_sample, SamplerKit, SamplerKitDto, SAMPLER_INSTRUMENT_ID};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqParams {
    pub low_gain_db: f32,
    pub low_freq: f32,
    pub mid_gain_db: f32,
    pub mid_freq: f32,
    pub mid_q: f32,
    pub high_gain_db: f32,
    pub high_freq: f32,
}

impl Default for EqParams {
    fn default() -> Self {
        Self {
            low_gain_db: 0.0,
            low_freq: 250.0,
            mid_gain_db: 0.0,
            mid_freq: 1000.0,
            mid_q: 1.0,
            high_gain_db: 0.0,
            high_freq: 4000.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32,
    pub knee_db: f32,
}

impl Default for CompressorParams {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_db: 0.0,
            knee_db: 6.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReverbParams {
    pub room_size: f32,
    pub damping: f32,
    pub wet: f32,
    pub dry: f32,
    pub width: f32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self {
            room_size: 0.5,
            damping: 0.5,
            wet: 0.33,
            dry: 1.0,
            width: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelayParams {
    pub delay_ms: f32,
    pub feedback: f32,
    pub wet: f32,
    pub dry: f32,
    pub ping_pong: bool,
}

impl Default for DelayParams {
    fn default() -> Self {
        Self {
            delay_ms: 250.0,
            feedback: 0.4,
            wet: 0.3,
            dry: 1.0,
            ping_pong: false,
        }
    }
}

/// Parameters for an external plugin effect (VST3 or CLAP).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginParams {
    /// Stable plugin identifier: `"vst3:<32-hex-cid>"` or `"clap:<plugin-id>"`.
    pub plugin_id: String,
    /// Human-readable name cached at insert time (so the UI doesn't need registry lookups).
    pub name: String,
    /// `"vst3"` | `"clap"` — shown as a badge in the UI.
    pub format: String,
    /// Normalised parameter values `(param_id, value ∈ [0,1])` persisted across compiles.
    #[serde(default)]
    pub params: Vec<(u32, f32)>,
    /// Run the plugin in its own `plugin_host_bridge` process (ADR-015); CLAP
    /// only, ignored for other formats.  Defaults to `false` (C-9), so older
    /// projects load in-process, unchanged (C-4).
    #[serde(default)]
    pub sandboxed: bool,
    /// Track id whose post-fader signal feeds this plugin's first auxiliary
    /// (sidechain) input bus (ADR-014 routing follow-up). `None` for every
    /// project saved before this field existed, so the graph stays
    /// bit-identical (C-4). Silently has no audible effect if the plugin
    /// declares no aux bus, the track doesn't exist, or the reference would
    /// create a routing cycle — see `AudioGraph::from_project`.
    #[serde(default)]
    pub sidechain_source: Option<String>,
}

/// An auxiliary output of a multi-out plugin, taken as a track's audio input
/// (ADR-014 multi-out).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginOutputSource {
    /// Placement of the plugin: an effect id, or `instrument:{track_id}`.
    pub effect_id: String,
    /// Plugin output port, `1..` — port 0 is the main output, which already
    /// flows down the plugin's own track.
    pub port: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "params", rename_all = "snake_case")]
pub enum EffectParams {
    Eq(EqParams),
    Compressor(CompressorParams),
    Reverb(ReverbParams),
    Delay(DelayParams),
    Plugin(PluginParams),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackEffect {
    pub id: String,
    pub enabled: bool,
    pub params: EffectParams,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LfoWaveform {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LfoParams {
    #[serde(default = "default_lfo_rate")]
    pub rate_hz: f32,
    #[serde(default)]
    pub waveform: LfoWaveform,
    /// When true, `beat_division` governs rate; `rate_hz` is ignored.
    #[serde(default)]
    pub tempo_sync: bool,
    /// Number of beats per LFO cycle.  `1.0` = one cycle per beat,
    /// `4.0` = one cycle per bar (4/4), `0.5` = two cycles per beat.
    #[serde(default = "default_lfo_beat_division")]
    pub beat_division: f32,
}

fn default_lfo_rate() -> f32 {
    1.0
}

fn default_lfo_beat_division() -> f32 {
    1.0
}

impl Default for LfoParams {
    fn default() -> Self {
        Self {
            rate_hz: 1.0,
            waveform: LfoWaveform::Sine,
            tempo_sync: false,
            beat_division: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroParams {
    #[serde(default)]
    pub value: f32,
}

impl Default for MacroParams {
    fn default() -> Self {
        Self { value: 0.0 }
    }
}

/// ADSR envelope modulator parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeParams {
    #[serde(default = "default_env_attack_ms")]
    pub attack_ms: f32,
    #[serde(default = "default_env_decay_ms")]
    pub decay_ms: f32,
    /// Sustain level [0..1].
    #[serde(default = "default_env_sustain")]
    pub sustain_level: f32,
    #[serde(default = "default_env_release_ms")]
    pub release_ms: f32,
}

fn default_env_attack_ms() -> f32 {
    10.0
}
fn default_env_decay_ms() -> f32 {
    100.0
}
fn default_env_sustain() -> f32 {
    0.7
}
fn default_env_release_ms() -> f32 {
    200.0
}

impl Default for EnvelopeParams {
    fn default() -> Self {
        Self {
            attack_ms: default_env_attack_ms(),
            decay_ms: default_env_decay_ms(),
            sustain_level: default_env_sustain(),
            release_ms: default_env_release_ms(),
        }
    }
}

/// Detection mode for the audio follower modulator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FollowerMode {
    /// Root-mean-square amplitude over the sliding window.
    #[default]
    Rms,
    /// Mean absolute value over the sliding window.
    Peak,
}

/// Parameters for the audio-follower modulator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFollowerParams {
    /// Sliding window length in milliseconds (default 50 ms).
    #[serde(default = "default_follower_window_ms")]
    pub window_ms: f32,
    #[serde(default)]
    pub mode: FollowerMode,
    /// Envelope attack in milliseconds (default 10 ms).
    #[serde(default = "default_follower_attack_ms")]
    pub attack_ms: f32,
    /// Envelope release in milliseconds (default 100 ms).
    #[serde(default = "default_follower_release_ms")]
    pub release_ms: f32,
    /// Tap from a specific effect in the chain.  `None` = track post-FX output.
    #[serde(default)]
    pub source_effect_id: Option<String>,
}

fn default_follower_window_ms() -> f32 {
    50.0
}
fn default_follower_attack_ms() -> f32 {
    10.0
}
fn default_follower_release_ms() -> f32 {
    100.0
}

impl Default for AudioFollowerParams {
    fn default() -> Self {
        Self {
            window_ms: default_follower_window_ms(),
            mode: FollowerMode::default(),
            attack_ms: default_follower_attack_ms(),
            release_ms: default_follower_release_ms(),
            source_effect_id: None,
        }
    }
}

/// Per-track modulation source (LFO, Envelope, Macro, AudioFollower).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModulatorParams {
    Lfo(LfoParams),
    Macro(MacroParams),
    Envelope(EnvelopeParams),
    AudioFollower(AudioFollowerParams),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectModulator {
    pub id: String,
    #[serde(flatten)]
    pub params: ModulatorParams,
}

/// Interpolation between consecutive automation points.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InterpolationMode {
    #[default]
    Linear,
    /// Hold the previous point's value until the next point (staircase).
    Step,
}

/// A single time+value knot on an automation curve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationPoint {
    /// Position on the project timeline in seconds.
    pub time_s: f64,
    /// Normalised parameter value [0..1].
    pub value: f32,
}

/// A clip of automation data placed on the timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationClip {
    pub id: String,
    pub start_s: f64,
    pub end_s: f64,
    #[serde(default)]
    pub points: Vec<AutomationPoint>,
}

/// One automation lane — drives a single parameter target from one or more clips.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationLane {
    pub id: String,
    pub target: ModulationTargetRef,
    #[serde(default)]
    pub clips: Vec<AutomationClip>,
    #[serde(default)]
    pub interpolation: InterpolationMode,
}

/// Identifies the parameter that a modulation route targets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModulationTargetRef {
    /// The track's main volume fader.
    TrackGain { track_id: String },
    /// A named parameter on a specific effect in the track's effect chain.
    EffectParam {
        track_id: String,
        effect_id: String,
        param: String,
    },
    /// A native parameter on an external plugin (VST3 / CLAP).
    /// `native_id` is the format-native parameter identifier (VST3 `ParamID`, CLAP `clap_id`).
    PluginParam { effect_id: String, native_id: u32 },
}

/// One modulation connection stored in the project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectModulationRoute {
    pub id: String,
    /// ID of the source `ProjectModulator` on this track.
    pub modulator_id: String,
    pub target: ModulationTargetRef,
    /// Modulation depth in the parameter's native range (signed).
    pub depth: f32,
}
/// One MIDI note stored in the project.  Coordinates are in beats (float).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiNote {
    pub id: String,
    /// MIDI pitch 0-127 (60 = C4).
    pub pitch: u8,
    /// Start position in beats, relative to the clip start.
    pub start: f32,
    /// Duration in beats (> 0).
    pub duration: f32,
    /// Velocity 0-127.
    pub velocity: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiControlKind {
    PitchBend,
    ControlChange,
    ChannelPressure,
    PolyPressure,
}

impl MidiControlKind {
    pub fn status(self) -> u8 {
        match self {
            Self::PolyPressure => 0xA0,
            Self::ControlChange => 0xB0,
            Self::ChannelPressure => 0xD0,
            Self::PitchBend => 0xE0,
        }
    }

    pub fn max_value(self) -> u16 {
        match self {
            Self::PitchBend => 0x3FFF,
            _ => 0x7F,
        }
    }
}

/// One channel-wide control value (ADR-029). Holds until the next point of the
/// same control. `number` is the CC number or the key of a polyphonic pressure,
/// `0` otherwise; `value` is 14-bit for a pitch bend, 7-bit for the others.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiControlPoint {
    /// Position in beats, relative to the clip content origin like `MidiNote::start`.
    pub start: f32,
    pub kind: MidiControlKind,
    #[serde(default)]
    pub number: u8,
    pub value: u16,
}

/// Drops invalid points, clamps values to their range and orders by position.
pub fn sanitize_control_points(mut points: Vec<MidiControlPoint>) -> Vec<MidiControlPoint> {
    points.retain(|point| point.start.is_finite() && point.start >= 0.0);
    for point in &mut points {
        point.number = point.number.min(0x7F);
        point.value = point.value.min(point.kind.max_value());
    }
    points.sort_by(|a, b| a.start.total_cmp(&b.start));
    points
}

/// A MIDI clip placed on the timeline.
/// `start`/`end` are in seconds (same coordinate space as audio clips).
/// Notes and controls are beat-relative to the clip content origin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppMidiClip {
    pub id: String,
    pub start: f64,
    pub end: f64,
    /// Seconds of content hidden past the left edge (left-trim). The note
    /// content origin sits at `start - offset`; the playable window is
    /// `[start, end]`. `0.0` for untrimmed clips.
    pub offset: f64,
    pub notes: Vec<MidiNote>,
    pub channel_controls: Vec<MidiControlPoint>,
}

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn generate_id() -> String {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let c = ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{:x}{:x}", t, c)
}

/// Minimum arrangement-region length in beats (matches the UI loop brace floor).
pub const MIN_REGION_BEATS: f64 = 1.0;

pub fn normalize_region_span(start_beats: f64, end_beats: f64) -> (f64, f64) {
    let start = start_beats.max(0.0);
    let end = end_beats.max(start + MIN_REGION_BEATS);
    (start, end)
}

pub fn default_marker_name(existing: &[ArrangementMarker]) -> String {
    format!("M{}", existing.len() + 1)
}

pub fn default_region_name(existing: &[ArrangementRegion]) -> String {
    let n = existing.len();
    if n < 26 {
        ((b'A' + n as u8) as char).to_string()
    } else {
        format!("R{}", n + 1)
    }
}

pub struct AppClip {
    pub id: String,
    pub start: f64,
    pub end: f64,
    /// Seconds skipped into the source file before playback begins (left-trim).
    /// `0.0` plays the file from its first sample at the clip's left edge.
    pub offset: f64,
    pub file_path: String,
    /// Decoded mono f32 samples.  Wrapped in `Arc` so multiple consumers
    /// (project, audio cache, audio thread via the graph compiler) can share
    /// the same heap allocation — graph recompilation must never clone this.
    pub raw_samples: Arc<Vec<f32>>,
    pub sample_rate: u32,
    pub peaks_64: Vec<[f32; 2]>,
    pub peaks_512: Vec<[f32; 2]>,
    pub peaks_4096: Vec<[f32; 2]>,
}

pub struct AppTrack {
    /// Stable identifier used by the event sourcing system.
    pub id: String,
    pub name: String,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub clips: Vec<AppClip>,
    pub effects: Vec<TrackEffect>,
    pub modulators: Vec<ProjectModulator>,
    pub mod_routes: Vec<ProjectModulationRoute>,
    /// Legacy track-level MIDI notes (kept for file backward-compat).
    pub midi_notes: Vec<MidiNote>,
    /// MIDI clips placed on the timeline.
    pub midi_clips: Vec<AppMidiClip>,
    /// Optional plugin instrument that generates this track's audio from its
    /// MIDI clips (Phase 5c).  `None` → the built-in `PolySynth` (legacy,
    /// bit-identical).  When `Some(device_id)`, the compiler instantiates that
    /// plugin as the instrument node's generator.
    pub instrument: Option<String>,
    /// Built-in sampler pads (ADR-028), kept when another instrument is chosen.
    pub sampler: Option<SamplerKit>,
    /// Plugin output mixed into this track's input, ahead of its effects.
    /// `None` for every project saved before multi-out existed (C-4).
    pub input_source: Option<PluginOutputSource>,
    /// Automation lanes — each drives one parameter from pre-recorded curves.
    pub automation_lanes: Vec<AutomationLane>,
    /// Optional UI color override (`#rrggbb`). `None` → frontend picks a stable default from id.
    pub color: Option<String>,
}

fn default_midi_grid_beats() -> f32 {
    0.25
}

fn default_key_root() -> String {
    "C".to_string()
}

fn default_key_mode() -> String {
    "major".to_string()
}

fn default_swing_percent() -> u8 {
    0
}

fn default_midi_ppq() -> u16 {
    960
}

fn default_left_panel_open() -> bool {
    true
}

fn default_left_panel_width() -> u16 {
    168
}

fn default_right_panel_width() -> u16 {
    360
}

fn default_bottom_panel_height() -> u16 {
    168
}

fn default_bottom_edit_height() -> u16 {
    480
}

/// Disposition des panneaux UI — persistée dans le fichier projet, hors undo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeftPanelLayout {
    #[serde(default = "default_left_panel_open")]
    pub open: bool,
    #[serde(default = "default_left_panel_width")]
    pub width: u16,
}

impl Default for LeftPanelLayout {
    fn default() -> Self {
        Self {
            open: true,
            width: default_left_panel_width(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RightPanelLayout {
    #[serde(default)]
    pub open: bool,
    #[serde(default = "default_right_panel_width")]
    pub width: u16,
}

impl Default for RightPanelLayout {
    fn default() -> Self {
        Self {
            open: false,
            width: default_right_panel_width(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BottomPanelLayout {
    #[serde(default)]
    pub open: bool,
    #[serde(default = "default_bottom_panel_height")]
    pub height: u16,
    #[serde(default = "default_bottom_edit_height")]
    pub saved_edit_height: u16,
    #[serde(default = "default_bottom_panel_height")]
    pub saved_preview_height: u16,
}

impl Default for BottomPanelLayout {
    fn default() -> Self {
        Self {
            open: false,
            height: default_bottom_panel_height(),
            saved_edit_height: default_bottom_edit_height(),
            saved_preview_height: default_bottom_panel_height(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewModeLayout {
    #[serde(default = "default_view_main")]
    pub main: String,
    #[serde(default = "default_graph_scope")]
    pub graph_scope: String,
}

fn default_view_main() -> String {
    "arrange".to_string()
}

fn default_graph_scope() -> String {
    "track".to_string()
}

impl Default for ViewModeLayout {
    fn default() -> Self {
        Self {
            main: default_view_main(),
            graph_scope: default_graph_scope(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectUiLayout {
    #[serde(default)]
    pub left_panel: LeftPanelLayout,
    #[serde(default)]
    pub right_panel: RightPanelLayout,
    #[serde(default)]
    pub bottom_panel: BottomPanelLayout,
    #[serde(default)]
    pub view_mode: ViewModeLayout,
}

/// Named point on the arrangement timeline (musical beats). Navigation only —
/// does not affect the audio graph or transport atomics by itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArrangementMarker {
    pub id: String,
    pub name: String,
    /// Position in quarter-note beats from the project start.
    pub position_beats: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Named span on the arrangement timeline (musical beats). Used for section
/// labels and as a source for the ephemeral playback loop (`SET_LOOP`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArrangementRegion {
    pub id: String,
    pub name: String,
    pub start_beats: f64,
    pub end_beats: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

pub struct AppProject {
    pub name: String,
    pub bpm: f32,
    pub numerator: u8,
    pub denominator: u8,
    /// Quantification éditeur (piano roll / accrochage) en fractions de noire (0.25 = 1/16).
    pub midi_grid_beats: f32,
    /// Tonalité (racine), ex. "C", "F#". Moteur / export MIDI à venir.
    pub key_root: String,
    /// "major" | "minor". Moteur à venir.
    pub key_mode: String,
    /// Swing 0–100 %. Lecture transport à venir.
    pub swing_percent: u8,
    /// Subdivision MIDI du projet — résolution PPQ (pulses per quarter note).
    pub midi_ppq: u16,
    /// Grille en triolet (subdivision T) — éditeur à venir.
    pub midi_grid_use_triplets: bool,
    pub tracks: Vec<AppTrack>,
    /// Locators — named points on the arrangement (beats). Empty in pre-marker projects.
    pub markers: Vec<ArrangementMarker>,
    /// Arrangement sections — named spans in beats. Empty in pre-region projects.
    pub regions: Vec<ArrangementRegion>,
    /// Binary plugin state (e.g. VST3 chunk), keyed by **`instance_key`** (the
    /// per-placement id = `effect.id`, or `instrument:{track_id}`) so two
    /// instances of the same plugin type persist distinct state (ADR-017).
    /// Harvested from the live `PluginHost` at save time and re-applied after
    /// the graph is compiled on load.  Not part of the event-sourced model —
    /// it mirrors live plugin internals, so it is excluded from undo snapshots.
    pub plugin_states: HashMap<String, Vec<u8>>,
    /// Panneaux et vue principale — persistés au save, exclus de l'undo.
    pub ui_layout: ProjectUiLayout,
}

impl AppProject {
    /// Latest timeline boundary in seconds (max of all audio and MIDI clip ends).
    pub fn timeline_end_secs(&self) -> f64 {
        self.tracks
            .iter()
            .flat_map(|t| {
                t.clips
                    .iter()
                    .map(|c| c.end)
                    .chain(t.midi_clips.iter().map(|c| c.end))
            })
            .fold(0.0_f64, f64::max)
    }

    pub fn new() -> Self {
        Self {
            name: "Sans titre".to_string(),
            bpm: 120.0,
            numerator: 4,
            denominator: 4,
            midi_grid_beats: default_midi_grid_beats(),
            key_root: default_key_root(),
            key_mode: default_key_mode(),
            swing_percent: default_swing_percent(),
            midi_ppq: default_midi_ppq(),
            midi_grid_use_triplets: false,
            tracks: vec![],
            markers: vec![],
            regions: vec![],
            plugin_states: HashMap::new(),
            ui_layout: ProjectUiLayout::default(),
        }
    }

    pub fn to_dto(&self) -> ProjectDto {
        ProjectDto {
            name: self.name.clone(),
            bpm: self.bpm,
            numerator: self.numerator,
            denominator: self.denominator,
            midi_grid_beats: self.midi_grid_beats,
            key_root: self.key_root.clone(),
            key_mode: self.key_mode.clone(),
            swing_percent: self.swing_percent,
            midi_ppq: self.midi_ppq,
            midi_grid_use_triplets: self.midi_grid_use_triplets,
            tracks: self.tracks.iter().map(AppTrack::to_dto).collect(),
            markers: self.markers.clone(),
            regions: self.regions.clone(),
            ui_layout: self.ui_layout.clone(),
        }
    }

    pub fn to_file(&self) -> ProjectFile {
        ProjectFile {
            version: 1,
            name: self.name.clone(),
            bpm: self.bpm,
            numerator: self.numerator,
            denominator: self.denominator,
            midi_grid_beats: self.midi_grid_beats,
            key_root: self.key_root.clone(),
            key_mode: self.key_mode.clone(),
            swing_percent: self.swing_percent,
            midi_ppq: self.midi_ppq,
            midi_grid_use_triplets: self.midi_grid_use_triplets,
            tracks: self
                .tracks
                .iter()
                .map(|t| TrackFile {
                    id: Some(t.id.clone()),
                    name: t.name.clone(),
                    volume: t.volume,
                    pan: t.pan,
                    muted: t.muted,
                    solo: t.solo,
                    clips: t
                        .clips
                        .iter()
                        .map(|c| ClipFile {
                            id: c.id.clone(),
                            start: c.start,
                            end: c.end,
                            offset: c.offset,
                            file_path: c.file_path.clone(),
                        })
                        .collect(),
                    effects: t.effects.clone(),
                    modulators: t.modulators.clone(),
                    mod_routes: t.mod_routes.clone(),
                    midi_notes: t.midi_notes.clone(),
                    midi_clips: t
                        .midi_clips
                        .iter()
                        .map(|mc| MidiClipFile {
                            id: mc.id.clone(),
                            start: mc.start,
                            end: mc.end,
                            offset: mc.offset,
                            notes: mc.notes.clone(),
                            channel_controls: mc.channel_controls.clone(),
                        })
                        .collect(),
                    instrument: t.instrument.clone(),
                    sampler: t.sampler.clone(),
                    input_source: t.input_source.clone(),
                    automation_lanes: t.automation_lanes.clone(),
                    color: t.color.clone(),
                })
                .collect(),
            markers: self.markers.clone(),
            regions: self.regions.clone(),
            // New per-instance format (ADR-017): keyed by `instance_key`.
            plugin_states: self
                .plugin_states
                .iter()
                .map(|(k, v)| (k.clone(), crate::plugin_system::state::encode_base64(v)))
                .collect(),
            // Legacy device_id-keyed field is no longer written (read-only for
            // backward compat on load).
            plugin_state_blobs: HashMap::new(),
            ui_layout: self.ui_layout.clone(),
        }
    }

    pub fn from_file(pf: ProjectFile) -> Result<AppProject, String> {
        let mut tracks = Vec::new();
        for tf in pf.tracks {
            let mut clips = Vec::new();
            for cf in tf.clips {
                let audio = AudioClip::from_file(&cf.file_path)
                    .map_err(|e| format!("Erreur lecture '{}': {}", cf.file_path, e))?;
                let data = audio.into_clip_data();
                clips.push(AppClip {
                    id: cf.id,
                    start: cf.start,
                    end: cf.end,
                    offset: cf.offset,
                    file_path: cf.file_path,
                    raw_samples: data.raw_samples,
                    sample_rate: data.sample_rate,
                    peaks_64: data.peaks_64,
                    peaks_512: data.peaks_512,
                    peaks_4096: data.peaks_4096,
                });
            }
            let track_id = tf.id.unwrap_or_else(generate_id);
            let mut sampler = tf.sampler;
            if let Some(kit) = &mut sampler {
                kit.load_samples(decode_pad_sample);
            }
            let midi_clips = tf
                .midi_clips
                .into_iter()
                .map(|mc| AppMidiClip {
                    id: mc.id,
                    start: mc.start,
                    end: mc.end,
                    offset: mc.offset,
                    notes: mc.notes,
                    channel_controls: sanitize_control_points(mc.channel_controls),
                })
                .collect();
            tracks.push(AppTrack {
                id: track_id,
                name: tf.name,
                volume: tf.volume,
                pan: tf.pan,
                muted: tf.muted,
                solo: tf.solo,
                clips,
                effects: tf.effects,
                modulators: tf.modulators,
                mod_routes: tf.mod_routes,
                midi_notes: tf.midi_notes,
                midi_clips,
                instrument: tf.instrument,
                sampler,
                input_source: tf.input_source,
                automation_lanes: tf.automation_lanes,
                color: tf.color,
            });
        }
        // Plugin state: prefer the per-instance format (ADR-017); fall back to
        // the legacy device_id-keyed blobs, mapped onto instances by scanning the
        // loaded placements (unambiguous for one-instance-per-type projects).
        let plugin_states = if !pf.plugin_states.is_empty() {
            pf.plugin_states
                .into_iter()
                .filter_map(|(k, v)| {
                    crate::plugin_system::state::decode_base64(&v).map(|bytes| (k, bytes))
                })
                .collect()
        } else {
            map_legacy_plugin_state(&tracks, pf.plugin_state_blobs)
        };
        Ok(Self {
            name: pf.name,
            bpm: pf.bpm,
            numerator: pf.numerator,
            denominator: pf.denominator,
            midi_grid_beats: pf.midi_grid_beats,
            key_root: pf.key_root,
            key_mode: pf.key_mode,
            swing_percent: pf.swing_percent,
            midi_ppq: pf.midi_ppq,
            midi_grid_use_triplets: pf.midi_grid_use_triplets,
            tracks,
            markers: pf.markers,
            regions: pf.regions,
            plugin_states,
            ui_layout: pf.ui_layout,
        })
    }
}

/// Map legacy `device_id`-keyed plugin state blobs onto per-instance
/// `instance_key`s by scanning the loaded placements (ADR-017 backward compat).
/// One instance per plugin type → unambiguous; duplicates in a legacy file (a
/// pre-ADR-017 collision) resolve to the first matching placement.
fn map_legacy_plugin_state(
    tracks: &[AppTrack],
    legacy: HashMap<String, String>,
) -> HashMap<String, Vec<u8>> {
    let mut by_device: HashMap<&str, String> = HashMap::new();
    for t in tracks {
        for e in &t.effects {
            if let EffectParams::Plugin(pp) = &e.params {
                by_device
                    .entry(pp.plugin_id.as_str())
                    .or_insert_with(|| e.id.clone());
            }
        }
        if let Some(device_id) = t.plugin_instrument() {
            by_device
                .entry(device_id)
                .or_insert_with(|| format!("instrument:{}", t.id));
        }
    }
    legacy
        .into_iter()
        .filter_map(|(device_id, v)| {
            let key = by_device.get(device_id.as_str())?.clone();
            let bytes = crate::plugin_system::state::decode_base64(&v)?;
            Some((key, bytes))
        })
        .collect()
}

impl AppTrack {
    /// The chosen instrument when it is a plugin (not a built-in device).
    pub fn plugin_instrument(&self) -> Option<&str> {
        self.instrument
            .as_deref()
            .filter(|id| *id != SAMPLER_INSTRUMENT_ID)
    }

    pub fn uses_sampler(&self) -> bool {
        self.instrument.as_deref() == Some(SAMPLER_INSTRUMENT_ID)
    }

    /// Choosing the sampler gives the track a default kit the first time.
    pub fn set_instrument(&mut self, instrument: Option<String>) {
        self.instrument = instrument;
        if self.uses_sampler() && self.sampler.is_none() {
            self.sampler = Some(SamplerKit::default());
        }
    }

    /// Like `set_instrument`, but a sampler track without a kit gets
    /// `starting_kit`. Returns the kit it gave, for the event log.
    pub fn set_instrument_with_starting_kit(
        &mut self,
        instrument: Option<String>,
        starting_kit: Option<SamplerKit>,
    ) -> Option<SamplerKit> {
        let choosing_sampler = instrument.as_deref() == Some(SAMPLER_INSTRUMENT_ID);
        let given_kit = starting_kit.filter(|_| choosing_sampler && self.sampler.is_none());
        if given_kit.is_some() {
            self.sampler = given_kit.clone();
        }
        self.set_instrument(instrument);
        given_kit
    }

    pub fn to_dto(&self) -> TrackDto {
        TrackDto {
            id: self.id.clone(),
            name: self.name.clone(),
            volume: self.volume,
            pan: self.pan,
            muted: self.muted,
            solo: self.solo,
            clips: self.clips.iter().map(|c| c.to_dto()).collect(),
            effects: self.effects.clone(),
            modulators: self.modulators.clone(),
            mod_routes: self.mod_routes.clone(),
            midi_notes: self.midi_notes.clone(),
            midi_clips: self
                .midi_clips
                .iter()
                .map(|mc| MidiClipDto {
                    id: mc.id.clone(),
                    start: mc.start,
                    end: mc.end,
                    offset: mc.offset,
                    notes: mc.notes.clone(),
                    channel_controls: mc.channel_controls.clone(),
                })
                .collect(),
            instrument: self.instrument.clone(),
            sampler: self.sampler.as_ref().map(SamplerKit::to_dto),
            input_source: self.input_source.clone(),
            automation_lanes: self.automation_lanes.clone(),
            color: self.color.clone(),
        }
    }
}

impl AppClip {
    pub fn source_duration_sec(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.raw_samples.len() as f64 / self.sample_rate as f64
    }

    /// DTO for the UI IPC boundary. Never includes PCM samples — sending
    /// multi-minute `raw_samples` as JSON freezes the WebView for seconds.
    pub fn to_dto(&self) -> ClipDto {
        // Cap the densest peak lane so long files stay light on the wire.
        // Arrange still has peaks_512 / peaks_4096 for overview zoom levels.
        const MAX_UI_PEAKS_64: usize = 120_000;
        let peaks_64 = if self.peaks_64.len() > MAX_UI_PEAKS_64 {
            Vec::new()
        } else {
            self.peaks_64.clone()
        };
        ClipDto {
            id: self.id.clone(),
            start: self.start,
            end: self.end,
            offset: self.offset,
            file_path: self.file_path.clone(),
            raw_samples: Arc::new(Vec::new()),
            sample_rate: self.sample_rate,
            source_duration_sec: self.source_duration_sec(),
            peaks_64,
            peaks_512: self.peaks_512.clone(),
            peaks_4096: self.peaks_4096.clone(),
        }
    }
}

// Keyed by clip_id (+ optional path index). Persists audio data across undo/redo
// so events can be replayed without re-reading audio files from disk.

pub struct AudioDataCache {
    by_clip_id: Mutex<HashMap<String, Arc<CachedAudioData>>>,
    by_path: Mutex<HashMap<String, Arc<CachedAudioData>>>,
}

pub struct CachedAudioData {
    pub raw_samples: Arc<Vec<f32>>,
    pub sample_rate: u32,
    pub peaks_64: Vec<[f32; 2]>,
    pub peaks_512: Vec<[f32; 2]>,
    pub peaks_4096: Vec<[f32; 2]>,
    /// False while decode is still appending immutable prefixes (ADR-024).
    pub complete: bool,
}

impl CachedAudioData {
    pub fn duration_sec(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.raw_samples.len() as f64 / self.sample_rate as f64
    }
}

fn norm_cache_path(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

impl AudioDataCache {
    pub fn new() -> Self {
        Self {
            by_clip_id: Mutex::new(HashMap::new()),
            by_path: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, clip_id: &str) -> Option<Arc<CachedAudioData>> {
        self.by_clip_id.lock().ok()?.get(clip_id).cloned()
    }

    pub fn get_by_path(&self, path: &str) -> Option<Arc<CachedAudioData>> {
        let key = norm_cache_path(path);
        self.by_path.lock().ok()?.get(&key).cloned()
    }

    pub fn insert(&self, clip_id: String, data: Arc<CachedAudioData>) {
        if let Ok(mut map) = self.by_clip_id.lock() {
            map.insert(clip_id, data);
        }
    }

    pub fn insert_path(&self, path: String, data: Arc<CachedAudioData>) {
        let key = norm_cache_path(&path);
        if let Ok(mut map) = self.by_path.lock() {
            map.insert(key, data);
        }
    }

    pub fn insert_for_clip_and_path(
        &self,
        clip_id: String,
        path: String,
        data: Arc<CachedAudioData>,
    ) {
        self.insert_path(path, Arc::clone(&data));
        self.insert(clip_id, data);
    }

    /// Drop a path entry regardless of completeness (e.g. orphan after cancel).
    pub fn remove_path(&self, path: &str) {
        let key = norm_cache_path(path);
        if let Ok(mut map) = self.by_path.lock() {
            map.remove(&key);
        }
    }

    /// Drop only an incomplete path prefix so a cancelled decode cannot be reused.
    pub fn remove_if_incomplete(&self, path: &str) {
        let key = norm_cache_path(path);
        if let Ok(mut map) = self.by_path.lock() {
            let drop = map.get(&key).is_some_and(|c| !c.complete);
            if drop {
                map.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod audio_cache_tests {
    use super::*;
    use std::sync::Arc;

    fn sample(complete: bool, n: usize) -> Arc<CachedAudioData> {
        Arc::new(CachedAudioData {
            raw_samples: Arc::new(vec![0.1; n]),
            sample_rate: 48_000,
            peaks_64: Vec::new(),
            peaks_512: Vec::new(),
            peaks_4096: Vec::new(),
            complete,
        })
    }

    #[test]
    fn path_lookup_is_case_and_slash_insensitive() {
        let cache = AudioDataCache::new();
        cache.insert_path(r"C:\Sounds\Kick.wav".into(), sample(true, 100));
        assert!(cache.get_by_path("c:/sounds/kick.wav").is_some());
    }

    #[test]
    fn remove_if_incomplete_keeps_complete_entries() {
        let cache = AudioDataCache::new();
        cache.insert_path("/a.wav".into(), sample(true, 100));
        cache.remove_if_incomplete("/a.wav");
        assert!(cache.get_by_path("/a.wav").is_some());
    }

    #[test]
    fn remove_if_incomplete_drops_prefix() {
        let cache = AudioDataCache::new();
        cache.insert_path("/a.wav".into(), sample(false, 10));
        cache.remove_if_incomplete("/a.wav");
        assert!(cache.get_by_path("/a.wav").is_none());
    }

    #[test]
    fn remove_path_allows_fresh_decode_after_orphan_prefix() {
        let cache = AudioDataCache::new();
        cache.insert_path("/a.wav".into(), sample(false, 10));
        cache.remove_path("/a.wav");
        assert!(cache.get_by_path("/a.wav").is_none());
        cache.insert_path("/a.wav".into(), sample(true, 1000));
        let hit = cache.get_by_path("/a.wav").unwrap();
        assert!(hit.complete);
        assert_eq!(hit.raw_samples.len(), 1000);
    }
}

pub struct AppState {
    pub project: Mutex<AppProject>,
    pub project_path: Mutex<Option<PathBuf>>,
    /// Event store — sole source of truth for project structure changes.
    pub event_store: Mutex<crate::engine_gateway::store::EventStore>,
    /// In-memory audio data cache — survives undo/redo so replaying events
    /// never re-reads audio files unnecessarily.
    pub audio_cache: AudioDataCache,
    /// Real-time audio engine — compiles the graph and owns the CPAL stream.
    pub audio_engine: Mutex<crate::audio_engine::engine::AudioEngine>,
    /// Live parameter registry — atomic cells for volume, mute/solo, modulator
    /// rate, modulation depth.  Written by gateway hot-path handlers, read
    /// lock-free by the audio thread via Arc handles embedded in nodes.
    pub live_params: Arc<crate::audio_graph::live_params::LiveParams>,
    /// Atomic transport state shared with the CPAL callback.
    /// Accessed directly (no lock) for PLAY / STOP / SEEK commands.
    pub transport: std::sync::Arc<crate::audio_engine::transport::SharedTransport>,
    /// Current audio I/O settings (device, sample rate, buffer size).
    /// Updated by `apply_audio_settings`; read back by `start_native_audio`
    /// so restarts always use the user's last chosen configuration.
    pub audio_settings: Mutex<crate::audio_settings::AudioSettings>,
    /// True when the project has unsaved changes. Reset to false after save /
    /// new / open; set to true after any structural mutation.
    pub dirty: AtomicBool,
    /// Open native plugin editor windows.  Maps `effect_id → HWND as usize`.
    /// Shared as Arc so it can be cloned into `run_on_main_thread` closures.
    pub open_plugin_editors: Arc<Mutex<HashMap<String, usize>>>,
    /// Hardware MIDI input device registry (ADR-022). Command-thread only —
    /// never touched by the audio thread.
    pub midi_input: Mutex<crate::midi_input::MidiInputRegistry>,
    /// The track that currently receives hardware MIDI / audio input, if any.
    /// Ephemeral session state, not part of `AppProject` (ADR-002 scope).
    pub armed_track_id: Arc<Mutex<Option<String>>>,
    /// Notes captured from the armed track while recording (ADR-022 amendment).
    pub midi_recorder: Arc<crate::midi_recording::MidiRecorder>,
    /// Shared CPAL input rings / monitor flag (ADR-027).
    pub audio_input: Arc<crate::audio_input::AudioInputHub>,
    /// Audio takes captured from the input while recording (ADR-027).
    pub audio_recorder: Arc<crate::audio_recording::AudioRecorder>,
    /// Absolute paths (normalized) whose in-flight `prepare_audio_import` should abort.
    pub import_cancels: Mutex<HashSet<String>>,
}

impl AppState {
    const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;
    const DEFAULT_BPM: f32 = 120.0;

    pub fn new() -> Self {
        // Seed the event store with the initial empty project so that the
        // first undo always has a valid snapshot to return to instead of
        // falling back to AppProject::new() on a None snapshot.
        let mut event_store = crate::engine_gateway::store::EventStore::new();
        event_store.init_from_project(AppProject::new());

        let transport = crate::audio_engine::transport::SharedTransport::new(
            Self::DEFAULT_SAMPLE_RATE,
            Self::DEFAULT_BPM,
        );

        let live_params = Arc::new(crate::audio_graph::live_params::LiveParams::new());
        let midi_recorder = Arc::new(crate::midi_recording::MidiRecorder::new(
            Arc::clone(&transport),
            Arc::clone(&live_params.live_input_clock),
        ));
        let audio_input = crate::audio_input::AudioInputHub::new();
        let audio_recorder = crate::audio_recording::AudioRecorder::new(
            Arc::clone(&transport),
            Arc::clone(&live_params.live_input_clock),
            Arc::clone(&audio_input),
        );

        Self {
            project: Mutex::new(AppProject::new()),
            project_path: Mutex::new(None),
            event_store: Mutex::new(event_store),
            audio_cache: AudioDataCache::new(),
            audio_engine: Mutex::new(crate::audio_engine::engine::AudioEngine::new(
                std::sync::Arc::clone(&transport),
                Arc::clone(&live_params),
                Arc::clone(&audio_input),
            )),
            transport,
            live_params,
            audio_settings: Mutex::new(crate::audio_settings::AudioSettings::default()),
            dirty: AtomicBool::new(false),
            open_plugin_editors: Arc::new(Mutex::new(HashMap::new())),
            midi_input: Mutex::new(crate::midi_input::MidiInputRegistry::new()),
            armed_track_id: Arc::new(Mutex::new(None)),
            midi_recorder,
            audio_input,
            audio_recorder,
            import_cancels: Mutex::new(HashSet::new()),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct ProjectFile {
    pub version: u32,
    pub name: String,
    pub bpm: f32,
    pub numerator: u8,
    pub denominator: u8,
    #[serde(default = "default_midi_grid_beats")]
    pub midi_grid_beats: f32,
    #[serde(default = "default_key_root")]
    pub key_root: String,
    #[serde(default = "default_key_mode")]
    pub key_mode: String,
    #[serde(default = "default_swing_percent")]
    pub swing_percent: u8,
    #[serde(default = "default_midi_ppq")]
    pub midi_ppq: u16,
    #[serde(default)]
    pub midi_grid_use_triplets: bool,
    pub tracks: Vec<TrackFile>,
    /// Arrangement markers (beats). Absent in older saves → empty.
    #[serde(default)]
    pub markers: Vec<ArrangementMarker>,
    /// Arrangement regions (beats). Absent in older saves → empty.
    #[serde(default)]
    pub regions: Vec<ArrangementRegion>,
    /// Base64-encoded per-**instance** plugin state, keyed by `instance_key`
    /// (ADR-017).  This is what new saves write; `#[serde(default)]` keeps older
    /// files (which only carry `plugin_state_blobs`) loading unchanged.
    #[serde(default)]
    pub plugin_states: HashMap<String, String>,
    /// Legacy base64 plugin state keyed by `device_id` (plugin type).  Read-only:
    /// kept so projects saved before ADR-017 still load (mapped to instances at
    /// `from_file`); new saves leave it empty.
    #[serde(default)]
    pub plugin_state_blobs: HashMap<String, String>,
    #[serde(default)]
    pub ui_layout: ProjectUiLayout,
}

#[derive(Serialize, Deserialize)]
pub struct TrackFile {
    /// Optional for backward-compat: older saves without track IDs are migrated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub volume: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub solo: bool,
    pub clips: Vec<ClipFile>,
    #[serde(default)]
    pub effects: Vec<TrackEffect>,
    #[serde(default)]
    pub modulators: Vec<ProjectModulator>,
    #[serde(default)]
    pub mod_routes: Vec<ProjectModulationRoute>,
    #[serde(default)]
    pub midi_notes: Vec<MidiNote>,
    #[serde(default)]
    pub midi_clips: Vec<MidiClipFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampler: Option<SamplerKit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_source: Option<PluginOutputSource>,
    #[serde(default)]
    pub automation_lanes: Vec<AutomationLane>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct MidiClipFile {
    pub id: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub offset: f64,
    #[serde(default)]
    pub notes: Vec<MidiNote>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel_controls: Vec<MidiControlPoint>,
}

#[derive(Serialize, Deserialize)]
pub struct ClipFile {
    pub id: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub offset: f64,
    pub file_path: String,
}

#[derive(Serialize, Clone)]
pub struct ProjectDto {
    pub name: String,
    pub bpm: f32,
    pub numerator: u8,
    pub denominator: u8,
    pub midi_grid_beats: f32,
    pub key_root: String,
    pub key_mode: String,
    pub swing_percent: u8,
    pub midi_ppq: u16,
    pub midi_grid_use_triplets: bool,
    pub tracks: Vec<TrackDto>,
    pub markers: Vec<ArrangementMarker>,
    pub regions: Vec<ArrangementRegion>,
    pub ui_layout: ProjectUiLayout,
}

#[derive(Serialize, Clone)]
pub struct MidiClipDto {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub offset: f64,
    pub notes: Vec<MidiNote>,
    pub channel_controls: Vec<MidiControlPoint>,
}

#[derive(Serialize, Clone)]
pub struct TrackDto {
    pub id: String,
    pub name: String,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub clips: Vec<ClipDto>,
    pub effects: Vec<TrackEffect>,
    pub modulators: Vec<ProjectModulator>,
    pub mod_routes: Vec<ProjectModulationRoute>,
    pub midi_notes: Vec<MidiNote>,
    pub midi_clips: Vec<MidiClipDto>,
    pub instrument: Option<String>,
    pub sampler: Option<SamplerKitDto>,
    pub input_source: Option<PluginOutputSource>,
    pub automation_lanes: Vec<AutomationLane>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct ClipDto {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub offset: f64,
    pub file_path: String,
    /// Always empty on the UI wire — PCM stays in the engine cache.
    #[serde(serialize_with = "crate::audio_clip::serialize_arc_vec_f32")]
    pub raw_samples: Arc<Vec<f32>>,
    pub sample_rate: u32,
    /// Full source-file length in seconds (independent of clip trim / peaks lanes).
    pub source_duration_sec: f64,
    pub peaks_64: Vec<[f32; 2]>,
    pub peaks_512: Vec<[f32; 2]>,
    pub peaks_4096: Vec<[f32; 2]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_end_secs_includes_midi_clips() {
        let mut project = AppProject::new();
        project.tracks.push(AppTrack {
            id: "t1".into(),
            name: "MIDI".into(),
            volume: 80.0,
            pan: 0.0,
            muted: false,
            solo: false,
            clips: vec![],
            effects: vec![],
            modulators: vec![],
            mod_routes: vec![],
            midi_notes: vec![],
            midi_clips: vec![AppMidiClip {
                id: "mc1".into(),
                start: 0.0,
                end: 12.5,
                offset: 0.0,
                notes: vec![],
                channel_controls: vec![],
            }],
            instrument: None,
            sampler: None,
            input_source: None,
            automation_lanes: vec![],
            color: None,
        });
        assert_eq!(project.timeline_end_secs(), 12.5);
    }

    #[test]
    fn markers_and_regions_default_empty_on_legacy_file() {
        let json = r#"{
            "version": 1,
            "name": "Legacy",
            "bpm": 120.0,
            "numerator": 4,
            "denominator": 4,
            "tracks": []
        }"#;
        let file: ProjectFile = serde_json::from_str(json).expect("deserialize legacy project");
        assert!(file.markers.is_empty());
        assert!(file.regions.is_empty());
        let project = AppProject::from_file(file).expect("from_file");
        assert!(project.markers.is_empty());
        assert!(project.regions.is_empty());
    }

    #[test]
    fn markers_and_regions_round_trip_through_file() {
        let mut project = AppProject::new();
        project.markers.push(ArrangementMarker {
            id: "m1".into(),
            name: "Intro".into(),
            position_beats: 16.0,
            color: Some("#3b82f6".into()),
        });
        project.regions.push(ArrangementRegion {
            id: "r1".into(),
            name: "A".into(),
            start_beats: 0.0,
            end_beats: 16.0,
            color: None,
        });

        let json = serde_json::to_string(&project.to_file()).expect("serialize");
        let file: ProjectFile = serde_json::from_str(&json).expect("deserialize");
        let restored = AppProject::from_file(file).expect("from_file");

        assert_eq!(restored.markers, project.markers);
        assert_eq!(restored.regions, project.regions);
    }

    /// C-4: a saved project from before Phase 5c (no `instrument` field) loads
    /// with `instrument == None` → the track keeps the built-in PolySynth.
    #[test]
    fn track_file_without_instrument_defaults_to_none() {
        let json = r#"{
            "name": "Legacy",
            "volume": 80.0,
            "clips": []
        }"#;
        let tf: TrackFile = serde_json::from_str(json).expect("deserialize legacy track");
        assert_eq!(tf.instrument, None);
        assert_eq!(tf.input_source, None);
        assert!(tf.sampler.is_none());
    }

    fn track_with_midi_clip(controls: Vec<MidiControlPoint>) -> AppTrack {
        let mut track = track_with_plugin("keys", "fx", "clap:any");
        track.midi_clips.push(AppMidiClip {
            id: "clip".into(),
            start: 0.0,
            end: 4.0,
            offset: 0.0,
            notes: vec![],
            channel_controls: controls,
        });
        track
    }

    /// Channel controls survive save → load, and a clip without any writes no
    /// `channel_controls` key (older builds read the file unchanged).
    #[test]
    fn channel_controls_round_trip_through_file() {
        let controls = vec![
            MidiControlPoint {
                start: 1.0,
                kind: MidiControlKind::PitchBend,
                number: 0,
                value: 0x3000,
            },
            MidiControlPoint {
                start: 2.0,
                kind: MidiControlKind::ControlChange,
                number: 64,
                value: 127,
            },
        ];
        let mut project = AppProject::new();
        project.tracks.push(track_with_midi_clip(controls.clone()));
        project.tracks.push(track_with_midi_clip(vec![]));

        let json = serde_json::to_string(&project.to_file()).expect("serialize");
        assert_eq!(json.matches("channel_controls").count(), 1);
        let file: ProjectFile = serde_json::from_str(&json).expect("deserialize");
        let restored = AppProject::from_file(file).expect("from_file");

        assert_eq!(restored.tracks[0].midi_clips[0].channel_controls, controls);
        assert!(restored.tracks[1].midi_clips[0].channel_controls.is_empty());
    }

    #[test]
    fn midi_clip_without_channel_controls_loads_from_an_older_file() {
        let json = r#"{"id":"c","start":0.0,"end":2.0,"notes":[]}"#;
        let clip: MidiClipFile = serde_json::from_str(json).expect("deserialize older clip");
        assert!(clip.channel_controls.is_empty());
    }

    #[test]
    fn control_points_are_sanitized_and_ordered() {
        let point = |start: f32, kind, value| MidiControlPoint {
            start,
            kind,
            number: 200,
            value,
        };
        let sanitized = sanitize_control_points(vec![
            point(2.0, MidiControlKind::PitchBend, 60_000),
            point(-1.0, MidiControlKind::ControlChange, 5),
            point(f32::NAN, MidiControlKind::ControlChange, 5),
            point(1.0, MidiControlKind::ControlChange, 300),
        ]);

        assert_eq!(sanitized.len(), 2);
        assert_eq!(sanitized[0].start, 1.0);
        assert_eq!(sanitized[0].value, 127);
        assert_eq!(sanitized[0].number, 127);
        assert_eq!(sanitized[1].value, 0x3FFF);
    }

    /// A track's plugin-output input survives a save to JSON and a reload.
    #[test]
    fn input_source_round_trips_through_a_saved_file() {
        let mut project = AppProject::new();
        let mut track = track_with_plugin("snare", "fx0", "clap:comp");
        track.input_source = Some(PluginOutputSource {
            effect_id: "instrument:drums".into(),
            port: 2,
        });
        project.tracks.push(track);

        let json = serde_json::to_string(&project.to_file()).expect("serialize");
        let file: ProjectFile = serde_json::from_str(&json).expect("deserialize");
        let restored = AppProject::from_file(file).expect("from_file");

        assert_eq!(
            restored.tracks[0].input_source,
            Some(PluginOutputSource {
                effect_id: "instrument:drums".into(),
                port: 2
            })
        );
    }

    /// The instrument device_id survives a file save → load round-trip.
    #[test]
    fn instrument_round_trips_through_file() {
        let mut project = AppProject::new();
        project.tracks.push(AppTrack {
            id: "t1".into(),
            name: "Synth".into(),
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
            instrument: Some("clap://surge.dll?org.surge-synth.surge-xt".into()),
            sampler: None,
            input_source: None,
            automation_lanes: vec![],
            color: None,
        });

        let file = project.to_file();
        let restored = AppProject::from_file(file).expect("from_file");
        assert_eq!(
            restored.tracks[0].instrument.as_deref(),
            Some("clap://surge.dll?org.surge-synth.surge-xt")
        );
    }

    /// Pad settings survive save → load; a vanished sample file leaves the
    /// pad silent and reported instead of failing the whole project (ADR-028).
    #[test]
    fn sampler_kit_round_trips_and_tolerates_a_missing_file() {
        let mut project = AppProject::new();
        let mut track = track_with_plugin("drums", "fx", "clap:any");
        track.effects.clear();
        track.set_instrument(Some(SAMPLER_INSTRUMENT_ID.into()));
        let pad = &mut track.sampler.as_mut().unwrap().pads[1];
        pad.sample_path = Some("Z:/nowhere/snare.wav".into());
        pad.tune_semitones = -2.5;
        project.tracks.push(track);

        let restored = AppProject::from_file(project.to_file()).expect("from_file");
        let track = &restored.tracks[0];
        assert!(track.uses_sampler());
        assert_eq!(track.plugin_instrument(), None);
        let pad = &track.sampler.as_ref().unwrap().pads[1];
        assert_eq!(pad.tune_semitones, -2.5);
        assert!(pad.is_missing());
    }

    #[test]
    fn starting_kit_is_given_once_and_only_to_the_sampler() {
        let mut starting_kit = SamplerKit::default();
        starting_kit.pads[0].choke_group = 1;
        let mut track = track_with_plugin("drums", "fx", "clap:any");

        assert!(track
            .set_instrument_with_starting_kit(None, Some(starting_kit.clone()))
            .is_none());
        assert!(track.sampler.is_none());

        let given = track
            .set_instrument_with_starting_kit(
                Some(SAMPLER_INSTRUMENT_ID.into()),
                Some(starting_kit.clone()),
            )
            .expect("kit given on first choice");
        assert_eq!(given.pads[0].choke_group, 1);
        assert_eq!(track.sampler.as_ref().unwrap().pads[0].choke_group, 1);

        track.sampler.as_mut().unwrap().pads[0].choke_group = 2;
        assert!(track
            .set_instrument_with_starting_kit(
                Some(SAMPLER_INSTRUMENT_ID.into()),
                Some(starting_kit)
            )
            .is_none());
        assert_eq!(track.sampler.as_ref().unwrap().pads[0].choke_group, 2);
    }

    fn track_with_plugin(track_id: &str, effect_id: &str, plugin_id: &str) -> AppTrack {
        AppTrack {
            id: track_id.into(),
            name: track_id.into(),
            volume: 80.0,
            pan: 0.0,
            muted: false,
            solo: false,
            clips: vec![],
            effects: vec![TrackEffect {
                id: effect_id.into(),
                enabled: true,
                params: EffectParams::Plugin(PluginParams {
                    plugin_id: plugin_id.into(),
                    name: "P".into(),
                    format: "vst3".into(),
                    params: vec![],
                    sandboxed: false,
                    sidechain_source: None,
                }),
            }],
            modulators: vec![],
            mod_routes: vec![],
            midi_notes: vec![],
            midi_clips: vec![],
            instrument: None,
            sampler: None,
            input_source: None,
            automation_lanes: vec![],
            color: None,
        }
    }

    /// ADR-017: two instances of the same plugin type (same `plugin_id`, distinct
    /// `effect.id`) persist **distinct** state across a save → load round-trip,
    /// keyed per placement by `instance_key`.
    #[test]
    fn plugin_states_round_trip_per_instance() {
        let mut project = AppProject::new();
        project
            .tracks
            .push(track_with_plugin("t1", "fx-a", "vst3://x"));
        project
            .tracks
            .push(track_with_plugin("t2", "fx-b", "vst3://x"));
        project.plugin_states.insert("fx-a".into(), vec![1, 2, 3]);
        project.plugin_states.insert("fx-b".into(), vec![9, 9]);

        let restored = AppProject::from_file(project.to_file()).expect("from_file");
        assert_eq!(restored.plugin_states.get("fx-a"), Some(&vec![1, 2, 3]));
        assert_eq!(restored.plugin_states.get("fx-b"), Some(&vec![9, 9]));
    }

    /// ADR-017 / C-4: a legacy project file carrying `device_id`-keyed state loads
    /// by mapping each blob onto the matching placement's `instance_key`.
    #[test]
    fn legacy_device_keyed_state_maps_to_instance() {
        let mut project = AppProject::new();
        project
            .tracks
            .push(track_with_plugin("t1", "fx-eq", "vst3://x"));

        // Simulate a pre-ADR-017 file: state under the new per-instance field is
        // absent; the legacy device_id-keyed field carries the blob.
        let mut file = project.to_file();
        file.plugin_states.clear();
        file.plugin_state_blobs.insert(
            "vst3://x".into(),
            crate::plugin_system::state::encode_base64(&[7, 7, 7]),
        );

        let restored = AppProject::from_file(file).expect("from_file");
        assert_eq!(
            restored.plugin_states.get("fx-eq"),
            Some(&vec![7, 7, 7]),
            "legacy device_id blob must map onto the effect's instance_key"
        );
    }
}
