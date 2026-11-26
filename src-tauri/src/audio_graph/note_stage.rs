//! Note effects stage (ADR-038): transpose, then scale, then chord, ahead of
//! the arpeggiator.
//!
//! Events in, events out — it knows nothing about the node or the device. Every
//! structure is a fixed array, so `process` never allocates.
//!
//! Each played pitch is mapped once, when its first `NoteOn` arrives, and the
//! output pitches are remembered against it: the matching `NoteOff` releases
//! exactly those, whatever the settings are by then.

use crate::audio_graph::arpeggiator::Routing;
use crate::audio_graph::midi::{MidiEvent, MidiEventKind, MidiEventList};
use crate::note_effect_settings::{NoteEffectSettings, MAX_CHORD_VOICES};
use crate::project::NoteExpressionKind;

const PITCH_COUNT: usize = 128;
const PITCH_CLASSES: i16 = 12;

#[derive(Clone, Copy)]
struct Voices {
    pitches: [u8; MAX_CHORD_VOICES],
    len: usize,
}

impl Voices {
    fn push_in_range(&mut self, pitch: i16) {
        if (0..PITCH_COUNT as i16).contains(&pitch) && self.len < MAX_CHORD_VOICES {
            self.pitches[self.len] = pitch as u8;
            self.len += 1;
        }
    }

    fn as_slice(&self) -> &[u8] {
        &self.pitches[..self.len]
    }
}

pub struct NoteStage {
    /// Output pitches of each played pitch, kept until its release is written.
    voices: [[u8; MAX_CHORD_VOICES]; PITCH_COUNT],
    voice_count: [u8; PITCH_COUNT],
    /// Played notes currently down, per pitch (two sources can hold one).
    held: [u8; PITCH_COUNT],
    /// Recorded played notes that hold each output pitch.
    out_count: [u8; PITCH_COUNT],
    /// Released played notes whose `NoteOff`s did not fit yet.
    owed: [bool; PITCH_COUNT],
    owed_count: u16,
    /// Recorded played notes whose output is not just their own pitch.
    reshaped: u16,
}

