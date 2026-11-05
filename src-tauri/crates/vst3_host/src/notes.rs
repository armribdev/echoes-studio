//! The notes of a block as VST3 events, and the NoteOffs a block that was not
//! delivered must not lose (ADR-013 amendment, ADR-033 step 8).
//!
//! The engine's `VstInstrument` and the bridge's VST3 adapter both build their
//! `IEventList` here, so an instrument in the Echoes process and one in the bridge
//! receive the same events.  Pitch bend, control changes and channel aftertouch are
//! not events in VST3: they go through `midi_mapping`.

use vst3_ffi::Event;

use crate::event_list::HostEventList;

/// The plugin matches a NoteOff to its NoteOn by pitch.
const NO_NOTE_ID: i32 = -1;
const MIDI_POLY_PRESSURE: u8 = 0xA0;
const PITCH_COUNT: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    NoteOn {
        pitch: u8,
        velocity: u8,
    },
    NoteOff {
        pitch: u8,
    },
    /// A raw MIDI message: status byte and its two data bytes.
    Control {
        message: [u8; 3],
    },
}

/// One note or control of a block, at a sample offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteMessage {
    pub offset: u32,
    pub kind: NoteKind,
}

/// What the plugin has been told about notes, so none is left sounding.
pub struct SoundingNotes {
    started: [u8; PITCH_COUNT],
    release_pending: [bool; PITCH_COUNT],
}

impl SoundingNotes {
    pub fn new() -> Self {
        SoundingNotes {
            started: [0; PITCH_COUNT],
            release_pending: [false; PITCH_COUNT],
        }
    }

    fn note_started(&mut self, pitch: u8) {
        if let Some(count) = self.started.get_mut(pitch as usize) {
            *count = count.saturating_add(1);
        }
    }

    fn note_ended(&mut self, pitch: u8) {
        if let Some(count) = self.started.get_mut(pitch as usize) {
            *count = count.saturating_sub(1);
        }
    }

    fn keep_release(&mut self, pitch: u8) {
        if let Some(pending) = self.release_pending.get_mut(pitch as usize) {
            *pending = true;
        }
    }

    /// Every note the plugin has not been told to stop is released by the next block.
    pub fn release_all(&mut self) {
        for (pending, &count) in self.release_pending.iter_mut().zip(&self.started) {
            *pending |= count > 0;
        }
    }

    /// A block the plugin did not process: the NoteOffs it would have received
    /// are kept for the next one.
    pub fn keep_releases_of(&mut self, messages: impl IntoIterator<Item = NoteMessage>) {
        for message in messages {
            if let NoteKind::NoteOff { pitch } = message.kind {
                if self.started.get(pitch as usize).is_some_and(|&n| n > 0) {
                    self.keep_release(pitch);
                }
            }
        }
    }

    /// The kept NoteOffs, at offset 0 and before the block's own events.
    pub fn push_pending_releases(&mut self, list: &mut HostEventList) {
        for pitch in 0..PITCH_COUNT {
            if !self.release_pending[pitch] {
                continue;
            }
            if list.push(Event::note_off(0, pitch as u8, NO_NOTE_ID)) {
                self.release_pending[pitch] = false;
                self.started[pitch] = 0;
            }
        }
    }
}

impl Default for SoundingNotes {
    fn default() -> Self {
        Self::new()
    }
}

