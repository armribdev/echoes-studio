//! Replay of recorded pitch bend, control changes and aftertouch (ADR-029).
//!
//! A control is a state, not an event: after a seek, a loop wrap or an edit the
//! instrument must get the value the timeline holds at the new position, and a
//! sustain pedal or a bend must not outlive the clip that set it. Everything
//! here runs on pre-allocated data: a fixed array of flags and the events
//! pushed into the block's `MidiEventList`.

use crate::audio_graph::midi::{beats_to_sample, MidiEvent, MidiEventKind, MidiEventList};
use crate::audio_graph::node::TransportInfo;

/// Controls whose value is held between two points: 128 control changes, then
/// the pitch bend and the channel pressure.
pub const CONTROL_SLOTS: usize = 130;
const PITCH_BEND_SLOT: usize = 128;
const CHANNEL_PRESSURE_SLOT: usize = 129;
const PITCH_BEND_CENTER: u16 = 0x2000;
const NEUTRAL_CONTROLLERS: [(u8, u16); 6] = [(1, 0), (11, 127), (64, 0), (65, 0), (66, 0), (67, 0)];

const STATUS_CONTROL_CHANGE: u8 = 0xB0;
const STATUS_CHANNEL_PRESSURE: u8 = 0xD0;
const STATUS_PITCH_BEND: u8 = 0xE0;

/// Slot of a held control, `None` for polyphonic pressure (tied to a note).
pub fn control_slot(status: u8, number: u8) -> Option<usize> {
    match status {
        STATUS_CONTROL_CHANGE => Some(number as usize & 0x7F),
        STATUS_PITCH_BEND => Some(PITCH_BEND_SLOT),
        STATUS_CHANNEL_PRESSURE => Some(CHANNEL_PRESSURE_SLOT),
        _ => None,
    }
}

/// Value a control returns to when playback leaves the clip that moved it, as
/// after a "reset all controllers". `None` for controls that keep their level
/// (volume, pan, filters…): there is no earlier value to restore.
pub fn control_neutral_value(status: u8, number: u8) -> Option<u16> {
    match status {
        STATUS_PITCH_BEND => Some(PITCH_BEND_CENTER),
        STATUS_CHANNEL_PRESSURE => Some(0),
        STATUS_CONTROL_CHANGE => NEUTRAL_CONTROLLERS
            .iter()
            .find(|(controller, _)| *controller == number)
            .map(|(_, value)| *value),
        _ => None,
    }
}

fn slot_status_and_number(slot: usize) -> (u8, u8) {
    match slot {
        PITCH_BEND_SLOT => (STATUS_PITCH_BEND, 0),
        CHANNEL_PRESSURE_SLOT => (STATUS_CHANNEL_PRESSURE, 0),
        controller => (STATUS_CONTROL_CHANGE, controller as u8),
    }
}

fn slot_neutral_value(slot: usize) -> Option<u16> {
    let (status, number) = slot_status_and_number(slot);
    control_neutral_value(status, number)
}

fn control_message(status: u8, number: u8, value: u16) -> [u8; 3] {
    match status {
        STATUS_PITCH_BEND => [status, (value & 0x7F) as u8, (value >> 7) as u8],
        STATUS_CHANNEL_PRESSURE => [status, value as u8, 0],
        _ => [status, number, value as u8],
    }
}

fn slot_event(offset: u32, slot: usize, value: u16) -> MidiEvent {
    let (status, number) = slot_status_and_number(slot);
    MidiEvent {
        offset,
        kind: MidiEventKind::Control {
            message: control_message(status, number, value),
        },
    }
}

/// One control point of a clip, in beat space like `MidiNoteSnapshot`.
#[derive(Debug, Clone, Copy)]
pub struct MidiControlSnapshot {
    /// Position in beats, relative to the clip content origin.
    pub start_beats: f32,
    /// MIDI status byte without channel: `0xA0`, `0xB0`, `0xD0` or `0xE0`.
    pub status: u8,
    pub number: u8,
    pub value: u16,
}

impl MidiControlSnapshot {
    fn slot(&self) -> Option<usize> {
        control_slot(self.status, self.number)
    }
}

