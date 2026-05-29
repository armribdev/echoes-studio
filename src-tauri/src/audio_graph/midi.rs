//! MIDI event types for intra-block note scheduling.
//!
//! All types here are stack-allocated and `Copy` so the real-time path can
//! use them without allocation.

// ── MIDI event ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct MidiEvent {
    /// Sample offset within the current block (0 ≤ offset < block_size).
    pub offset: u32,
    pub kind: MidiEventKind,
}

#[derive(Debug, Clone, Copy)]
pub enum MidiEventKind {
    NoteOn  { pitch: u8, velocity: u8 },
    NoteOff { pitch: u8 },
}

// ── MidiEventList ─────────────────────────────────────────────────────────────

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
        MidiEventList {
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

    /// Sort events by sample offset (ascending).  At equal offsets, NoteOn
    /// precedes NoteOff so a zero-length note still triggers correctly.
    ///
    /// In-place — no allocation.
    #[inline]
    pub fn sort_by_offset(&mut self) {
        self.events[..self.len].sort_by(|a, b| {
            a.offset.cmp(&b.offset).then_with(|| {
                let priority = |event: &MidiEvent| match event.kind {
                    MidiEventKind::NoteOn { .. } => 0,
                    MidiEventKind::NoteOff { .. } => 1,
                };
                priority(a).cmp(&priority(b))
            })
        });
    }
}

// ── MidiNoteSnapshot ──────────────────────────────────────────────────────────

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
