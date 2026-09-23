//! Bypass node — keeps a disabled effect alive while its audio passes through.
//!
//! A disabled plugin must keep its instance (and so its settings and editor):
//! destroying it on bypass lost the user's preset (ADR-020).  The wrapped node
//! is prepared like any other so the plugin stays active, but its `process` is
//! never called.

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

pub struct BypassNode {
    inner: Box<dyn AudioNode>,
}

impl BypassNode {
    pub fn new(inner: Box<dyn AudioNode>) -> Self {
        Self { inner }
    }
}

impl AudioNode for BypassNode {
    fn node_id(&self) -> NodeId {
        self.inner.node_id()
    }

    fn label(&self) -> &str {
        "BypassNode"
    }

    fn num_input_ports(&self) -> usize {
        self.inner.num_input_ports()
    }

    fn num_output_ports(&self) -> usize {
        self.inner.num_output_ports()
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.inner.prepare(ctx);
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    /// Main input → main output; aux inputs (sidechain) are ignored.
    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], transport: &TransportInfo) {
        let (Some(input), Some(output)) = (inputs.first(), outputs.first_mut()) else {
            return;
        };
        let channels = output.num_channels.min(input.num_channels);
        let frames = transport.block_size.min(output.num_frames).min(input.num_frames);
        for ch in 0..channels {
            output.channel_mut(ch)[..frames].copy_from_slice(&input.channel(ch)[..frames]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Inverter;

    impl AudioNode for Inverter {
        fn node_id(&self) -> NodeId {
            NodeId(7)
        }
        fn num_input_ports(&self) -> usize {
            2
        }
        fn num_output_ports(&self) -> usize {
            1
        }
        fn prepare(&mut self, _ctx: PrepareContext) {}
        fn reset(&mut self) {}
        fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], _t: &TransportInfo) {
            for (o, i) in outputs[0].channel_mut(0).iter_mut().zip(inputs[0].channel(0)) {
                *o = -*i;
            }
        }
    }

    #[test]
    fn passes_audio_through_without_processing() {
        let mut node = BypassNode::new(Box::new(Inverter));
        assert_eq!(node.node_id(), NodeId(7));
        assert_eq!(node.num_input_ports(), 2, "keeps the wrapped node's ports (sidechain)");

        let mut input = AudioBuffer::new(2, 4);
        input.channel_mut(0).copy_from_slice(&[0.1, 0.2, 0.3, 0.4]);
        input.channel_mut(1).copy_from_slice(&[0.5, 0.6, 0.7, 0.8]);
        let mut outputs = [AudioBuffer::new(2, 4)];
        let mut transport = TransportInfo::stopped(48_000.0);
        transport.block_size = 4;

        node.process(&[&input], &mut outputs, &transport);

        assert_eq!(outputs[0].channel(0), input.channel(0));
        assert_eq!(outputs[0].channel(1), input.channel(1));
    }
}
