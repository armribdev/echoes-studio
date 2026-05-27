//! Legacy snapshot types — kept for backward compatibility.
//!
//! These types are no longer used by the audio engine itself (which now uses
//! `audio_graph::GraphScheduler` and `audio_graph::RuntimeSnapshot`), but
//! they are retained so that any code that directly constructs or inspects
//! `AudioGraphSnapshot` continues to compile during the migration period.
//!
//! ## Migration status
//!
//! The new pipeline is:
//!
//! ```text
//! AppProject
//!   → AudioGraph::from_project()        (audio_graph::graph)
//!   → GraphCompiler::compile()          (audio_graph::compiler)
//!   → GraphScheduler                    (audio_graph::scheduler)
//!   → RuntimeSnapshot::enqueue()        (audio_graph::snapshot)
//!   → AudioEngine::update_scheduler()
//! ```
//!
//! The old `AudioGraphSnapshot` / `TrackSnapshot` / `ClipSnapshot` types are
//! no longer on the hot path.  They will be removed once the migration is
//! complete and all call sites have been updated.

use std::sync::Arc;

/// Legacy: full snapshot passed to the (now retired) flat audio callback.
#[derive(Clone, Debug)]
pub struct AudioGraphSnapshot {
    pub sample_rate: u32,
    pub tracks: Vec<TrackSnapshot>,
}

impl AudioGraphSnapshot {
    pub fn empty(sample_rate: u32) -> Self {
        AudioGraphSnapshot { sample_rate, tracks: vec![] }
    }
}

/// Legacy per-track snapshot.
#[derive(Clone, Debug)]
pub struct TrackSnapshot {
    pub id: String,
    pub volume: f32,
    pub clips: Vec<ClipSnapshot>,
}

/// Legacy per-clip snapshot.
#[derive(Clone, Debug)]
pub struct ClipSnapshot {
    pub id: String,
    pub start_sec: f64,
    pub end_sec: f64,
    pub sample_rate: u32,
    pub samples: Arc<Vec<f32>>,
}

impl ClipSnapshot {
    pub fn duration_sec(&self) -> f64 { self.end_sec - self.start_sec }

    pub fn is_active_at(&self, position: f64) -> bool {
        position >= self.start_sec && position < self.end_sec
    }

    pub fn sample_offset(&self, position: f64) -> Option<usize> {
        if !self.is_active_at(position) { return None; }
        let offset_sec = position - self.start_sec;
        Some((offset_sec * self.sample_rate as f64) as usize)
    }
}
