//! Stereo panner — equal-power pan law.
//!
//! `position` range: −1.0 (hard left) … 0.0 (centre) … +1.0 (hard right).
//!
//! Uses a constant-power (sin/cos) pan law so that the total power is
//! preserved when panning a mono signal in a stereo field.

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

const PAN_POSITION_MIN: f32 = -1.0;
const PAN_POSITION_MAX: f32 = 1.0;
const PAN_ANGLE_QUARTER_SCALE: f32 = 0.25;

pub struct PanNode {
    id: NodeId,
    position: f32,
    gain_l: f32,
    gain_r: f32,
}

impl PanNode {
    pub fn new(id: NodeId, position: f32) -> Self {
        let mut node = PanNode {
            id,
            position: 0.0,
            gain_l: 1.0,
            gain_r: 1.0,
        };
        node.set_position(position);
        node
    }

    pub fn set_position(&mut self, position: f32) {
        self.position = position.clamp(PAN_POSITION_MIN, PAN_POSITION_MAX);
        let angle =
            (self.position - PAN_POSITION_MIN) * PAN_ANGLE_QUARTER_SCALE * std::f32::consts::PI;
        self.gain_l = angle.cos();
        self.gain_r = angle.sin();
    }

    #[cfg(test)]
    pub fn position(&self) -> f32 {
        self.position
    }
}

impl AudioNode for PanNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "PanNode"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, _ctx: PrepareContext) {}

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        let input = &inputs[0];
        let output = &mut outputs[0];

        if output.num_channels < 2 {
            let frames = transport.block_size.min(output.num_frames).min(input.num_frames);
            let in_ch = input.channel(0);
            let out_ch = output.channel_mut(0);
            out_ch[..frames].copy_from_slice(&in_ch[..frames]);
            return;
        }

        let frames = transport.block_size.min(output.num_frames).min(input.num_frames);

        if input.num_channels >= 2 {
            let in_l = input.channel(0);
            let in_r = input.channel(1);
            let (out_l, out_r) = output.two_channels_mut(0, 1);
            for i in 0..frames {
                out_l[i] = in_l[i] * self.gain_l;
                out_r[i] = in_r[i] * self.gain_r;
            }
        } else {
            let in_ch = input.channel(0);
            let (out_l, out_r) = output.two_channels_mut(0, 1);
            for i in 0..frames {
                out_l[i] = in_ch[i] * self.gain_l;
                out_r[i] = in_ch[i] * self.gain_r;
            }
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::node::{NodeId, TimeSignature};

    fn transport(frames: usize) -> TransportInfo {
        TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: frames,
        }
    }

    fn stereo_from_mono(frames: usize, node: &mut PanNode) -> (Vec<f32>, Vec<f32>) {
        let mut input = AudioBuffer::new(1, frames);
        input.channel_mut(0).fill(1.0);
        let mut output = [AudioBuffer::new(2, frames)];
        node.process(&[&input], &mut output, &transport(frames));
        (output[0].channel(0).to_vec(), output[0].channel(1).to_vec())
    }

    #[test]
    fn centre_pan_is_equal_power_both_sides() {
        let mut node = PanNode::new(NodeId(1), 0.0);
        let (left, right) = stereo_from_mono(4, &mut node);
        let expected = std::f32::consts::FRAC_1_SQRT_2;
        for &s in &left {
            assert!((s - expected).abs() < 1e-6, "left {s} != {expected}");
        }
        for &s in &right {
            assert!((s - expected).abs() < 1e-6, "right {s} != {expected}");
        }
    }

    #[test]
    fn hard_left_silences_right_channel() {
        let mut node = PanNode::new(NodeId(1), -1.0);
        let (left, right) = stereo_from_mono(4, &mut node);
        assert!(left.iter().all(|&s| (s - 1.0).abs() < 1e-6));
        assert!(right.iter().all(|&s| s.abs() < 1e-6));
    }

    #[test]
    fn hard_right_silences_left_channel() {
        let mut node = PanNode::new(NodeId(1), 1.0);
        let (left, right) = stereo_from_mono(4, &mut node);
        assert!(left.iter().all(|&s| s.abs() < 1e-6));
        assert!(right.iter().all(|&s| (s - 1.0).abs() < 1e-6));
    }

    #[test]
    fn position_is_clamped_to_valid_range() {
        let node = PanNode::new(NodeId(1), 5.0);
        assert_eq!(node.position(), PAN_POSITION_MAX);
        let node = PanNode::new(NodeId(1), -5.0);
        assert_eq!(node.position(), PAN_POSITION_MIN);
    }

    #[test]
    fn mono_output_passes_input_through_unpanned() {
        let mut node = PanNode::new(NodeId(1), -1.0);
        let mut input = AudioBuffer::new(1, 3);
        input.channel_mut(0).copy_from_slice(&[1.0, 2.0, 3.0]);
        let mut output = [AudioBuffer::new(1, 3)];
        node.process(&[&input], &mut output, &transport(3));
        assert_eq!(output[0].channel(0), &[1.0, 2.0, 3.0]);
    }
}
