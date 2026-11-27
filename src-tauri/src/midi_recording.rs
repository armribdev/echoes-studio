//! Recording of hardware MIDI notes and controls into a clip (ADR-022
//! amendment, ADR-029).
//!
//! The MIDI capture thread hands every message of the armed track to
//! `MidiRecorder::capture`, which stamps it with the transport sample that was
//! playing when the key was pressed. When the recording ends — record button,
//! pause or stop — the notes are paired into a clip, with the pitch bend,
//! control changes and aftertouch alongside, that the frontend adds through
//! the regular `ADD_MIDI_CLIP` command (one undo step).
//!
//! Nothing here runs on the audio thread: the only audio-side cost is the
//! transport position published next to the live-input window
//! (`LiveInputClock::set_event_window`).

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::audio_engine::transport::SharedTransport;
use crate::audio_graph::midi::{LiveInputClock, MidiEventKind, MidiLiveInputMessage};
use crate::audio_graph::midi_controls::control_neutral_value;
use crate::project::{
    generate_id, sanitize_control_points, sanitize_note_expressions, MidiControlKind,
    MidiControlPoint, MidiNote, NoteExpressionKind, NoteExpressionPoint,
};

/// Emitted when a recording ends. Payload: `Option<RecordedMidiClip>`, `null`
/// when no note was played.
pub const MIDI_RECORDING_FINISHED_EVENT: &str = "midi-recording-finished";

/// Emitted for every note edge while recording, so the arrange view can draw
/// the clip as it grows. Payload: `RecordedNote`.
pub const MIDI_RECORDING_NOTE_EVENT: &str = "midi-recording-note";

const MIN_NOTE_BEATS: f64 = 0.0625;

/// A note's expression curves keep one value per kind in this many beats, like
/// the control lanes of the editor (ADR-039 §5).
const EXPRESSION_BUCKET_BEATS: f64 = 1.0 / 32.0;

