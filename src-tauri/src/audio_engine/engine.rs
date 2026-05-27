//! `AudioEngine` — owns the CPAL stream and the current `GraphScheduler`.
//!
//! ## Architecture
//!
//! ```text
//! Command thread                    Audio thread
//! ──────────────                    ────────────
//! AudioGraphCompiler::compile()
//! GraphScheduler::new(compiled)
//! AudioEngine::update_scheduler()
//!   → snapshot.enqueue(scheduler)  ──► audio_callback:
//!                                         snapshot.try_take() → swap in
//!                                         scheduler.process_block()
//!                                         write to CPAL output buffer
//! ```
//!
//! ## Real-time safety contract
//!
//! The audio callback:
//!   ✓ Never allocates
//!   ✓ Never acquires a blocking lock (uses `try_lock` / `try_take`)
//!   ✓ Never performs I/O
//!   ✓ Never calls `async`/`await`
//!   ✓ Only reads from `GraphScheduler` (owned exclusively during callback)
//!
//! ## CPAL integration
//!
//! CPAL stream creation is intentionally deferred.  During development the
//! Web Audio path in the frontend handles playback.  To activate native audio,
//! implement `start_stream` and uncomment the CPAL callback below.

use std::sync::{Arc, Mutex};

use crate::audio_graph::{
    CompileConfig, GraphScheduler, RuntimeSnapshot, TransportInfo,
};
use crate::audio_engine::graph::AudioGraphCompiler;
use crate::project::AppProject;

// ── AudioEngine ───────────────────────────────────────────────────────────────

/// The audio engine.  Held behind `Mutex<AudioEngine>` in `AppState`.
pub struct AudioEngine {
    // ── Real-time state ───────────────────────────────────────────────────────

    /// Pending scheduler waiting to be picked up by the audio callback.
    snapshot: Arc<RuntimeSnapshot>,

    /// The scheduler currently in use on the audio thread.
    /// Swapped at block boundaries via `snapshot.try_take()`.
    ///
    /// Wrapped in `Mutex` so the non-RT thread can hand it off at startup.
    /// During normal streaming the audio thread owns it exclusively through
    /// CPAL's callback closure.
    current_scheduler: Mutex<GraphScheduler>,

    // ── Transport ─────────────────────────────────────────────────────────────
    pub is_playing: bool,
    pub position_samples: u64,
    pub sample_rate: f32,
    pub bpm: f32,

    // ── Device config ─────────────────────────────────────────────────────────
    /// Audio device block size.
    pub block_size: usize,

    // Future: cpal::Stream stored here once native audio is started
    // _stream: Option<Box<dyn std::any::Any + Send>>,
}

impl AudioEngine {
    pub fn new() -> Self {
        let sample_rate = 48_000.0_f32;
        AudioEngine {
            snapshot: Arc::new(RuntimeSnapshot::new()),
            current_scheduler: Mutex::new(GraphScheduler::empty(sample_rate)),
            is_playing: false,
            position_samples: 0,
            sample_rate,
            bpm: 120.0,
            block_size: 512,
        }
    }

    // ── Scheduler update ──────────────────────────────────────────────────────

    /// Compile a new scheduler from `project` and enqueue it for atomic swap.
    ///
    /// Called by the gateway after any structural change.
    /// Never called from the audio thread.
    pub fn update_from_project(&mut self, project: &AppProject) {
        let config = CompileConfig {
            sample_rate: self.sample_rate,
            max_block_size: self.block_size,
            num_channels: 2,
        };
        let scheduler = AudioGraphCompiler::compile_scheduler(project, config);
        self.snapshot.enqueue(scheduler);
    }

    /// Directly enqueue a pre-built scheduler (used in tests / advanced cases).
    pub fn update_scheduler(&self, scheduler: GraphScheduler) {
        self.snapshot.enqueue(scheduler);
    }

    // ── Transport ─────────────────────────────────────────────────────────────

    pub fn play(&mut self) {
        self.is_playing = true;
    }

    pub fn stop(&mut self) {
        self.is_playing = false;
        self.position_samples = 0;
        // Reset DSP state on next block (via scheduler.reset in callback)
    }

    pub fn pause(&mut self) {
        self.is_playing = false;
    }

    pub fn seek(&mut self, position_seconds: f64) {
        self.position_samples = (position_seconds * self.sample_rate as f64) as u64;
    }

    pub fn position_seconds(&self) -> f64 {
        self.position_samples as f64 / self.sample_rate as f64
    }

    // ── Audio callback (CPAL) ─────────────────────────────────────────────────
    //
    // This is the shape of the audio callback when native CPAL streaming is
    // active.  Currently commented out — activate by:
    //   1. Implementing `start_stream` below.
    //   2. Moving `snapshot` and transport into an `Arc<Mutex<_>>` shared with
    //      the closure.
    //
    // The callback must satisfy the real-time contract:
    //   ✓ No heap allocation
    //   ✓ No blocking lock (try_lock only)
    //   ✓ No I/O
    //   ✓ Reads only from GraphScheduler (owned for the callback's duration)
    //
    // fn audio_callback(
    //     scheduler:  &mut GraphScheduler,
    //     snapshot:   &Arc<RuntimeSnapshot>,
    //     transport:  &mut TransportInfo,
    //     output:     &mut [f32],
    //     block_size: usize,
    // ) {
    //     // Atomically swap in a new scheduler if one is pending
    //     if let Some(new_sched) = snapshot.try_take() {
    //         *scheduler = new_sched;
    //     }
    //
    //     if transport.is_playing {
    //         scheduler.process_block(transport, output);
    //         transport.position_samples += block_size as u64;
    //     } else {
    //         output.fill(0.0);
    //     }
    // }

    // ── Synchronous render (for offline / export use) ─────────────────────────

    /// Render one block synchronously and return the interleaved f32 output.
    ///
    /// Used for WAV export and testing.  Acquires the scheduler mutex for the
    /// duration of the call — safe on non-RT threads.
    pub fn render_block(&mut self, output: &mut [f32]) {
        // Drain any pending scheduler
        if let Some(new_sched) = self.snapshot.try_take() {
            if let Ok(mut guard) = self.current_scheduler.lock() {
                *guard = new_sched;
            }
        }

        let transport = TransportInfo {
            is_playing: self.is_playing,
            position_samples: self.position_samples,
            position_seconds: self.position_seconds(),
            bpm: self.bpm,
            sample_rate: self.sample_rate,
            block_size: self.block_size,
        };

        if let Ok(mut sched) = self.current_scheduler.lock() {
            sched.process_block(&transport, output);
        } else {
            output.fill(0.0);
        }

        if self.is_playing {
            self.position_samples += self.block_size as u64;
        }
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}
