use crate::audio_clip::AudioClip;
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
        EqParams {
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
        CompressorParams {
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
        ReverbParams {
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
        DelayParams {
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
    /// Per-device opt-in for the out-of-process sandbox (ADR-015, Phase 6).
    ///
    /// **Declared but not yet consumed** (step 6a): the flag is persisted with
    /// the project so the user's intent survives save/load round-trips, but the
    /// engine still always instantiates the plugin in-process.  The compiler /
    /// host wiring lands in step 6b, the bridge in 6c–6e.
    ///
    /// `#[serde(default)]` keeps projects saved before Phase 6 loading with
    /// `false` → in-process, behaviour strictly unchanged (C-2, C-4, C-9).
    #[serde(default)]
    pub sandboxed: bool,
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
}

fn default_lfo_rate() -> f32 {
    1.0
}

impl Default for LfoParams {
    fn default() -> Self {
        LfoParams {
            rate_hz: 1.0,
            waveform: LfoWaveform::Sine,
        }
    }
}

/// Per-track modulation source (LFO, Envelope, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModulatorParams {
    Lfo(LfoParams),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectModulator {
    pub id: String,
    #[serde(flatten)]
    pub params: ModulatorParams,
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

/// A MIDI clip placed on the timeline.
/// `start`/`end` are in seconds (same coordinate space as audio clips).
/// Notes are beat-relative to the clip start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppMidiClip {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub notes: Vec<MidiNote>,
}

use std::collections::HashMap;
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

pub struct AppClip {
    pub id: String,
    pub start: f64,
    pub end: f64,
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
}

pub struct AppProject {
    pub name: String,
    pub bpm: f32,
    pub numerator: u8,
    pub denominator: u8,
    pub tracks: Vec<AppTrack>,
    /// Per-device binary plugin state (e.g. VST3 chunk), keyed by `device_id`.
    /// Harvested from the live `PluginHost` at save time and re-applied after
    /// the graph is compiled on load.  Not part of the event-sourced model —
    /// it mirrors live plugin internals, so it is excluded from undo snapshots.
    pub plugin_state_blobs: HashMap<String, Vec<u8>>,
}

impl AppProject {
    pub fn new() -> Self {
        AppProject {
            name: "Sans titre".to_string(),
            bpm: 120.0,
            numerator: 4,
            denominator: 4,
            tracks: vec![],
            plugin_state_blobs: HashMap::new(),
        }
    }

    pub fn to_dto(&self) -> ProjectDto {
        ProjectDto {
            name: self.name.clone(),
            bpm: self.bpm,
            numerator: self.numerator,
            denominator: self.denominator,
            tracks: self.tracks.iter().map(AppTrack::to_dto).collect(),
        }
    }

    pub fn to_file(&self) -> ProjectFile {
        ProjectFile {
            version: 1,
            name: self.name.clone(),
            bpm: self.bpm,
            numerator: self.numerator,
            denominator: self.denominator,
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
                            notes: mc.notes.clone(),
                        })
                        .collect(),
                    instrument: t.instrument.clone(),
                })
                .collect(),
            plugin_state_blobs: self
                .plugin_state_blobs
                .iter()
                .map(|(k, v)| (k.clone(), crate::plugin_system::state::encode_base64(v)))
                .collect(),
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
                    file_path: cf.file_path,
                    raw_samples: data.raw_samples,
                    sample_rate: data.sample_rate,
                    peaks_64: data.peaks_64,
                    peaks_512: data.peaks_512,
                    peaks_4096: data.peaks_4096,
                });
            }
            let track_id = tf.id.unwrap_or_else(generate_id);
            let midi_clips = tf
                .midi_clips
                .into_iter()
                .map(|mc| AppMidiClip {
                    id: mc.id,
                    start: mc.start,
                    end: mc.end,
                    notes: mc.notes,
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
            });
        }
        let plugin_state_blobs = pf
            .plugin_state_blobs
            .into_iter()
            .filter_map(|(k, v)| {
                crate::plugin_system::state::decode_base64(&v).map(|bytes| (k, bytes))
            })
            .collect();
        Ok(AppProject {
            name: pf.name,
            bpm: pf.bpm,
            numerator: pf.numerator,
            denominator: pf.denominator,
            tracks,
            plugin_state_blobs,
        })
    }
}

