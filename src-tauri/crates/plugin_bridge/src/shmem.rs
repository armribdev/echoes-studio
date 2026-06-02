//! Region-agnostic SPSC ring buffer for the plugin bridge (ADR-015, Phase 6c-i).
//!
//! ## Why not reuse the engine's in-process `ParamQueue`?
//!
//! `plugin_system::param_queue::ParamQueue` is single-producer / single-consumer
//! and lock-free, but it stores its slots in a `Box<[UnsafeCell<_>]>` — heap
//! pointers that are
//! **meaningless in another process's address space**.  The bridge needs a ring
//! whose entire state (control indices *and* slot data) lives inside one
//! contiguous **raw byte region** that both processes map.  This type is that
//! ring: it owns no memory, it is a *view* over a caller-supplied region.
//!
//! ```text
//!   region base ─►┌──────────────────────────────── RingHeader (16 B) ──┐
//!                 │ capacity:u32 │ slot_size:u32 │ head:AtomicU32 │ tail │
//!                 ├──────────────────────────────────────────────────────┤
//!                 │ slot 0: [len:u32][payload … slot_size B]              │
//!                 │ slot 1: [len:u32][payload …]                          │
//!                 │ …                                                     │
//!                 └──────────────────────────────────────────────────────┘
//! ```
//!
//! ## Real-time contract (relevant from step 6d, validated here)
//!
//! - No allocation, no lock, no syscall in [`push`](ShmemRing::push) /
//!   [`pop`](ShmemRing::pop): each is a bounds check plus a fixed-size
//!   `copy_nonoverlapping`.
//! - **Exactly one producer and one consumer.**  In production they are
//!   different processes; the producer owns the `tail`, the consumer owns the
//!   `head`.  The `Release`/`Acquire` pairing on `tail`/`head` is what makes the
//!   slot writes/reads non-overlapping.
//! - Overflow policy: a full ring makes `push` return `false` (drop newest),
//!   mirroring `ParamQueue`.
//!
//! ## Memory placement & alignment
//!
//! The control indices are `AtomicU32` embedded at the start of the region, so
//! the region base must be at least 4-byte aligned.  An OS shared-memory mapping
//! is page-aligned; the test backing [`OwnedRegion`] is 8-byte aligned.  `init`
//! / `attach` assert the alignment.

use std::sync::atomic::{AtomicU32, Ordering};

/// Bytes occupied by the ring header at the start of the region.
/// Layout: `capacity:u32 | slot_size:u32 | head:AtomicU32 | tail:AtomicU32`.
const HEADER_BYTES: usize = 16;

const OFF_CAPACITY: usize = 0;
const OFF_SLOT_SIZE: usize = 4;
const OFF_HEAD: usize = 8;
const OFF_TAIL: usize = 12;

#[inline]
const fn align_up(n: usize, align: usize) -> usize {
    (n + align - 1) & !(align - 1)
}

/// Describes a ring's shape so a caller can size the shared region before
/// mapping it.  `capacity` is rounded up to a power of two (minimum 2).
#[derive(Debug, Clone, Copy)]
pub struct RingLayout {
    /// Number of message slots (power of two after construction).
    pub capacity: usize,
    /// Maximum payload bytes per message.
    pub slot_size: usize,
}

impl RingLayout {
    /// Build a layout, rounding `capacity` up to a power of two (min 2).
    pub fn new(capacity: usize, slot_size: usize) -> Self {
        RingLayout {
            capacity: capacity.next_power_of_two().max(2),
            slot_size,
        }
    }

    /// Per-slot stride: a 4-byte length prefix plus the payload, 4-byte aligned.
    #[inline]
    fn stride(&self) -> usize {
        align_up(4 + self.slot_size, 4)
    }

    /// Total bytes the shared region must provide for this ring.
    pub fn bytes_required(&self) -> usize {
        HEADER_BYTES + self.capacity * self.stride()
    }
}