/// The control points of one clip with its playable window (ADR-018), ordered
/// by position.
#[derive(Debug, Clone)]
pub struct MidiControlClip {
    /// Content origin on the project timeline, in seconds.
    pub clip_start_sec: f64,
    pub window_start_sample: u64,
    pub window_end_sample: u64,
    pub points: Vec<MidiControlSnapshot>,
}

impl MidiControlClip {
    fn point_sample(&self, point: &MidiControlSnapshot, transport: &TransportInfo) -> u64 {
        beats_to_sample(
            self.clip_start_sec,
            point.start_beats,
            transport.bpm,
            transport.sample_rate,
        )
    }

    fn used_slots(&self) -> [bool; CONTROL_SLOTS] {
        let mut used = [false; CONTROL_SLOTS];
        for slot in self.points.iter().filter_map(MidiControlSnapshot::slot) {
            used[slot] = true;
        }
        used
    }
}

/// Held value of each control at `position`: the last point at or before it in
/// a clip whose window covers it. Points hidden by a left-trim count, so a clip
/// entered mid-way starts with the right value.
fn held_values(
    clips: &[MidiControlClip],
    position: u64,
    transport: &TransportInfo,
) -> [Option<u16>; CONTROL_SLOTS] {
    let mut latest = [None::<(u64, u16)>; CONTROL_SLOTS];
    for clip in clips {
        if position < clip.window_start_sample || position >= clip.window_end_sample {
            continue;
        }
        for point in &clip.points {
            let sample = clip.point_sample(point, transport);
            if sample > position {
                break;
            }
            let Some(slot) = point.slot() else {
                continue;
            };
            if latest[slot].map_or(true, |(at, _)| sample >= at) {
                latest[slot] = Some((sample, point.value));
            }
        }
    }
    latest.map(|entry| entry.map(|(_, value)| value))
}

/// Which controls the timeline has moved since the last neutral, per instrument.
pub struct ControlState {
    moved: [bool; CONTROL_SLOTS],
}

impl ControlState {
    pub fn new() -> Self {
        Self {
            moved: [false; CONTROL_SLOTS],
        }
    }

    pub fn reset(&mut self) {
        self.moved = [false; CONTROL_SLOTS];
    }

    /// The playhead jumped, or the clips changed under it: bring every held
    /// control to the value the timeline has at the block start (neutral where
    /// no clip covers it), at offset 0.
    pub fn sync(
        &mut self,
        clips: &[MidiControlClip],
        events: &mut MidiEventList,
        transport: &TransportInfo,
    ) {
        let held = held_values(clips, transport.position_samples, transport);
        for (slot, held_value) in held.iter().enumerate() {
            let value = match held_value {
                Some(value) => Some(*value),
                None if self.moved[slot] => slot_neutral_value(slot),
                None => None,
            };
            self.moved[slot] = held_value.is_some();
            if let Some(value) = value {
                events.push_control(slot_event(0, slot, value));
            }
        }
    }

    /// Continuous playback of a block. Only samples from `from` on are played:
    /// the block start itself belongs to `sync` when it ran for this block.
    ///
    /// Ends come first, then starts, then points. At one offset the stable sort
    /// keeps this order, so a clip ending where another begins hands over cleanly.
    pub fn play_block(
        &mut self,
        clips: &[MidiControlClip],
        events: &mut MidiEventList,
        transport: &TransportInfo,
        from: u64,
    ) {
        let block = BlockWindow {
            start: transport.position_samples,
            from,
            to: transport.position_samples + transport.block_size as u64,
        };
        self.end_clips(clips, events, &block);
        self.start_clips(clips, events, &block, transport);
        self.play_points(clips, events, &block, transport);
    }

    /// Pause or stop: neutral for what the timeline had moved.
    pub fn release(&mut self, events: &mut MidiEventList) {
        for slot in 0..CONTROL_SLOTS {
            if !self.moved[slot] {
                continue;
            }
            self.moved[slot] = false;
            if let Some(neutral) = slot_neutral_value(slot) {
                events.push_control(slot_event(0, slot, neutral));
            }
        }
    }