impl NoteStage {
    pub fn new() -> Self {
        Self {
            voices: [[0; MAX_CHORD_VOICES]; PITCH_COUNT],
            voice_count: [0; PITCH_COUNT],
            held: [0; PITCH_COUNT],
            out_count: [0; PITCH_COUNT],
            owed: [false; PITCH_COUNT],
            owed_count: 0,
            reshaped: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// `input` must be sorted by offset. `output` is only written when the
    /// result is `Routing::Replaced`; the caller clears it.
    pub fn process(
        &mut self,
        settings: &NoteEffectSettings,
        input: &[MidiEvent],
        output: &mut MidiEventList,
    ) -> Routing {
        let replacing = settings.any_enabled() || self.reshaped > 0 || self.owed_count > 0;
        if self.owed_count > 0 {
            self.release_owed(output);
        }
        for event in input {
            match event.kind {
                MidiEventKind::NoteOn { pitch, velocity } => {
                    self.note_on(settings, event.offset, pitch, velocity, replacing, output);
                }
                MidiEventKind::NoteOff { pitch } => {
                    self.note_off(event.offset, pitch, replacing, output);
                }
                MidiEventKind::Control { .. } if replacing => output.push(*event),
                MidiEventKind::Control { .. } => {}
                MidiEventKind::NoteExpression { pitch, kind, value } if replacing => {
                    self.note_expression(event.offset, pitch, kind, value, output);
                }
                MidiEventKind::NoteExpression { .. } => {}
            }
        }
        if replacing {
            Routing::Replaced
        } else {
            Routing::Passthrough
        }
    }

    fn note_on(
        &mut self,
        settings: &NoteEffectSettings,
        offset: u32,
        pitch: u8,
        velocity: u8,
        replacing: bool,
        output: &mut MidiEventList,
    ) {
        let played = pitch as usize;
        if played >= PITCH_COUNT || self.owed[played] {
            return;
        }
        if self.held[played] > 0 {
            self.held[played] = self.held[played].saturating_add(1);
            return;
        }
        let voices = map_pitch(settings, pitch);
        // The release of every voice must fit in this block too, so a chord is
        // never half-played and never left without its way out.
        if replacing && output.remaining_capacity() < 2 * voices.len {
            return;
        }
        self.held[played] = 1;
        self.voice_count[played] = voices.len as u8;
        self.voices[played] = voices.pitches;
        if !is_identity(pitch, &voices) {
            self.reshaped += 1;
        }
        for &voice in voices.as_slice() {
            let count = &mut self.out_count[voice as usize];
            *count = count.saturating_add(1);
            if *count == 1 && replacing {
                output.push(MidiEvent {
                    offset,
                    kind: MidiEventKind::NoteOn {
                        pitch: voice,
                        velocity,
                    },
                });
            }
        }
    }

    /// An expression of a played note goes to every voice that note sounds.
    fn note_expression(
        &mut self,
        offset: u32,
        pitch: u8,
        kind: NoteExpressionKind,
        value: f32,
        output: &mut MidiEventList,
    ) {
        let played = pitch as usize;
        if played >= PITCH_COUNT || self.held[played] == 0 {
            return;
        }
        let count = self.voice_count[played] as usize;
        for &voice in &self.voices[played][..count] {
            output.push_control(MidiEvent {
                offset,
                kind: MidiEventKind::NoteExpression {
                    pitch: voice,
                    kind,
                    value,
                },
            });
        }
    }

    fn note_off(&mut self, offset: u32, pitch: u8, replacing: bool, output: &mut MidiEventList) {
        let played = pitch as usize;
        if played >= PITCH_COUNT || self.held[played] == 0 {
            return;
        }
        self.held[played] -= 1;
        if self.held[played] > 0 {
            return;
        }
        if !self.release(played, offset, replacing, output) {
            self.owed[played] = true;
            self.owed_count += 1;
        }
    }

    /// Writes the `NoteOff`s of one played note. `false` when they do not fit.
    fn release(
        &mut self,
        played: usize,
        offset: u32,
        replacing: bool,
        output: &mut MidiEventList,
    ) -> bool {
        let count = self.voice_count[played] as usize;
        if replacing && output.remaining_capacity() < count {
            return false;
        }
        let voices = Voices {
            pitches: self.voices[played],
            len: count,
        };
        if !is_identity(played as u8, &voices) {
            self.reshaped -= 1;
        }
        for &voice in voices.as_slice() {
            let held_by = &mut self.out_count[voice as usize];
            *held_by -= 1;
            if *held_by == 0 && replacing {
                output.push(MidiEvent {
                    offset,
                    kind: MidiEventKind::NoteOff { pitch: voice },
                });
            }
        }
        self.voice_count[played] = 0;
        true
    }

    /// Releases owed from an earlier block first, at offset 0.
    fn release_owed(&mut self, output: &mut MidiEventList) {
        for played in 0..PITCH_COUNT {
            if self.owed[played] && self.release(played, 0, true, output) {
                self.owed[played] = false;
                self.owed_count -= 1;
            }
        }
    }

    #[cfg(test)]
    fn sounding_output_pitches(&self) -> usize {
        self.out_count.iter().filter(|&&count| count > 0).count()
    }
}

fn is_identity(pitch: u8, voices: &Voices) -> bool {
    voices.len == 1 && voices.pitches[0] == pitch
}

/// Transpose, then scale, then chord. Output pitches outside 0–127 are dropped.
fn map_pitch(settings: &NoteEffectSettings, pitch: u8) -> Voices {
    let mut base = pitch as i16;
    if settings.transpose.enabled {
        base += settings.transpose.semitones as i16;
    }
    if settings.scale.enabled {
        base = nearest_in_scale(base, settings.scale.root, settings.scale.scale.degrees());
    }
    let mut voices = Voices {
        pitches: [0; MAX_CHORD_VOICES],
        len: 0,
    };
    if settings.chord.enabled {
        for &offset in settings.chord.shape.offsets() {
            voices.push_in_range(base + offset as i16);
        }
    } else {
        voices.push_in_range(base);
    }
    voices
}

/// The scale pitch closest to `pitch`; the lower one when both are as close.
fn nearest_in_scale(pitch: i16, root: u8, degrees: &[u8]) -> i16 {
    let relative = (pitch - root as i16).rem_euclid(PITCH_CLASSES);
    let below = degrees
        .iter()
        .rev()
        .map(|&degree| degree as i16)
        .find(|&degree| degree <= relative)
        .unwrap_or(0);
    let above = degrees
        .iter()
        .map(|&degree| degree as i16)
        .find(|&degree| degree > relative)
        .unwrap_or(PITCH_CLASSES);
    if relative - below <= above - relative {
        pitch - (relative - below)
    } else {
        pitch + (above - relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note_effect_settings::{
        ChordSettings, ChordShape, ScaleKind, ScaleSettings, TransposeSettings,
    };

    type Played = Vec<(u32, MidiEventKind)>;

    fn on(offset: u32, pitch: u8) -> MidiEvent {
        MidiEvent {
            offset,
            kind: MidiEventKind::NoteOn {
                pitch,
                velocity: 100,
            },
        }
    }

    fn off(offset: u32, pitch: u8) -> MidiEvent {
        MidiEvent {
            offset,
            kind: MidiEventKind::NoteOff { pitch },
        }
    }

    fn note_on(offset: u32, pitch: u8) -> (u32, MidiEventKind) {
        (
            offset,
            MidiEventKind::NoteOn {
                pitch,
                velocity: 100,
            },
        )
    }

    fn note_off(offset: u32, pitch: u8) -> (u32, MidiEventKind) {
        (offset, MidiEventKind::NoteOff { pitch })
    }

    fn run(
        stage: &mut NoteStage,
        settings: &NoteEffectSettings,
        input: &[MidiEvent],
    ) -> (Routing, Played) {
        let mut output = MidiEventList::new();
        let routing = stage.process(settings, input, &mut output);
        (routing, collected(&output))
    }

    fn collected(output: &MidiEventList) -> Played {
        output
            .as_slice()
            .iter()
            .map(|event| (event.offset, event.kind))
            .collect()
    }

    fn transposing(semitones: i8) -> NoteEffectSettings {
        NoteEffectSettings {
            transpose: TransposeSettings {
                enabled: true,
                semitones,
            },
            ..NoteEffectSettings::default()
        }
    }

    fn scaled(root: u8, scale: ScaleKind) -> NoteEffectSettings {
        NoteEffectSettings {
            scale: ScaleSettings {
                enabled: true,
                root,
                scale,
            },
            ..NoteEffectSettings::default()
        }
    }

    fn chorded(shape: ChordShape) -> NoteEffectSettings {
        NoteEffectSettings {
            chord: ChordSettings {
                enabled: true,
                shape,
            },
            ..NoteEffectSettings::default()
        }
    }

    fn mapped(settings: &NoteEffectSettings, pitch: u8) -> Vec<u8> {
        map_pitch(settings, pitch).as_slice().to_vec()
    }

    #[test]
    fn transpose_moves_the_note_and_its_release() {
        let mut stage = NoteStage::new();
        let settings = transposing(5);
        let (routing, played) = run(&mut stage, &settings, &[on(3, 60), off(40, 60)]);
        assert_eq!(routing, Routing::Replaced);
        assert_eq!(played, vec![note_on(3, 65), note_off(40, 65)]);
    }

    #[test]
    fn a_scale_pulls_a_pitch_to_the_nearest_scale_pitch_down_on_a_tie() {
        let c_major = scaled(0, ScaleKind::Major);
        assert_eq!(mapped(&c_major, 60), vec![60]);
        assert_eq!(mapped(&c_major, 61), vec![60]);
        assert_eq!(mapped(&c_major, 63), vec![62]);
        assert_eq!(mapped(&c_major, 66), vec![65]);
        assert_eq!(mapped(&c_major, 70), vec![69]);

        let c_pentatonic = scaled(0, ScaleKind::PentatonicMajor);
        assert_eq!(mapped(&c_pentatonic, 65), vec![64]);
        assert_eq!(mapped(&c_pentatonic, 66), vec![67]);
        assert_eq!(mapped(&c_pentatonic, 71), vec![72]);
    }

    #[test]
    fn a_scale_follows_its_root() {
        let d_dorian = scaled(2, ScaleKind::Dorian);
        assert_eq!(mapped(&d_dorian, 62), vec![62]);
        assert_eq!(mapped(&d_dorian, 71), vec![71]);
        assert_eq!(mapped(&d_dorian, 70), vec![69]);
    }

    #[test]
    fn a_chord_stacks_its_shape_on_the_played_pitch() {
        let mut stage = NoteStage::new();
        let settings = chorded(ChordShape::Minor7);
        let (_, played) = run(&mut stage, &settings, &[on(8, 60), off(50, 60)]);
        assert_eq!(
            played,
            vec![
                note_on(8, 60),
                note_on(8, 63),
                note_on(8, 67),
                note_on(8, 70),
                note_off(50, 60),
                note_off(50, 63),
                note_off(50, 67),
                note_off(50, 70),
            ]
        );
    }

    fn expression(offset: u32, pitch: u8, value: f32) -> MidiEvent {
        MidiEvent {
            offset,
            kind: MidiEventKind::NoteExpression {
                pitch,
                kind: NoteExpressionKind::Pressure,
                value,
            },
        }
    }

    fn pressure_of(offset: u32, pitch: u8, value: f32) -> (u32, MidiEventKind) {
        (offset, expression(offset, pitch, value).kind)
    }

    #[test]
    fn an_expression_reaches_every_voice_of_a_chord() {
        let mut stage = NoteStage::new();
        let settings = chorded(ChordShape::Minor7);
        let (_, played) = run(
            &mut stage,
            &settings,
            &[on(0, 60), expression(10, 60, 0.5), off(50, 60)],
        );
        assert_eq!(
            &played[4..8],
            [
                pressure_of(10, 60, 0.5),
                pressure_of(10, 63, 0.5),
                pressure_of(10, 67, 0.5),
                pressure_of(10, 70, 0.5),
            ]
        );
    }

    #[test]
    fn an_expression_follows_the_transposed_pitch() {
        let mut stage = NoteStage::new();
        let (_, played) = run(
            &mut stage,
            &transposing(12),
            &[on(0, 60), expression(5, 60, 0.25)],
        );
        assert_eq!(played[1], pressure_of(5, 72, 0.25));
    }

    #[test]
    fn an_expression_of_a_note_that_is_not_held_is_dropped() {
        let mut stage = NoteStage::new();
        let (_, played) = run(
            &mut stage,
            &transposing(12),
            &[on(0, 60), off(4, 60), expression(5, 60, 0.25)],
        );
        assert!(played
            .iter()
            .all(|(_, kind)| !matches!(kind, MidiEventKind::NoteExpression { .. })));
    }

    #[test]
    fn effects_apply_in_the_order_transpose_scale_chord() {
        let settings = NoteEffectSettings {
            transpose: TransposeSettings {
                enabled: true,
                semitones: 1,
            },
            scale: ScaleSettings {
                enabled: true,
                root: 0,
                scale: ScaleKind::Major,
            },
            chord: ChordSettings {
                enabled: true,
                shape: ChordShape::Fifth,
            },
        };
        // 60 → 61 → scale pulls it down to 60 → fifth adds 67.
        assert_eq!(mapped(&settings, 60), vec![60, 67]);
    }

    #[test]
    fn pitches_outside_the_midi_range_are_dropped() {
        assert_eq!(mapped(&chorded(ChordShape::Major), 125), vec![125]);
        assert_eq!(mapped(&transposing(24), 120), Vec::<u8>::new());
        assert_eq!(mapped(&transposing(-24), 10), Vec::<u8>::new());
        let low = NoteEffectSettings {
            transpose: TransposeSettings {
                enabled: true,
                semitones: -3,
            },
            chord: ChordSettings {
                enabled: true,
                shape: ChordShape::Octave,
            },
            ..NoteEffectSettings::default()
        };
        // 1 - 3 = -2 is dropped, its octave (10) is kept.
        assert_eq!(mapped(&low, 1), vec![10]);
    }

    #[test]
    fn a_dropped_note_still_has_a_matching_release_that_writes_nothing() {
        let mut stage = NoteStage::new();
        let settings = transposing(24);
        let (_, played) = run(&mut stage, &settings, &[on(0, 120), off(10, 120)]);
        assert!(played.is_empty());
        assert_eq!(stage.reshaped, 0);
        assert_eq!(stage.sounding_output_pitches(), 0);
    }

    #[test]
    fn a_setting_changed_while_a_note_is_held_does_not_change_its_release() {
        let mut stage = NoteStage::new();
        let (_, started) = run(&mut stage, &chorded(ChordShape::Major), &[on(0, 60)]);
        assert_eq!(started.len(), 3);

        let disabled = NoteEffectSettings::default();
        let (routing, released) = run(&mut stage, &disabled, &[off(7, 60)]);
        assert_eq!(routing, Routing::Replaced);
        assert_eq!(
            released,
            vec![note_off(7, 60), note_off(7, 64), note_off(7, 67)]
        );

        let (routing, _) = run(&mut stage, &disabled, &[on(0, 62)]);
        assert_eq!(routing, Routing::Passthrough);
    }

    #[test]
    fn a_note_started_before_enabling_is_released_as_played() {
        let mut stage = NoteStage::new();
        let disabled = NoteEffectSettings::default();
        let (routing, _) = run(&mut stage, &disabled, &[on(0, 60)]);
        assert_eq!(routing, Routing::Passthrough);

        let (routing, played) = run(&mut stage, &transposing(7), &[off(5, 60), on(5, 62)]);
        assert_eq!(routing, Routing::Replaced);
        assert_eq!(played, vec![note_off(5, 60), note_on(5, 69)]);
    }

    #[test]
    fn a_shared_output_pitch_is_released_with_its_last_holder() {
        let mut stage = NoteStage::new();
        let settings = chorded(ChordShape::Major);
        // 60 → 60 64 67, 64 → 64 68 71: both hold 64.
        let (_, started) = run(&mut stage, &settings, &[on(0, 60), on(0, 64)]);
        assert_eq!(
            started,
            vec![
                note_on(0, 60),
                note_on(0, 64),
                note_on(0, 67),
                note_on(0, 68),
                note_on(0, 71),
            ]
        );
        let (_, first) = run(&mut stage, &settings, &[off(1, 60)]);
        assert_eq!(first, vec![note_off(1, 60), note_off(1, 67)]);
        let (_, last) = run(&mut stage, &settings, &[off(2, 64)]);
        assert_eq!(
            last,
            vec![note_off(2, 64), note_off(2, 68), note_off(2, 71)]
        );
        assert_eq!(stage.sounding_output_pitches(), 0);
    }

    #[test]
    fn a_pitch_held_by_two_sources_is_released_by_the_second_note_off() {
        let mut stage = NoteStage::new();
        let settings = transposing(2);
        let (_, started) = run(&mut stage, &settings, &[on(0, 60), on(4, 60)]);
        assert_eq!(started, vec![note_on(0, 62)]);
        let (_, first) = run(&mut stage, &settings, &[off(1, 60)]);
        assert!(first.is_empty());
        let (_, second) = run(&mut stage, &settings, &[off(2, 60)]);
        assert_eq!(second, vec![note_off(2, 62)]);
    }

    #[test]
    fn controls_pass_through_at_their_offsets() {
        let mut stage = NoteStage::new();
        let control = MidiEvent {
            offset: 9,
            kind: MidiEventKind::Control {
                message: [0xE0, 0, 0x40],
            },
        };
        let (_, played) = run(&mut stage, &transposing(1), &[on(0, 60), control]);
        assert_eq!(played[1], (9, control.kind));
    }

    #[test]
    fn a_note_on_without_room_for_its_release_is_skipped_whole() {
        let mut stage = NoteStage::new();
        let settings = chorded(ChordShape::Major);
        let mut output = MidiEventList::new();
        while output.remaining_capacity() > 5 {
            output.push(on(0, 1));
        }
        stage.process(&settings, &[on(0, 60), off(9, 60)], &mut output);
        assert_eq!(output.remaining_capacity(), 5);
        assert_eq!(stage.sounding_output_pitches(), 0);
    }

    #[test]
    fn releases_that_do_not_fit_are_written_first_in_the_next_block() {
        let mut stage = NoteStage::new();
        let settings = chorded(ChordShape::Major);
        run(&mut stage, &settings, &[on(0, 60)]);

        let mut crowded = MidiEventList::new();
        while crowded.remaining_capacity() > 2 {
            crowded.push(on(0, 1));
        }
        stage.process(&settings, &[off(4, 60)], &mut crowded);
        assert_eq!(crowded.remaining_capacity(), 2);
        assert_eq!(stage.owed_count, 1);

        let disabled = NoteEffectSettings::default();
        let (routing, played) = run(&mut stage, &disabled, &[on(0, 72)]);
        assert_eq!(routing, Routing::Replaced);
        assert_eq!(
            played,
            vec![
                note_off(0, 60),
                note_off(0, 64),
                note_off(0, 67),
                note_on(0, 72),
            ]
        );
        assert_eq!(stage.owed_count, 0);
        assert_eq!(stage.reshaped, 0);
    }

    #[test]
    fn reset_forgets_every_note() {
        let mut stage = NoteStage::new();
        run(&mut stage, &chorded(ChordShape::Major), &[on(0, 60)]);
        stage.reset();
        assert_eq!(stage.sounding_output_pitches(), 0);
        assert_eq!(stage.reshaped, 0);
        let (routing, _) = run(&mut stage, &NoteEffectSettings::default(), &[off(0, 60)]);
        assert_eq!(routing, Routing::Passthrough);
    }
}
