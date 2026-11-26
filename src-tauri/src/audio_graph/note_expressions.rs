//! Replay of the curves drawn on a note (ADR-039).
//!
//! A curve is held between two points, like a channel control (ADR-029): after
//! a seek, a loop wrap or an edit, a note that is already sounding gets the
//! value its curve holds at the new position. Nothing here allocates: it reads
//! the baked points and pushes events into the block's pre-allocated list.

use crate::audio_graph::midi::{
    beats_to_sample, MidiEvent, MidiEventKind, MidiEventList, MidiNoteSnapshot,
    NoteExpressionSnapshot,
};
use crate::project::NoteExpressionKind;

/// Where a note plays, in engine samples, once clipped to its clip's window.
pub struct NoteSpan {
    pub start: u64,
    pub end: u64,
}

/// The block being rendered, in engine samples.
pub struct BlockSpan {
    pub start: u64,
    pub end: u64,
    pub bpm: f32,
    pub sample_rate: f32,
    /// A seek, a loop wrap or a content swap: sounding notes need their
    /// current values, not only the points that fall in the block.
    pub resync: bool,
}

/// Pushes the expression events of `note` for one block.
///
/// A note that was already sounding when a `resync` block begins first gets the
/// value each curve holds at the block start (offset 0); its points then play
/// from the next sample, so the event stream does not depend on the block size.
pub fn push_block_expressions(
    note: &MidiNoteSnapshot,
    span: &NoteSpan,
    block: &BlockSpan,
    list: &mut MidiEventList,
) {
    if note.expressions.is_empty() || span.start >= block.end || span.end <= block.start {
        return;
    }
    let sounding_at_start = span.start < block.start && block.start < span.end;
    let synced = block.resync && sounding_at_start;
    if synced {
        push_held_values(note, block, list);
    }
    let first = block.start + synced as u64;
    for point in &note.expressions {
        let at = point_sample(note, point, block);
        if at >= block.end || at >= span.end {
            break;
        }
        if at >= first && at >= span.start {
            list.push_control(expression_event(
                note.pitch,
                point,
                (at - block.start) as u32,
            ));
        }
    }
}

fn push_held_values(note: &MidiNoteSnapshot, block: &BlockSpan, list: &mut MidiEventList) {
    for kind in NoteExpressionKind::ALL {
        let held = note
            .expressions
            .iter()
            .take_while(|point| point_sample(note, point, block) <= block.start)
            .filter(|point| point.kind == kind)
            .last();
        if let Some(point) = held {
            list.push_control(expression_event(note.pitch, point, 0));
        }
    }
}

fn point_sample(note: &MidiNoteSnapshot, point: &NoteExpressionSnapshot, block: &BlockSpan) -> u64 {
    beats_to_sample(
        note.clip_start_sec,
        point.start_beats,
        block.bpm,
        block.sample_rate,
    )
}

