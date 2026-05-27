use crate::audio_clip::AudioClip;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
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

// ── In-memory structs ────────────────────────────────────────────────────────

pub struct AppClip {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub file_path: String,
    pub raw_samples: Vec<f32>,
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
}

pub struct AppProject {
    pub name: String,
    pub bpm: f32,
    pub numerator: u8,
    pub denominator: u8,
    pub tracks: Vec<AppTrack>,
}

impl AppProject {
    pub fn new() -> Self {
        AppProject {
            name: "Sans titre".to_string(),
            bpm: 120.0,
            numerator: 4,
            denominator: 4,
            tracks: vec![],
        }
    }

    pub fn to_dto(&self) -> ProjectDto {
        ProjectDto {
            name: self.name.clone(),
            bpm: self.bpm,
            numerator: self.numerator,
            denominator: self.denominator,
            tracks: self.tracks.iter().map(|t| t.to_dto()).collect(),
        }
    }

    pub fn to_file(&self) -> ProjectFile {
        ProjectFile {
            version: 1,
            name: self.name.clone(),
            bpm: self.bpm,
            numerator: self.numerator,
            denominator: self.denominator,
            tracks: self.tracks.iter().map(|t| TrackFile {
                id: Some(t.id.clone()),
                name: t.name.clone(),
                volume: t.volume,
                pan: t.pan,
                muted: t.muted,
                solo: t.solo,
                clips: t.clips.iter().map(|c| ClipFile {
                    id: c.id.clone(),
                    start: c.start,
                    end: c.end,
                    file_path: c.file_path.clone(),
                }).collect(),
            }).collect(),
        }
    }

    pub fn from_file(pf: ProjectFile) -> Result<AppProject, String> {
        let mut tracks = Vec::new();
        for tf in pf.tracks {
            let mut clips = Vec::new();
            for cf in tf.clips {
                let audio = AudioClip::from_wav_file(&cf.file_path)
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
            // Migrate: generate id if not present in file
            let track_id = tf.id.unwrap_or_else(generate_id);
            tracks.push(AppTrack {
                id: track_id,
                name: tf.name,
                volume: tf.volume,
                pan: tf.pan,
                muted: tf.muted,
                solo: tf.solo,
                clips,
            });
        }
        Ok(AppProject {
            name: pf.name,
            bpm: pf.bpm,
            numerator: pf.numerator,
            denominator: pf.denominator,
            tracks,
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
            raw_samples: self.raw_samples.clone(),
            sample_rate: self.sample_rate,
            peaks_64: self.peaks_64.clone(),
            peaks_512: self.peaks_512.clone(),
            peaks_4096: self.peaks_4096.clone(),
        }
    }
}

// ── Audio data cache ─────────────────────────────────────────────────────────
//
// Keyed by clip_id. Persists audio data across undo/redo so events can be
// replayed without re-reading audio files from disk.

pub struct AudioDataCache(pub Mutex<HashMap<String, Arc<CachedAudioData>>>);

pub struct CachedAudioData {
    pub raw_samples: Vec<f32>,
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

// ── Application state ────────────────────────────────────────────────────────

pub struct AppState {
    pub project: Mutex<AppProject>,
    pub project_path: Mutex<Option<PathBuf>>,
    /// Event store — sole source of truth for project structure changes.
    pub event_store: Mutex<crate::engine_gateway::store::EventStore>,
    /// In-memory audio data cache — survives undo/redo so replaying events
    /// never re-reads audio files unnecessarily.
    pub audio_cache: AudioDataCache,
    /// Real-time audio engine — reads immutable snapshots only.
    pub audio_engine: Mutex<crate::audio_engine::engine::AudioEngine>,
}

impl AppState {
    pub fn new() -> Self {
        // Seed the event store with the initial empty project so that the
        // first undo always has a valid snapshot to return to instead of
        // falling back to AppProject::new() on a None snapshot.
        let mut event_store = crate::engine_gateway::store::EventStore::new();
        event_store.init_from_project(AppProject::new());
        AppState {
            project: Mutex::new(AppProject::new()),
            project_path: Mutex::new(None),
            event_store: Mutex::new(event_store),
            audio_cache: AudioDataCache::new(),
            audio_engine: Mutex::new(crate::audio_engine::engine::AudioEngine::new()),
        }
    }
}

// ── File format (project saved to disk) ─────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub struct ProjectFile {
    pub version: u32,
    pub name: String,
    pub bpm: f32,
    pub numerator: u8,
    pub denominator: u8,
    pub tracks: Vec<TrackFile>,
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
}

#[derive(Serialize, Deserialize)]
pub struct ClipFile {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub file_path: String,
}

// ── DTOs returned to frontend ────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct ProjectDto {
    pub name: String,
    pub bpm: f32,
    pub numerator: u8,
    pub denominator: u8,
    pub tracks: Vec<TrackDto>,
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
}

#[derive(Serialize, Clone)]
pub struct ClipDto {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub file_path: String,
    pub raw_samples: Vec<f32>,
    pub sample_rate: u32,
    pub peaks_64: Vec<[f32; 2]>,
    pub peaks_512: Vec<[f32; 2]>,
    pub peaks_4096: Vec<[f32; 2]>,
}
