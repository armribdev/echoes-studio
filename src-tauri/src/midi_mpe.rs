//! MPE live input (ADR-039, cycle 2).
//!
//! A MPE controller plays each note on its own MIDI channel (lower zone: channel
//! 1 is the master channel, channels 2 to 16 are member channels) so that pitch
//! bend, channel pressure and CC 74 belong to that note alone. This turns those
//! messages into the per-note expressions the engine already plays, aimed at the
//! key of the note held on the channel. The master channel keeps the ordinary
//! channel controls of ADR-029.
//!
//! One translator per open device, owned by its capture callback: no lock, and
//! nothing here runs on the audio thread.

use crate::audio_graph::midi::{MidiEventKind, MidiLiveInputMessage};
use crate::midi_input::parse_live_message;
use crate::project::{NoteExpressionKind, PITCH_EXPRESSION_SEMITONES};

const CHANNEL_COUNT: usize = 16;
const NOTE_OFF: u8 = 0x80;
const NOTE_ON: u8 = 0x90;
const CONTROL_CHANGE: u8 = 0xB0;
const CHANNEL_PRESSURE: u8 = 0xD0;
const PITCH_BEND: u8 = 0xE0;
const TIMBRE_CONTROLLER: u8 = 74;
const PITCH_BEND_CENTER: i32 = 8192;
/// The reach of a member channel's pitch bend, per the MPE specification.
const MEMBER_BEND_SEMITONES: f32 = 48.0;

#[derive(Clone, Copy)]
struct MemberChannel {
    held_pitch: Option<u8>,
    /// Values sent before the note: MPE controllers set bend, pressure and
    /// timbre first, so the note starts with them.
    pending: [(NoteExpressionKind, Option<f32>); 3],
}

impl MemberChannel {
    fn new() -> Self {
        Self {
            held_pitch: None,
            pending: NoteExpressionKind::ALL.map(|kind| (kind, None)),
        }
    }

    fn remember(&mut self, kind: NoteExpressionKind, value: f32) {
        for slot in &mut self.pending {
            if slot.0 == kind {
                slot.1 = Some(value);
            }
        }
    }
}

pub struct MpeTranslator {
    members: [MemberChannel; CHANNEL_COUNT],
}

impl MpeTranslator {
    pub fn new() -> Self {
        Self {
            members: [MemberChannel::new(); CHANNEL_COUNT],
        }
    }

    /// Calls `emit` for each live event `message` stands for, in order.
    pub fn translate(
        &mut self,
        message: &[u8],
        received_at: u64,
        emit: &mut impl FnMut(MidiLiveInputMessage),
    ) {
        let Some(&status) = message.first() else {
            return;
        };
        let channel = (status & 0x0F) as usize;
        let is_voice_message = (NOTE_OFF..0xF0).contains(&status);
        if !is_voice_message || channel == 0 {
            if let Some(parsed) = parse_live_message(message, received_at) {
                emit(parsed);
            }
            return;
        }

        let member = &mut self.members[channel];
        let mut send = |kind| emit(MidiLiveInputMessage { kind, received_at });
        match (status & 0xF0, message) {
            (NOTE_ON, &[_, pitch, velocity]) if velocity > 0 => {
                member.held_pitch = Some(pitch);
                send(MidiEventKind::NoteOn { pitch, velocity });
                let pending = member.pending;
                member.pending = MemberChannel::new().pending;
                for (kind, value) in pending {
                    if let Some(value) = value {
                        send(MidiEventKind::NoteExpression { pitch, kind, value });
                    }
                }
            }
            (NOTE_ON | NOTE_OFF, &[_, pitch, _]) => {
                if member.held_pitch == Some(pitch) {
                    *member = MemberChannel::new();
                }
                send(MidiEventKind::NoteOff { pitch });
            }
            (PITCH_BEND, &[_, low, high]) => {
                let bend = (high as i32) << 7 | low as i32;
                let semitones = (bend - PITCH_BEND_CENTER) as f32 / PITCH_BEND_CENTER as f32
                    * MEMBER_BEND_SEMITONES;
                let limit = PITCH_EXPRESSION_SEMITONES;
                member.express(
                    NoteExpressionKind::Pitch,
                    semitones.clamp(-limit, limit),
                    &mut send,
                );
            }
            (CHANNEL_PRESSURE, &[_, pressure]) => {
                member.express(
                    NoteExpressionKind::Pressure,
                    pressure as f32 / 127.0,
                    &mut send,
                );
            }
            (CONTROL_CHANGE, &[_, TIMBRE_CONTROLLER, value]) => {
                member.express(NoteExpressionKind::Timbre, value as f32 / 127.0, &mut send);
            }
            _ => {}
        }
    }
}

