//! Gain node — applies a linear gain multiplier to its input.
//!
//! Used for both track volume faders and explicit gain stages.
//! Handles stereo (and multichannel) transparently.

use std::f32::consts::TAU;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo, MAX_CONTROL_POINTS};

/// Per-sample gain-smoothing cutoff (ADR-019 Tranche 2).  Removes zipper on
/// modulated/automated gain without audible lag.  ~12 Hz ≈ 13 ms time constant.
/// Derived from the sample rate only, so the ramp is block-size independent.
const GAIN_SMOOTH_CUTOFF_HZ: f32 = 12.0;

/// Track fader. Slower than the modulation smoother, and applied in dB so a
/// loud plugin (full-scale harmonics) does not click on a fast move.
pub(crate) const FADER_SMOOTH_CUTOFF_HZ: f32 = 4.0;

/// One-pole gain smoother driven by modulation on the absolute control grid
/// (ADR-019).  The target switches at each grid point, so the ramp depends only
/// on absolute sample positions, never on where block boundaries fall.
pub(crate) struct SmoothedGain {
    smoothed: f32,
    /// Decibel state for `advance_db`. Snapped to the linear gain in `prepare`.
    db: f32,
    coeff: f32,
    /// Delta from the block start up to the first interior grid point.
    base_delta: f32,
    point_offsets: [u32; MAX_CONTROL_POINTS],
    point_deltas: [f32; MAX_CONTROL_POINTS],
    point_count: usize,
}

impl SmoothedGain {
    pub(crate) fn new(initial_gain: f32) -> Self {
        Self {
            smoothed: initial_gain,
            db: lin_to_db(initial_gain),
            coeff: 0.0,
            base_delta: 0.0,
            point_offsets: [0; MAX_CONTROL_POINTS],
            point_deltas: [0.0; MAX_CONTROL_POINTS],
            point_count: 0,
        }
    }

    pub(crate) fn prepare(&mut self, sample_rate: f32, gain: f32) {
        self.set_cutoff(sample_rate, GAIN_SMOOTH_CUTOFF_HZ);
        self.snap_to(gain);
    }

    pub(crate) fn set_cutoff(&mut self, sample_rate: f32, cutoff_hz: f32) {
        if sample_rate <= 0.0 {
            return;
        }
        self.coeff = 1.0 - (-TAU * cutoff_hz / sample_rate).exp();
    }

    pub(crate) fn snap_to(&mut self, gain: f32) {
        self.smoothed = gain;
        self.db = lin_to_db(gain);
    }

    pub(crate) fn clear_modulation(&mut self) {
        self.base_delta = 0.0;
        self.point_count = 0;
    }

    pub(crate) fn add_block_start_delta(&mut self, delta: f32) {
        self.base_delta += delta;
    }

    /// Grid offsets arrive ascending per route; routes sharing an offset sum.
    pub(crate) fn add_delta_at(&mut self, sample_offset: u32, delta: f32) {
        let points = &self.point_offsets[..self.point_count];
        if let Some(k) = points.iter().position(|&offset| offset == sample_offset) {
            self.point_deltas[k] += delta;
        } else if self.point_count < MAX_CONTROL_POINTS {
            self.point_offsets[self.point_count] = sample_offset;
            self.point_deltas[self.point_count] = delta;
            self.point_count += 1;
        }
    }

    /// One sample of the one-pole. At target the step is exactly 0.
    #[inline]
    pub(crate) fn advance(&mut self, target: f32) -> f32 {
        self.smoothed += self.coeff * (target - self.smoothed);
        self.smoothed
    }

    /// True once a ramp has fully landed on `target` (the next step is a no-op).
    #[inline]
    pub(crate) fn is_settled_at(&self, target: f32) -> bool {
        self.smoothed == target
    }

    /// One sample of a decibel ramp. Once settled, returns `target_lin` exactly
    /// so a resting fader stays bit-identical.
    #[inline]
    pub(crate) fn advance_db(&mut self, target_lin: f32) -> f32 {
        let target_lin = target_lin.max(0.0);
        let target_db = lin_to_db(target_lin);
        if (self.db - target_db).abs() < 0.01 {
            self.db = target_db;
            self.smoothed = target_lin;
            return target_lin;
        }
        self.db += self.coeff * (target_db - self.db);
        let gain = db_to_lin(self.db);
        self.smoothed = gain;
        gain
    }

