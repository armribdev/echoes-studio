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
}

// ── MidiNoteSnapshot ──────────────────────────────────────────────────────────

/// A MIDI note converted to absolute project-sample positions.
///
/// Created at graph compile time (non-RT) from `AppMidiClip` note data.
/// Stored inside `InstrumentNode` to drive per-block event scheduling.
#[derive(Debug, Clone)]
pub struct MidiNoteSnapshot {
    pub pitch: u8,
    pub velocity: u8,
    /// Absolute start in engine samples.
    pub start_sample: u64,
    /// Absolute end in engine samples.
    pub end_sample: u64,
}