impl MemberChannel {
    fn express(
        &mut self,
        kind: NoteExpressionKind,
        value: f32,
        send: &mut impl FnMut(MidiEventKind),
    ) {
        match self.held_pitch {
            Some(pitch) => send(MidiEventKind::NoteExpression { pitch, kind, value }),
            None => self.remember(kind, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translate(translator: &mut MpeTranslator, message: &[u8]) -> Vec<MidiEventKind> {
        let mut events = Vec::new();
        translator.translate(message, 7, &mut |parsed| {
            assert_eq!(parsed.received_at, 7);
            events.push(parsed.kind);
        });
        events
    }

    fn expression(pitch: u8, kind: NoteExpressionKind, value: f32) -> MidiEventKind {
        MidiEventKind::NoteExpression { pitch, kind, value }
    }

    #[test]
    fn the_master_channel_keeps_the_channel_controls() {
        let mut translator = MpeTranslator::new();
        assert_eq!(
            translate(&mut translator, &[0xB0, 64, 127]),
            vec![MidiEventKind::Control {
                message: [0xB0, 64, 127]
            }]
        );
        assert_eq!(
            translate(&mut translator, &[0x90, 60, 100]),
            vec![MidiEventKind::NoteOn {
                pitch: 60,
                velocity: 100
            }]
        );
    }

    #[test]
    fn a_member_channel_shapes_only_the_note_it_holds() {
        let mut translator = MpeTranslator::new();
        translate(&mut translator, &[0x91, 60, 100]);
        translate(&mut translator, &[0x92, 64, 100]);

        assert_eq!(
            translate(&mut translator, &[0xD1, 127]),
            vec![expression(60, NoteExpressionKind::Pressure, 1.0)]
        );
        assert_eq!(
            translate(&mut translator, &[0xB2, 74, 0]),
            vec![expression(64, NoteExpressionKind::Timbre, 0.0)]
        );
    }

    #[test]
    fn a_member_bend_spans_forty_eight_semitones_clamped_to_the_model() {
        let mut translator = MpeTranslator::new();
        translate(&mut translator, &[0x91, 60, 100]);

        assert_eq!(
            translate(&mut translator, &[0xE1, 0x00, 0x40]),
            vec![expression(60, NoteExpressionKind::Pitch, 0.0)]
        );
        assert_eq!(
            translate(&mut translator, &[0xE1, 0x7F, 0x7F]),
            vec![expression(60, NoteExpressionKind::Pitch, 24.0)]
        );
        assert_eq!(
            translate(&mut translator, &[0xE1, 0x00, 0x44]),
            vec![expression(60, NoteExpressionKind::Pitch, 3.0)]
        );
        assert_eq!(
            translate(&mut translator, &[0xE1, 0x00, 0x00]),
            vec![expression(60, NoteExpressionKind::Pitch, -24.0)]
        );
    }

    #[test]
    fn values_sent_before_the_note_start_it() {
        let mut translator = MpeTranslator::new();
        assert!(translate(&mut translator, &[0xD3, 64]).is_empty());
        assert!(translate(&mut translator, &[0xB3, 74, 127]).is_empty());

        let events = translate(&mut translator, &[0x93, 55, 90]);
        assert_eq!(
            events[0],
            MidiEventKind::NoteOn {
                pitch: 55,
                velocity: 90
            }
        );
        assert_eq!(events.len(), 3);
        assert!(events.contains(&expression(55, NoteExpressionKind::Pressure, 64.0 / 127.0)));
        assert!(events.contains(&expression(55, NoteExpressionKind::Timbre, 1.0)));
    }

    #[test]
    fn a_released_note_leaves_nothing_for_the_next_one_on_its_channel() {
        let mut translator = MpeTranslator::new();
        translate(&mut translator, &[0x91, 60, 100]);
        translate(&mut translator, &[0xD1, 90]);
        assert_eq!(
            translate(&mut translator, &[0x81, 60, 0]),
            vec![MidiEventKind::NoteOff { pitch: 60 }]
        );

        assert!(translate(&mut translator, &[0xD1, 90]).is_empty());
        assert_eq!(
            translate(&mut translator, &[0x91, 62, 100]).len(),
            2,
            "only the pressure sent after the release waits for the next note"
        );
    }

    #[test]
    fn a_note_on_with_velocity_zero_is_a_note_off() {
        let mut translator = MpeTranslator::new();
        translate(&mut translator, &[0x91, 60, 100]);
        assert_eq!(
            translate(&mut translator, &[0x91, 60, 0]),
            vec![MidiEventKind::NoteOff { pitch: 60 }]
        );
        assert!(translate(&mut translator, &[0xD1, 10]).is_empty());
    }

    #[test]
    fn other_member_messages_and_system_messages_are_ignored() {
        let mut translator = MpeTranslator::new();
        translate(&mut translator, &[0x91, 60, 100]);
        assert!(translate(&mut translator, &[0xB1, 64, 127]).is_empty());
        assert!(translate(&mut translator, &[0xA1, 60, 50]).is_empty());
        assert!(translate(&mut translator, &[0xC1, 5]).is_empty());
        assert!(translate(&mut translator, &[0xF8]).is_empty());
        assert!(translate(&mut translator, &[]).is_empty());
    }
}