/// Append a block's messages as VST3 events: notes as note events (velocity
/// `0..127` → `0..1`), polyphonic aftertouch as poly pressure.
///
/// Pure — no FFI, no allocation.  A NoteOff the full list cannot take is kept in
/// `notes` for the next block.
pub fn push_events(
    messages: impl IntoIterator<Item = NoteMessage>,
    list: &mut HostEventList,
    notes: &mut SoundingNotes,
) {
    for message in messages {
        match message.kind {
            NoteKind::NoteOn { pitch, velocity } => {
                let note_on =
                    Event::note_on(message.offset, pitch, velocity as f32 / 127.0, NO_NOTE_ID);
                if list.push(note_on) {
                    notes.note_started(pitch);
                }
            }
            NoteKind::NoteOff { pitch } => {
                if list.push(Event::note_off(message.offset, pitch, NO_NOTE_ID)) {
                    notes.note_ended(pitch);
                } else {
                    notes.keep_release(pitch);
                }
            }
            NoteKind::Control { message: midi } => {
                if midi[0] & 0xF0 == MIDI_POLY_PRESSURE {
                    list.push(Event::poly_pressure(
                        message.offset,
                        midi[1],
                        midi[2] as f32 / 127.0,
                        NO_NOTE_ID,
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vst3_ffi::{K_NOTE_OFF_EVENT, K_NOTE_ON_EVENT, K_POLY_PRESSURE_EVENT};

    fn on(offset: u32, pitch: u8, velocity: u8) -> NoteMessage {
        NoteMessage {
            offset,
            kind: NoteKind::NoteOn { pitch, velocity },
        }
    }

    fn off(offset: u32, pitch: u8) -> NoteMessage {
        NoteMessage {
            offset,
            kind: NoteKind::NoteOff { pitch },
        }
    }

    fn control(offset: u32, message: [u8; 3]) -> NoteMessage {
        NoteMessage {
            offset,
            kind: NoteKind::Control { message },
        }
    }

    /// `(kind, sample offset, pitch)` of every event in the list.
    fn summary(list: &HostEventList) -> Vec<(u16, i32, i16)> {
        list.as_slice()
            .iter()
            .map(|event| {
                let pitch = match event.kind {
                    K_NOTE_ON_EVENT => unsafe { event.payload.note_on.pitch },
                    K_NOTE_OFF_EVENT => unsafe { event.payload.note_off.pitch },
                    _ => unsafe { event.payload.poly_pressure.pitch },
                };
                (event.kind, event.sample_offset, pitch)
            })
            .collect()
    }

    fn convert(messages: &[NoteMessage], notes: &mut SoundingNotes) -> Box<HostEventList> {
        let mut list = HostEventList::boxed();
        notes.push_pending_releases(&mut list);
        push_events(messages.iter().copied(), &mut list, notes);
        list
    }

    #[test]
    fn notes_keep_their_order_offsets_and_scaled_velocity() {
        let mut notes = SoundingNotes::new();
        let list = convert(&[on(0, 60, 127), on(24, 64, 64), off(100, 60)], &mut notes);

        assert_eq!(
            summary(&list),
            vec![
                (K_NOTE_ON_EVENT, 0, 60),
                (K_NOTE_ON_EVENT, 24, 64),
                (K_NOTE_OFF_EVENT, 100, 60),
            ]
        );
        let first = unsafe { list.as_slice()[0].payload.note_on };
        let second = unsafe { list.as_slice()[1].payload.note_on };
        assert_eq!((first.velocity, first.channel, first.note_id), (1.0, 0, -1));
        assert!((second.velocity - 64.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn polyphonic_aftertouch_becomes_poly_pressure() {
        let list = convert(&[control(5, [0xA0, 62, 127])], &mut SoundingNotes::new());

        assert_eq!(summary(&list), vec![(K_POLY_PRESSURE_EVENT, 5, 62)]);
        let pressure = unsafe { list.as_slice()[0].payload.poly_pressure.pressure };
        assert_eq!(pressure, 1.0);
    }

    #[test]
    fn controls_are_not_sent_as_events() {
        let list = convert(
            &[
                control(0, [0xE0, 0, 0x60]),
                control(0, [0xB0, 1, 64]),
                control(0, [0xD0, 50, 0]),
            ],
            &mut SoundingNotes::new(),
        );
        assert!(list.as_slice().is_empty());
    }

    #[test]
    fn a_reset_releases_each_sounding_note_once() {
        let mut notes = SoundingNotes::new();
        convert(&[on(0, 60, 100), on(0, 67, 100), off(10, 67)], &mut notes);

        notes.release_all();
        let next_block = convert(&[], &mut notes);
        assert_eq!(summary(&next_block), vec![(K_NOTE_OFF_EVENT, 0, 60)]);

        let after = convert(&[], &mut notes);
        assert!(after.as_slice().is_empty());
    }

    #[test]
    fn a_note_off_of_a_skipped_block_is_sent_with_the_next_one() {
        let mut notes = SoundingNotes::new();
        convert(&[on(0, 60, 100)], &mut notes);

        notes.keep_releases_of([off(30, 60)]);

        let next_block = convert(&[], &mut notes);
        assert_eq!(summary(&next_block), vec![(K_NOTE_OFF_EVENT, 0, 60)]);
    }

    #[test]
    fn a_skipped_note_off_for_a_note_the_plugin_never_heard_is_ignored() {
        let mut notes = SoundingNotes::new();
        notes.keep_releases_of([off(0, 60)]);
        let list = convert(&[], &mut notes);
        assert!(list.as_slice().is_empty());
    }

    #[test]
    fn a_note_off_the_full_list_cannot_take_is_kept() {
        let mut notes = SoundingNotes::new();
        convert(&[on(0, 60, 100)], &mut notes);

        let mut full = HostEventList::boxed();
        while full.push(Event::note_on(0, 1, 1.0, -1)) {}
        push_events([off(8, 60)], &mut full, &mut notes);

        let next_block = convert(&[], &mut notes);
        assert_eq!(summary(&next_block), vec![(K_NOTE_OFF_EVENT, 0, 60)]);
    }

    #[test]
    fn a_stray_note_off_does_not_underflow_the_count() {
        let mut notes = SoundingNotes::new();
        convert(&[off(0, 60)], &mut notes);
        notes.release_all();
        let list = convert(&[], &mut notes);
        assert!(list.as_slice().is_empty());
    }
}
