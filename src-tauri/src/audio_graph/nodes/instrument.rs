//! Instrument node — schedules MIDI events from clips and drives an `InstrumentDevice`.
//!
//! ## Role in the graph
//!
//! `InstrumentNode` is a **source** (0 audio inputs, one stereo output per
//! output its device declares — 1 unless a multi-out plugin says otherwise).
//! It converts timeline MIDI clips into per-block `MidiEventList` entries and
//! delegates audio synthesis to an inner `Box<dyn InstrumentDevice>`.
//!
//! The device abstraction means the same node works with the built-in `PolySynth`
//! today and with a VST3 wrapper tomorrow — the graph runtime never sees the
//! difference.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::live_params::EnvelopeCells;
use crate::audio_graph::midi::{
    MidiEvent, MidiEventKind, MidiEventList, MidiNotePublisher, MidiNoteSnapshot, MidiPreviewQueue,
};
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::audio_graph::plugin_params::{parse_plugin_param_name, PluginParamControl, PluginParamControls};

/// Anything that turns MIDI events into audio.
///
/// Implemented by `PolySynth` (built-in) and future VST3/AU wrappers.
/// `InstrumentNode` holds a `Box<dyn InstrumentDevice>` and calls
/// `process_midi` once per block.
pub trait InstrumentDevice: Send + 'static {
    /// Stereo outputs rendered: the main one, then any auxiliary outputs a
    /// multi-out plugin declares (ADR-014).  Final once `prepare` has run.
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, ctx: PrepareContext);

    /// Render one block of audio from the provided MIDI events.
    ///
    /// **Must never allocate or block.**
    fn process_midi(
        &mut self,
        events: &MidiEventList,
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    );

    fn reset(&mut self);

    /// Plugin instruments receive the parameters targeted by modulation or
    /// automation (ADR-021).  Default: the device has no controllable parameters.
    fn inject_param_controls(&mut self, _controls: Vec<PluginParamControl>) {}

    fn param_controls_mut(&mut self) -> Option<&mut PluginParamControls> {
        None
    }
}

/// Stands in for an instrument plugin that could not be loaded: the track stays
/// silent instead of playing a different sound.  The scheduler clears the
/// output buffers before `process`, so doing nothing yields silence.
pub struct SilentInstrument;

impl InstrumentDevice for SilentInstrument {
    fn prepare(&mut self, _ctx: PrepareContext) {}

    fn process_midi(
        &mut self,
        _events: &MidiEventList,
        _outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
    }

    fn reset(&mut self) {}
}

pub struct InstrumentNode {
    id: NodeId,
    /// All notes for this instrument, in any order.
    /// Stored in beat space; sample positions are derived per block from transport BPM.
    /// Replaced in place when the piano roll edits notes — no graph rebuild.
    notes: Arc<MidiNotePublisher>,
    /// Piano-roll audition. Independent of transport and of the timeline note list.
    preview: Option<Arc<MidiPreviewQueue>>,
    /// Voices opened by audition, so a seek or stop does not release them.
    preview_count: [u8; 128],
    device: Box<dyn InstrumentDevice>,
    /// Reused every block — no heap allocation in the RT path.
    event_buf: MidiEventList,
    /// Expected `position_samples` at the start of the next playing block.
    /// Used to detect seeks and other non-continuous jumps.
    next_expected_block_start: u64,
    /// Previous block's `is_playing` — detects play/pause transitions.
    was_playing: bool,
    /// Per-pitch voice count: incremented on NoteOn, decremented on NoteOff.
    /// On seek/pause, any pitch with count > 0 receives a NoteOff so plugins
    /// don't hang. A count handles same-pitch polyphony correctly where a
    /// plain bitset would clear too early.
    active_pitch_count: [u8; 128],
    /// Envelope modulators on the same track.  Injected at compile time;
    /// the node writes the gate state after processing each block so that
    /// `EnvelopeNode::process` sees the correct gate on the next block.
    envelope_cells: Vec<Arc<EnvelopeCells>>,
}

