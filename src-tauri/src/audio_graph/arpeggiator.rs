//! Arpeggiator note stage (ADR-030): turns the chord a track receives into
//! single notes on a tempo grid, ahead of the instrument.
//!
//! Events in, events out — it knows nothing about the node or the device. Every
//! structure is a fixed array, so `process` never allocates.
//!
//! Clock: while the transport plays, step *k* starts at the sample of beat
//! *k* × step (the grid clip notes use, ADR-012). While it is stopped there is no
//! position to lock to, so the grid starts where the chord does and counts
//! samples the stage owns. Neither reads a wall clock.

use crate::arpeggiator_settings::{ArpMode, ArpeggiatorSettings};
use crate::audio_graph::midi::{beats_to_sample, MidiEvent, MidiEventKind, MidiEventList};
use crate::audio_graph::node::TransportInfo;

pub const MAX_HELD_NOTES: usize = 16;
const PITCH_COUNT: usize = 128;
const RANDOM_SEED: u32 = 0x9E37_79B9;
/// Room a step needs in the output: the previous note's release, its NoteOn
/// and its own release.
const STEP_EVENT_ROOM: usize = 3;

#[derive(Debug, PartialEq, Eq)]
pub enum Routing {
    /// The instrument receives its input events unchanged.
    Passthrough,
    /// The instrument receives the output list instead.
    Replaced,
}

#[derive(Clone, Copy)]
struct HeldNote {
    pitch: u8,
    velocity: u8,
    count: u8,
}

struct SoundingNote {
    pitch: u8,
    release_at: u64,
}

enum Due {
    Release,
    Input,
    Step,
}

struct Block {
    start: u64,
    length: u64,
    playing: bool,
    bpm: f32,
    sample_rate: f32,
    step_beats: f64,
    gate: f64,
}

impl Block {
    fn new(settings: &ArpeggiatorSettings, transport: &TransportInfo, start: u64) -> Self {
        Self {
            start,
            length: transport.block_size as u64,
            playing: transport.is_playing,
            bpm: transport.bpm.max(1.0),
            sample_rate: transport.sample_rate,
            step_beats: settings.step_beats(),
            gate: settings.gate_percent as f64 / 100.0,
        }
    }

    fn end(&self) -> u64 {
        self.start + self.length
    }

    fn step_samples(&self) -> f64 {
        self.step_beats * 60.0 / self.bpm as f64 * self.sample_rate as f64
    }

    fn gate_samples(&self) -> u64 {
        ((self.step_samples() * self.gate) as u64).max(1)
    }

    fn grid_sample(&self, index: u64) -> u64 {
        let beats = (index as f64 * self.step_beats) as f32;
        beats_to_sample(0.0, beats, self.bpm, self.sample_rate)
    }

    fn grid_boundary_at_or_after(&self, time: u64) -> u64 {
        let beats = time as f64 / self.sample_rate as f64 * self.bpm as f64 / 60.0;
        let mut index = (beats / self.step_beats) as u64;
        while self.grid_sample(index) < time {
            index += 1;
        }
        while index > 0 && self.grid_sample(index - 1) >= time {
            index -= 1;
        }
        self.grid_sample(index)
    }
}

pub struct Arpeggiator {
    held: [HeldNote; MAX_HELD_NOTES],
    held_len: usize,
    /// Indices into `held`, lowest pitch first.
    by_pitch: [u8; MAX_HELD_NOTES],
    /// NoteOns handed to the instrument unchanged and not yet released.
    through: [u8; PITCH_COUNT],
    engaged: bool,
    sounding: Option<SoundingNote>,
    step_pending: bool,
    not_before: u64,
    free_next: f64,
    step_index: u32,
    random: u32,
    was_playing: bool,
    expected_start: u64,
    free_clock: u64,
}