/// A lock-free SPSC ring **view** over a raw byte region.
///
/// The view owns no memory; the region it points into must outlive it and must
/// not be moved.  Create the producer side with [`ShmemRing::init`] (which
/// stamps the header and zeroes the indices) and the consumer side with
/// [`ShmemRing::attach`] (which reads the header without touching the indices).
pub struct ShmemRing {
    head_ptr: *const AtomicU32,
    tail_ptr: *const AtomicU32,
    data: *mut u8,
    capacity: usize,
    slot_size: usize,
    stride: usize,
    mask: usize,
}

// SAFETY: the only shared mutable state is the slot array and the two atomic
// indices, all guarded by the single-producer / single-consumer index protocol
// (Release/Acquire pairing on tail/head — identical to `ParamQueue`).  A view is
// used by exactly one side; moving a view to its owning thread/process requires
// `Send`.  We do NOT implement `Sync`: a single view is never shared between
// threads — the two ends hold two distinct views over the same region.
unsafe impl Send for ShmemRing {}

impl ShmemRing {
    /// Initialise a ring at `base` (producer/owner side): write the header and
    /// reset `head`/`tail` to 0.  `len` must be at least `layout.bytes_required()`.
    ///
    /// # Safety
    /// - `base` must point to at least `len` bytes of writable memory, 4-byte
    ///   aligned, valid for the lifetime of the returned view.
    /// - No other `ShmemRing` may be initialising the same region concurrently.
    pub unsafe fn init(base: *mut u8, len: usize, layout: RingLayout) -> Self {
        assert!(!base.is_null(), "ShmemRing::init: null region");
        assert_eq!(
            base as usize % 4,
            0,
            "ShmemRing::init: region must be 4-byte aligned"
        );
        assert!(
            len >= layout.bytes_required(),
            "ShmemRing::init: region too small ({len} < {})",
            layout.bytes_required()
        );

        // Stamp header.  capacity/slot_size are immutable for the ring's life.
        core::ptr::write_unaligned(base.add(OFF_CAPACITY) as *mut u32, layout.capacity as u32);
        core::ptr::write_unaligned(base.add(OFF_SLOT_SIZE) as *mut u32, layout.slot_size as u32);
        (*(base.add(OFF_HEAD) as *const AtomicU32)).store(0, Ordering::Release);
        (*(base.add(OFF_TAIL) as *const AtomicU32)).store(0, Ordering::Release);

        Self::from_region(base, layout.capacity, layout.slot_size)
    }

    /// Attach to a ring another party already initialised (consumer/peer side):
    /// read the header, leave `head`/`tail` untouched.
    ///
    /// # Safety
    /// Same pointer/alignment/lifetime requirements as [`init`](Self::init); the
    /// region must already have been initialised by the producer.
    pub unsafe fn attach(base: *mut u8, len: usize) -> Self {
        assert!(!base.is_null(), "ShmemRing::attach: null region");
        assert_eq!(
            base as usize % 4,
            0,
            "ShmemRing::attach: region must be 4-byte aligned"
        );
        assert!(
            len >= HEADER_BYTES,
            "ShmemRing::attach: region smaller than header"
        );

        let capacity = core::ptr::read_unaligned(base.add(OFF_CAPACITY) as *const u32) as usize;
        let slot_size = core::ptr::read_unaligned(base.add(OFF_SLOT_SIZE) as *const u32) as usize;
        assert!(
            capacity.is_power_of_two(),
            "ShmemRing::attach: corrupt capacity {capacity}"
        );

        let layout = RingLayout {
            capacity,
            slot_size,
        };
        assert!(
            len >= layout.bytes_required(),
            "ShmemRing::attach: region too small for declared shape"
        );

        Self::from_region(base, capacity, slot_size)
    }

    unsafe fn from_region(base: *mut u8, capacity: usize, slot_size: usize) -> Self {
        let stride = align_up(4 + slot_size, 4);
        ShmemRing {
            head_ptr: base.add(OFF_HEAD) as *const AtomicU32,
            tail_ptr: base.add(OFF_TAIL) as *const AtomicU32,
            data: base.add(HEADER_BYTES),
            capacity,
            slot_size,
            stride,
            mask: capacity - 1,
        }
    }

    /// Maximum payload bytes a single message may carry.
    #[inline]
    pub fn slot_size(&self) -> usize {
        self.slot_size
    }

