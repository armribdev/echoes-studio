//! Real-time audio engine.
//!
//! Architecture invariants (must never be violated):
//! - The audio thread reads ONLY from `AudioGraphSnapshot` (immutable, Arc-wrapped).
//! - The audio callback never acquires a Mutex; it uses `try_lock` to avoid
//!   blocking. If the lock is contended (graph recompile in progress) the
//!   callback silently continues with the previous snapshot.
//! - No heap allocation inside the audio callback.
//! - No async runtime interaction from the audio thread.
//!
//! The `AudioEngine` struct owns the CPAL stream and the current snapshot
//! pointer. Graph recompilation happens on the command thread via
//! `AudioGraphCompiler::compile`, which produces a new `AudioGraphSnapshot`.
//! The engine swaps in the new snapshot atomically before the next callback.

pub mod engine;
pub mod graph;
pub mod snapshot;