impl Arpeggiator {
    pub fn new() -> Self {
        Self {
            held: [HeldNote {
                pitch: 0,
                velocity: 0,
                count: 0,
            }; MAX_HELD_NOTES],
            held_len: 0,
            by_pitch: [0; MAX_HELD_NOTES],
            through: [0; PITCH_COUNT],
            engaged: false,
            sounding: None,
            step_pending: false,
            not_before: 0,
            free_next: 0.0,
            step_index: 0,
            random: RANDOM_SEED,
            was_playing: false,
            expected_start: 0,
            free_clock: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    #[cfg(test)]
    pub fn is_sounding(&self) -> bool {
        self.sounding.is_some()
    }

    /// `input` must be sorted by offset. `output` is only written when the
    /// result is `Routing::Replaced`.
    pub fn process(
        &mut self,
        settings: &ArpeggiatorSettings,
        transport: &TransportInfo,
        input: &[MidiEvent],
        output: &mut MidiEventList,
    ) -> Routing {
        let start = if transport.is_playing {
            transport.position_samples
        } else {
            self.free_clock
        };
        let block = Block::new(settings, transport, start);
        let resync = block.playing != self.was_playing
            || (block.playing && block.start != self.expected_start);
        self.was_playing = block.playing;
        if block.playing {
            self.expected_start = block.end();
        } else {
            self.free_clock = block.end();
        }

        match (settings.enabled, self.engaged) {
            (false, false) => {
                self.observe(input);
                Routing::Passthrough
            }
            (false, true) => {
                self.disengage(output);
                self.forward(input, output);
                Routing::Replaced
            }
            (true, engaged) => {
                if !engaged {
                    self.engage(&block, output);
                } else if resync {
                    self.resync(&block, output);
                }
                self.run(&block, settings, input, output);
                Routing::Replaced
            }
        }
    }

    fn observe(&mut self, input: &[MidiEvent]) {
        for event in input {
            match event.kind {
                MidiEventKind::NoteOn { pitch, velocity } => {
                    self.note_on(pitch, velocity);
                    let count = &mut self.through[pitch as usize];
                    *count = count.saturating_add(1);
                }
                MidiEventKind::NoteOff { pitch } => {
                    self.note_off(pitch);
                    let count = &mut self.through[pitch as usize];
                    *count = count.saturating_sub(1);
                }
                MidiEventKind::Control { .. } | MidiEventKind::NoteExpression { .. } => {}
            }
        }
    }

    fn forward(&mut self, input: &[MidiEvent], output: &mut MidiEventList) {
        self.observe(input);
        for event in input {
            output.push(*event);
        }
    }

    fn engage(&mut self, block: &Block, output: &mut MidiEventList) {
        for (pitch, count) in self.through.iter_mut().enumerate() {
            if *count > 0 {
                output.push(note_off(0, pitch as u8));
                *count = 0;
            }
        }
        self.engaged = true;
        self.restart_pattern(block.start);
    }

    fn disengage(&mut self, output: &mut MidiEventList) {
        self.release_sounding(0, output);
        self.step_pending = false;
        self.engaged = false;
        for held in &self.held[..self.held_len] {
            output.push(note_on(0, held.pitch, held.velocity));
            self.through[held.pitch as usize] = held.count;
        }
    }

    fn resync(&mut self, block: &Block, output: &mut MidiEventList) {
        self.release_sounding(0, output);
        self.step_pending = false;
        if self.held_len > 0 {
            self.schedule_from(block.start);
        }
    }

    fn restart_pattern(&mut self, at: u64) {
        self.step_index = 0;
        self.random = RANDOM_SEED;
        if self.held_len > 0 {
            self.schedule_from(at);
        }
    }

    fn schedule_from(&mut self, at: u64) {
        self.step_pending = true;
        self.not_before = at;
        self.free_next = at as f64;
    }

    fn run(
        &mut self,
        block: &Block,
        settings: &ArpeggiatorSettings,
        input: &[MidiEvent],
        output: &mut MidiEventList,
    ) {
        let mut next_input = 0;
        while let Some((time, due)) = self.next_due(block, input, next_input) {
            let offset = (time - block.start) as u32;
            match due {
                Due::Release => self.release_sounding(offset, output),
                Due::Input => {
                    let event = input[next_input];
                    next_input += 1;
                    self.handle_input(event, time, output);
                }
                Due::Step => self.play_step(block, settings, time, output),
            }
        }
    }

    fn next_due(
        &self,
        block: &Block,
        input: &[MidiEvent],
        next_input: usize,
    ) -> Option<(u64, Due)> {
        let release = self
            .sounding
            .as_ref()
            .map(|note| (note.release_at.max(block.start), 0u8));
        let next_event = input
            .get(next_input)
            .map(|event| (block.start + event.offset as u64, 1u8));
        let step = self.step_pending.then(|| (self.next_step_time(block), 2u8));
        let (time, priority) = [release, next_event, step]
            .into_iter()
            .flatten()
            .filter(|(time, _)| *time < block.end())
            .min()?;
        let due = match priority {
            0 => Due::Release,
            1 => Due::Input,
            _ => Due::Step,
        };
        Some((time, due))
    }

    fn next_step_time(&self, block: &Block) -> u64 {
        if block.playing {
            block.grid_boundary_at_or_after(self.not_before.max(block.start))
        } else {
            (self.free_next as u64).max(block.start)
        }
    }

    fn handle_input(&mut self, event: MidiEvent, time: u64, output: &mut MidiEventList) {
        match event.kind {
            MidiEventKind::NoteOn { pitch, velocity } => {
                if self.note_on(pitch, velocity) {
                    self.restart_pattern(time);
                }
            }
            MidiEventKind::NoteOff { pitch } => {
                if self.note_off(pitch) {
                    self.release_sounding(event.offset, output);
                    self.step_pending = false;
                }
            }
            MidiEventKind::Control { .. } => output.push_control(event),
            // The notes it plays are its own, not the held ones the curve was drawn for.
            MidiEventKind::NoteExpression { .. } => {}
        }
    }

    fn play_step(
        &mut self,
        block: &Block,
        settings: &ArpeggiatorSettings,
        time: u64,
        output: &mut MidiEventList,
    ) {
        self.not_before = time + 1;
        self.free_next += block.step_samples();
        let index = self.step_index;
        self.step_index = index.wrapping_add(1);
        let picked = self.pick(settings, index);
        if output.remaining_capacity() < STEP_EVENT_ROOM {
            return;
        }
        let offset = (time - block.start) as u32;
        self.release_sounding(offset, output);
        let Some((pitch, velocity)) = picked else {
            return;
        };
        output.push(note_on(offset, pitch, velocity));
        self.sounding = Some(SoundingNote {
            pitch,
            release_at: time + block.gate_samples(),
        });
    }

    fn release_sounding(&mut self, offset: u32, output: &mut MidiEventList) {
        if let Some(note) = self.sounding.take() {
            output.push(note_off(offset, note.pitch));
        }
    }

    fn pick(&mut self, settings: &ArpeggiatorSettings, index: u32) -> Option<(u8, u8)> {
        let held_count = self.held_len;
        if held_count == 0 {
            return None;
        }
        let total = held_count * settings.octaves.max(1) as usize;
        let step = index as usize;
        let position = match settings.mode {
            ArpMode::Up | ArpMode::Played => step % total,
            ArpMode::Down => total - 1 - step % total,
            ArpMode::UpDown => bounce(step, total),
            ArpMode::Random => {
                self.random = xorshift32(self.random);
                self.random as usize % total
            }
        };
        let slot = position % held_count;
        let octave = (position / held_count) as u16;
        let held_index = match settings.mode {
            ArpMode::Played => slot,
            _ => self.by_pitch[slot] as usize,
        };
        let held = self.held[held_index];
        let pitch = held.pitch as u16 + 12 * octave;
        (pitch < PITCH_COUNT as u16).then_some((pitch as u8, held.velocity))
    }

    /// Returns true when this note starts a chord (nothing was held before).
    fn note_on(&mut self, pitch: u8, velocity: u8) -> bool {
        if let Some(held) = self.held[..self.held_len]
            .iter_mut()
            .find(|held| held.pitch == pitch)
        {
            held.count = held.count.saturating_add(1);
            held.velocity = velocity;
            return false;
        }
        if self.held_len == MAX_HELD_NOTES {
            return false;
        }
        let starts_chord = self.held_len == 0;
        self.held[self.held_len] = HeldNote {
            pitch,
            velocity,
            count: 1,
        };
        self.held_len += 1;
        self.sort_by_pitch();
        starts_chord
    }

    /// Returns true when this note was the last one held.
    fn note_off(&mut self, pitch: u8) -> bool {
        let Some(position) = self.held[..self.held_len]
            .iter()
            .position(|held| held.pitch == pitch)
        else {
            return false;
        };
        let held = &mut self.held[position];
        held.count -= 1;
        if held.count > 0 {
            return false;
        }
        self.held.copy_within(position + 1..self.held_len, position);
        self.held_len -= 1;
        self.sort_by_pitch();
        self.held_len == 0
    }

    fn sort_by_pitch(&mut self) {
        for index in 0..self.held_len {
            self.by_pitch[index] = index as u8;
        }
        for sorted_end in 1..self.held_len {
            let mut position = sorted_end;
            while position > 0
                && self.held[self.by_pitch[position - 1] as usize].pitch
                    > self.held[self.by_pitch[position] as usize].pitch
            {
                self.by_pitch.swap(position - 1, position);
                position -= 1;
            }
        }
    }
}

/// Up then down without repeating the end notes (C E G → C E G E C E G …).
fn bounce(step: usize, total: usize) -> usize {
    if total == 1 {
        return 0;
    }
    let period = 2 * total - 2;
    let phase = step % period;
    if phase < total {
        phase
    } else {
        period - phase
    }
}

fn xorshift32(mut state: u32) -> u32 {
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    state
}

fn note_on(offset: u32, pitch: u8, velocity: u8) -> MidiEvent {
    MidiEvent {
        offset,
        kind: MidiEventKind::NoteOn { pitch, velocity },
    }
}

fn note_off(offset: u32, pitch: u8) -> MidiEvent {
    MidiEvent {
        offset,
        kind: MidiEventKind::NoteOff { pitch },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arpeggiator_settings::ArpRate;
    use crate::audio_graph::node::TimeSignature;

    const SAMPLE_RATE: f32 = 48_000.0;
    /// 1/16 note at 120 BPM.
    const SIXTEENTH: u64 = 6_000;

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Played {
        On(u8),
        Off(u8),
        Control(u8),
        Expression,
    }

    type Timeline = Vec<(u64, Played)>;

    fn key_down(time: u64, pitch: u8) -> (u64, MidiEventKind) {
        (
            time,
            MidiEventKind::NoteOn {
                pitch,
                velocity: 100,
            },
        )
    }

    fn key_up(time: u64, pitch: u8) -> (u64, MidiEventKind) {
        (time, MidiEventKind::NoteOff { pitch })
    }

    struct Rig {
        arp: Arpeggiator,
        settings: ArpeggiatorSettings,
        bpm: f32,
        playing: bool,
        time: u64,
        block_size: usize,
        played: Timeline,
        overflowed: bool,
    }

    impl Rig {
        fn playing(settings: ArpeggiatorSettings) -> Self {
            Self::new(settings, true)
        }

        fn stopped(settings: ArpeggiatorSettings) -> Self {
            Self::new(settings, false)
        }

        fn new(settings: ArpeggiatorSettings, playing: bool) -> Self {
            Self {
                arp: Arpeggiator::new(),
                settings,
                bpm: 120.0,
                playing,
                time: 0,
                block_size: 512,
                played: Vec::new(),
                overflowed: false,
            }
        }

        fn with_block_size(mut self, block_size: usize) -> Self {
            self.block_size = block_size;
            self
        }

        fn run_until(&mut self, end: u64, keys: &[(u64, MidiEventKind)]) {
            while self.time < end {
                let size = (self.block_size as u64).min(end - self.time) as usize;
                self.block(size, keys);
            }
        }

        fn block(&mut self, size: usize, keys: &[(u64, MidiEventKind)]) {
            let start = self.time;
            let input: Vec<MidiEvent> = keys
                .iter()
                .filter(|(time, _)| *time >= start && *time < start + size as u64)
                .map(|(time, kind)| MidiEvent {
                    offset: (*time - start) as u32,
                    kind: *kind,
                })
                .collect();
            let transport = TransportInfo {
                is_playing: self.playing,
                position_samples: if self.playing { start } else { 0 },
                position_seconds: start as f64 / SAMPLE_RATE as f64,
                bpm: self.bpm,
                time_signature: TimeSignature::default(),
                sample_rate: SAMPLE_RATE,
                block_size: size,
            };
            let mut output = MidiEventList::new();
            let routing = self
                .arp
                .process(&self.settings, &transport, &input, &mut output);
            let events = match routing {
                Routing::Passthrough => &input[..],
                Routing::Replaced => output.as_slice(),
            };
            self.overflowed |= output.remaining_capacity() == 0;
            for event in events {
                let played = match event.kind {
                    MidiEventKind::NoteOn { pitch, .. } => Played::On(pitch),
                    MidiEventKind::NoteOff { pitch } => Played::Off(pitch),
                    MidiEventKind::Control { message } => Played::Control(message[1]),
                    MidiEventKind::NoteExpression { .. } => Played::Expression,
                };
                self.played.push((start + event.offset as u64, played));
            }
            self.time += size as u64;
        }

        fn note_ons(&self) -> Vec<(u64, u8)> {
            self.played
                .iter()
                .filter_map(|(time, played)| match played {
                    Played::On(pitch) => Some((*time, *pitch)),
                    _ => None,
                })
                .collect()
        }

        fn pitches(&self) -> Vec<u8> {
            self.note_ons()
                .into_iter()
                .map(|(_, pitch)| pitch)
                .collect()
        }

        fn note_offs(&self) -> Vec<(u64, u8)> {
            self.played
                .iter()
                .filter_map(|(time, played)| match played {
                    Played::Off(pitch) => Some((*time, *pitch)),
                    _ => None,
                })
                .collect()
        }
    }

    fn enabled(mode: ArpMode) -> ArpeggiatorSettings {
        ArpeggiatorSettings {
            enabled: true,
            mode,
            ..ArpeggiatorSettings::default()
        }
    }

    fn chord(pitches: &[u8]) -> Vec<(u64, MidiEventKind)> {
        pitches.iter().map(|&pitch| key_down(0, pitch)).collect()
    }

    const TRIAD: [u8; 3] = [60, 64, 67];

    #[test]
    fn up_plays_the_chord_note_by_note_on_the_grid() {
        let mut rig = Rig::playing(enabled(ArpMode::Up));
        rig.run_until(4 * SIXTEENTH, &chord(&TRIAD));

        assert_eq!(
            rig.note_ons(),
            vec![
                (0, 60),
                (SIXTEENTH, 64),
                (2 * SIXTEENTH, 67),
                (3 * SIXTEENTH, 60)
            ]
        );
    }

    #[test]
    fn each_mode_orders_the_notes() {
        let pressed_out_of_order = [67, 60, 64];
        let sequence = |mode| {
            let mut rig = Rig::playing(enabled(mode));
            let keys: Vec<_> = pressed_out_of_order
                .iter()
                .map(|&pitch| key_down(0, pitch))
                .collect();
            rig.run_until(7 * SIXTEENTH, &keys);
            rig.pitches()
        };

        assert_eq!(sequence(ArpMode::Up), [60, 64, 67, 60, 64, 67, 60]);
        assert_eq!(sequence(ArpMode::Down), [67, 64, 60, 67, 64, 60, 67]);
        assert_eq!(sequence(ArpMode::UpDown), [60, 64, 67, 64, 60, 64, 67]);
        assert_eq!(sequence(ArpMode::Played), [67, 60, 64, 67, 60, 64, 67]);
    }

    #[test]
    fn random_is_repeatable_and_stays_inside_the_chord() {
        let sequence = || {
            let mut rig = Rig::playing(enabled(ArpMode::Random));
            rig.run_until(16 * SIXTEENTH, &chord(&TRIAD));
            rig.pitches()
        };

        let first = sequence();
        assert_eq!(first, sequence());
        assert_eq!(first.len(), 16);
        assert!(first.iter().all(|pitch| TRIAD.contains(pitch)));
        assert!(
            first.windows(2).any(|pair| pair[0] != pair[1]),
            "a random pattern must not be constant"
        );
    }

    #[test]
    fn random_restarts_the_same_pattern_for_every_chord() {
        let mut rig = Rig::playing(enabled(ArpMode::Random));
        let mut keys = chord(&TRIAD);
        keys.extend(TRIAD.iter().map(|&pitch| key_up(8 * SIXTEENTH, pitch)));
        keys.extend(TRIAD.iter().map(|&pitch| key_down(8 * SIXTEENTH, pitch)));
        rig.run_until(16 * SIXTEENTH, &keys);

        let pitches = rig.pitches();
        assert_eq!(pitches[..8], pitches[8..16]);
    }

    #[test]
    fn rates_set_the_step_length() {
        let spacing = |rate, triplet| {
            let mut rig = Rig::playing(ArpeggiatorSettings {
                rate,
                triplet,
                ..enabled(ArpMode::Up)
            });
            rig.run_until(40_000, &chord(&[60, 64]));
            let times: Vec<u64> = rig.note_ons().iter().map(|(time, _)| *time).collect();
            times[1] - times[0]
        };

        assert_eq!(spacing(ArpRate::Quarter, false), 24_000);
        assert_eq!(spacing(ArpRate::Eighth, false), 12_000);
        assert_eq!(spacing(ArpRate::Sixteenth, false), 6_000);
        assert_eq!(spacing(ArpRate::ThirtySecond, false), 3_000);
        assert_eq!(spacing(ArpRate::Eighth, true), 8_000);
    }

    #[test]
    fn gate_sets_the_note_length() {
        let length = |gate_percent| {
            let mut rig = Rig::playing(ArpeggiatorSettings {
                gate_percent,
                ..enabled(ArpMode::Up)
            });
            rig.run_until(SIXTEENTH + 1, &chord(&[60]));
            rig.note_offs()[0].0
        };

        assert_eq!(length(50), 3_000);
        assert_eq!(length(80), 4_800);
        assert_eq!(length(100), SIXTEENTH);
    }

    #[test]
    fn a_full_gate_releases_before_the_next_note_starts() {
        let mut rig = Rig::playing(ArpeggiatorSettings {
            gate_percent: 100,
            ..enabled(ArpMode::Up)
        });
        rig.run_until(2 * SIXTEENTH + 1, &chord(&[60]));

        let at_second_step: Vec<Played> = rig
            .played
            .iter()
            .filter(|(time, _)| *time == SIXTEENTH)
            .map(|(_, played)| *played)
            .collect();
        assert_eq!(at_second_step, [Played::Off(60), Played::On(60)]);
    }

    #[test]
    fn octaves_extend_the_pattern_upwards() {
        let sequence = |octaves, pitches: &[u8], steps: u64| {
            let mut rig = Rig::playing(ArpeggiatorSettings {
                octaves,
                ..enabled(ArpMode::Up)
            });
            rig.run_until(steps * SIXTEENTH, &chord(pitches));
            rig.pitches()
        };

        assert_eq!(sequence(1, &[60], 3), [60, 60, 60]);
        assert_eq!(sequence(2, &[60], 4), [60, 72, 60, 72]);
        assert_eq!(sequence(3, &[60, 64], 7), [60, 64, 72, 76, 84, 88, 60]);
        assert_eq!(
            sequence(4, &[60, 64, 67, 71], 17)[12..17],
            [96, 100, 103, 107, 60]
        );
    }

    #[test]
    fn notes_past_the_top_of_the_keyboard_are_skipped_but_keep_the_rhythm() {
        let mut rig = Rig::playing(ArpeggiatorSettings {
            octaves: 2,
            ..enabled(ArpMode::Up)
        });
        rig.run_until(4 * SIXTEENTH, &chord(&[120]));

        assert_eq!(rig.note_ons(), vec![(0, 120), (2 * SIXTEENTH, 120)]);
    }

    #[test]
    fn chords_of_one_three_and_four_notes_repeat_their_length() {
        for pitches in [&[60][..], &[60, 64, 67][..], &[60, 64, 67, 71][..]] {
            let mut rig = Rig::playing(enabled(ArpMode::Up));
            rig.run_until(2 * pitches.len() as u64 * SIXTEENTH, &chord(pitches));

            let heard = rig.pitches();
            assert_eq!(heard.len(), 2 * pitches.len());
            assert_eq!(heard[..pitches.len()], heard[pitches.len()..]);
        }
    }

    #[test]
    fn releasing_the_last_key_stops_the_note_at_once() {
        let mut rig = Rig::playing(enabled(ArpMode::Up));
        let mut keys = chord(&[60]);
        keys.push(key_up(1_000, 60));
        rig.run_until(4 * SIXTEENTH, &keys);

        assert_eq!(rig.note_ons(), vec![(0, 60)]);
        assert_eq!(rig.note_offs(), vec![(1_000, 60)]);
    }

    #[test]
    fn a_new_chord_starts_its_pattern_over() {
        let mut rig = Rig::playing(enabled(ArpMode::Up));
        let mut keys = chord(&TRIAD);
        keys.extend(TRIAD.iter().map(|&pitch| key_up(2 * SIXTEENTH, pitch)));
        keys.extend(
            chord(&[72, 76])
                .into_iter()
                .map(|(_, kind)| (3 * SIXTEENTH, kind)),
        );
        rig.run_until(6 * SIXTEENTH, &keys);

        assert_eq!(rig.pitches(), [60, 64, 72, 76, 72]);
    }

    #[test]
    fn a_chord_pressed_between_steps_waits_for_the_next_grid_line() {
        let mut rig = Rig::playing(enabled(ArpMode::Up));
        let keys = vec![key_down(1_000, 60)];
        rig.run_until(2 * SIXTEENTH, &keys);

        assert_eq!(rig.note_ons(), vec![(SIXTEENTH, 60)]);
    }

    #[test]
    fn the_tempo_changes_the_step_from_the_next_one() {
        let mut rig = Rig::playing(enabled(ArpMode::Up));
        let keys = chord(&[60, 64]);
        rig.run_until(2 * SIXTEENTH, &keys);
        rig.bpm = 240.0;
        rig.run_until(2 * SIXTEENTH + 9_001, &keys);

        let times: Vec<u64> = rig.note_ons().iter().map(|(time, _)| *time).collect();
        assert_eq!(times, [0, 6_000, 12_000, 15_000, 18_000, 21_000]);
    }

    #[test]
    fn the_result_does_not_depend_on_the_block_size() {
        let mut keys = chord(&TRIAD);
        keys.push(key_down(7_321, 72));
        keys.push(key_up(40_001, 64));
        keys.push(key_up(61_111, 60));
        let reference = {
            let mut rig = Rig::playing(enabled(ArpMode::UpDown));
            rig.run_until(90_000, &keys);
            rig.played
        };
        assert!(reference.len() > 20);

        for block_size in [4096, 441, 64, 37] {
            let mut rig = Rig::playing(enabled(ArpMode::UpDown)).with_block_size(block_size);
            rig.run_until(90_000, &keys);
            assert_eq!(rig.played, reference, "block size {block_size}");
        }
    }

    #[test]
    fn controls_pass_through_where_they_were() {
        let mut rig = Rig::playing(enabled(ArpMode::Up));
        let mut keys = chord(&[60]);
        keys.push((
            777,
            MidiEventKind::Control {
                message: [0xB0, 64, 127],
            },
        ));
        rig.run_until(SIXTEENTH, &keys);

        assert!(rig.played.contains(&(777, Played::Control(64))));
    }

    #[test]
    fn a_stopped_transport_anchors_the_grid_on_the_chord() {
        let mut rig = Rig::stopped(enabled(ArpMode::Up));
        let keys = vec![key_down(1_000, 60), key_down(1_000, 64)];
        rig.run_until(1_000 + 3 * SIXTEENTH, &keys);

        assert_eq!(
            rig.note_ons(),
            vec![
                (1_000, 60),
                (1_000 + SIXTEENTH, 64),
                (1_000 + 2 * SIXTEENTH, 60)
            ]
        );
    }

    #[test]
    fn the_stopped_grid_does_not_depend_on_the_block_size() {
        let keys = vec![key_down(1_000, 60), key_down(1_500, 64), key_up(30_000, 60)];
        let reference = {
            let mut rig = Rig::stopped(enabled(ArpMode::Up));
            rig.run_until(50_000, &keys);
            rig.played
        };
        for block_size in [4096, 441, 37] {
            let mut rig = Rig::stopped(enabled(ArpMode::Up)).with_block_size(block_size);
            rig.run_until(50_000, &keys);
            assert_eq!(rig.played, reference, "block size {block_size}");
        }
    }

    #[test]
    fn starting_playback_while_a_key_is_held_moves_to_the_song_grid() {
        let mut rig = Rig::stopped(enabled(ArpMode::Up));
        let keys = vec![key_down(0, 60), key_down(0, 64)];
        rig.run_until(1_024, &keys);
        assert!(rig.arp.is_sounding());

        rig.playing = true;
        rig.time = 6_000 * 4 + 1_000;
        rig.block(512, &[]);

        assert_eq!(
            rig.played.last(),
            Some(&(6_000 * 4 + 1_000, Played::Off(60)))
        );
    }

    #[test]
    fn a_seek_releases_the_sounding_note() {
        let mut rig = Rig::playing(enabled(ArpMode::Up));
        rig.run_until(512, &chord(&[60]));
        assert!(rig.arp.is_sounding());

        rig.time = 100_000;
        rig.block(512, &[]);

        assert!(rig.played.contains(&(100_000, Played::Off(60))));
    }

    #[test]
    fn switching_off_hands_the_held_chord_to_the_instrument() {
        let mut rig = Rig::playing(enabled(ArpMode::Up));
        rig.run_until(512, &chord(&[60, 64]));
        rig.settings.enabled = false;
        rig.block(512, &[]);

        let after: Vec<Played> = rig.played.iter().skip(1).map(|(_, p)| *p).collect();
        assert_eq!(after, [Played::Off(60), Played::On(60), Played::On(64)]);

        let released = [key_up(1_100, 60), key_up(1_100, 64)];
        rig.run_until(1_536, &released);
        assert!(rig.played.contains(&(1_100, Played::Off(64))));
    }

    #[test]
    fn switching_on_releases_the_notes_the_instrument_was_holding() {
        let mut rig = Rig::playing(ArpeggiatorSettings::default());
        rig.run_until(512, &chord(&[60, 64]));
        assert_eq!(rig.pitches(), [60, 64]);

        rig.settings.enabled = true;
        rig.block(512, &[]);

        let offs: Vec<u8> = rig
            .note_offs()
            .into_iter()
            .map(|(_, pitch)| pitch)
            .collect();
        assert_eq!(offs, [60, 64]);
    }

    #[test]
    fn a_disabled_arpeggiator_leaves_the_stream_alone() {
        let mut arp = Arpeggiator::new();
        let input = [
            note_on(10, 60, 100),
            MidiEvent {
                offset: 20,
                kind: MidiEventKind::Control {
                    message: [0xE0, 0, 64],
                },
            },
            note_off(30, 60),
            note_off(40, 61),
        ];
        let transport = TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: TimeSignature::default(),
            sample_rate: SAMPLE_RATE,
            block_size: 64,
        };

        let routing = arp.process(
            &ArpeggiatorSettings::default(),
            &transport,
            &input,
            &mut MidiEventList::new(),
        );

        assert_eq!(routing, Routing::Passthrough);
    }

    #[test]
    fn a_seventeenth_note_is_ignored() {
        let mut rig = Rig::playing(enabled(ArpMode::Up));
        let pitches: Vec<u8> = (40..57).collect();
        rig.run_until(17 * SIXTEENTH, &chord(&pitches));

        let heard = rig.pitches();
        assert_eq!(heard.len(), 17);
        assert!(!heard.contains(&56));
    }

    #[test]
    fn a_dense_block_skips_steps_but_never_a_release() {
        let settings = ArpeggiatorSettings {
            rate: ArpRate::ThirtySecond,
            triplet: true,
            gate_percent: 100,
            ..enabled(ArpMode::Up)
        };
        let mut rig = Rig::playing(settings).with_block_size(60_000);
        rig.bpm = 300.0;
        let mut keys = chord(&TRIAD);
        keys.extend(TRIAD.iter().map(|&pitch| key_up(100_000, pitch)));
        rig.run_until(180_000, &keys);

        assert!(rig.overflowed || rig.note_ons().len() > 100);
        assert!(!rig.arp.is_sounding());
        let on = rig.note_ons().len();
        let off = rig.note_offs().len();
        assert_eq!(on, off, "every NoteOn is released");
    }
}