    /// Number of message slots.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    fn head(&self) -> &AtomicU32 {
        unsafe { &*self.head_ptr }
    }
    #[inline]
    fn tail(&self) -> &AtomicU32 {
        unsafe { &*self.tail_ptr }
    }

    /// Producer side.  Enqueue `bytes` as one message.  Returns `false` if the
    /// payload exceeds `slot_size` or the ring is full (newest dropped).
    pub fn push(&self, bytes: &[u8]) -> bool {
        if bytes.len() > self.slot_size {
            return false;
        }
        let head = self.head().load(Ordering::Acquire);
        let tail = self.tail().load(Ordering::Relaxed);
        if tail.wrapping_sub(head) >= self.capacity as u32 {
            return false; // full
        }
        let off = (tail as usize & self.mask) * self.stride;
        // SAFETY: this slot is not being read by the consumer because we have not
        // yet published it via the `tail` store below.  `off + stride` is within
        // the region (init/attach validated the size).
        unsafe {
            let slot = self.data.add(off);
            core::ptr::write_unaligned(slot as *mut u32, bytes.len() as u32);
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), slot.add(4), bytes.len());
        }
        self.tail().store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    /// Consumer side.  Dequeue the oldest message into `out`, returning its
    /// length, or `None` if the ring is empty.
    ///
    /// `out` should be at least [`slot_size`](Self::slot_size) bytes; a shorter
    /// buffer truncates the copy while the returned length still reports the true
    /// payload size.
    pub fn pop(&self, out: &mut [u8]) -> Option<usize> {
        let tail = self.tail().load(Ordering::Acquire);
        let head = self.head().load(Ordering::Relaxed);
        if head == tail {
            return None; // empty
        }
        let off = (head as usize & self.mask) * self.stride;
        // SAFETY: the producer published this slot (it advanced `tail` past it)
        // and will not overwrite it until we advance `head` below.  Clamp the
        // declared length to `slot_size` to stay in-bounds even if the region
        // was corrupted by a faulty peer.
        let len = unsafe {
            (core::ptr::read_unaligned(self.data.add(off) as *const u32) as usize)
                .min(self.slot_size)
        };
        let n = len.min(out.len());
        unsafe {
            let slot = self.data.add(off);
            core::ptr::copy_nonoverlapping(slot.add(4), out.as_mut_ptr(), n);
        }
        self.head().store(head.wrapping_add(1), Ordering::Release);
        Some(len)
    }
}

/// Owned, aligned heap backing for a ring — **test / single-process use only**.
///
/// Production code maps an OS shared-memory object instead; this exists so the
/// ring can be exercised in isolation (the plan's "unit-tested in isolation")
/// without spawning a second process.  Backed by `Vec<u64>` to guarantee 8-byte
/// alignment (≥ the 4-byte the header needs).
pub struct OwnedRegion {
    _buf: Vec<u64>,
    ptr: *mut u8,
    len: usize,
}

impl OwnedRegion {
    /// Allocate a zeroed region of at least `bytes`.
    pub fn new(bytes: usize) -> Self {
        let words = align_up(bytes, 8) / 8;
        let mut buf = vec![0u64; words.max(1)];
        let ptr = buf.as_mut_ptr() as *mut u8;
        let len = buf.len() * 8;
        OwnedRegion {
            _buf: buf,
            ptr,
            len,
        }
    }

