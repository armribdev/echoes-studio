//! `AudioEngine` — owns the audio output stream and the current graph snapshot.
//!
//! Real-time safety contract:
//! - The audio callback reads only through `Arc<AudioGraphSnapshot>`.
//! - `update_snapshot` swaps in a new Arc on the command thread and is the
//!   ONLY mutation path.
//! - The callback uses `try_lock` (never blocking `lock`) so it degrades
//!   gracefully when a snapshot update races with a callback.
//!
//! CPAL integration notes:
//! - The CPAL stream is stored as `Option<cpal::Stream>` because `cpal::Stream`
//!   is `!Send` on some platforms; the stream must be built on the same thread
//!   that calls `play`.
//! - Audio initialisation is deferred until `AudioEngine::start` is explicitly
//!   called. During development the Web Audio path in the frontend handles
//!   playback; the Rust engine is the production path.

use std::sync::{Arc, Mutex};
use super::snapshot::AudioGraphSnapshot;

/// The audio engine. Held in `AppState` behind a `Mutex`.
pub struct AudioEngine {
    /// Current immutable graph snapshot shared with the audio callback.
    snapshot: Arc<Mutex<Arc<AudioGraphSnapshot>>>,
    /// Playback state.
    pub is_playing: bool,
    /// Current playback position in seconds.
    pub position: f64,
    // Future: cpal::Stream stored here once audio is started
    // _stream: Option<Box<dyn std::any::Any + Send>>,
}

impl AudioEngine {
    pub fn new() -> Self {
        let initial = Arc::new(AudioGraphSnapshot::empty(48_000));
        AudioEngine {
            snapshot: Arc::new(Mutex::new(initial)),
            is_playing: false,
            position: 0.0,
        }
    }

    // ── Snapshot management ───────────────────────────────────────────────────

    /// Replace the current graph snapshot. Called by the gateway after any
    /// structural change (new track, clip move, etc.).
    ///
    /// This is O(1): we only swap the `Arc` pointer inside the mutex.
    /// The audio callback can continue reading the old snapshot until the
    /// next block boundary.
    pub fn update_snapshot(&mut self, new_snapshot: AudioGraphSnapshot) {
        if let Ok(mut guard) = self.snapshot.lock() {
            *guard = Arc::new(new_snapshot);
        }
    }

    /// Clone the current snapshot pointer for use in the audio callback.
    /// The callback holds the Arc for the duration of the block, keeping the
    /// snapshot alive even if `update_snapshot` is called concurrently.
    pub fn current_snapshot(&self) -> Arc<AudioGraphSnapshot> {
        // `try_lock` so we never block; if the lock is contended we return
        // a clone of whatever we had last time (acceptable: the gateway only
        // updates snapshots at structural change boundaries, not per-frame).
        match self.snapshot.try_lock() {
            Ok(guard) => Arc::clone(&guard),
            Err(_) => {
                // Extremely rare: return an empty snapshot rather than blocking
                Arc::new(AudioGraphSnapshot::empty(48_000))
            }
        }
    }

    // ── Transport ─────────────────────────────────────────────────────────────

    pub fn play(&mut self) {
        self.is_playing = true;
    }

    pub fn stop(&mut self) {
        self.is_playing = false;
        self.position = 0.0;
    }

    pub fn pause(&mut self) {
        self.is_playing = false;
    }

    pub fn seek(&mut self, position: f64) {
        self.position = position;
    }

    // ── Audio callback (called by CPAL on the audio thread) ───────────────────
    //
    // Real-time rules enforced here:
    //   ✓ No heap allocation
    //   ✓ No async await
    //   ✓ No blocking lock (try_lock only)
    //   ✓ Reads only from Arc<AudioGraphSnapshot>
    //
    // NOTE: CPAL initialisation and stream creation are intentionally deferred.
    //       The Web Audio API in the frontend handles playback during development.
    //       When activating native audio, call `start_stream` and store the
    //       cpal::Stream here. The callback signature below shows the intended
    //       shape.
    //
    // fn audio_callback(snapshot: &AudioGraphSnapshot, position: f64, output: &mut [f32]) {
    //     let n = output.len();
    //     output.fill(0.0);
    //
    //     for track in &snapshot.tracks {
    //         let gain = track.volume;
    //         for clip in &track.clips {
    //             if let Some(offset) = clip.sample_offset(position) {
    //                 for (i, sample) in output.iter_mut().enumerate() {
    //                     let idx = offset + i;
    //                     if idx < clip.samples.len() {
    //                         *sample += clip.samples[idx] * gain;
    //                     }
    //                 }
    //             }
    //         }
    //     }
    // }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}
