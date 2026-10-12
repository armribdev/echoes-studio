//! Recording of hardware MIDI notes into a clip (ADR-022 amendment).
//!
//! The MIDI capture thread hands every note of the armed track to
//! `MidiRecorder::capture`, which stamps it with the transport sample that was
//! playing when the key was pressed. When the recording ends — record button,
//! pause or stop — the notes are paired into a clip that the frontend adds
//! through the regular `ADD_MIDI_CLIP` command (one undo step).
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
use crate::project::{generate_id, MidiNote};

/// Emitted when a recording ends. Payload: `Option<RecordedMidiClip>`, `null`
/// when no note was played.
pub const MIDI_RECORDING_FINISHED_EVENT: &str = "midi-recording-finished";

/// Emitted for every note edge while recording, so the arrange view can draw
/// the clip as it grows. Payload: `RecordedNote`.
pub const MIDI_RECORDING_NOTE_EVENT: &str = "midi-recording-note";

const MIN_NOTE_BEATS: f64 = 0.0625;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedMidiClip {
    pub track_id: String,
    /// Seconds, same coordinate space as `AppMidiClip`.
    pub start: f64,
    pub end: f64,
    pub notes: Vec<MidiNote>,
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

struct Recording {
    track_id: String,
    start_position_samples: u64,
    events: Vec<RecordedEvent>,
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
        });
        Ok(start_position_samples as f64 / self.transport.sample_rate() as f64)
    }

    /// MIDI capture thread. Ignores notes played during the count-in, notes
    /// that are not for the recorded track, and controls (clips hold notes only);
    /// returns the recorded note so the caller can show it right away.
    pub fn capture(&self, track_id: &str, message: &MidiLiveInputMessage) -> Option<RecordedNote> {
        let (note_on, pitch, velocity) = match message.kind {
            MidiEventKind::NoteOn { pitch, velocity } => (true, pitch, velocity),
            MidiEventKind::NoteOff { pitch } => (false, pitch, 0),
            MidiEventKind::Control { .. } => return None,
        };
        if self.transport.count_in_remaining() > 0 {
            return None;
        }
        let mut recording = self.lock();
        let recording = recording.as_mut().filter(|r| r.track_id == track_id)?;
        let sample_rate = self.transport.sample_rate();
        let position_samples = self
            .clock
            .transport_position_at(message.received_at, sample_rate)?;
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
            position: position_samples as f64 / sample_rate as f64,
        })
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
    let notes = spans
        .iter()
        .map(|span| MidiNote {
            id: generate_id(),
            pitch: span.pitch,
            start: ((span.on_samples - clip_start) as f64 * beats_per_sample) as f32,
            duration: ((span.off_samples - span.on_samples) as f64 * beats_per_sample) as f32,
            velocity: span.velocity,
        })
        .collect();

    Some(RecordedMidiClip {
        track_id: recording.track_id,
        start: clip_start as f64 / sample_rate,
        end: clip_end as f64 / sample_rate,
        notes,
    })
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

    #[test]
    fn controls_are_not_recorded() {
        let transport = SharedTransport::new(48_000.0, 120.0);
        let clock = Arc::new(LiveInputClock::new());
        clock.set_event_window(10_000_000, 0);
        let recorder = MidiRecorder::new(Arc::clone(&transport), clock);
        recorder.start("track".into()).unwrap();
        let bend = MidiLiveInputMessage {
            kind: MidiEventKind::Control {
                message: [0xE0, 0, 0x60],
            },
            received_at: 20_000_000,
        };

        assert!(recorder.capture("track", &bend).is_none());
        assert_eq!(recorder.finish().map(|clip| clip.is_none()), Some(true));
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
