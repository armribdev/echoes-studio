//! MIDI event types for intra-block note scheduling.
//!
//! Event types are stack-allocated and `Copy`. The note list itself is a
//! `Vec` published off the audio thread and swapped in with an atomic pointer.

use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub struct MidiEvent {
    /// Sample offset within the current block (0 ≤ offset < block_size).
    pub offset: u32,
    pub kind: MidiEventKind,
}

#[derive(Debug, Clone, Copy)]
pub enum MidiEventKind {
    NoteOn { pitch: u8, velocity: u8 },
    NoteOff { pitch: u8 },
}

const MAX_EVENTS_PER_BLOCK: usize = 256;
const EMPTY_EVENT: MidiEvent = MidiEvent {
    offset: 0,
    kind: MidiEventKind::NoteOff { pitch: 0 },
};

/// Fixed-capacity list of MIDI events for one audio block.
///
/// Pre-allocated: never allocates in the real-time path.
pub struct MidiEventList {
    events: [MidiEvent; MAX_EVENTS_PER_BLOCK],
    len: usize,
}

impl MidiEventList {
    pub fn new() -> Self {
        Self {
            events: [EMPTY_EVENT; MAX_EVENTS_PER_BLOCK],
            len: 0,
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }

    #[inline]
    pub fn push(&mut self, event: MidiEvent) {
        if self.len < MAX_EVENTS_PER_BLOCK {
            self.events[self.len] = event;
            self.len += 1;
        }
    }

    #[inline]
    pub fn as_slice(&self) -> &[MidiEvent] {
        &self.events[..self.len]
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Sort events by sample offset (ascending).  At equal offsets, NoteOff
    /// precedes NoteOn so that a re-triggered note (NoteOff + NoteOn at the
    /// same offset, e.g. after a seek) releases cleanly before retriggering.
    ///
    /// In-place — no allocation.
    #[inline]
    pub fn sort_by_offset(&mut self) {
        self.events[..self.len].sort_by(|a, b| {
            a.offset.cmp(&b.offset).then_with(|| {
                let priority = |event: &MidiEvent| match event.kind {
                    MidiEventKind::NoteOff { .. } => 0,
                    MidiEventKind::NoteOn { .. } => 1,
                };
                priority(a).cmp(&priority(b))
            })
        });
    }
}

/// A MIDI note stored in beat space for runtime tempo conversion.
///
/// Created at graph compile time (non-RT) from `AppMidiClip` note data.
/// `clip_start_sec` is the clip anchor on the timeline (seconds); note beats
/// are relative to that anchor.  Sample positions are derived per block from
/// the live transport BPM so tempo changes stay in sync without recompiling.
#[derive(Debug, Clone)]
pub struct MidiNoteSnapshot {
    pub pitch: u8,
    pub velocity: u8,
    /// Clip start on the project timeline, in seconds.
    pub clip_start_sec: f64,
    /// Note start in beats, relative to `clip_start_sec`.
    pub start_beats: f32,
    /// Note end in beats, relative to `clip_start_sec`.
    pub end_beats: f32,
    /// Playable window start (clip `start`) in engine samples — tempo-independent.
    /// Notes starting before this are trimmed away (left-trim).
    pub window_start_sample: u64,
    /// Playable window end (clip `end`) in engine samples — tempo-independent.
    /// Notes are silenced past this edge (right-trim).
    pub window_end_sample: u64,
}

impl MidiNoteSnapshot {
    /// Absolute start in engine samples for the given tempo.
    #[inline]
    pub fn start_sample(&self, bpm: f32, sample_rate: f32) -> u64 {
        beats_to_sample(self.clip_start_sec, self.start_beats, bpm, sample_rate)
    }

    /// Absolute end in engine samples for the given tempo.
    #[inline]
    pub fn end_sample(&self, bpm: f32, sample_rate: f32) -> u64 {
        beats_to_sample(self.clip_start_sec, self.end_beats, bpm, sample_rate)
    }
}

/// Convert a beat offset (relative to `clip_start_sec`) to absolute samples.
#[inline]
pub fn beats_to_sample(clip_start_sec: f64, beats: f32, bpm: f32, sample_rate: f32) -> u64 {
    let secs = clip_start_sec + beats as f64 * (60.0 / bpm as f64);
    (secs * sample_rate as f64) as u64
}

const MIDI_DROP_RING_CAPACITY: usize = 8;

/// Lock-free note list shared by the command thread (writer) and the audio
/// thread (reader). A note edit publishes a new `Vec` here; the instrument
/// node picks it up on the next block without rebuilding the graph.
///
/// Displaced lists are returned through an SPSC ring and freed on the command
/// thread. The audio thread never drops them.
pub struct MidiNotePublisher {
    pending: AtomicPtr<Vec<MidiNoteSnapshot>>,
    current: AtomicPtr<Vec<MidiNoteSnapshot>>,
    slots: [AtomicPtr<Vec<MidiNoteSnapshot>>; MIDI_DROP_RING_CAPACITY],
    head: AtomicUsize,
    tail: AtomicUsize,
}

unsafe impl Send for MidiNotePublisher {}
unsafe impl Sync for MidiNotePublisher {}

impl MidiNotePublisher {
    pub fn with_notes(notes: Vec<MidiNoteSnapshot>) -> Arc<Self> {
        Arc::new(Self {
            pending: AtomicPtr::new(ptr::null_mut()),
            current: AtomicPtr::new(Box::into_raw(Box::new(notes))),
            slots: std::array::from_fn(|_| AtomicPtr::new(ptr::null_mut())),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        })
    }

    /// Command thread. Replaces the pending list; the audio thread adopts it
    /// on the next block. A list still waiting in `pending` is dropped here.
    pub fn publish(&self, notes: Vec<MidiNoteSnapshot>) {
        self.drain_retired();
        let new_ptr = Box::into_raw(Box::new(notes));
        let old_ptr = self.pending.swap(new_ptr, Ordering::AcqRel);
        if !old_ptr.is_null() {
            // SAFETY: published by a previous `publish` and never taken by the
            // audio thread (it would have swapped null into `pending`).
            unsafe { drop(Box::from_raw(old_ptr)) };
        }
    }

    /// Audio thread. Adopts a pending list if one is waiting. The displaced
    /// list is pushed to the retire ring — never dropped here.
    pub fn try_swap(&self) -> bool {
        let new_ptr = self.pending.swap(ptr::null_mut(), Ordering::AcqRel);
        if new_ptr.is_null() {
            return false;
        }
        let old_ptr = self.current.swap(new_ptr, Ordering::AcqRel);
        if !old_ptr.is_null() && !self.retire(old_ptr) {
            // Ring full — leak rather than drop on the audio thread.
        }
        true
    }

    /// Audio thread. The returned slice stays valid for this block: `current`
    /// is only freed after the node has left the audio thread, or after a
    /// later `try_swap` has retired it and the command thread has drained.
    pub fn notes(&self) -> &[MidiNoteSnapshot] {
        let ptr = self.current.load(Ordering::Acquire);
        if ptr.is_null() {
            &[]
        } else {
            // SAFETY: `current` is installed by `with_notes` or `try_swap`
            // from `Box::into_raw`, and is not freed while this thread is
            // inside `process`.
            unsafe { &*ptr }
        }
    }

    fn retire(&self, ptr: *mut Vec<MidiNoteSnapshot>) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % MIDI_DROP_RING_CAPACITY;
        if next == self.tail.load(Ordering::Acquire) {
            return false;
        }
        self.slots[head].store(ptr, Ordering::Release);
        self.head.store(next, Ordering::Release);
        true
    }

    fn drain_retired(&self) {
        let head = self.head.load(Ordering::Acquire);
        let mut tail = self.tail.load(Ordering::Relaxed);
        while tail != head {
            let ptr = self.slots[tail].swap(ptr::null_mut(), Ordering::Acquire);
            if !ptr.is_null() {
                // SAFETY: retired by `try_swap` via `Box::into_raw` of the
                // previous `current`. SPSC: this thread is the only consumer.
                unsafe { drop(Box::from_raw(ptr)) };
            }
            tail = (tail + 1) % MIDI_DROP_RING_CAPACITY;
        }
        self.tail.store(tail, Ordering::Release);
    }
}

/// One audition message from the piano roll (command thread → audio thread).
#[derive(Debug, Clone, Copy)]
pub struct MidiPreviewMessage {
    pub note_on: bool,
    pub pitch: u8,
    pub velocity: u8,
}

const PREVIEW_CAPACITY: usize = 64;

/// Lock-free SPSC ring of piano-roll audition messages.
///
/// The command thread is the only producer. The live instrument node is the
/// only consumer, and a recompile reuses the same queue so the new node takes
/// over after the snapshot swap — the previous node is no longer processed.
pub struct MidiPreviewQueue {
    slots: Box<[std::cell::UnsafeCell<MidiPreviewMessage>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

unsafe impl Send for MidiPreviewQueue {}
unsafe impl Sync for MidiPreviewQueue {}

impl MidiPreviewQueue {
    pub fn new() -> Self {
        let empty = MidiPreviewMessage {
            note_on: false,
            pitch: 0,
            velocity: 0,
        };
        let mut slots = Vec::with_capacity(PREVIEW_CAPACITY);
        for _ in 0..PREVIEW_CAPACITY {
            slots.push(std::cell::UnsafeCell::new(empty));
        }
        Self {
            slots: slots.into_boxed_slice(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Command thread. Drops the newest message when the ring is full.
    pub fn push(&self, message: MidiPreviewMessage) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) >= PREVIEW_CAPACITY {
            return false;
        }
        unsafe {
            *self.slots[tail % PREVIEW_CAPACITY].get() = message;
        }
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    /// Audio thread. Drains every pending message in order.
    pub fn drain(&self, mut f: impl FnMut(MidiPreviewMessage)) {
        loop {
            let head = self.head.load(Ordering::Relaxed);
            let tail = self.tail.load(Ordering::Acquire);
            if head == tail {
                return;
            }
            let message = unsafe { *self.slots[head % PREVIEW_CAPACITY].get() };
            self.head.store(head.wrapping_add(1), Ordering::Release);
            f(message);
        }
    }
}

impl Drop for MidiNotePublisher {
    fn drop(&mut self) {
        self.drain_retired();
        for ptr in [
            self.pending.swap(ptr::null_mut(), Ordering::AcqRel),
            self.current.swap(ptr::null_mut(), Ordering::AcqRel),
        ] {
            if !ptr.is_null() {
                unsafe { drop(Box::from_raw(ptr)) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks the beats→samples resolution: at constant tempo it must be exact
    /// and deterministic.  This is the bit-identical guard (C-2 / BP-3) — any
    /// future change to the resolution maths trips this test.
    #[test]
    fn beats_to_sample_is_deterministic_at_constant_tempo() {
        assert_eq!(beats_to_sample(0.0, 0.0, 120.0, 48_000.0), 0);
        assert_eq!(beats_to_sample(0.0, 1.0, 120.0, 48_000.0), 24_000);
        assert_eq!(beats_to_sample(0.0, 2.0, 120.0, 48_000.0), 48_000);
    }

    /// The clip anchor is tempo-independent (seconds); only the beat offset
    /// scales with tempo.  Verifies the mixed seconds/beats project model.
    #[test]
    fn clip_anchor_is_tempo_independent() {
        assert_eq!(beats_to_sample(1.0, 0.0, 120.0, 48_000.0), 48_000);
        assert_eq!(beats_to_sample(1.0, 0.0, 240.0, 48_000.0), 48_000);
    }

    /// The same snapshot resolves to a different sample position when the
    /// tempo changes — no rebake, no recompile.  This is the core ADR-012
    /// property exercised at the snapshot level.
    #[test]
    fn tempo_change_rescales_note_without_rebake() {
        let note = MidiNoteSnapshot {
            pitch: 60,
            velocity: 100,
            clip_start_sec: 0.0,
            start_beats: 1.0,
            end_beats: 2.0,
            window_start_sample: 0,
            window_end_sample: u64::MAX,
        };
        assert_eq!(note.start_sample(120.0, 48_000.0), 24_000);
        assert_eq!(note.end_sample(120.0, 48_000.0), 48_000);
        assert_eq!(note.start_sample(240.0, 48_000.0), 12_000);
        assert_eq!(note.end_sample(240.0, 48_000.0), 24_000);
    }
}