impl InstrumentNode {
    pub fn new(
        id: NodeId,
        notes: Arc<Vec<MidiNoteSnapshot>>,
        device: Box<dyn InstrumentDevice>,
    ) -> Self {
        Self::from_publisher(id, MidiNotePublisher::with_notes(notes.as_ref().clone()), device)
    }

    pub fn set_preview(&mut self, queue: Arc<MidiPreviewQueue>) {
        self.preview = Some(queue);
    }

    fn drain_preview(&mut self) {
        let Some(queue) = self.preview.clone() else {
            return;
        };
        queue.drain(|message| {
            let pitch = message.pitch as usize;
            if message.note_on {
                self.event_buf.push(MidiEvent {
                    offset: 0,
                    kind: MidiEventKind::NoteOn {
                        pitch: message.pitch,
                        velocity: message.velocity,
                    },
                });
                self.preview_count[pitch] = self.preview_count[pitch].saturating_add(1);
            } else if self.preview_count[pitch] > 0 {
                self.event_buf.push(MidiEvent {
                    offset: 0,
                    kind: MidiEventKind::NoteOff { pitch: message.pitch },
                });
                self.preview_count[pitch] -= 1;
            }
        });
    }

    pub fn from_publisher(
        id: NodeId,
        notes: Arc<MidiNotePublisher>,
        device: Box<dyn InstrumentDevice>,
    ) -> Self {
        Self {
            id,
            notes,
            preview: None,
            preview_count: [0u8; 128],
            device,
            event_buf: MidiEventList::new(),
            next_expected_block_start: 0,
            was_playing: false,
            active_pitch_count: [0u8; 128],
            envelope_cells: Vec::new(),
        }
    }
}

impl InstrumentNode {
    /// Align sounding voices with a note list that just replaced the previous one.
    /// Pitches that are still covered keep their voices; only the difference is
    /// turned off or on, so an edit elsewhere does not retrigger the chord.
    fn reconcile_sounding_notes(&mut self, block_start: u64, bpm: f32, sample_rate: f32) {
        let mut should_sound = [0u8; 128];
        let mut velocity = [0u8; 128];
        for note in self.notes.notes() {
            let start_sample = note
                .start_sample(bpm, sample_rate)
                .max(note.window_start_sample);
            let end_sample = note
                .end_sample(bpm, sample_rate)
                .min(note.window_end_sample);
            if start_sample < end_sample && start_sample < block_start && block_start < end_sample {
                let pitch = note.pitch as usize;
                should_sound[pitch] = should_sound[pitch].saturating_add(1);
                velocity[pitch] = note.velocity;
            }
        }
        for pitch in 0..128 {
            while self.active_pitch_count[pitch] > should_sound[pitch] {
                self.event_buf.push(MidiEvent {
                    offset: 0,
                    kind: MidiEventKind::NoteOff { pitch: pitch as u8 },
                });
                self.active_pitch_count[pitch] -= 1;
            }
            while self.active_pitch_count[pitch] < should_sound[pitch] {
                self.event_buf.push(MidiEvent {
                    offset: 0,
                    kind: MidiEventKind::NoteOn {
                        pitch: pitch as u8,
                        velocity: velocity[pitch],
                    },
                });
                self.active_pitch_count[pitch] += 1;
            }
        }
    }

    /// NoteOff at offset 0 for every pitch a NoteOn was sent for and not yet released.
    fn release_sounding_notes(&mut self) {
        for (pitch, count) in self.active_pitch_count.iter_mut().enumerate() {
            if *count > 0 {
                self.event_buf.push(MidiEvent {
                    offset: 0,
                    kind: MidiEventKind::NoteOff { pitch: pitch as u8 },
                });
                *count = 0;
            }
        }
    }
}

