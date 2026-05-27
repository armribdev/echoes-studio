//! Bus node — sums multiple input signals into one output.
//!
//! Accepts up to `num_inputs` active connections and mixes them at unity gain.
//! Used for group buses, master bus, and return channels.

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

pub struct BusNode {
    id: NodeId,
    num_inputs: usize,
}

impl BusNode {
    pub fn new(id: NodeId, num_inputs: usize) -> Self {
        BusNode { id, num_inputs: num_inputs.max(1) }
    }
}

impl AudioNode for BusNode {
    fn node_id(&self) -> NodeId { self.id }
    fn label(&self) -> &str { "BusNode" }
    fn num_input_ports(&self) -> usize { self.num_inputs }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, _ctx: PrepareContext) {}

    /// Sum all connected input ports into the output.
    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
        let output = &mut outputs[0];
        output.clear();

        for input in inputs {
            let channels = output.num_channels.min(input.num_channels);
            let frames = output.num_frames.min(input.num_frames);
            for ch in 0..channels {
                let in_ch = input.channel(ch);
                let out_ch = output.channel_mut(ch);
                for i in 0..frames {
                    out_ch[i] += in_ch[i];
                }
            }
        }
    }

    fn reset(&mut self) {}
}
