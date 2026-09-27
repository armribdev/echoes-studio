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

    #[test]
    fn scales_signal_by_send_gain() {
        let mut node = SendNode::new(NodeId(1), 0.25);
        let mut input = AudioBuffer::new(1, 3);
        input.channel_mut(0).copy_from_slice(&[1.0, -2.0, 4.0]);
        let mut output = [AudioBuffer::new(1, 3)];

        node.process(&[&input], &mut output, &transport(3));

        assert_eq!(output[0].channel(0), &[0.25, -0.5, 1.0]);
    }

    #[test]
    fn zero_gain_produces_silence() {
        let mut node = SendNode::new(NodeId(1), 0.0);
        let mut input = AudioBuffer::new(1, 2);
        input.channel_mut(0).copy_from_slice(&[1.0, 1.0]);
        let mut output = [AudioBuffer::new(1, 2)];

        node.process(&[&input], &mut output, &transport(2));

        assert!(output[0].is_silent());
    }
}