impl AudioNode for InstrumentNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "InstrumentNode"
    }
    fn num_input_ports(&self) -> usize {
        0
    }
    fn num_output_ports(&self) -> usize {
        self.device.num_output_ports()
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.device.prepare(ctx);
    }

    fn process(
        &mut self,
        _inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        self.event_buf.clear();
        let notes_swapped = self.notes.try_swap();

        if transport.is_playing {
            let block_start = transport.position_samples;
            let block_end = block_start + transport.block_size as u64;
            let bpm = transport.bpm;
            let sample_rate = transport.sample_rate;
            let discontinuity = !self.was_playing || block_start != self.next_expected_block_start;

            if discontinuity {
                self.release_sounding_notes();

                for note in self.notes.notes() {
                    let start_sample = note.start_sample(bpm, sample_rate).max(note.window_start_sample);
                    let end_sample = note.end_sample(bpm, sample_rate).min(note.window_end_sample);
                    if start_sample < end_sample && start_sample < block_start && block_start < end_sample {
                        self.event_buf.push(MidiEvent {
                            offset: 0,
                            kind: MidiEventKind::NoteOn {
                                pitch: note.pitch,
                                velocity: note.velocity,
                            },
                        });
                        self.active_pitch_count[note.pitch as usize] =
                            self.active_pitch_count[note.pitch as usize].saturating_add(1);
                    }
                }
            } else if notes_swapped {
                self.reconcile_sounding_notes(block_start, bpm, sample_rate);
            }

            for note in self.notes.notes() {
                // Clamp the note to the clip's playable window (trim).
                let start_sample = note.start_sample(bpm, sample_rate).max(note.window_start_sample);
                let end_sample = note.end_sample(bpm, sample_rate).min(note.window_end_sample);
                if start_sample >= end_sample {
                    continue;
                }
                if start_sample >= block_start && start_sample < block_end {
                    self.event_buf.push(MidiEvent {
                        offset: (start_sample - block_start) as u32,
                        kind: MidiEventKind::NoteOn {
                            pitch: note.pitch,
                            velocity: note.velocity,
                        },
                    });
                    self.active_pitch_count[note.pitch as usize] =
                        self.active_pitch_count[note.pitch as usize].saturating_add(1);
                }
                if end_sample >= block_start && end_sample < block_end {
                    self.event_buf.push(MidiEvent {
                        offset: (end_sample - block_start) as u32,
                        kind: MidiEventKind::NoteOff { pitch: note.pitch },
                    });
                    self.active_pitch_count[note.pitch as usize] =
                        self.active_pitch_count[note.pitch as usize].saturating_sub(1);
                }
            }

            self.drain_preview();
            self.event_buf.sort_by_offset();
            self.next_expected_block_start = block_end;
        } else {
            // Play → pause/stop: the playhead may already be reset (Stop, end of
            // song), so release what was actually turned on, not what the notes
            // say should sound at the current position.
            if self.was_playing {
                self.release_sounding_notes();
            }
            self.drain_preview();
            self.next_expected_block_start = transport.position_samples;
        }

        self.was_playing = transport.is_playing;

        // Notify envelope modulators: gate = any note still sounding this block.
        if !self.envelope_cells.is_empty() {
            let any_active = self.active_pitch_count.iter().any(|&c| c > 0)
                || self.preview_count.iter().any(|&c| c > 0);
            for cells in &self.envelope_cells {
                cells.gate.store(any_active, Ordering::Relaxed);
            }
        }

        self.device
            .process_midi(&self.event_buf, outputs, transport);
    }

    fn reset(&mut self) {
        self.device.reset();
        self.next_expected_block_start = 0;
        self.was_playing = false;
        self.active_pitch_count = [0u8; 128];
        self.preview_count = [0u8; 128];
        // Release envelopes on graph reset so they don't hang in Sustain.
        for cells in &self.envelope_cells {
            cells.gate.store(false, Ordering::Relaxed);
        }
    }

    fn inject_envelope_cells(&mut self, cells: Vec<Arc<EnvelopeCells>>) {
        self.envelope_cells = cells;
    }

    fn param_id(&self, name: &str) -> Option<u32> {
        parse_plugin_param_name(name)
    }

    fn reset_modulation(&mut self) {
        if let Some(controls) = self.device.param_controls_mut() {
            controls.clear_block();
        }
    }

    fn apply_modulation(&mut self, param_id: u32, delta: f32) {
        if let Some(controls) = self.device.param_controls_mut() {
            controls.modulate(param_id, delta);
        }
    }

    fn apply_modulation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        if let Some(controls) = self.device.param_controls_mut() {
            controls.modulate_at(param_id, sample_offset, value);
        }
    }

    fn apply_automation(&mut self, param_id: u32, value: f32) {
        if let Some(controls) = self.device.param_controls_mut() {
            controls.automate(param_id, value);
        }
    }

    fn apply_automation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        if let Some(controls) = self.device.param_controls_mut() {
            controls.automate_at(param_id, sample_offset, value);
        }
    }

    fn inject_plugin_param_controls(&mut self, controls: Vec<PluginParamControl>) {
        self.device.inject_param_controls(controls);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A test-only `InstrumentDevice` that records the events handed to it each
    /// block.  The `Mutex` lives only in test code — never on the RT path.
    struct CaptureDevice {
        captured: Arc<Mutex<Vec<MidiEvent>>>,
    }

    impl InstrumentDevice for CaptureDevice {
        fn prepare(&mut self, _ctx: PrepareContext) {}
        fn process_midi(
            &mut self,
            events: &MidiEventList,
            _outputs: &mut [AudioBuffer],
            _transport: &TransportInfo,
        ) {
            let mut g = self.captured.lock().unwrap();
            g.clear();
            g.extend_from_slice(events.as_slice());
        }
        fn reset(&mut self) {}
    }

    fn one_beat_note() -> MidiNoteSnapshot {
        MidiNoteSnapshot {
            pitch: 60,
            velocity: 100,
            clip_start_sec: 0.0,
            start_beats: 1.0,
            end_beats: 2.0,
            window_start_sample: 0,
            window_end_sample: u64::MAX,
        }
    }

    fn make_node(captured: Arc<Mutex<Vec<MidiEvent>>>) -> InstrumentNode {
        let mut node = InstrumentNode::new(
            NodeId(1),
            Arc::new(vec![one_beat_note()]),
            Box::new(CaptureDevice { captured }),
        );
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64_000,
            num_channels: 2,
            live: true,
        });
        node
    }

    fn run_block(node: &mut InstrumentNode, transport: &TransportInfo) {
        let inputs: [&AudioBuffer; 0] = [];
        let mut outputs = vec![AudioBuffer::new(2, transport.block_size)];
        node.process(&inputs, &mut outputs, transport);
    }

    /// At 120 BPM a beat-1 NoteOn must land at sample offset 24_000 within a
    /// block that starts at the project origin.
    #[test]
    fn emits_note_on_at_correct_offset() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut node = make_node(captured.clone());

        let transport = TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: 48_001, // covers both NoteOn (24_000) and NoteOff (48_000)
        };
        run_block(&mut node, &transport);

        let events = captured.lock().unwrap();
        let note_on = events
            .iter()
            .find(|e| matches!(e.kind, MidiEventKind::NoteOn { .. }))
            .expect("NoteOn emitted");
        assert_eq!(note_on.offset, 24_000);
    }

    /// Pausing while a note is sounding must emit a NoteOff so the voice is
    /// released instead of ringing until the transport later crosses its end.
    #[test]
    fn pause_while_note_sounding_emits_note_off() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut node = make_node(captured.clone());

        // Block straddling the note start (24_000 at 120 BPM) so the voice is on.
        let playing = TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: 30_000,
        };
        run_block(&mut node, &playing);

        // Pause with the playhead inside the note (start 24_000, end 48_000).
        let paused = TransportInfo {
            is_playing: false,
            position_samples: 30_000,
            position_seconds: 30_000.0 / 48_000.0,
            bpm: 120.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: 512,
        };
        run_block(&mut node, &paused);

        let events = captured.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e.kind, MidiEventKind::NoteOff { pitch: 60 })),
            "pausing inside a note must release it"
        );
    }

    /// Same node, notes unchanged: changing `transport.bpm` between blocks
    /// re-resolves the note position — no rebake, no recompile.  This is the
    /// ADR-012 property at the node level (BP-3).
    #[test]
    fn tempo_change_between_blocks_reschedules_same_node() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut node = make_node(captured.clone());

        let block1 = TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: 1_000,
        };
        run_block(&mut node, &block1);
        assert!(
            captured
                .lock()
                .unwrap()
                .iter()
                .all(|e| !matches!(e.kind, MidiEventKind::NoteOn { .. })),
            "no NoteOn yet at 120 BPM in frames 0..1_000"
        );

        let block2 = TransportInfo {
            is_playing: true,
            position_samples: 1_000,
            position_seconds: 1_000.0 / 48_000.0,
            bpm: 240.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: 20_000, // frames 1_000..21_000
        };
        run_block(&mut node, &block2);

        let events = captured.lock().unwrap();
        let note_on = events
            .iter()
            .find(|e| matches!(e.kind, MidiEventKind::NoteOn { .. }))
            .expect("NoteOn emitted at the new tempo");
        assert_eq!(note_on.offset, 11_000);
    }

    fn transport_at(is_playing: bool, position_samples: u64, block_size: usize) -> TransportInfo {
        TransportInfo {
            is_playing,
            position_samples,
            position_seconds: position_samples as f64 / 48_000.0,
            bpm: 120.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size,
        }
    }

    fn released_pitch(captured: &Arc<Mutex<Vec<MidiEvent>>>, pitch: u8) -> bool {
        captured
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e.kind, MidiEventKind::NoteOff { pitch: p } if p == pitch))
    }

    /// Stop resets the playhead to 0, so the stop block no longer "sees" the
    /// sounding note at its position; the note must still be released.
    #[test]
    fn stop_while_note_sounding_emits_note_off() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut node = make_node(captured.clone());

        run_block(&mut node, &transport_at(true, 0, 30_000));
        run_block(&mut node, &transport_at(false, 0, 512));

        assert!(released_pitch(&captured, 60), "Stop must release the sounding note");
    }

    /// The note ends exactly where a block ends and the song auto-stops before
    /// the next block: its NoteOff must not be lost (stuck last note).
    #[test]
    fn note_ending_on_the_last_block_boundary_is_released_at_stop() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut node = make_node(captured.clone());

        run_block(&mut node, &transport_at(true, 0, 48_000));
        assert!(!released_pitch(&captured, 60), "the NoteOff falls on the next block");
        run_block(&mut node, &transport_at(false, 0, 512));

        assert!(released_pitch(&captured, 60), "auto-stop must release the last note");
    }

    /// A note-list swap while playing releases a deleted pitch and leaves the
    /// pitch that is still in the list sounding — no NoteOn, so no click.
    #[test]
    fn note_swap_releases_only_the_deleted_pitch() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let publisher = MidiNotePublisher::with_notes(vec![
            one_beat_note(),
            MidiNoteSnapshot {
                pitch: 64,
                ..one_beat_note()
            },
        ]);
        let mut node = InstrumentNode::from_publisher(
            NodeId(1),
            Arc::clone(&publisher),
            Box::new(CaptureDevice {
                captured: Arc::clone(&captured),
            }),
        );
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64_000,
            num_channels: 2,
            live: true,
        });

        run_block(&mut node, &transport_at(true, 0, 30_000));
        run_block(&mut node, &transport_at(true, 30_000, 512));
        publisher.publish(vec![one_beat_note()]);
        run_block(&mut node, &transport_at(true, 30_512, 512));

        let events = captured.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e.kind, MidiEventKind::NoteOff { pitch: 64 })),
            "deleted pitch must be released"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, MidiEventKind::NoteOn { pitch: 60, .. })),
            "the note that stayed must not be retriggered"
        );
    }

    #[test]
    fn audition_plays_while_transport_is_stopped() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut node = make_node(captured.clone());
        let queue = Arc::new(crate::audio_graph::midi::MidiPreviewQueue::new());
        node.set_preview(Arc::clone(&queue));
        queue.push(crate::audio_graph::midi::MidiPreviewMessage {
            note_on: true,
            pitch: 72,
            velocity: 90,
        });

        run_block(&mut node, &transport_at(false, 0, 512));

        let events = captured.lock().unwrap();
        assert!(
            events.iter().any(|e| matches!(
                e.kind,
                MidiEventKind::NoteOn { pitch: 72, velocity: 90 }
            )),
            "a placed note must sound even when playback is stopped"
        );
    }
}
