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

// ── InstrumentDevice trait ────────────────────────────────────────────────────

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

// ── InstrumentNode ────────────────────────────────────────────────────────────

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
        InstrumentNode {
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
    fn node_id(&self) -> NodeId { self.id }
    fn label(&self) -> &str { "InstrumentNode" }
    fn num_input_ports(&self) -> usize { 0 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.device.prepare(ctx.sample_rate, ctx.max_block_size, ctx.num_channels);
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
            let block_end   = block_start + transport.block_size as u64;
            let bpm = transport.bpm;
            let sample_rate = transport.sample_rate;
            let discontinuity = !self.was_playing
                || block_start != self.next_expected_block_start;

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
                        kind: MidiEventKind::NoteOn { pitch: note.pitch, velocity: note.velocity },
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
            self.next_expected_block_start = transport.position_samples;
        }

        self.was_playing = transport.is_playing;
        self.device.process_midi(&self.event_buf, outputs, transport);
    }

    fn reset(&mut self) {
        self.device.reset();
        self.next_expected_block_start = 0;
        self.was_playing = false;
    }
}