impl AppTrack {
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
                    notes: mc.notes.clone(),
                })
                .collect(),
            instrument: self.instrument.clone(),
        }
    }
}

impl AppClip {
    pub fn to_dto(&self) -> ClipDto {
        ClipDto {
            id: self.id.clone(),
            start: self.start,
            end: self.end,
            file_path: self.file_path.clone(),
            raw_samples: Arc::clone(&self.raw_samples),
            sample_rate: self.sample_rate,
            peaks_64: self.peaks_64.clone(),
            peaks_512: self.peaks_512.clone(),
            peaks_4096: self.peaks_4096.clone(),
        }
    }
}

// Keyed by clip_id. Persists audio data across undo/redo so events can be
// replayed without re-reading audio files from disk.

pub struct AudioDataCache(pub Mutex<HashMap<String, Arc<CachedAudioData>>>);

pub struct CachedAudioData {
    pub raw_samples: Arc<Vec<f32>>,
    pub sample_rate: u32,
    pub peaks_64: Vec<[f32; 2]>,
    pub peaks_512: Vec<[f32; 2]>,
    pub peaks_4096: Vec<[f32; 2]>,
}

impl AudioDataCache {
    pub fn new() -> Self {
        AudioDataCache(Mutex::new(HashMap::new()))
    }

    pub fn get(&self, clip_id: &str) -> Option<Arc<CachedAudioData>> {
        self.0.lock().ok()?.get(clip_id).cloned()
    }

    pub fn insert(&self, clip_id: String, data: Arc<CachedAudioData>) {
        if let Ok(mut map) = self.0.lock() {
            map.insert(clip_id, data);
        }
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
}

impl AppState {
    pub fn new() -> Self {
        // Seed the event store with the initial empty project so that the
        // first undo always has a valid snapshot to return to instead of
        // falling back to AppProject::new() on a None snapshot.
        let mut event_store = crate::engine_gateway::store::EventStore::new();
        event_store.init_from_project(AppProject::new());

        let transport = crate::audio_engine::transport::SharedTransport::new(48_000.0, 120.0);

        let live_params = Arc::new(crate::audio_graph::live_params::LiveParams::new());

        AppState {
            project: Mutex::new(AppProject::new()),
            project_path: Mutex::new(None),
            event_store: Mutex::new(event_store),
            audio_cache: AudioDataCache::new(),
            audio_engine: Mutex::new(crate::audio_engine::engine::AudioEngine::new(
                std::sync::Arc::clone(&transport),
                Arc::clone(&live_params),
            )),
            transport,
            live_params,
            audio_settings: Mutex::new(crate::audio_settings::AudioSettings::default()),
            dirty: AtomicBool::new(false),
            open_plugin_editors: Arc::new(Mutex::new(HashMap::new())),
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
    pub tracks: Vec<TrackFile>,
    /// Base64-encoded per-device plugin state (VST3 chunk), keyed by
    /// `device_id`.  `#[serde(default)]` keeps projects saved before Phase 4b
    /// loading unchanged (empty map → default plugin presets, as before).
    #[serde(default)]
    pub plugin_state_blobs: HashMap<String, String>,
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
}

#[derive(Serialize, Deserialize)]
pub struct MidiClipFile {
    pub id: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub notes: Vec<MidiNote>,
}

#[derive(Serialize, Deserialize)]
pub struct ClipFile {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub file_path: String,
}

#[derive(Serialize, Clone)]
pub struct ProjectDto {
    pub name: String,
    pub bpm: f32,
    pub numerator: u8,
    pub denominator: u8,
    pub tracks: Vec<TrackDto>,
}

#[derive(Serialize, Clone)]
pub struct MidiClipDto {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub notes: Vec<MidiNote>,
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
}

#[derive(Serialize, Clone)]
pub struct ClipDto {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub file_path: String,
    #[serde(serialize_with = "crate::audio_clip::serialize_arc_vec_f32")]
    pub raw_samples: Arc<Vec<f32>>,
    pub sample_rate: u32,
    pub peaks_64: Vec<[f32; 2]>,
    pub peaks_512: Vec<[f32; 2]>,
    pub peaks_4096: Vec<[f32; 2]>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        });

        let file = project.to_file();
        let restored = AppProject::from_file(file).expect("from_file");
        assert_eq!(
            restored.tracks[0].instrument.as_deref(),
            Some("clap://surge.dll?org.surge-synth.surge-xt")
        );
    }
}
