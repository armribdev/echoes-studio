//! Real-time audio engine.
//!
//! ## Architecture (updated)
//!
//! The engine is now backed by the `audio_graph` subsystem:
//!
//! ```text
//! AppProject
//!   → AudioGraphCompiler::compile_scheduler()   (engine/graph.rs bridge)
//!       → AudioGraph::from_project()            (audio_graph::graph)
//!       → GraphCompiler::compile()              (audio_graph::compiler)
//!       → GraphScheduler::new()                 (audio_graph::scheduler)
//!   → RuntimeSnapshot::enqueue()                (audio_graph::snapshot)
//!   → AudioEngine::render_block() / CPAL CB     (engine/engine.rs)
//! ```
//!
//! ## Invariants (must never be violated)
//!
//! - The audio callback reads ONLY from `GraphScheduler` (immutable plan +
//!   owned mutable DSP state).
//! - The callback never acquires a blocking `Mutex`; it uses
//!   `RuntimeSnapshot::try_swap` (lock-free `AtomicPtr`).
//! - No heap allocation inside the audio callback.
//! - No `drop` inside the audio callback; displaced schedulers are handed back
//!   to the command thread via `RuntimeSnapshot`'s SPSC drop ring.
//! - No async runtime interaction from the audio thread.
//!
//! ## Legacy types
//!
//! `snapshot.rs` retains `AudioGraphSnapshot` / `TrackSnapshot` / `ClipSnapshot`
//! for backward compatibility during the migration.  They will be removed once
//! all references are updated.

pub mod engine;
pub mod graph;
pub mod offline;
#[cfg(test)]
mod render_harness;
pub mod snapshot;
pub mod transport;
