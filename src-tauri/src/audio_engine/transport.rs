//! Shared atomic transport state.
//!
//! `SharedTransport` is the single point of truth for playback state that
//! must be readable from **both** the Tauri command thread (PLAY/STOP/SEEK)
//! and the CPAL audio callback, without any blocking synchronisation.
//!
//! ## Threading contract
//!
//! All fields use `Relaxed` ordering.  We only need *visibility* (the audio
//! thread will eventually see the new value within a handful of blocks) — we
//! do **not** need happens-before ordering between the command thread and the
//! audio thread for correctness.
//!
//! ## f32-in-AtomicU32 pattern
//!
//! `f32` has no atomic variant in `std`.  We store the bit-pattern as `u32`
//! and reinterpret with `f32::from_bits` / `f32::to_bits`.  This is safe
//! because we never read back a bit-pattern that was not written by us.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::audio_graph::node::TransportInfo;

// ── SharedTransport ───────────────────────────────────────────────────────────

/// Atomic transport state shared between the command thread and the CPAL callback.
pub struct SharedTransport {
    /// `true` while the engine is rendering audio.
    pub is_playing: AtomicBool,

    /// Playback cursor — absolute position in samples from the start of the
    /// project.  Incremented by the audio callback each block.
    pub position_samples: AtomicU64,

    /// End of the last clip in samples.  `0` means no clips → no auto-stop.
    /// Updated every time the graph is recompiled.
    pub duration_samples: AtomicU64,

    /// BPM stored as the bit-pattern of an `f32`.
    bpm_bits: AtomicU32,

    /// Device sample rate stored as the bit-pattern of an `f32`.
    sample_rate_bits: AtomicU32,
}

impl SharedTransport {
    pub fn new(sample_rate: f32, bpm: f32) -> Arc<Self> {
        Arc::new(SharedTransport {
            is_playing: AtomicBool::new(false),
            position_samples: AtomicU64::new(0),
            duration_samples: AtomicU64::new(0),
            bpm_bits: AtomicU32::new(bpm.to_bits()),
            sample_rate_bits: AtomicU32::new(sample_rate.to_bits()),
        })
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    #[inline]
    pub fn sample_rate(&self) -> f32 {
        f32::from_bits(self.sample_rate_bits.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn set_sample_rate(&self, sr: f32) {
        self.sample_rate_bits.store(sr.to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn bpm(&self) -> f32 {
        f32::from_bits(self.bpm_bits.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn set_bpm(&self, bpm: f32) {
        self.bpm_bits.store(bpm.to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn set_duration(&self, samples: u64) {
        self.duration_samples.store(samples, Ordering::Relaxed);
    }

    // ── Transport commands ────────────────────────────────────────────────────

    /// Start playback from the current position.
    pub fn play(&self) {
        self.is_playing.store(true, Ordering::Relaxed);
    }

    /// Stop playback and reset the cursor to the beginning.
    pub fn stop(&self) {
        self.is_playing.store(false, Ordering::Relaxed);
        self.position_samples.store(0, Ordering::Relaxed);
    }

    /// Pause playback without resetting the cursor.
    pub fn pause(&self) {
        self.is_playing.store(false, Ordering::Relaxed);
    }

    /// Seek to an absolute position in seconds.
    pub fn seek(&self, position_seconds: f64) {
        let pos = (position_seconds * self.sample_rate() as f64) as u64;
        self.position_samples.store(pos, Ordering::Relaxed);
    }

    // ── Audio-thread helpers ──────────────────────────────────────────────────

    /// Snapshot the current transport state into a `TransportInfo` value.
    ///
    /// Called at the start of every audio block (inside the CPAL callback).
    /// No locking — pure atomic reads.
    #[inline]
    pub fn snapshot(&self, block_size: usize) -> TransportInfo {
        let sr = self.sample_rate();
        let pos = self.position_samples.load(Ordering::Relaxed);
        TransportInfo {
            is_playing: self.is_playing.load(Ordering::Relaxed),
            position_samples: pos,
            position_seconds: pos as f64 / sr as f64,
            bpm: self.bpm(),
            sample_rate: sr,
            block_size,
        }
    }
}
