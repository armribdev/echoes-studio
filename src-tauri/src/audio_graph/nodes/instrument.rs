//! Instrument node — schedules MIDI events from clips and drives an `InstrumentDevice`.
//!
//! ## Role in the graph
//!
//! `InstrumentNode` is a **source** (0 audio inputs, 1 stereo output).
//! It converts timeline MIDI clips into per-block `MidiEventList` entries and
//! delegates audio synthesis to an inner `Box<dyn InstrumentDevice>`.
//!
//! The device abstraction means the same node works with the built-in `PolySynth`
//! today and with a VST3 wrapper tomorrow — the graph runtime never sees the
//! difference.

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::{MidiEvent, MidiEventKind, MidiEventList, MidiNoteSnapshot};
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

/// Anything that turns MIDI events into audio.
///
/// Implemented by `PolySynth` (built-in) and future VST3/AU wrappers.
/// `InstrumentNode` holds a `Box<dyn InstrumentDevice>` and calls
/// `process_midi` once per block.
pub trait InstrumentDevice: Send + 'static {
    fn prepare(&mut self, sample_rate: f32, max_block_size: usize, num_channels: usize);

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
}

pub struct InstrumentNode {
    id: NodeId,
    /// All notes for this instrument, in any order.
    /// Stored in beat space; sample positions are derived per block from transport BPM.
    notes: Arc<Vec<MidiNoteSnapshot>>,
    device: Box<dyn InstrumentDevice>,
    /// Reused every block — no heap allocation in the RT path.
    event_buf: MidiEventList,
    /// Expected `position_samples` at the start of the next playing block.
    /// Used to detect seeks and other non-continuous jumps.
    next_expected_block_start: u64,
    /// Previous block's `is_playing` — detects play/pause transitions.
    was_playing: bool,
}

impl InstrumentNode {
    pub fn new(
        id: NodeId,
        notes: Arc<Vec<MidiNoteSnapshot>>,
        device: Box<dyn InstrumentDevice>,
    ) -> Self {
        Self {
            id,
            notes,
            device,
            event_buf: MidiEventList::new(),
            next_expected_block_start: 0,
            was_playing: false,
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
        1
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.device
            .prepare(ctx.sample_rate, ctx.max_block_size, ctx.num_channels);
    }

    fn process(
        &mut self,
        _inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        self.event_buf.clear();

        if transport.is_playing {
            let block_start = transport.position_samples;
            let block_end = block_start + transport.block_size as u64;
            let bpm = transport.bpm;
            let sample_rate = transport.sample_rate;
            let discontinuity = !self.was_playing || block_start != self.next_expected_block_start;

            if discontinuity {
                self.device.reset();
                for note in self.notes.iter() {
                    let start_sample = note.start_sample(bpm, sample_rate);
                    let end_sample = note.end_sample(bpm, sample_rate);
                    // Note already started but not yet ended — emit a catch-up
                    // NoteOn at the beginning of this block (seek / resume).
                    if start_sample < block_start && block_start < end_sample {
                        self.event_buf.push(MidiEvent {
                            offset: 0,
                            kind: MidiEventKind::NoteOn {
                                pitch: note.pitch,
                                velocity: note.velocity,
                            },
                        });
                    }
                }
            }

            for note in self.notes.iter() {
                let start_sample = note.start_sample(bpm, sample_rate);
                let end_sample = note.end_sample(bpm, sample_rate);
                if start_sample >= block_start && start_sample < block_end {
                    self.event_buf.push(MidiEvent {
                        offset: (start_sample - block_start) as u32,
                        kind: MidiEventKind::NoteOn {
                            pitch: note.pitch,
                            velocity: note.velocity,
                        },
                    });
                }
                if end_sample >= block_start && end_sample < block_end {
                    self.event_buf.push(MidiEvent {
                        offset: (end_sample - block_start) as u32,
                        kind: MidiEventKind::NoteOff { pitch: note.pitch },
                    });
                }
            }

            self.event_buf.sort_by_offset();
            self.next_expected_block_start = block_end;
        } else {
            // Play → pause: release notes still sounding at the pause point so
            // voices fade out instead of ringing until the transport later
            // crosses their end. Reuses the NoteOff path (PolySynth + CLAP).
            if self.was_playing {
                let pause_point = transport.position_samples;
                let bpm = transport.bpm;
                let sample_rate = transport.sample_rate;
                for note in self.notes.iter() {
                    let start_sample = note.start_sample(bpm, sample_rate);
                    let end_sample = note.end_sample(bpm, sample_rate);
                    if start_sample <= pause_point && pause_point < end_sample {
                        self.event_buf.push(MidiEvent {
                            offset: 0,
                            kind: MidiEventKind::NoteOff { pitch: note.pitch },
                        });
                    }
                }
            }
            self.next_expected_block_start = transport.position_samples;
        }

        self.was_playing = transport.is_playing;
        self.device
            .process_midi(&self.event_buf, outputs, transport);
    }

    fn reset(&mut self) {
        self.device.reset();
        self.next_expected_block_start = 0;
        self.was_playing = false;
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
        fn prepare(&mut self, _sr: f32, _mb: usize, _nc: usize) {}
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
}
