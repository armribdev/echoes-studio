use crate::audio_clip::AudioClip;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
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
    pub name: String,
    pub volume: f32,
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
                name: t.name.clone(),
                volume: t.volume,
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
            tracks.push(AppTrack { name: tf.name, volume: tf.volume, clips });
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
            name: self.name.clone(),
            volume: self.volume,
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

// ── Application state ────────────────────────────────────────────────────────

pub struct AppState {
    pub project: Mutex<AppProject>,
    pub project_path: Mutex<Option<PathBuf>>,
}

impl AppState {
    pub fn new() -> Self {
        AppState {
            project: Mutex::new(AppProject::new()),
            project_path: Mutex::new(None),
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
    pub name: String,
    pub volume: f32,
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
    pub name: String,
    pub volume: f32,
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