/// A fast bend or mod-wheel sweep sends hundreds of values per second; one per
/// bucket keeps the gesture and the clip small.
const CONTROL_BUCKET_SECONDS: f64 = 0.005;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedMidiClip {
    pub track_id: String,
    /// Seconds, same coordinate space as `AppMidiClip`.
    pub start: f64,
    pub end: f64,
    pub notes: Vec<MidiNote>,
    pub channel_controls: Vec<MidiControlPoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedNote {
    pub note_on: bool,
    pub pitch: u8,
    pub velocity: u8,
    /// Seconds on the timeline, latency-compensated like the final clip.
    pub position: f64,
}

struct RecordedEvent {
    note_on: bool,
    pitch: u8,
    velocity: u8,
    position_samples: u64,
}

#[derive(Clone, Copy)]
struct RecordedControl {
    kind: MidiControlKind,
    number: u8,
    value: u16,
    position_samples: u64,
}

impl RecordedControl {
    fn key(&self) -> (MidiControlKind, u8) {
        (self.kind, self.number)
    }
}

#[derive(Clone, Copy)]
struct RecordedExpression {
    pitch: u8,
    kind: NoteExpressionKind,
    value: f32,
    position_samples: u64,
}

struct Recording {
    track_id: String,
    start_position_samples: u64,
    events: Vec<RecordedEvent>,
    controls: Vec<RecordedControl>,
    expressions: Vec<RecordedExpression>,
    /// Last value of each held control seen during the count-in, so a pedal
    /// pressed before the first beat is down at the start of the clip.
    count_in_controls: HashMap<(MidiControlKind, u8), u16>,
}

pub struct MidiRecorder {
    recording: Mutex<Option<Recording>>,
    transport: Arc<SharedTransport>,
    clock: Arc<LiveInputClock>,
}

impl MidiRecorder {
    pub fn new(transport: Arc<SharedTransport>, clock: Arc<LiveInputClock>) -> Self {
        Self {
            recording: Mutex::new(None),
            transport,
            clock,
        }
    }

    /// Returns the timeline position, in seconds, where the recording starts.
    pub fn start(&self, track_id: String) -> Result<f64, String> {
        let mut recording = self.lock();
        if recording.is_some() {
            return Err("Un enregistrement MIDI est déjà en cours".into());
        }
        let start_position_samples = self.transport.position_samples.load(Ordering::Relaxed);
        self.transport.set_recording(true);
        *recording = Some(Recording {
            track_id,
            start_position_samples,
            events: Vec::new(),
            controls: Vec::new(),
            expressions: Vec::new(),
            count_in_controls: HashMap::new(),
        });
        Ok(start_position_samples as f64 / self.transport.sample_rate() as f64)
    }

    /// MIDI capture thread. Ignores messages that are not for the recorded
    /// track; notes played during the count-in are dropped, and controls played
    /// then only set the value the clip starts with. Returns the recorded note
    /// so the caller can show it right away.
    pub fn capture(&self, track_id: &str, message: &MidiLiveInputMessage) -> Option<RecordedNote> {
        let counting_in = self.transport.count_in_remaining() > 0;
        let mut recording = self.lock();
        let recording = recording.as_mut().filter(|r| r.track_id == track_id)?;

        let (note_on, pitch, velocity) = match message.kind {
            MidiEventKind::NoteOn { pitch, velocity } => (true, pitch, velocity),
            MidiEventKind::NoteOff { pitch } => (false, pitch, 0),
            MidiEventKind::NoteExpression { pitch, kind, value } => {
                if !counting_in {
                    recording.expressions.push(RecordedExpression {
                        pitch,
                        kind,
                        value,
                        position_samples: self.position_of(message)?,
                    });
                }
                return None;
            }
            MidiEventKind::Control { message: raw } => {
                let (kind, number, value) = decode_control(raw)?;
                if counting_in {
                    if kind != MidiControlKind::PolyPressure {
                        recording.count_in_controls.insert((kind, number), value);
                    }
                    return None;
                }
                let position_samples = self.position_of(message)?;
                recording.controls.push(RecordedControl {
                    kind,
                    number,
                    value,
                    position_samples,
                });
                return None;
            }
        };
        if counting_in {
            return None;
        }
        let position_samples = self.position_of(message)?;
        recording.events.push(RecordedEvent {
            note_on,
            pitch,
            velocity,
            position_samples,
        });
        Some(RecordedNote {
            note_on,
            pitch,
            velocity,
            position: position_samples as f64 / self.transport.sample_rate() as f64,
        })
    }

    fn position_of(&self, message: &MidiLiveInputMessage) -> Option<u64> {
        self.clock
            .transport_position_at(message.received_at, self.transport.sample_rate())
    }

    /// Ends the recording, if any. `Some(None)` when it held no note.
    pub fn finish(&self) -> Option<Option<RecordedMidiClip>> {
        let recording = self.lock().take()?;
        self.transport.set_recording(false);
        self.transport.cancel_count_in();
        let end_position_samples = self.transport.position_samples.load(Ordering::Relaxed);
        Some(build_clip(
            recording,
            end_position_samples,
            self.transport.sample_rate() as f64,
            self.transport.bpm() as f64,
        ))
    }

    pub fn is_recording(&self) -> bool {
        self.lock().is_some()
    }

    fn lock(&self) -> MutexGuard<'_, Option<Recording>> {
        self.recording
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Ends the recording and tells the frontend. Must run before a stop resets the
/// transport position, which is where unreleased notes get closed.
pub fn finish_and_emit(app: &AppHandle, recorder: &MidiRecorder) {
    if let Some(clip) = recorder.finish() {
        let _ = app.emit(MIDI_RECORDING_FINISHED_EVENT, clip);
    }
}

struct NoteSpan {
    pitch: u8,
    velocity: u8,
    on_samples: u64,
    off_samples: u64,
}

fn build_clip(
    recording: Recording,
    end_position_samples: u64,
    sample_rate: f64,
    bpm: f64,
) -> Option<RecordedMidiClip> {
    let min_note_samples = (MIN_NOTE_BEATS * 60.0 / bpm * sample_rate) as u64;
    let close = |pitch: u8, velocity: u8, on_samples: u64, off_samples: u64| NoteSpan {
        pitch,
        velocity,
        on_samples,
        off_samples: off_samples.max(on_samples + min_note_samples),
    };

    let mut open: HashMap<u8, (u8, u64)> = HashMap::new();
    let mut spans: Vec<NoteSpan> = Vec::new();
    for event in &recording.events {
        let previous = if event.note_on {
            open.insert(event.pitch, (event.velocity, event.position_samples))
        } else {
            open.remove(&event.pitch)
        };
        if let Some((velocity, on_samples)) = previous {
            spans.push(close(
                event.pitch,
                velocity,
                on_samples,
                event.position_samples,
            ));
        }
    }
    for (pitch, (velocity, on_samples)) in open {
        spans.push(close(pitch, velocity, on_samples, end_position_samples));
    }
    if spans.is_empty() {
        return None;
    }
    spans.sort_by_key(|span| (span.on_samples, span.pitch));

    let first_note = spans.iter().map(|span| span.on_samples).min()?;
    let last_note_end = spans.iter().map(|span| span.off_samples).max()?;
    let clip_start = recording.start_position_samples.min(first_note);
    let clip_end = end_position_samples.max(last_note_end);

    let beats_per_sample = bpm / 60.0 / sample_rate;
    let channel_controls = build_controls(
        &recording,
        clip_start,
        beats_per_sample,
        (CONTROL_BUCKET_SECONDS * sample_rate) as u64,
    );
    let expression_bucket_samples = (EXPRESSION_BUCKET_BEATS / beats_per_sample) as u64;
    let notes = spans
        .iter()
        .map(|span| {
            let duration = ((span.off_samples - span.on_samples) as f64 * beats_per_sample) as f32;
            let curves = note_expressions(
                span,
                &recording.expressions,
                beats_per_sample,
                expression_bucket_samples,
            );
            MidiNote {
                id: generate_id(),
                pitch: span.pitch,
                start: ((span.on_samples - clip_start) as f64 * beats_per_sample) as f32,
                duration,
                velocity: span.velocity,
                expressions: sanitize_note_expressions(&curves, duration),
            }
        })
        .collect();

    Some(RecordedMidiClip {
        track_id: recording.track_id,
        start: clip_start as f64 / sample_rate,
        end: clip_end as f64 / sample_rate,
        notes,
        channel_controls,
    })
}

/// The curves played on `span`'s key while it sounded: the last value of each
/// kind per bucket, without the values that repeat the previous one. Positions
/// are in beats from the start of the note.
fn note_expressions(
    span: &NoteSpan,
    expressions: &[RecordedExpression],
    beats_per_sample: f64,
    bucket_samples: u64,
) -> Vec<NoteExpressionPoint> {
    let bucket_samples = bucket_samples.max(1);
    let during_note = |expression: &&RecordedExpression| {
        expression.pitch == span.pitch
            && (span.on_samples..span.off_samples).contains(&expression.position_samples)
    };
    let mut points = Vec::new();
    for kind in NoteExpressionKind::ALL {
        let mut kept: Vec<(u64, NoteExpressionPoint)> = Vec::new();
        for expression in expressions
            .iter()
            .filter(during_note)
            .filter(|expression| expression.kind == kind)
        {
            let elapsed = expression.position_samples - span.on_samples;
            let point = NoteExpressionPoint {
                start: (elapsed as f64 * beats_per_sample) as f32,
                kind,
                value: expression.value,
            };
            match kept.last_mut() {
                Some((bucket, last)) if *bucket == elapsed / bucket_samples => *last = point,
                _ => kept.push((elapsed / bucket_samples, point)),
            }
        }
        let mut previous_value = None;
        for (_, point) in kept {
            if previous_value.replace(point.value) != Some(point.value) {
                points.push(point);
            }
        }
    }
    points
}

fn decode_control(raw: [u8; 3]) -> Option<(MidiControlKind, u8, u16)> {
    match raw[0] {
        0xA0 => Some((MidiControlKind::PolyPressure, raw[1], raw[2] as u16)),
        0xB0 => Some((MidiControlKind::ControlChange, raw[1], raw[2] as u16)),
        0xD0 => Some((MidiControlKind::ChannelPressure, 0, raw[1] as u16)),
        0xE0 => Some((
            MidiControlKind::PitchBend,
            0,
            raw[1] as u16 | (raw[2] as u16) << 7,
        )),
        _ => None,
    }
}

/// Keeps the last value of each control per bucket, then drops values that
/// repeat the previous one.
fn thin_controls(controls: &[RecordedControl], bucket_samples: u64) -> Vec<RecordedControl> {
    let bucket_samples = bucket_samples.max(1);
    let mut kept: Vec<RecordedControl> = Vec::new();
    let mut last_kept: HashMap<(MidiControlKind, u8), usize> = HashMap::new();
    for control in controls {
        let same_bucket = last_kept.get(&control.key()).copied().filter(|&index| {
            kept[index].position_samples / bucket_samples
                == control.position_samples / bucket_samples
        });
        match same_bucket {
            Some(index) => kept[index] = *control,
            None => {
                last_kept.insert(control.key(), kept.len());
                kept.push(*control);
            }
        }
    }

    let mut last_value: HashMap<(MidiControlKind, u8), u16> = HashMap::new();
    kept.retain(|control| last_value.insert(control.key(), control.value) != Some(control.value));
    kept
}

fn build_controls(
    recording: &Recording,
    clip_start: u64,
    beats_per_sample: f64,
    bucket_samples: u64,
) -> Vec<MidiControlPoint> {
    let held_at_start = recording
        .count_in_controls
        .iter()
        .filter(|((kind, number), value)| {
            control_neutral_value(kind.status(), *number) != Some(**value)
        })
        .map(|((kind, number), value)| MidiControlPoint {
            start: 0.0,
            kind: *kind,
            number: *number,
            value: *value,
        });
    let played = thin_controls(&recording.controls, bucket_samples)
        .into_iter()
        .map(|control| MidiControlPoint {
            start: (control.position_samples.saturating_sub(clip_start) as f64 * beats_per_sample)
                as f32,
            kind: control.kind,
            number: control.number,
            value: control.value,
        });
    sanitize_control_points(held_at_start.chain(played).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: f64 = 48_000.0;
    const BPM: f64 = 120.0;
    const SAMPLES_PER_BEAT: u64 = 24_000;

    fn event(note_on: bool, pitch: u8, position_samples: u64) -> RecordedEvent {
        RecordedEvent {
            note_on,
            pitch,
            velocity: if note_on { 100 } else { 0 },
            position_samples,
        }
    }

    fn recording(start_position_samples: u64, events: Vec<RecordedEvent>) -> Recording {
        Recording {
            track_id: "track".into(),
            start_position_samples,
            events,
            controls: vec![],
            expressions: vec![],
            count_in_controls: HashMap::new(),
        }
    }

    fn clip(start: u64, events: Vec<RecordedEvent>, end: u64) -> Option<RecordedMidiClip> {
        build_clip(recording(start, events), end, SAMPLE_RATE, BPM)
    }

    #[test]
    fn recording_keeps_the_transport_from_auto_stopping() {
        let transport = SharedTransport::new(48_000.0, 120.0);
        let recorder = MidiRecorder::new(Arc::clone(&transport), Arc::new(LiveInputClock::new()));
        recorder.start("track".into()).unwrap();
        assert!(transport.is_recording());
        assert!(recorder.finish().is_some());
        assert!(!transport.is_recording());
    }

    #[test]
    fn notes_played_during_the_count_in_are_not_recorded() {
        let transport = SharedTransport::new(48_000.0, 120.0);
        let clock = Arc::new(LiveInputClock::new());
        clock.set_event_window(10_000_000, 0);
        let recorder = MidiRecorder::new(Arc::clone(&transport), clock);
        recorder.start("track".into()).unwrap();
        let note = MidiLiveInputMessage {
            kind: MidiEventKind::NoteOn {
                pitch: 60,
                velocity: 100,
            },
            received_at: 20_000_000,
        };

        transport.start_count_in(1_000);
        assert!(recorder.capture("track", &note).is_none());

        transport.cancel_count_in();
        assert!(recorder.capture("track", &note).is_some());
    }

    fn recorder_with_clock() -> (MidiRecorder, Arc<SharedTransport>) {
        let transport = SharedTransport::new(48_000.0, 120.0);
        let clock = Arc::new(LiveInputClock::new());
        clock.set_event_window(10_000_000, 0);
        let recorder = MidiRecorder::new(Arc::clone(&transport), clock);
        recorder.start("track".into()).unwrap();
        (recorder, transport)
    }

    fn control_message(raw: [u8; 3], received_at: u64) -> MidiLiveInputMessage {
        MidiLiveInputMessage {
            kind: MidiEventKind::Control { message: raw },
            received_at,
        }
    }

    fn played_note(received_at: u64) -> MidiLiveInputMessage {
        MidiLiveInputMessage {
            kind: MidiEventKind::NoteOn {
                pitch: 60,
                velocity: 100,
            },
            received_at,
        }
    }

    #[test]
    fn controls_alone_do_not_make_a_clip() {
        let (recorder, _) = recorder_with_clock();
        recorder.capture("track", &control_message([0xE0, 0, 0x60], 20_000_000));

        assert_eq!(recorder.finish().map(|clip| clip.is_none()), Some(true));
    }

    #[test]
    fn controls_are_recorded_next_to_the_notes() {
        let (recorder, _) = recorder_with_clock();
        recorder.capture("track", &played_note(20_000_000));
        recorder.capture("track", &control_message([0xB0, 64, 127], 520_000_000));
        recorder.capture("track", &control_message([0xE0, 0x00, 0x60], 1_020_000_000));

        let clip = recorder.finish().unwrap().unwrap();
        assert_eq!(clip.channel_controls.len(), 2);
        assert_eq!(
            clip.channel_controls[0].kind,
            MidiControlKind::ControlChange
        );
        assert_eq!(clip.channel_controls[0].number, 64);
        assert_eq!(clip.channel_controls[0].value, 127);
        assert!((clip.channel_controls[0].start - 1.02).abs() < 1e-4);
        assert_eq!(clip.channel_controls[1].kind, MidiControlKind::PitchBend);
        assert_eq!(clip.channel_controls[1].value, 0x3000);
        assert!((clip.channel_controls[1].start - 2.02).abs() < 1e-4);
    }

    fn expression_message(
        pitch: u8,
        kind: NoteExpressionKind,
        value: f32,
        received_at: u64,
    ) -> MidiLiveInputMessage {
        MidiLiveInputMessage {
            kind: MidiEventKind::NoteExpression { pitch, kind, value },
            received_at,
        }
    }

    fn released_note(received_at: u64) -> MidiLiveInputMessage {
        MidiLiveInputMessage {
            kind: MidiEventKind::NoteOff { pitch: 60 },
            received_at,
        }
    }

    #[test]
    fn a_note_keeps_the_curves_played_while_it_sounded() {
        let (recorder, _) = recorder_with_clock();
        let pressure = |value, at| expression_message(60, NoteExpressionKind::Pressure, value, at);
        recorder.capture("track", &played_note(20_000_000));
        recorder.capture("track", &pressure(0.5, 20_000_000));
        recorder.capture("track", &pressure(0.6, 520_000_000));
        recorder.capture("track", &pressure(0.9, 525_000_000));
        recorder.capture(
            "track",
            &expression_message(60, NoteExpressionKind::Pitch, -2.0, 1_020_000_000),
        );
        recorder.capture(
            "track",
            &expression_message(64, NoteExpressionKind::Pressure, 1.0, 1_020_000_000),
        );
        recorder.capture("track", &released_note(2_020_000_000));
        recorder.capture("track", &pressure(0.1, 2_520_000_000));

        let clip = recorder.finish().unwrap().unwrap();
        let curves: Vec<_> = clip.notes[0]
            .expressions
            .iter()
            .map(|point| (point.kind, point.start, point.value))
            .collect();
        assert_eq!(curves.len(), 3);
        assert_eq!(curves[0], (NoteExpressionKind::Pressure, 0.0, 0.5));
        assert_eq!(curves[1].0, NoteExpressionKind::Pressure);
        assert!(
            (curves[1].1 - 1.01).abs() < 1e-4,
            "the last point of the bucket"
        );
        assert_eq!(curves[1].2, 0.9, "the last value of a bucket wins");
        assert_eq!(curves[2].0, NoteExpressionKind::Pitch);
        assert!((curves[2].1 - 2.0).abs() < 1e-4);
        assert_eq!(curves[2].2, -2.0);
    }

    #[test]
    fn a_value_that_repeats_the_previous_one_is_dropped() {
        let (recorder, _) = recorder_with_clock();
        let timbre = |at| expression_message(60, NoteExpressionKind::Timbre, 0.4, at);
        recorder.capture("track", &played_note(20_000_000));
        recorder.capture("track", &timbre(20_000_000));
        recorder.capture("track", &timbre(520_000_000));
        recorder.capture("track", &released_note(1_020_000_000));

        let clip = recorder.finish().unwrap().unwrap();
        assert_eq!(clip.notes[0].expressions.len(), 1);
    }

    #[test]
    fn expressions_played_during_the_count_in_are_not_recorded() {
        let (recorder, transport) = recorder_with_clock();
        transport.start_count_in(1_000);
        recorder.capture(
            "track",
            &expression_message(60, NoteExpressionKind::Timbre, 0.4, 20_000_000),
        );
        transport.cancel_count_in();
        recorder.capture("track", &played_note(30_000_000));

        let clip = recorder.finish().unwrap().unwrap();
        assert!(clip.notes[0].expressions.is_empty());
    }

    #[test]
    fn controls_for_another_track_are_ignored() {
        let (recorder, _) = recorder_with_clock();
        recorder.capture("track", &played_note(20_000_000));
        recorder.capture("other", &control_message([0xB0, 1, 50], 30_000_000));

        assert!(recorder
            .finish()
            .unwrap()
            .unwrap()
            .channel_controls
            .is_empty());
    }

    #[test]
    fn pedal_pressed_during_the_count_in_is_down_at_the_clip_start() {
        let (recorder, transport) = recorder_with_clock();
        transport.start_count_in(1_000);
        recorder.capture("track", &control_message([0xB0, 64, 127], 15_000_000));
        recorder.capture("track", &control_message([0xE0, 0, 0x40], 16_000_000));
        transport.cancel_count_in();
        recorder.capture("track", &played_note(20_000_000));

        let clip = recorder.finish().unwrap().unwrap();
        assert_eq!(clip.channel_controls.len(), 1);
        assert_eq!(clip.channel_controls[0].number, 64);
        assert_eq!(clip.channel_controls[0].value, 127);
        assert_eq!(clip.channel_controls[0].start, 0.0);
    }

    #[test]
    fn channel_pressure_and_poly_pressure_are_decoded() {
        assert_eq!(
            decode_control([0xD0, 70, 0]),
            Some((MidiControlKind::ChannelPressure, 0, 70))
        );
        assert_eq!(
            decode_control([0xA0, 60, 40]),
            Some((MidiControlKind::PolyPressure, 60, 40))
        );
        assert_eq!(
            decode_control([0xE0, 0x7F, 0x7F]),
            Some((MidiControlKind::PitchBend, 0, 0x3FFF))
        );
    }

    fn bend_at(position_samples: u64, value: u16) -> RecordedControl {
        RecordedControl {
            kind: MidiControlKind::PitchBend,
            number: 0,
            value,
            position_samples,
        }
    }

    #[test]
    fn a_sweep_keeps_the_last_value_of_each_bucket() {
        let sweep = vec![
            bend_at(0, 8200),
            bend_at(48, 8300),
            bend_at(96, 8400),
            bend_at(300, 8500),
        ];
        let thinned = thin_controls(&sweep, 240);
        let values: Vec<u16> = thinned.iter().map(|control| control.value).collect();
        assert_eq!(values, vec![8400, 8500]);
    }

    #[test]
    fn repeated_values_are_dropped() {
        let held = vec![bend_at(0, 9000), bend_at(1_000, 9000), bend_at(2_000, 9500)];
        assert_eq!(thin_controls(&held, 240).len(), 2);
    }

    #[test]
    fn thinning_is_per_control() {
        let mut mod_wheel = bend_at(10, 40);
        mod_wheel.kind = MidiControlKind::ControlChange;
        mod_wheel.number = 1;
        let thinned = thin_controls(&[bend_at(0, 9000), mod_wheel], 240);
        assert_eq!(thinned.len(), 2);
    }

    #[test]
    fn finishing_cancels_a_pending_count_in() {
        let transport = SharedTransport::new(48_000.0, 120.0);
        let recorder = MidiRecorder::new(Arc::clone(&transport), Arc::new(LiveInputClock::new()));
        recorder.start("track".into()).unwrap();
        transport.start_count_in(1_000);
        recorder.finish();
        assert_eq!(transport.count_in_remaining(), 0);
    }

    #[test]
    fn no_note_gives_no_clip() {
        assert!(clip(0, vec![], SAMPLES_PER_BEAT * 4).is_none());
    }

    #[test]
    fn note_is_placed_in_beats_from_the_clip_start() {
        let start = SAMPLES_PER_BEAT * 4;
        let events = vec![
            event(true, 60, start + SAMPLES_PER_BEAT),
            event(false, 60, start + SAMPLES_PER_BEAT * 2),
        ];
        let clip = clip(start, events, start + SAMPLES_PER_BEAT * 4).unwrap();
        assert_eq!(clip.notes.len(), 1);
        assert_eq!(clip.notes[0].start, 1.0);
        assert_eq!(clip.notes[0].duration, 1.0);
        assert_eq!(clip.notes[0].velocity, 100);
        assert_eq!(clip.start, 2.0);
        assert_eq!(clip.end, 4.0);
    }

    #[test]
    fn held_note_is_closed_where_the_recording_ends() {
        let events = vec![event(true, 64, SAMPLES_PER_BEAT)];
        let clip = clip(0, events, SAMPLES_PER_BEAT * 3).unwrap();
        assert_eq!(clip.notes[0].duration, 2.0);
    }

    #[test]
    fn retriggered_pitch_closes_the_previous_note() {
        let events = vec![
            event(true, 60, 0),
            event(true, 60, SAMPLES_PER_BEAT),
            event(false, 60, SAMPLES_PER_BEAT * 2),
        ];
        let clip = clip(0, events, SAMPLES_PER_BEAT * 2).unwrap();
        assert_eq!(clip.notes.len(), 2);
        assert_eq!(clip.notes[0].duration, 1.0);
        assert_eq!(clip.notes[1].start, 1.0);
    }

    #[test]
    fn notes_are_ordered_by_start_then_pitch() {
        let events = vec![
            event(true, 67, 0),
            event(true, 60, 0),
            event(false, 67, SAMPLES_PER_BEAT),
            event(false, 60, SAMPLES_PER_BEAT),
        ];
        let clip = clip(0, events, SAMPLES_PER_BEAT).unwrap();
        let pitches: Vec<u8> = clip.notes.iter().map(|note| note.pitch).collect();
        assert_eq!(pitches, vec![60, 67]);
    }

    #[test]
    fn note_off_without_note_on_is_ignored() {
        assert!(clip(0, vec![event(false, 60, 100)], SAMPLES_PER_BEAT).is_none());
    }

    #[test]
    fn very_short_note_keeps_a_minimum_length() {
        let events = vec![event(true, 60, 0), event(false, 60, 0)];
        let clip = clip(0, events, SAMPLES_PER_BEAT).unwrap();
        assert_eq!(clip.notes[0].duration, MIN_NOTE_BEATS as f32);
    }

    #[test]
    fn note_played_before_the_start_position_extends_the_clip_backwards() {
        let events = vec![
            event(true, 60, SAMPLES_PER_BEAT - 100),
            event(false, 60, SAMPLES_PER_BEAT * 2),
        ];
        let clip = clip(SAMPLES_PER_BEAT, events, SAMPLES_PER_BEAT * 3).unwrap();
        assert_eq!(clip.notes[0].start, 0.0);
        assert!(clip.start < 0.5);
    }
}
