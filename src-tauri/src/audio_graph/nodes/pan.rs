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
