//! Modulator nodes — produce normalised (−1…+1) modulation signals.
//!
//! These nodes have zero audio inputs and one audio output.  The output buffer
//! carries the raw modulation curve for one block; the scheduler reads it and
//! delivers scaled deltas to target nodes via `apply_modulation`.

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::live_params::ModulatorCells;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::project::LfoWaveform;

/// LFO with selectable waveform.  Output is normalised to −1…+1.
///
/// Phase accumulates across blocks; `reset()` returns it to 0.
///
/// `rate_cells` lets the command thread retune the LFO without triggering a
/// graph recompile.  When `None`, falls back to the constant `rate_hz` baked
/// at construction time (used by placeholder modulators).
pub struct LfoNode {
    id: NodeId,
    rate_hz: f32,
    rate_cells: Option<Arc<ModulatorCells>>,
    waveform: LfoWaveform,
    phase: f32,
}

impl LfoNode {
    pub fn new(id: NodeId, rate_hz: f32, waveform: LfoWaveform) -> Self {
        Self {
            id,
            rate_hz,
            rate_cells: None,
            waveform,
            phase: 0.0,
        }
    }

    pub fn with_rate_cells(mut self, cells: Arc<ModulatorCells>) -> Self {
        self.rate_cells = Some(cells);
        self
    }

    #[inline]
    fn sample(phase: f32, waveform: &LfoWaveform) -> f32 {
        match waveform {
            LfoWaveform::Sine => (phase * std::f32::consts::TAU).sin(),
            // Triangle: 0 → +1 → 0 → -1 → 0 over one cycle
            LfoWaveform::Triangle => {
                if phase < 0.25 {
                    4.0 * phase
                } else if phase < 0.75 {
                    2.0 - 4.0 * phase
                } else {
                    4.0 * phase - 4.0
                }
            }
            // Rising sawtooth: −1 → +1
            LfoWaveform::Saw => 2.0 * phase - 1.0,
            // Square: +1 for first half, −1 for second half
            LfoWaveform::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
}

impl AudioNode for LfoNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "LFO"
    }
    fn num_input_ports(&self) -> usize {
        0
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, _ctx: PrepareContext) {}

    fn process(
        &mut self,
        _inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        if outputs.is_empty() {
            return;
        }
        let output = &mut outputs[0];
        let block_size = transport.block_size.min(output.num_frames);
        let rate_hz = match &self.rate_cells {
            Some(cells) => cells.rate_hz(),
            None => self.rate_hz,
        };
        let phase_inc = rate_hz / transport.sample_rate;

        {
            let ch0 = output.channel_mut(0);
            let mut phase = self.phase;
            for i in 0..block_size {
                ch0[i] = Self::sample(phase, &self.waveform);
                phase = (phase + phase_inc).fract();
            }
            self.phase = phase;
        }
        output.mono_to_all_channels();
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }
}
