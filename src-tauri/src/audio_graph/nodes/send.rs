//! Send node — taps a signal at a given gain and forwards it to a return bus.
//!
//! In the current implementation, the send is implemented as a side-effect-
//! free gain node.  Actual cross-routing to return buses requires a shared
//! ring-buffer mechanism (future work).  The interface is stable so callers
//! do not need to change when the full implementation lands.

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

pub struct SendNode {
    id: NodeId,
    gain: f32,
}

impl SendNode {
    pub fn new(id: NodeId, gain: f32) -> Self {
        Self { id, gain }
    }
}

impl AudioNode for SendNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "SendNode"
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
        let channels = output.num_channels.min(input.num_channels);
        let frames = transport.block_size.min(output.num_frames).min(input.num_frames);
        for ch in 0..channels {
            let in_ch = input.channel(ch);
            let out_ch = output.channel_mut(ch);
            for i in 0..frames {
                out_ch[i] = in_ch[i] * self.gain;
            }
        }
    }

    fn reset(&mut self) {}
}