    /// Frame-outer so the smoothed gain advances once per frame, shared across
    /// channels.  At target (static gain, no modulation) the step is exactly 0,
    /// so an unmodulated render stays bit-identical.
    pub(crate) fn process(
        &mut self,
        gain: f32,
        input: &AudioBuffer,
        output: &mut AudioBuffer,
        frames: usize,
    ) {
        let channels = output.num_channels.min(input.num_channels);
        let mut frame = 0;
        for k in 0..=self.point_count {
            let segment_end = if k < self.point_count {
                (self.point_offsets[k] as usize).min(frames)
            } else {
                frames
            };
            if segment_end <= frame {
                continue;
            }
            let delta = if k == 0 {
                self.base_delta
            } else {
                self.point_deltas[k - 1]
            };
            let target = (gain + delta).max(0.0);
            for i in frame..segment_end {
                self.smoothed += self.coeff * (target - self.smoothed);
                for ch in 0..channels {
                    output.channel_mut(ch)[i] = input.channel(ch)[i] * self.smoothed;
                }
            }
            frame = segment_end;
        }
    }

    /// Same as `process`, but the base gain is a fader smoothed in dB.  The
    /// modulation ramp keeps its ADR-019 speed; with the fader at rest
    /// `advance_db` returns `fader_target` exactly, so this is `process` bit for bit.
    pub(crate) fn process_over_fader(
        &mut self,
        fader: &mut SmoothedGain,
        fader_target: f32,
        input: &AudioBuffer,
        output: &mut AudioBuffer,
        frames: usize,
    ) {
        let channels = output.num_channels.min(input.num_channels);
        let mut frame = 0;
        for k in 0..=self.point_count {
            let segment_end = if k < self.point_count {
                (self.point_offsets[k] as usize).min(frames)
            } else {
                frames
            };
            if segment_end <= frame {
                continue;
            }
            let delta = if k == 0 {
                self.base_delta
            } else {
                self.point_deltas[k - 1]
            };
            for i in frame..segment_end {
                let target = (fader.advance_db(fader_target) + delta).max(0.0);
                self.smoothed += self.coeff * (target - self.smoothed);
                for ch in 0..channels {
                    output.channel_mut(ch)[i] = input.channel(ch)[i] * self.smoothed;
                }
            }
            frame = segment_end;
        }
    }
}

fn lin_to_db(gain: f32) -> f32 {
    if gain <= 1.0e-8 {
        -160.0
    } else {
        20.0 * gain.log10()
    }
}

fn db_to_lin(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

pub struct GainNode {
    gain: f32,
    smoother: SmoothedGain,
}

impl GainNode {
    pub fn new(gain: f32) -> Self {
        Self {
            gain,
            smoother: SmoothedGain::new(gain),
        }
    }
}

impl AudioNode for GainNode {
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
        self.smoother.prepare(ctx.sample_rate, self.gain);
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        let input = inputs[0];
        let output = &mut outputs[0];
        let frames = transport
            .block_size
            .min(output.num_frames)
            .min(input.num_frames);
        self.smoother.process(self.gain, input, output, frames);
    }

    fn reset(&mut self) {
        self.reset_modulation();
        self.smoother.snap_to(self.gain);
    }

    fn reset_modulation(&mut self) {
        self.smoother.clear_modulation();
    }

    fn param_id(&self, name: &str) -> Option<u32> {
        match name {
            "gain" => Some(PARAM_GAIN),
            _ => None,
        }
    }

    fn apply_modulation(&mut self, param_id: u32, delta: f32) {
        if param_id == PARAM_GAIN {
            self.smoother.add_block_start_delta(delta);
        }
    }

    fn apply_modulation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        if param_id == PARAM_GAIN {
            self.smoother.add_delta_at(sample_offset, value);
        }
    }
}

pub(crate) const PARAM_GAIN: u32 = 0;

#[cfg(test)]
mod tests {
    use super::*;

    fn transport(block: usize) -> TransportInfo {
        TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: block,
        }
    }

    fn ctx(block: usize) -> PrepareContext {
        PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: block,
            num_channels: 1,
            live: true,
        }
    }

    #[test]
    fn static_gain_is_bit_identical() {
        let mut node = GainNode::new(0.5);
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
        let mut node = GainNode::new(1.0);
        node.prepare(ctx(64));
        node.apply_modulation(PARAM_GAIN, 1.0); // target gain 2.0
        let mut input = AudioBuffer::new(1, 64);
        input.channel_mut(0).copy_from_slice(&[1.0; 64]);
        let mut outputs = [AudioBuffer::new(1, 64)];
        node.process(&[&input], &mut outputs, &transport(64));
        let out = outputs[0].channel(0);
        assert!(
            out[0] > 1.0 && out[0] < 2.0,
            "first sample must be a partial step, got {}",
            out[0]
        );
        assert!(
            out[63] > out[0],
            "gain must ramp upward toward the modulated target"
        );
        assert!(
            out[63] < 2.0,
            "13 ms smoother must not reach target within ~1.3 ms"
        );
    }
}
