//! Immutable audio graph snapshot — the only data structure the audio thread
//! is allowed to touch.
//!
//! Design:
//! - Every field is either a primitive or wrapped in `Arc` (shared, immutable).
//! - The snapshot is created on the command thread and handed to the engine
//!   via `AudioEngine::update_snapshot`. The audio callback takes a clone of
//!   the `Arc<AudioGraphSnapshot>` at the start of each block and reads freely,
//!   without holding any lock for the duration of audio processing.
//! - Audio sample data (`Arc<Vec<f32>>`) is shared across snapshots when a
//!   clip has not changed, so recompilation is allocation-free for unchanged
//!   clips.

use std::sync::Arc;

/// Full description of the audio graph at a point in time.
/// Passed as `Arc<AudioGraphSnapshot>` to the audio callback.
#[derive(Clone, Debug)]
pub struct AudioGraphSnapshot {
    /// Sample rate the engine was initialised with.
    pub sample_rate: u32,
    /// All tracks in playback order.
    pub tracks: Vec<TrackSnapshot>,
}

impl AudioGraphSnapshot {
    pub fn empty(sample_rate: u32) -> Self {
        AudioGraphSnapshot { sample_rate, tracks: vec![] }
    }
}

/// Per-track immutable snapshot.
#[derive(Clone, Debug)]
pub struct TrackSnapshot {
    pub id: String,
    pub volume: f32,
    pub clips: Vec<ClipSnapshot>,
}

/// Per-clip immutable snapshot.
/// Audio data is held in `Arc<Vec<f32>>` so it is zero-copy when the same
/// file is referenced from multiple snapshots (e.g., after a clip move).
#[derive(Clone, Debug)]
pub struct ClipSnapshot {
    pub id: String,
    /// Position of the clip start in the project timeline, in seconds.
    pub start_sec: f64,
    /// Position of the clip end in the project timeline, in seconds.
    pub end_sec: f64,
    /// Native sample rate of the audio data.
    pub sample_rate: u32,
    /// Raw mono samples (normalised f32, −1 … +1).
    pub samples: Arc<Vec<f32>>,
}

impl ClipSnapshot {
    /// Duration of the clip in the timeline (seconds).
    #[inline]
    pub fn duration_sec(&self) -> f64 {
        self.end_sec - self.start_sec
    }

    /// True if the clip should be audible at `position` seconds.
    #[inline]
    pub fn is_active_at(&self, position: f64) -> bool {
        position >= self.start_sec && position < self.end_sec
    }

    /// Sample index within `self.samples` for a given timeline position.
    /// Returns `None` if `position` is outside the clip.
    #[inline]
    pub fn sample_offset(&self, position: f64) -> Option<usize> {
        if !self.is_active_at(position) {
            return None;
        }
        let offset_sec = position - self.start_sec;
        Some((offset_sec * self.sample_rate as f64) as usize)
    }
}
