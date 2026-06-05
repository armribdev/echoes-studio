//! Gain node — applies a linear gain multiplier to its input.
//!
//! Used for both track volume faders and explicit gain stages.
//! Handles stereo (and multichannel) transparently.

use std::f32::consts::TAU;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

/// Per-sample gain-smoothing cutoff (ADR-019 Tranche 2).  Removes zipper on
/// modulated/automated gain without audible lag.  ~12 Hz ≈ 13 ms time constant.
/// Derived from the sample rate only, so the ramp is block-size independent.
const GAIN_SMOOTH_CUTOFF_HZ: f32 = 12.0;

pub struct GainNode {
    id: NodeId,
    gain: f32,
    /// Accumulated modulation delta for the current block (reset each block).
    mod_delta: f32,
    /// Per-sample smoothed gain; ramps toward `gain + mod_delta` (anti-zipper).
    smoothed_gain: f32,
    /// One-pole coefficient set in `prepare` from the sample rate.
    gain_coeff: f32,
}

impl GainNode {
    pub fn new(id: NodeId, gain: f32) -> Self {
        Self {
            id,
            gain,
            mod_delta: 0.0,
            smoothed_gain: gain,
            gain_coeff: 0.0,
        }
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }
}

impl AudioNode for GainNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "GainNode"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.gain_coeff = 1.0 - (-TAU * GAIN_SMOOTH_CUTOFF_HZ / ctx.sample_rate).exp();
        self.smoothed_gain = self.gain;
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        let input = &inputs[0];
        let output = &mut outputs[0];

        let channels = output.num_channels.min(input.num_channels);
        let frames = transport.block_size.min(output.num_frames).min(input.num_frames);
        let target = (self.gain + self.mod_delta).max(0.0);
        let coeff = self.gain_coeff;

        // Frame-outer so the smoothed gain advances once per frame, shared across
        // channels.  When already at target (static gain) the step is exactly 0,
        // so an unmodulated node stays bit-identical.
        for i in 0..frames {
            self.smoothed_gain += coeff * (target - self.smoothed_gain);
            let g = self.smoothed_gain;
            for ch in 0..channels {
                output.channel_mut(ch)[i] = input.channel(ch)[i] * g;
            }
        }
    }

    fn reset(&mut self) {
        self.reset_modulation();
        self.smoothed_gain = self.gain;
    }

    fn reset_modulation(&mut self) {
        self.mod_delta = 0.0;
    }

    fn param_id(&self, name: &str) -> Option<u32> {
        match name {
            "gain" => Some(PARAM_GAIN),
            _ => None,
        }
    }

    fn apply_modulation(&mut self, param_id: u32, delta: f32) {
        if param_id == PARAM_GAIN {
            self.mod_delta += delta;
        }
    }
}

const PARAM_GAIN: u32 = 0;

#[cfg(test)]
mod tests {
    use super::*;

    fn transport(block: usize) -> TransportInfo {
        TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            sample_rate: 48_000.0,
            block_size: block,
        }
    }

    fn ctx(block: usize) -> PrepareContext {
        PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: block,
            num_channels: 1,
        }
    }

    #[test]
    fn static_gain_is_bit_identical() {
        let mut node = GainNode::new(NodeId(1), 0.5);
        node.prepare(ctx(8));
        let mut input = AudioBuffer::new(1, 8);
        input.channel_mut(0).copy_from_slice(&[1.0; 8]);
        let mut outputs = [AudioBuffer::new(1, 8)];
        node.process(&[&input], &mut outputs, &transport(8));
        for &s in outputs[0].channel(0) {
            assert_eq!(s, 0.5, "unmodulated static gain must not be smoothed");
        }
    }

    #[test]
    fn modulation_ramps_instead_of_jumping() {
        let mut node = GainNode::new(NodeId(1), 1.0);
        node.prepare(ctx(64));
        node.apply_modulation(PARAM_GAIN, 1.0); // target gain 2.0
        let mut input = AudioBuffer::new(1, 64);
        input.channel_mut(0).copy_from_slice(&[1.0; 64]);
        let mut outputs = [AudioBuffer::new(1, 64)];
        node.process(&[&input], &mut outputs, &transport(64));
        let out = outputs[0].channel(0);
        assert!(out[0] > 1.0 && out[0] < 2.0, "first sample must be a partial step, got {}", out[0]);
        assert!(out[63] > out[0], "gain must ramp upward toward the modulated target");
        assert!(out[63] < 2.0, "13 ms smoother must not reach target within ~1.3 ms");
    }
}
