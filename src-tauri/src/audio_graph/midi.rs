//! MIDI event types for intra-block note scheduling.
//!
//! Event types are stack-allocated and `Copy`. The note list itself is a
//! `Vec` published off the audio thread and swapped in with an atomic pointer.

use std::ptr;
use std::sync::atomic::{fence, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy)]
pub struct MidiEvent {
    /// Sample offset within the current block (0 ≤ offset < block_size).
    pub offset: u32,
    pub kind: MidiEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MidiEventKind {
    NoteOn {
        pitch: u8,
        velocity: u8,
    },
    NoteOff {
        pitch: u8,
    },
    /// Pitch bend, control change or aftertouch from a hardware keyboard, as
    /// raw MIDI bytes on channel 1. Plugin instruments only; built-in ones ignore it.
    Control {
        message: [u8; 3],
    },
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

    /// Sort events by sample offset (ascending).  At equal offsets, NoteOff
    /// precedes NoteOn so that a re-triggered note (NoteOff + NoteOn at the
    /// same offset, e.g. after a seek) releases cleanly before retriggering,
    /// and a control lands before the NoteOn it should shape (bend, mod wheel).
    ///
    /// In-place — no allocation.
    #[inline]
    pub fn sort_by_offset(&mut self) {
        self.events[..self.len].sort_by(|a, b| {
            a.offset.cmp(&b.offset).then_with(|| {
                let priority = |event: &MidiEvent| match event.kind {
                    MidiEventKind::NoteOff { .. } => 0,
                    MidiEventKind::Control { .. } => 1,
                    MidiEventKind::NoteOn { .. } => 2,
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

/// Shared time base for live hardware MIDI (ADR-022 amendment).
///
/// The capture thread stamps each event on arrival; the audio callback stamps
/// each rendered slice. An event is then played exactly one callback period
/// after it arrived, at the matching sample — constant latency, instead of
/// "next block start" whose delay jitters between zero and a whole block.
pub struct LiveInputClock {
    epoch: Instant,
    /// Wall time (ns since `epoch`) that maps to sample 0 of the slice being
    /// rendered. 0 = no live stream (offline render, tests).
    event_window_start: AtomicU64,
    /// Transport sample rendered at `event_window_start`, for MIDI recording.
    event_window_position: AtomicU64,
    /// Delay (ns) between a note reaching the capture thread and the moment the
    /// player hears the timeline position it belongs to. Refreshed every callback.
    record_latency_nanos: AtomicU64,
    /// Seqlock counter so the capture thread never pairs a start with the
    /// position of another slice. Odd while the audio callback is writing.
    window_version: AtomicU64,
}

impl LiveInputClock {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            event_window_start: AtomicU64::new(0),
            event_window_position: AtomicU64::new(0),
            record_latency_nanos: AtomicU64::new(0),
            window_version: AtomicU64::new(0),
        }
    }

    /// Capture thread and audio callback. Monotonic, no lock, no allocation.
    pub fn now_nanos(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    /// Audio callback, before each rendered slice: wall time `nanos` maps to
    /// transport sample `position_samples`. `nanos` 0 = place events at once.
    /// Wait-free (a few atomic stores).
    pub fn set_event_window(&self, nanos: u64, position_samples: u64) {
        let version = self.window_version.load(Ordering::Relaxed);
        self.window_version
            .store(version.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.event_window_position
            .store(position_samples, Ordering::Relaxed);
        self.event_window_start.store(nanos, Ordering::Relaxed);
        self.window_version
            .store(version.wrapping_add(2), Ordering::Release);
    }

    /// Audio callback, once per callback. Wait-free.
    pub fn set_record_latency(&self, nanos: u64) {
        self.record_latency_nanos.store(nanos, Ordering::Relaxed);
    }

    /// Capture thread. Transport sample the player was hearing at wall time
    /// `received_at` — the one they played along with — or `None` when no audio
    /// stream feeds the clock.
    pub fn transport_position_at(&self, received_at: u64, sample_rate: f32) -> Option<u64> {
        let (window_start, window_position) = self.read_event_window();
        if window_start == 0 {
            return None;
        }
        let latency = self.record_latency_nanos.load(Ordering::Relaxed);
        let elapsed_nanos = received_at as i128 - window_start as i128 - latency as i128;
        let elapsed_samples = elapsed_nanos * sample_rate as i128 / 1_000_000_000;
        Some((window_position as i128 + elapsed_samples).max(0) as u64)
    }

    fn read_event_window(&self) -> (u64, u64) {
        loop {
            let before = self.window_version.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let start = self.event_window_start.load(Ordering::Relaxed);
            let position = self.event_window_position.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            if self.window_version.load(Ordering::Relaxed) == before {
                return (start, position);
            }
        }
    }

    fn event_window_start(&self) -> u64 {
        self.event_window_start.load(Ordering::Acquire)
    }
}

/// Where a live event lands relative to the slice being rendered.
#[derive(Debug, PartialEq)]
enum LiveEventSlot {
    At(u32),
    /// Due in a later slice — stays queued.
    Later,
}

fn live_event_slot(
    received_at: u64,
    window_start: u64,
    sample_rate: f32,
    block_size: usize,
) -> LiveEventSlot {
    // No clock, or older than one period (e.g. after a stall): play at once.
    if window_start == 0 || received_at <= window_start {
        return LiveEventSlot::At(0);
    }
    let offset = ((received_at - window_start) as f64 * sample_rate as f64 / 1e9) as usize;
    if offset >= block_size {
        LiveEventSlot::Later
    } else {
        LiveEventSlot::At(offset as u32)
    }
}

/// One hardware MIDI event (MIDI capture thread → audio thread).
#[derive(Debug, Clone, Copy)]
pub struct MidiLiveInputMessage {
    pub kind: MidiEventKind,
    /// `LiveInputClock::now_nanos` when the capture thread received it.
    pub received_at: u64,
}

/// Sized for a dense pitch-bend / mod-wheel stream: a full ring drops the
/// newest message, which could be a NoteOff and leave a note hanging.
const LIVE_INPUT_CAPACITY: usize = 1024;

/// Ring of live hardware MIDI events, lock-free on the consumer side.
///
/// Producers are the MIDI capture threads — one per enabled device, so there
/// can be several; `producer` serializes them (never taken by the audio
/// thread). The live instrument node is the only consumer, and a recompile
/// reuses the same queue so the new node takes over after the snapshot swap.
///
/// Kept separate from `MidiPreviewQueue`: a single consumer per ring, and the
/// audition producer (command thread) must not contend with capture threads.
pub struct MidiLiveInputQueue {
    slots: Box<[std::cell::UnsafeCell<MidiLiveInputMessage>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    producer: Mutex<()>,
    clock: Arc<LiveInputClock>,
}

unsafe impl Send for MidiLiveInputQueue {}
unsafe impl Sync for MidiLiveInputQueue {}

impl MidiLiveInputQueue {
    pub fn new(clock: Arc<LiveInputClock>) -> Self {
        let empty = MidiLiveInputMessage {
            kind: EMPTY_EVENT.kind,
            received_at: 0,
        };
        let mut slots = Vec::with_capacity(LIVE_INPUT_CAPACITY);
        for _ in 0..LIVE_INPUT_CAPACITY {
            slots.push(std::cell::UnsafeCell::new(empty));
        }
        Self {
            slots: slots.into_boxed_slice(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            producer: Mutex::new(()),
            clock,
        }
    }

    /// MIDI capture threads. Drops the newest message when the ring is full.
    pub fn push(&self, message: MidiLiveInputMessage) -> bool {
        let _producer = self
            .producer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) >= LIVE_INPUT_CAPACITY {
            return false;
        }
        unsafe {
            *self.slots[tail % LIVE_INPUT_CAPACITY].get() = message;
        }
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    /// Audio thread. Hands over, in order, each message due in the slice being
    /// rendered with its sample offset; the first one due later stays queued.
    pub fn drain_due(
        &self,
        sample_rate: f32,
        block_size: usize,
        mut f: impl FnMut(MidiLiveInputMessage, u32),
    ) {
        let window_start = self.clock.event_window_start();
        loop {
            let head = self.head.load(Ordering::Relaxed);
            let tail = self.tail.load(Ordering::Acquire);
            if head == tail {
                return;
            }
            let message = unsafe { *self.slots[head % LIVE_INPUT_CAPACITY].get() };
            let LiveEventSlot::At(offset) =
                live_event_slot(message.received_at, window_start, sample_rate, block_size)
            else {
                return;
            };
            self.head.store(head.wrapping_add(1), Ordering::Release);
            f(message, offset);
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

    fn live_note(note_on: bool, pitch: u8, received_at: u64) -> MidiLiveInputMessage {
        let kind = if note_on {
            MidiEventKind::NoteOn {
                pitch,
                velocity: 100,
            }
        } else {
            MidiEventKind::NoteOff { pitch }
        };
        MidiLiveInputMessage { kind, received_at }
    }

    fn drain_all(queue: &MidiLiveInputQueue, block_size: usize) -> Vec<(u8, u32)> {
        let mut drained = Vec::new();
        queue.drain_due(48_000.0, block_size, |message, offset| {
            let (MidiEventKind::NoteOn { pitch, .. } | MidiEventKind::NoteOff { pitch }) =
                message.kind
            else {
                panic!("unexpected control message");
            };
            drained.push((pitch, offset))
        });
        drained
    }

    #[test]
    fn control_sorts_between_note_off_and_note_on_at_the_same_offset() {
        let bend = MidiEventKind::Control {
            message: [0xE0, 0, 0x60],
        };
        let mut list = MidiEventList::new();
        for kind in [
            MidiEventKind::NoteOn {
                pitch: 60,
                velocity: 100,
            },
            bend,
            MidiEventKind::NoteOff { pitch: 60 },
        ] {
            list.push(MidiEvent { offset: 8, kind });
        }
        list.sort_by_offset();
        let kinds: Vec<MidiEventKind> = list.as_slice().iter().map(|e| e.kind).collect();
        assert_eq!(kinds[0], MidiEventKind::NoteOff { pitch: 60 });
        assert_eq!(kinds[1], bend);
        assert!(matches!(kinds[2], MidiEventKind::NoteOn { .. }));
    }

    #[test]
    fn live_input_queue_drains_in_order() {
        let queue = MidiLiveInputQueue::new(Arc::new(LiveInputClock::new()));
        assert!(queue.push(live_note(true, 60, 0)));
        assert!(queue.push(live_note(false, 60, 0)));
        assert_eq!(drain_all(&queue, 256), vec![(60, 0), (60, 0)]);
    }

    #[test]
    fn live_input_queue_drops_newest_when_full() {
        let queue = MidiLiveInputQueue::new(Arc::new(LiveInputClock::new()));
        for i in 0..LIVE_INPUT_CAPACITY {
            assert!(queue.push(live_note(true, i as u8, 0)));
        }
        assert!(!queue.push(live_note(true, 127, 0)));
        assert_eq!(drain_all(&queue, 256).len(), LIVE_INPUT_CAPACITY);
    }

    /// 1 ms after the window start at 48 kHz = sample 48: the event keeps its
    /// position within the period instead of snapping to the block start.
    #[test]
    fn live_event_keeps_its_position_within_the_period() {
        let clock = Arc::new(LiveInputClock::new());
        clock.set_event_window(10_000_000, 0);
        let queue = MidiLiveInputQueue::new(Arc::clone(&clock));
        queue.push(live_note(true, 60, 11_000_000));
        assert_eq!(drain_all(&queue, 256), vec![(60, 48)]);
    }

    /// An event due after this slice waits in the queue for the next one.
    #[test]
    fn live_event_due_later_stays_queued() {
        let clock = Arc::new(LiveInputClock::new());
        clock.set_event_window(10_000_000, 0);
        let queue = MidiLiveInputQueue::new(Arc::clone(&clock));
        queue.push(live_note(true, 60, 11_000_000));
        assert!(drain_all(&queue, 32).is_empty());

        clock.set_event_window(10_000_000 + 32 * 1_000_000_000 / 48_000, 0);
        assert_eq!(drain_all(&queue, 32), vec![(60, 16)]);
    }

    /// 10 ms after a slice that starts at transport sample 96_000, at 48 kHz.
    #[test]
    fn transport_position_follows_the_event_window() {
        let clock = LiveInputClock::new();
        clock.set_event_window(10_000_000, 96_000);
        assert_eq!(
            clock.transport_position_at(20_000_000, 48_000.0),
            Some(96_480)
        );
    }

    #[test]
    fn transport_position_before_the_window_goes_back() {
        let clock = LiveInputClock::new();
        clock.set_event_window(10_000_000, 96_000);
        assert_eq!(
            clock.transport_position_at(9_000_000, 48_000.0),
            Some(95_952)
        );
    }

    /// 5 ms of latency moves a note 240 samples back at 48 kHz.
    #[test]
    fn transport_position_compensates_the_heard_latency() {
        let clock = LiveInputClock::new();
        clock.set_event_window(10_000_000, 96_000);
        clock.set_record_latency(5_000_000);
        assert_eq!(
            clock.transport_position_at(20_000_000, 48_000.0),
            Some(96_240)
        );
    }

    #[test]
    fn transport_position_is_unknown_without_an_audio_stream() {
        assert_eq!(
            LiveInputClock::new().transport_position_at(5, 48_000.0),
            None
        );
    }

    #[test]
    fn live_event_older_than_the_window_plays_at_once() {
        assert_eq!(live_event_slot(5, 10, 48_000.0, 256), LiveEventSlot::At(0));
    }

    #[test]
    fn without_a_live_clock_events_play_at_once() {
        assert_eq!(live_event_slot(99, 0, 48_000.0, 256), LiveEventSlot::At(0));
    }
}
