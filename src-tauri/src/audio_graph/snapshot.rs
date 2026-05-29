//! Atomic runtime snapshot — the mechanism by which the compiler thread hands
//! a freshly compiled graph to the audio thread without locking.
//!
//! ## Swap protocol
//!
//! ```text
//! Compiler / command thread             Audio thread
//! ─────────────────────────             ────────────
//! drain dead schedulers
//! build CompiledGraph
//! GraphScheduler::new(compiled)
//! snapshot.enqueue(scheduler)  ───────► snapshot.try_swap(&mut current)
//!                                       (returns the old scheduler to the
//!                                        drop ring; the command thread drains
//!                                        it on its next `enqueue`)
//! ```
//!
//! ## Why this design
//!
//! Earlier revisions wrapped the pending slot in `Mutex<Option<GraphScheduler>>`
//! and let the audio thread *take* the value, which had two real-time-safety
//! violations:
//!
//! 1. `Mutex::try_lock` on Windows calls `AcquireSRWLockExclusive`, which is
//!    not strictly lock-free.
//! 2. The audio thread would then assign the new scheduler into its local
//!    variable, dropping the old one **on the audio thread** — every node, every
//!    output buffer, every clip-sample `Arc` (when last-ref), every reverb
//!    comb/allpass buffer would be deallocated by the OS allocator in the
//!    callback.  A textbook way to produce sporadic underruns.
//!
//! The new implementation:
//!
//! - Publishes pending schedulers through a single `AtomicPtr<GraphScheduler>`.
//!   No mutex anywhere on the audio path.
//! - Provides `try_swap(&mut current)` which atomically replaces the audio
//!   thread's `Box<GraphScheduler>` and pushes the displaced one into a bounded
//!   SPSC ring buffer.  The command thread drains the ring on its next
//!   `enqueue`, performing the actual `drop` off the real-time thread.

use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use crate::audio_graph::scheduler::GraphScheduler;

// ── Drop ring (audio thread → command thread) ────────────────────────────────

/// How many "dead" schedulers we can stash before the command thread drains
/// them.  In steady-state operation the depth is 0 or 1: every audio swap
/// requires a preceding `enqueue`, and every `enqueue` drains the ring.
/// Anything beyond that would require many enqueues without any intervening
/// `enqueue` call to drain — which is impossible by construction.
const DROP_RING_CAPACITY: usize = 8;

/// Bounded single-producer / single-consumer ring buffer of raw pointers
/// awaiting deallocation.
///
/// - Producer = audio thread (in `try_swap`).
/// - Consumer = command thread (in `enqueue` / `Drop`).
struct DropRing {
    slots: [AtomicPtr<GraphScheduler>; DROP_RING_CAPACITY],
    /// Next index the producer will write to.
    head: AtomicUsize,
    /// Next index the consumer will read from.
    tail: AtomicUsize,
}

impl DropRing {
    fn new() -> Self {
        DropRing {
            slots: [
                AtomicPtr::new(ptr::null_mut()),
                AtomicPtr::new(ptr::null_mut()),
                AtomicPtr::new(ptr::null_mut()),
                AtomicPtr::new(ptr::null_mut()),
                AtomicPtr::new(ptr::null_mut()),
                AtomicPtr::new(ptr::null_mut()),
                AtomicPtr::new(ptr::null_mut()),
                AtomicPtr::new(ptr::null_mut()),
            ],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Audio-thread push.  Lock-free, allocation-free, never blocks.
    ///
    /// Returns `true` on success.  On a full ring (unreachable in practice
    /// under the protocol described above) the caller must leak rather than
    /// drop on the real-time thread.
    fn push(&self, ptr: *mut GraphScheduler) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % DROP_RING_CAPACITY;
        if next == self.tail.load(Ordering::Acquire) {
            return false;
        }
        self.slots[head].store(ptr, Ordering::Release);
        self.head.store(next, Ordering::Release);
        true
    }

    /// Command-thread drain.  Drops every queued scheduler.
    fn drain(&self) {
        let head = self.head.load(Ordering::Acquire);
        let mut tail = self.tail.load(Ordering::Relaxed);
        while tail != head {
            let ptr = self.slots[tail].swap(ptr::null_mut(), Ordering::Acquire);
            if !ptr.is_null() {
                // SAFETY: pointers in the ring originate from `Box::into_raw`
                // in `RuntimeSnapshot::try_swap`; ownership has been transferred
                // to this thread by the SPSC discipline.
                unsafe { drop(Box::from_raw(ptr)) };
            }
            tail = (tail + 1) % DROP_RING_CAPACITY;
        }
        self.tail.store(tail, Ordering::Release);
    }
}

// ── RuntimeSnapshot ──────────────────────────────────────────────────────────

/// Shared between the command thread (writer) and the audio thread (reader).
///
/// Holds *at most one* pending `GraphScheduler`.  If the command thread
/// produces a second update before the audio thread has consumed the first,
/// the first is dropped on the command thread (the audio thread will pick up
/// the latest state).
pub struct RuntimeSnapshot {
    /// Pending scheduler waiting to be picked up by the audio thread.
    ///
    /// Stores ownership transferred from `Box::into_raw`.  A null pointer
    /// means "no update pending".
    pending: AtomicPtr<GraphScheduler>,

