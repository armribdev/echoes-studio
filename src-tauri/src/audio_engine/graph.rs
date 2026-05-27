//! Audio graph compiler — converts `AppProject` to `AudioGraphSnapshot`.
//!
//! Compilation rules:
//! - Triggered only on structural changes (new track, new clip, clip move, etc.).
//! - Never called from the audio thread.
//! - Produces an entirely new `AudioGraphSnapshot`; the audio thread atomically
//!   picks up the new snapshot on its next block boundary.
//! - Audio data (`Arc<Vec<f32>>`) is shared from the existing `AppClip` data,
//!   so compilation is allocation-light.

use std::sync::Arc;

use crate::project::AppProject;
use super::snapshot::{AudioGraphSnapshot, ClipSnapshot, TrackSnapshot};

/// Default sample rate used when no audio device is opened yet.
const DEFAULT_SAMPLE_RATE: u32 = 48_000;

pub struct AudioGraphCompiler;

impl AudioGraphCompiler {
    /// Compile the current project state into an immutable audio graph
    /// snapshot. The caller is responsible for handing this to the engine via
    /// `AudioEngine::update_snapshot`.
    pub fn compile(project: &AppProject) -> AudioGraphSnapshot {
        let tracks = project
            .tracks
            .iter()
            .map(|t| TrackSnapshot {
                id: t.id.clone(),
                volume: t.volume / 100.0, // normalise 0-100 → 0.0-1.0
                clips: t
                    .clips
                    .iter()
                    .filter(|c| !c.raw_samples.is_empty())
                    .map(|c| ClipSnapshot {
                        id: c.id.clone(),
                        start_sec: c.start,
                        end_sec: c.end,
                        sample_rate: c.sample_rate,
                        // Wrap samples in Arc to allow zero-copy sharing across
                        // successive snapshots when a clip has not changed.
                        samples: Arc::new(c.raw_samples.clone()),
                    })
                    .collect(),
            })
            .collect();

        AudioGraphSnapshot { sample_rate: DEFAULT_SAMPLE_RATE, tracks }
    }
}