fn expression_event(pitch: u8, point: &NoteExpressionSnapshot, offset: u32) -> MidiEvent {
    MidiEvent {
        offset,
        kind: MidiEventKind::NoteExpression {
            pitch,
            kind: point.kind,
            value: point.value,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: f32 = 48_000.0;
    const BPM: f32 = 120.0;
    /// One beat, in samples.
    const BEAT: u64 = 24_000;

    fn note_with(points: &[(f32, NoteExpressionKind, f32)]) -> MidiNoteSnapshot {
        MidiNoteSnapshot {
            pitch: 60,
            velocity: 100,
            clip_start_sec: 0.0,
            start_beats: 0.0,
            end_beats: 4.0,
            window_start_sample: 0,
            window_end_sample: u64::MAX,
            expressions: points
                .iter()
                .map(|&(start_beats, kind, value)| NoteExpressionSnapshot {
                    start_beats,
                    kind,
                    value,
                })
                .collect(),
        }
    }

    fn span(note: &MidiNoteSnapshot) -> NoteSpan {
        NoteSpan {
            start: note
                .start_sample(BPM, SAMPLE_RATE)
                .max(note.window_start_sample),
            end: note
                .end_sample(BPM, SAMPLE_RATE)
                .min(note.window_end_sample),
        }
    }

    fn events_of(
        note: &MidiNoteSnapshot,
        span: &NoteSpan,
        block: BlockSpan,
    ) -> Vec<(u32, NoteExpressionKind, f32)> {
        let mut list = MidiEventList::new();
        push_block_expressions(note, span, &block, &mut list);
        list.as_slice()
            .iter()
            .map(|event| match event.kind {
                MidiEventKind::NoteExpression { kind, value, .. } => (event.offset, kind, value),
                other => panic!("unexpected event {other:?}"),
            })
            .collect()
    }

    fn block(start: u64, size: u64, resync: bool) -> BlockSpan {
        BlockSpan {
            start,
            end: start + size,
            bpm: BPM,
            sample_rate: SAMPLE_RATE,
            resync,
        }
    }

    /// Every event of the note, played block after block from its start.
    fn played_in_blocks(note: &MidiNoteSnapshot, size: u64) -> Vec<(u64, NoteExpressionKind, f32)> {
        let span = span(note);
        let mut all = Vec::new();
        let mut start = 0;
        while start < span.end + size {
            for (offset, kind, value) in events_of(note, &span, block(start, size, false)) {
                all.push((start + offset as u64, kind, value));
            }
            start += size;
        }
        all
    }

    #[test]
    fn a_point_plays_at_its_sample_inside_the_block() {
        let note = note_with(&[(1.0, NoteExpressionKind::Pressure, 0.5)]);
        let events = events_of(&note, &span(&note), block(BEAT - 100, 400, false));
        assert_eq!(events, [(100, NoteExpressionKind::Pressure, 0.5)]);
    }

    #[test]
    fn the_stream_is_the_same_at_every_block_size() {
        let note = note_with(&[
            (0.0, NoteExpressionKind::Pressure, 0.2),
            (0.5, NoteExpressionKind::Timbre, 0.9),
            (1.25, NoteExpressionKind::Pressure, 0.8),
            (3.9, NoteExpressionKind::Pitch, -2.0),
        ]);
        let reference = played_in_blocks(&note, 4096);
        assert_eq!(reference.len(), 4);
        for size in [512, 441, 64, 37] {
            assert_eq!(
                played_in_blocks(&note, size),
                reference,
                "block size {size}"
            );
        }
    }

    #[test]
    fn a_seek_into_the_note_hands_over_the_held_value_of_each_curve() {
        let note = note_with(&[
            (0.0, NoteExpressionKind::Pressure, 0.2),
            (0.5, NoteExpressionKind::Timbre, 0.9),
            (1.0, NoteExpressionKind::Pressure, 0.6),
            (3.0, NoteExpressionKind::Pressure, 1.0),
        ]);
        let events = events_of(&note, &span(&note), block(2 * BEAT, 512, true));
        assert_eq!(
            events,
            [
                (0, NoteExpressionKind::Pressure, 0.6),
                (0, NoteExpressionKind::Timbre, 0.9),
            ]
        );
    }

    #[test]
    fn a_point_on_the_resync_block_start_is_sent_once() {
        let note = note_with(&[
            (0.0, NoteExpressionKind::Pressure, 0.2),
            (1.0, NoteExpressionKind::Pressure, 0.6),
        ]);
        let events = events_of(&note, &span(&note), block(BEAT, 512, true));
        assert_eq!(events, [(0, NoteExpressionKind::Pressure, 0.6)]);
    }

    #[test]
    fn a_note_starting_in_a_resync_block_is_not_synced() {
        let note = note_with(&[(0.0, NoteExpressionKind::Pressure, 0.4)]);
        let events = events_of(&note, &span(&note), block(0, 512, true));
        assert_eq!(events, [(0, NoteExpressionKind::Pressure, 0.4)]);
    }

    #[test]
    fn points_outside_the_clip_window_do_not_play() {
        let mut note = note_with(&[
            (0.5, NoteExpressionKind::Pressure, 0.3),
            (2.0, NoteExpressionKind::Pressure, 0.7),
        ]);
        note.window_start_sample = BEAT;
        note.window_end_sample = 3 * BEAT / 2;
        let played = played_in_blocks(&note, 512);
        assert!(played.is_empty(), "{played:?}");
    }

    #[test]
    fn a_note_without_curves_pushes_nothing() {
        let note = note_with(&[]);
        assert!(events_of(&note, &span(&note), block(0, 512, true)).is_empty());
    }
}