    /// SPSC ring used by the audio thread to hand displaced schedulers back
    /// to the command thread for deallocation.
    drop_ring: DropRing,
}

// SAFETY:  `GraphScheduler: Send` (declared unsafely in `scheduler.rs`).
// `AtomicPtr` is unconditionally `Send + Sync`, but it does not by itself
// verify that the pointee can cross threads.  Since we always store and
// recover via `Box<GraphScheduler>`, this is sound exactly because
// `Box<GraphScheduler>: Send`.
unsafe impl Send for RuntimeSnapshot {}
unsafe impl Sync for RuntimeSnapshot {}

impl RuntimeSnapshot {
    pub fn new() -> Self {
        RuntimeSnapshot {
            pending: AtomicPtr::new(ptr::null_mut()),
            drop_ring: DropRing::new(),
        }
    }

    // ── Command-thread write path ─────────────────────────────────────────────

    /// Enqueue a new scheduler.  Called on the command thread.
    ///
    /// Two pieces of housekeeping happen here:
    /// 1. Drains the drop ring, freeing every scheduler the audio thread has
    ///    handed back since the last call.  This is the *only* place those
    ///    deallocations happen.
    /// 2. Atomically publishes the new scheduler.  If a previously published
    ///    scheduler was still waiting (audio thread had not picked it up), it
    ///    is dropped here — never on the audio thread.
    pub fn enqueue(&self, scheduler: GraphScheduler) {
        self.drop_ring.drain();

        let new_ptr = Box::into_raw(Box::new(scheduler));
        let old_ptr = self.pending.swap(new_ptr, Ordering::AcqRel);
        if !old_ptr.is_null() {
            // SAFETY: we previously published this pointer via `Box::into_raw`
            // and the audio thread never observed it (it would have swapped in
            // null).  Ownership is ours.
            unsafe { drop(Box::from_raw(old_ptr)) };
        }
    }

    // ── Audio-thread read path ────────────────────────────────────────────────

    /// Non-blocking, allocation-free, drop-free swap.
    ///
    /// If a new scheduler is pending, `*current` is replaced with it and the
    /// previous scheduler is pushed into the drop ring for the command thread
    /// to free.  Returns `true` if a swap happened, `false` otherwise.
    ///
    /// Called at the **start** of each audio block before `process_block`.
    pub fn try_swap(&self, current: &mut Box<GraphScheduler>) -> bool {
        let new_ptr = self.pending.swap(ptr::null_mut(), Ordering::AcqRel);
        if new_ptr.is_null() {
            return false;
        }
        // SAFETY: pointer was produced by `Box::into_raw` in `enqueue` from
        // a `Box<GraphScheduler>`.  We are the single consumer of the pending
        // slot, so ownership is now ours.
        let new_box = unsafe { Box::from_raw(new_ptr) };
        let old_box = std::mem::replace(current, new_box);
        let old_ptr = Box::into_raw(old_box);

        if !self.drop_ring.push(old_ptr) {
            // Ring full — by construction unreachable (see `DROP_RING_CAPACITY`).
            // Falling through here would leak; we choose that over `drop` on
            // the real-time thread.  The leaked scheduler will be reclaimed
            // when the process exits.
        }
        true
    }
}

impl Drop for RuntimeSnapshot {
    fn drop(&mut self) {
        // Drain any schedulers the audio thread queued for deallocation…
        self.drop_ring.drain();
        // …and reclaim anything still parked in the pending slot.
        let ptr = self.pending.swap(ptr::null_mut(), Ordering::AcqRel);
        if !ptr.is_null() {
            // SAFETY: same provenance argument as in `enqueue`.
            unsafe { drop(Box::from_raw(ptr)) };
        }
    }
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self::new()
    }
}
