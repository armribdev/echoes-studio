//! Lock-free SPSC parameter queue — ADR-007.
//!
//! ## Why this exists
//!
//! Before this, a plugin knob turn went `UPDATE_EFFECT → recompile`, and
//! `PluginHost::set_parameter_normalised` only stashed a JSON blob the plugin
//! never saw (audit P0-1).  The wrapper passed `input_param_changes = null`
//! every block, so external plugins were deaf to host-side parameter changes:
//! automation, modulation, MIDI-learn and preset morph were all silent.
//!
//! ADR-007 introduces a bounded single-producer / single-consumer ring per
//! plugin instance.  The command thread pushes [`ParamChange`]s; the wrapper
//! drains them at the top of `process` and converts them into the format-native
//! parameter event (VST3 `IParameterChanges`, CLAP `clap_event_param_value`).
//!
//! ## Real-time contract
//!
//! - The queue is allocated once (off the audio thread) with a fixed capacity.
//!   No allocation, no resize, ever, on either side.
//! - Producer ([`ParamQueue::push`], command thread) and consumer
//!   ([`ParamQueue::drain`], audio thread) each touch only atomic indices and
//!   their own end of the ring.  No mutex, no blocking.
//! - **Exactly one producer and one consumer.**  The producer is the command
//!   thread (gateway → `PluginHost`); the consumer is the audio thread
//!   (wrapper `process`).  This is the only discipline that makes the
//!   `unsafe impl Sync` below sound.
//!
//! ## Overflow policy
//!
//! On a full ring `push` returns `false` and **drops the newest** value.  At a
//! 60 Hz UI a 1024-slot ring buffers ~17 s of edits, so overflow only happens
//! if the audio thread has stalled — in which case the UI will resend the
//! current value on its next tick anyway.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One pending parameter change destined for a plugin instance.
#[derive(Debug, Clone, Copy)]
pub struct ParamChange {
    /// Format-native parameter identifier (VST3 `ParamID`, CLAP `clap_id`).
    /// The frontend already addresses plugin parameters by this id, so no
    /// key→id translation is needed on the hot path.
    pub native_id: u32,
    /// Normalised value in `[0, 1]`.  VST3 consumes this directly; the CLAP
    /// wrapper denormalises it to the parameter's plain range before emitting
    /// `clap_event_param_value`.
    pub normalised: f32,
    /// Intra-block frame offset.  Always `0` in Phase 3 (block-rate, BP-7);
    /// sample-accurate scheduling arrives behind a feature flag later.
    pub sample_offset: u32,
}

/// Fixed-capacity lock-free SPSC ring of [`ParamChange`].
pub struct ParamQueue {
    buf: Box<[UnsafeCell<ParamChange>]>,
    /// Capacity (power of two).
    capacity: usize,
    mask: usize,
    /// Next slot the consumer will read (monotonic, wraps via `mask`).
    head: AtomicUsize,
    /// Next slot the producer will write (monotonic, wraps via `mask`).
    tail: AtomicUsize,
}

// SAFETY: the only shared mutable state is the slot array, guarded by the
// single-producer / single-consumer index protocol (Release/Acquire pairing on
// `tail`/`head`).  A slot is only written by the producer while `tail` has not
// yet been advanced past it, and only read by the consumer after observing the
// advanced `tail` — so producer writes and consumer reads of the same slot are
// never concurrent.  The caller must uphold the one-producer/one-consumer
// invariant documented at the module level.
unsafe impl Send for ParamQueue {}
unsafe impl Sync for ParamQueue {}

impl ParamQueue {
    /// Create a queue with the given capacity, rounded up to a power of two
    /// (minimum 2).  Pre-allocates every slot.
    pub fn with_capacity(requested: usize) -> Self {
        let capacity = requested.next_power_of_two().max(2);
        let mut v = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            v.push(UnsafeCell::new(ParamChange {
                native_id: 0,
                normalised: 0.0,
                sample_offset: 0,
            }));
        }
        Self {
            buf: v.into_boxed_slice(),
            capacity,
            mask: capacity - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Producer side (command thread).  Returns `false` if the ring is full
    /// (newest dropped).
    pub fn push(&self, change: ParamChange) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) >= self.capacity {
            return false; // full — drop newest
        }
        // SAFETY: this slot is not being read by the consumer because we have
        // not yet published it via the `tail` store below.
        unsafe {
            *self.buf[tail & self.mask].get() = change;
        }
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    /// Consumer side (audio thread).  Drains every pending change, calling `f`
    /// for each in FIFO order.
    pub fn drain(&self, mut f: impl FnMut(ParamChange)) {
        loop {
            let head = self.head.load(Ordering::Relaxed);
            let tail = self.tail.load(Ordering::Acquire);
            if head == tail {
                break;
            }
            // SAFETY: the producer published this slot (tail moved past it) and
            // will not overwrite it until we advance `head` below.
            let change = unsafe { *self.buf[head & self.mask].get() };
            self.head.store(head.wrapping_add(1), Ordering::Release);
            f(change);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(id: u32, v: f32) -> ParamChange {
        ParamChange {
            native_id: id,
            normalised: v,
            sample_offset: 0,
        }
    }

    #[test]
    fn capacity_rounds_to_power_of_two() {
        assert_eq!(ParamQueue::with_capacity(1000).capacity, 1024);
        assert_eq!(ParamQueue::with_capacity(1).capacity, 2);
    }

    #[test]
    fn fifo_roundtrip() {
        let q = ParamQueue::with_capacity(8);
        assert!(q.push(pc(1, 0.1)));
        assert!(q.push(pc(2, 0.2)));
        let mut out = Vec::new();
        q.drain(|c| out.push((c.native_id, c.normalised)));
        assert_eq!(out, vec![(1, 0.1), (2, 0.2)]);
        // Drained empty → no more
        let mut count = 0;
        q.drain(|_| count += 1);
        assert_eq!(count, 0);
    }

    #[test]
    fn overflow_drops_newest() {
        let q = ParamQueue::with_capacity(2); // capacity 2
        assert!(q.push(pc(1, 0.0)));
        assert!(q.push(pc(2, 0.0)));
        assert!(!q.push(pc(3, 0.0))); // full → dropped
        let mut ids = Vec::new();
        q.drain(|c| ids.push(c.native_id));
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn wraps_around_many_cycles() {
        let q = ParamQueue::with_capacity(4);
        for round in 0..1000u32 {
            assert!(q.push(pc(round, round as f32)));
            let mut got = None;
            q.drain(|c| got = Some(c.native_id));
            assert_eq!(got, Some(round));
        }
    }
}