    fn end_clips(
        &mut self,
        clips: &[MidiControlClip],
        events: &mut MidiEventList,
        block: &BlockWindow,
    ) {
        for clip in clips {
            if !block.contains(clip.window_end_sample) {
                continue;
            }
            for (slot, used) in clip.used_slots().into_iter().enumerate() {
                let Some(neutral) = slot_neutral_value(slot).filter(|_| used) else {
                    continue;
                };
                events.push_control(slot_event(
                    block.offset(clip.window_end_sample),
                    slot,
                    neutral,
                ));
                self.moved[slot] = false;
            }
        }
    }

    fn start_clips(
        &mut self,
        clips: &[MidiControlClip],
        events: &mut MidiEventList,
        block: &BlockWindow,
        transport: &TransportInfo,
    ) {
        for clip in clips {
            let start = clip.window_start_sample;
            if !block.contains(start) {
                continue;
            }
            let held = held_values(std::slice::from_ref(clip), start, transport);
            for (slot, value) in held.iter().enumerate() {
                if let Some(value) = value {
                    events.push_control(slot_event(block.offset(start), slot, *value));
                    self.moved[slot] = true;
                }
            }
        }
    }

    fn play_points(
        &mut self,
        clips: &[MidiControlClip],
        events: &mut MidiEventList,
        block: &BlockWindow,
        transport: &TransportInfo,
    ) {
        for clip in clips {
            let lower = block.from.max(clip.window_start_sample);
            let upper = block.to.min(clip.window_end_sample);
            let first = clip
                .points
                .partition_point(|point| clip.point_sample(point, transport) < lower);
            for point in &clip.points[first..] {
                let sample = clip.point_sample(point, transport);
                if sample >= upper {
                    break;
                }
                let slot = point.slot();
                // A held control at the window start was already sent by `start_clips`.
                if slot.is_some() && sample == clip.window_start_sample {
                    continue;
                }
                events.push_control(MidiEvent {
                    offset: block.offset(sample),
                    kind: MidiEventKind::Control {
                        message: control_message(point.status, point.number, point.value),
                    },
                });
                if let Some(slot) = slot {
                    self.moved[slot] = true;
                }
            }
        }
    }
}

struct BlockWindow {
    start: u64,
    from: u64,
    to: u64,
}

impl BlockWindow {
    fn contains(&self, sample: u64) -> bool {
        sample >= self.from && sample < self.to
    }

