//! MIDI event types for intra-block note scheduling.
//!
//! All types here are stack-allocated and `Copy` so the real-time path can
//! use them without allocation.

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
