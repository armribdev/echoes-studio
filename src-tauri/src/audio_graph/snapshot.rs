//! Atomic runtime snapshot — the mechanism by which the compiler thread hands
//! a freshly compiled graph to the audio thread without locking.
//!
//! ## Swap protocol
//!
//! ```text
//! Compiler thread                   Audio thread
//! ───────────────                   ────────────
//! build CompiledGraph
//! GraphScheduler::new(compiled)
//! snapshot.enqueue(scheduler)  ───► snapshot.try_take()  (at block boundary)
//!                                   if Some(s) → replace current scheduler
//!                                   else       → continue with old scheduler
//! ```
//!
//! `RuntimeSnapshot` wraps `Mutex<Option<GraphScheduler>>`.  The audio thread
//! calls `try_lock` (non-blocking); if contended it simply keeps its current
//! scheduler for one more block.  Since the compiler only enqueues at
//! structural-change boundaries (never per-frame), contention is near-zero.

use std::sync::Mutex;

use crate::audio_graph::scheduler::GraphScheduler;

/// Shared between the compiler thread (writer) and the audio thread (reader).
///
/// Holds *at most one* pending `GraphScheduler`.  If the compiler produces a
/// second update before the audio thread has consumed the first, the first is
/// silently dropped (the audio thread will pick up the latest state).
pub struct RuntimeSnapshot {
    /// `Some` when a new scheduler is waiting to be picked up by the audio thread.
    pending: Mutex<Option<GraphScheduler>>,
}

impl RuntimeSnapshot {
    pub fn new() -> Self {
        RuntimeSnapshot {
            pending: Mutex::new(None),
        }
    }

    // ── Compiler-thread write path ─────────────────────────────────────────────

    /// Enqueue a new scheduler.  Called on the compiler/command thread.
    ///
    /// Replaces any previously enqueued scheduler that the audio thread has not
    /// yet consumed.
    pub fn enqueue(&self, scheduler: GraphScheduler) {
        if let Ok(mut guard) = self.pending.lock() {
            *guard = Some(scheduler);
        }
    }

    // ── Audio-thread read path ─────────────────────────────────────────────────

    /// Non-blocking attempt to take a pending scheduler.
    ///
    /// Returns `Some(GraphScheduler)` if a new one was waiting; `None` if the
    /// lock was contended or nothing was pending.
    ///
    /// Called at the **start** of each audio block before `process_block`.
    pub fn try_take(&self) -> Option<GraphScheduler> {
        self.pending.try_lock().ok()?.take()
    }
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self::new()
    }
}