    /// Raw base pointer (stable for the lifetime of the region).
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.ptr
    }
    /// Region length in bytes.
    pub fn len(&self) -> usize {
        self.len
    }
    /// Whether the region is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(capacity: usize, slot_size: usize) -> (OwnedRegion, ShmemRing) {
        let layout = RingLayout::new(capacity, slot_size);
        let region = OwnedRegion::new(layout.bytes_required());
        let ring = unsafe { ShmemRing::init(region.as_mut_ptr(), region.len(), layout) };
        (region, ring)
    }

    #[test]
    fn layout_rounds_capacity_and_sizes_region() {
        let l = RingLayout::new(1000, 12);
        assert_eq!(l.capacity, 1024);
        // stride = align_up(4 + 12, 4) = 16
        assert_eq!(l.bytes_required(), HEADER_BYTES + 1024 * 16);
    }

    #[test]
    fn push_pop_roundtrips_exact_payload() {
        let (_r, ring) = ring(8, 8);
        assert!(ring.push(&[1, 2, 3, 4]));
        let mut out = [0u8; 8];
        assert_eq!(ring.pop(&mut out), Some(4));
        assert_eq!(&out[..4], &[1, 2, 3, 4]);
        assert_eq!(ring.pop(&mut out), None);
    }

    #[test]
    fn preserves_fifo_order() {
        let (_r, ring) = ring(8, 4);
        assert!(ring.push(&[10]));
        assert!(ring.push(&[20]));
        assert!(ring.push(&[30]));
        let mut out = [0u8; 4];
        for expected in [10u8, 20, 30] {
            assert_eq!(ring.pop(&mut out), Some(1));
            assert_eq!(out[0], expected);
        }
    }

    #[test]
    fn full_ring_drops_newest() {
        let (_r, ring) = ring(2, 4); // capacity 2
        assert!(ring.push(&[1]));
        assert!(ring.push(&[2]));
        assert!(!ring.push(&[3])); // full
        let mut out = [0u8; 4];
        assert_eq!(ring.pop(&mut out).map(|_| out[0]), Some(1));
        assert_eq!(ring.pop(&mut out).map(|_| out[0]), Some(2));
        assert_eq!(ring.pop(&mut out), None);
    }

    #[test]
    fn rejects_oversized_payload() {
        let (_r, ring) = ring(4, 4);
        assert!(!ring.push(&[0u8; 5])); // > slot_size
        assert!(ring.push(&[0u8; 4])); // == slot_size ok
    }

    #[test]
    fn wraps_around_many_cycles() {
        let (_r, ring) = ring(4, 4);
        let mut out = [0u8; 4];
        for round in 0u32..1000 {
            assert!(ring.push(&round.to_le_bytes()));
            assert_eq!(ring.pop(&mut out), Some(4));
            assert_eq!(u32::from_le_bytes(out), round);
        }
    }

    #[test]
    fn attach_reads_producer_header() {
        let layout = RingLayout::new(16, 32);
        let region = OwnedRegion::new(layout.bytes_required());
        let producer = unsafe { ShmemRing::init(region.as_mut_ptr(), region.len(), layout) };
        let consumer = unsafe { ShmemRing::attach(region.as_mut_ptr(), region.len()) };
        assert_eq!(consumer.capacity(), 16);
        assert_eq!(consumer.slot_size(), 32);

        producer.push(b"hello").then_some(()).unwrap();
        let mut out = [0u8; 32];
        assert_eq!(consumer.pop(&mut out), Some(5));
        assert_eq!(&out[..5], b"hello");
    }

    #[test]
    fn two_views_cross_thread_simulates_two_processes() {
        // One region, two independent views (init + attach), moved to two
        // threads: the SPSC discipline a real cross-process bridge relies on.
        let layout = RingLayout::new(64, 8);
        let region = OwnedRegion::new(layout.bytes_required());
        let base = region.as_mut_ptr();
        let len = region.len();
        let producer = unsafe { ShmemRing::init(base, len, layout) };
        let consumer = unsafe { ShmemRing::attach(base, len) };

        const N: u32 = 100_000;
        std::thread::scope(|s| {
            s.spawn(move || {
                let mut i = 0u32;
                while i < N {
                    if producer.push(&i.to_le_bytes()) {
                        i += 1;
                    } else {
                        std::hint::spin_loop(); // ring full — let consumer catch up
                    }
                }
            });
            s.spawn(move || {
                let mut out = [0u8; 8];
                let mut next = 0u32;
                while next < N {
                    if consumer.pop(&mut out).is_some() {
                        assert_eq!(u32::from_le_bytes([out[0], out[1], out[2], out[3]]), next);
                        next += 1;
                    } else {
                        std::hint::spin_loop(); // empty — let producer get ahead
                    }
                }
            });
        });
        // region drops here, after both threads joined.
    }
}
