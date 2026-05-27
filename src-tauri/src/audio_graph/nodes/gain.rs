//! Gain node — applies a linear gain multiplier to its input.
//!
//! Used for both track volume faders and explicit gain stages.
//! Handles stereo (and multichannel) transparently.

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

pub struct GainNode {
    id: NodeId,
    gain: f32,
}

impl GainNode {
    pub fn new(id: NodeId, gain: f32) -> Self {
        GainNode { id, gain }
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }
}

impl AudioNode for GainNode {
    fn node_id(&self) -> NodeId { self.id }
    fn label(&self) -> &str { "GainNode" }
    fn num_input_ports(&self) -> usize { 1 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, _ctx: PrepareContext) {}

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
        let input = &inputs[0];
        let output = &mut outputs[0];

        let channels = output.num_channels.min(input.num_channels);
        let frames = output.num_frames.min(input.num_frames);
        let gain = self.gain;

        for ch in 0..channels {
            let in_ch = input.channel(ch);
            let out_ch = output.channel_mut(ch);
            for i in 0..frames {
                out_ch[i] = in_ch[i] * gain;
            }
        }
    }

    fn reset(&mut self) {}
}