    fn offset(&self, sample: u64) -> u32 {
        (sample - self.start) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::node::TimeSignature;

    const SAMPLES_PER_BEAT: u64 = 24_000;
    const SUSTAIN_ON: [u8; 3] = [0xB0, 64, 127];
    const SUSTAIN_OFF: [u8; 3] = [0xB0, 64, 0];
    const BEND_UP: [u8; 3] = [0xE0, 0, 0x60];
    const BEND_CENTER: [u8; 3] = [0xE0, 0, 0x40];

    fn transport(position_samples: u64, block_size: usize) -> TransportInfo {
        TransportInfo {
            is_playing: true,
            position_samples,
            position_seconds: position_samples as f64 / 48_000.0,
            bpm: 120.0,
            time_signature: TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size,
        }
    }

    fn sustain(start_beats: f32, on: bool) -> MidiControlSnapshot {
        MidiControlSnapshot {
            start_beats,
            status: 0xB0,
            number: 64,
            value: if on { 127 } else { 0 },
        }
    }

    fn bend(start_beats: f32, value: u16) -> MidiControlSnapshot {
        MidiControlSnapshot {
            start_beats,
            status: 0xE0,
            number: 0,
            value,
        }
    }

    fn clip(
        window_beats: (u64, u64),
        content_origin_sec: f64,
        points: Vec<MidiControlSnapshot>,
    ) -> MidiControlClip {
        MidiControlClip {
            clip_start_sec: content_origin_sec,
            window_start_sample: window_beats.0 * SAMPLES_PER_BEAT,
            window_end_sample: window_beats.1 * SAMPLES_PER_BEAT,
            points,
        }
    }

    fn messages(events: &MidiEventList) -> Vec<(u32, [u8; 3])> {
        events
            .as_slice()
            .iter()
            .map(|event| match event.kind {
                MidiEventKind::Control { message } => (event.offset, message),
                other => panic!("unexpected event {other:?}"),
            })
            .collect()
    }

    fn sync_at(
        clips: &[MidiControlClip],
        state: &mut ControlState,
        position: u64,
    ) -> Vec<(u32, [u8; 3])> {
        let mut events = MidiEventList::new();
        state.sync(clips, &mut events, &transport(position, 512));
        messages(&events)
    }

    /// Every control event of a run, as absolute sample positions, sorted.
    fn play(clips: &[MidiControlClip], block_size: usize, length: u64) -> Vec<(u64, [u8; 3])> {
        let mut state = ControlState::new();
        let mut played = Vec::new();
        let mut position = 0;
        let mut first = true;
        while position < length {
            let transport = transport(position, block_size);
            let mut events = MidiEventList::new();
            if first {
                state.sync(clips, &mut events, &transport);
            }
            state.play_block(clips, &mut events, &transport, position + first as u64);
            events.sort_by_offset();
            played.extend(
                messages(&events)
                    .into_iter()
                    .map(|(offset, message)| (position + offset as u64, message)),
            );
            first = false;
            position += block_size as u64;
        }
        played
    }

    #[test]
    fn seek_sends_the_value_the_timeline_holds() {
        let clips = [clip(
            (0, 8),
            0.0,
            vec![bend(1.0, 0x3000), bend(3.0, 0x2000)],
        )];
        let mut state = ControlState::new();
        assert_eq!(
            sync_at(&clips, &mut state, 2 * SAMPLES_PER_BEAT),
            vec![(0, [0xE0, 0, 0x60])]
        );
        assert_eq!(
            sync_at(&clips, &mut state, 4 * SAMPLES_PER_BEAT),
            vec![(0, BEND_CENTER)]
        );
    }

    #[test]
    fn seek_before_any_point_leaves_controls_alone() {
        let clips = [clip((0, 8), 0.0, vec![sustain(2.0, true)])];
        let mut state = ControlState::new();
        assert!(sync_at(&clips, &mut state, SAMPLES_PER_BEAT).is_empty());
    }

    #[test]
    fn seek_out_of_every_clip_returns_a_moved_control_to_neutral() {
        let clips = [clip((0, 4), 0.0, vec![sustain(1.0, true)])];
        let mut state = ControlState::new();
        sync_at(&clips, &mut state, 2 * SAMPLES_PER_BEAT);
        assert_eq!(
            sync_at(&clips, &mut state, 6 * SAMPLES_PER_BEAT),
            vec![(0, SUSTAIN_OFF)]
        );
        assert!(sync_at(&clips, &mut state, 7 * SAMPLES_PER_BEAT).is_empty());
    }

    #[test]
    fn point_hidden_by_a_left_trim_still_holds_at_the_window_start() {
        let clips = [clip((2, 8), 0.0, vec![sustain(1.0, true)])];
        let mut state = ControlState::new();
        assert_eq!(
            sync_at(&clips, &mut state, 2 * SAMPLES_PER_BEAT),
            vec![(0, SUSTAIN_ON)]
        );
    }

    #[test]
    fn entering_a_trimmed_clip_sends_its_held_value_at_the_window_start() {
        let clips = [clip((2, 8), 0.0, vec![sustain(1.0, true)])];
        let played = play(&clips, 512, 3 * SAMPLES_PER_BEAT);
        assert_eq!(played, vec![(2 * SAMPLES_PER_BEAT, SUSTAIN_ON)]);
    }

    #[test]
    fn clip_end_releases_a_pedal_that_was_down() {
        let clips = [clip((0, 4), 0.0, vec![sustain(1.0, true)])];
        let played = play(&clips, 512, 5 * SAMPLES_PER_BEAT);
        assert_eq!(
            played,
            vec![
                (SAMPLES_PER_BEAT, SUSTAIN_ON),
                (4 * SAMPLES_PER_BEAT, SUSTAIN_OFF)
            ]
        );
    }

    #[test]
    fn control_without_a_neutral_keeps_its_level_after_the_clip() {
        let filter = MidiControlSnapshot {
            start_beats: 0.5,
            status: 0xB0,
            number: 74,
            value: 90,
        };
        let clips = [clip((0, 2), 0.0, vec![filter])];
        let played = play(&clips, 512, 4 * SAMPLES_PER_BEAT);
        assert_eq!(played, vec![(SAMPLES_PER_BEAT / 2, [0xB0, 74, 90])]);
    }

    #[test]
    fn adjacent_clips_hand_the_pedal_over_without_a_gap() {
        let clips = [
            clip((0, 2), 0.0, vec![sustain(0.0, true)]),
            clip((2, 4), 1.0, vec![sustain(0.0, true)]),
        ];
        let played = play(&clips, 4096, 5 * SAMPLES_PER_BEAT);
        let at_handover: Vec<[u8; 3]> = played
            .iter()
            .filter(|(sample, _)| *sample == 2 * SAMPLES_PER_BEAT)
            .map(|(_, message)| *message)
            .collect();
        assert_eq!(at_handover, vec![SUSTAIN_OFF, SUSTAIN_ON]);
    }

    #[test]
    fn events_do_not_depend_on_the_block_size() {
        let clips = [clip(
            (1, 6),
            0.0,
            vec![
                bend(0.5, 0x3000),
                sustain(2.0, true),
                bend(2.5, 0x2000),
                sustain(4.0, false),
            ],
        )];
        let length = 7 * SAMPLES_PER_BEAT;
        let reference = play(&clips, 4096, length);
        assert!(!reference.is_empty());
        for block_size in [512, 441, 64, 37] {
            assert_eq!(
                play(&clips, block_size, length),
                reference,
                "block size {block_size}"
            );
        }
    }

    #[test]
    fn polyphonic_pressure_is_played_but_never_held() {
        let pressure = MidiControlSnapshot {
            start_beats: 1.0,
            status: 0xA0,
            number: 60,
            value: 80,
        };
        let clips = [clip((0, 4), 0.0, vec![pressure])];
        assert_eq!(
            play(&clips, 512, 2 * SAMPLES_PER_BEAT),
            vec![(SAMPLES_PER_BEAT, [0xA0, 60, 80])]
        );
        let mut state = ControlState::new();
        assert!(sync_at(&clips, &mut state, 3 * SAMPLES_PER_BEAT).is_empty());
    }

    #[test]
    fn channel_pressure_uses_two_bytes() {
        let pressure = MidiControlSnapshot {
            start_beats: 0.0,
            status: 0xD0,
            number: 0,
            value: 70,
        };
        let clips = [clip((0, 4), 0.0, vec![pressure])];
        assert_eq!(
            play(&clips, 512, SAMPLES_PER_BEAT),
            vec![(0, [0xD0, 70, 0])]
        );
    }

    #[test]
    fn release_recenters_what_the_timeline_moved() {
        let clips = [clip(
            (0, 8),
            0.0,
            vec![bend(0.0, 0x3000), sustain(0.0, true)],
        )];
        let mut state = ControlState::new();
        let mut events = MidiEventList::new();
        state.sync(&clips, &mut events, &transport(0, 512));

        let mut released = MidiEventList::new();
        state.release(&mut released);
        let sent = messages(&released);
        assert!(sent.contains(&(0, SUSTAIN_OFF)));
        assert!(sent.contains(&(0, BEND_CENTER)));

        let mut again = MidiEventList::new();
        state.release(&mut again);
        assert!(again.as_slice().is_empty());
    }

    #[test]
    fn controls_never_take_the_places_kept_for_notes() {
        let mut events = MidiEventList::new();
        for _ in 0..1000 {
            events.push_control(slot_event(0, 1, 5));
        }
        events.push(MidiEvent {
            offset: 0,
            kind: MidiEventKind::NoteOff { pitch: 60 },
        });
        assert!(events
            .as_slice()
            .iter()
            .any(|event| matches!(event.kind, MidiEventKind::NoteOff { pitch: 60 })));
    }

    #[test]
    fn bend_value_splits_into_seven_bit_halves() {
        assert_eq!(control_message(0xE0, 0, 0x2000), BEND_CENTER);
        assert_eq!(control_message(0xE0, 0, 0x3000), BEND_UP);
    }
}
