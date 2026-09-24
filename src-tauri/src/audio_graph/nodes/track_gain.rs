//! Track gain node — applies volume + mute/solo gating for one track.
//!
//! ## Why it differs from `GainNode`
//!
//! `GainNode` holds a constant baked at compile time.  Turning a volume knob
//! therefore required a full graph recompile (reallocating reverbs, resetting
//! envelopes, cloning every clip buffer — see audit C3 / I1).
//!
//! `TrackGainNode` reads its parameters from `Arc<TrackParams>` cells held in
//! the `LiveParams` registry.  Volume, mute, solo, and the global `any_solo`
//! flag are loaded once per block and used to derive the effective linear
//! gain.  The command thread can write these atomics directly without
//! triggering a recompile.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::live_params::TrackParams;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::audio_graph::nodes::gain::{SmoothedGain, FADER_SMOOTH_CUTOFF_HZ, PARAM_GAIN};

pub struct TrackGainNode {
    id: NodeId,
    params: Arc<TrackParams>,
    any_solo: Arc<AtomicBool>,
    /// Fader, mute and solo: slow ramp in dB so a fast move does not click.
    fader: SmoothedGain,
    /// Volume modulation/automation on top of the fader, at ADR-019 speed.
    smoother: SmoothedGain,
    /// Equal-power balance, smoothed so a pan move does not step the signal.
    pan_l: SmoothedGain,
    pan_r: SmoothedGain,
}

impl TrackGainNode {
    pub fn new(id: NodeId, params: Arc<TrackParams>, any_solo: Arc<AtomicBool>) -> Self {
        let mut node = Self {
            id,
            params,
            any_solo,
            fader: SmoothedGain::new(0.0),
            smoother: SmoothedGain::new(0.0),
            pan_l: SmoothedGain::new(1.0),
            pan_r: SmoothedGain::new(1.0),
        };
        let gain = node.current_gain();
        node.fader.snap_to(gain);
        node.smoother.snap_to(gain);
        let (left, right) = balance_gains(node.params.pan());
        node.pan_l.snap_to(left);
        node.pan_r.snap_to(right);
        node
    }

    /// Compute the linear gain for this block from the atomic cells.
    ///
    /// Logic: `volume × mute_gate × solo_gate`.
    /// - mute_gate = 0 if muted, else 1.
    /// - solo_gate = 1 if no track is solo'd, or this track is solo'd; else 0.
    #[inline]
    fn current_gain(&self) -> f32 {
        let volume = self.params.volume();
        if self.params.muted() {
            return 0.0;
        }
        if self.any_solo.load(Ordering::Relaxed) && !self.params.solo() {
            return 0.0;
        }
        volume
    }
}

/// Stereo balance, unity at centre. `pan` is −1…+1.
fn balance_gains(pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    (1.0 - pan.max(0.0), 1.0 + pan.min(0.0))
}

impl AudioNode for TrackGainNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "TrackGainNode"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        let gain = self.current_gain();
        self.smoother.prepare(ctx.sample_rate, gain);
        self.fader.prepare(ctx.sample_rate, gain);
        self.fader.set_cutoff(ctx.sample_rate, FADER_SMOOTH_CUTOFF_HZ);
        let (left, right) = balance_gains(self.params.pan());
        self.pan_l.prepare(ctx.sample_rate, left);
        self.pan_r.prepare(ctx.sample_rate, right);
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        let input = inputs[0];
        let output = &mut outputs[0];
        let frames = transport.block_size.min(output.num_frames).min(input.num_frames);
        let gain = self.current_gain();
        self.smoother.process_over_fader(&mut self.fader, gain, input, output, frames);
        let (target_l, target_r) = balance_gains(self.params.pan());
        if output.num_channels >= 2 {
            let (out_l, out_r) = output.two_channels_mut(0, 1);
            for i in 0..frames {
                let gain_l = self.pan_l.advance(target_l);
                let gain_r = self.pan_r.advance(target_r);
                out_l[i] *= gain_l;
                out_r[i] *= gain_r;
            }
        }
    }

    fn reset(&mut self) {
        self.reset_modulation();
        let gain = self.current_gain();
        self.fader.snap_to(gain);
        self.smoother.snap_to(gain);
        let (left, right) = balance_gains(self.params.pan());
        self.pan_l.snap_to(left);
        self.pan_r.snap_to(right);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::buffer::AudioBuffer;
    use crate::audio_graph::live_params::LiveParams;
    use crate::audio_graph::node::{NodeId, PrepareContext, TimeSignature, TransportInfo};

    fn transport(block: usize) -> TransportInfo {
        TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: block,
        }
    }

    const FRAMES_13_MS: usize = 624;

    fn node_at_unity(live: &LiveParams) -> TrackGainNode {
        let cells = live.get_or_init_track("t", 1.0, 0.0, false, false);
        let mut node = TrackGainNode::new(NodeId(1), cells, Arc::clone(&live.any_solo));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: FRAMES_13_MS,
            num_channels: 1,
        });
        node
    }

    fn render_ones(node: &mut TrackGainNode) -> Vec<f32> {
        let mut input = AudioBuffer::new(1, FRAMES_13_MS);
        input.channel_mut(0).fill(1.0);
        let mut output = [AudioBuffer::new(1, FRAMES_13_MS)];
        node.process(&[&input], &mut output, &transport(FRAMES_13_MS));
        output[0].channel(0).to_vec()
    }

    #[test]
    fn volume_modulation_keeps_modulation_speed() {
        let live = LiveParams::new();
        let mut node = node_at_unity(&live);
        node.apply_modulation(PARAM_GAIN, 1.0);

        let out = render_ones(&mut node);

        // One 12 Hz time constant covers ~63 % of the step; the 4 Hz fader ramp would reach ~22 %.
        let last = out[FRAMES_13_MS - 1];
        assert!(last > 1.55, "modulation step from 1.0 towards 2.0 reached only {last} after 13 ms");
    }

    #[test]
    fn fader_move_ramps_slowly() {
        let live = LiveParams::new();
        let mut node = node_at_unity(&live);
        live.set_track_volume("t", 0.0);

        let out = render_ones(&mut node);

        assert!(out[0] > 0.999, "fader move stepped the signal to {}", out[0]);
        assert!(out[FRAMES_13_MS - 1] > 0.3, "fader ramp is too fast: {}", out[FRAMES_13_MS - 1]);
    }

    #[test]
    fn pan_step_ramps_instead_of_jumping() {
        let live = LiveParams::new();
        let cells = live.get_or_init_track("t", 1.0, 0.0, false, false);
        let mut node = TrackGainNode::new(NodeId(1), cells, Arc::clone(&live.any_solo));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 8,
            num_channels: 2,
        });
        live.set_track_pan("t", 1.0);

        let mut input = AudioBuffer::new(2, 8);
        for i in 0..8 {
            input.channel_mut(0)[i] = 1.0;
            input.channel_mut(1)[i] = 1.0;
        }
        let mut output = [AudioBuffer::new(2, 8)];
        node.process(&[&input], &mut output, &transport(8));

        let left = output[0].channel(0)[0];
        assert!(left > 0.99, "first sample jumped to {left}");
    }
}
