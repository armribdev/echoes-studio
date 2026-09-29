//! Bus node — sums multiple input signals into one output.
//!
//! Accepts up to `num_inputs` active connections and mixes them at unity gain.
//! Used for group buses, master bus, and return channels.

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};

pub struct BusNode {
    num_inputs: usize,
}

impl BusNode {
    pub fn new(num_inputs: usize) -> Self {
        Self {
            num_inputs: num_inputs.max(1),
        }
    }
}

impl AudioNode for BusNode {
    fn label(&self) -> &str {
        "BusNode"
    }
    fn num_input_ports(&self) -> usize {
        self.num_inputs
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, _ctx: PrepareContext) {}

    /// Sum all connected input ports into the output.
    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        let output = &mut outputs[0];
        output.clear();

        for input in inputs {
            let channels = output.num_channels.min(input.num_channels);
            let frames = transport
                .block_size
                .min(output.num_frames)
                .min(input.num_frames);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::node::TimeSignature;

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
    fn sums_all_connected_inputs_at_unity_gain() {
        let mut node = BusNode::new(2);
        let mut a = AudioBuffer::new(1, 3);
        a.channel_mut(0).copy_from_slice(&[1.0, 2.0, 3.0]);
        let mut b = AudioBuffer::new(1, 3);
        b.channel_mut(0).copy_from_slice(&[0.5, 0.5, 0.5]);
        let mut output = [AudioBuffer::new(1, 3)];

        node.process(&[&a, &b], &mut output, &transport(3));

        assert_eq!(output[0].channel(0), &[1.5, 2.5, 3.5]);
    }

    #[test]
    fn clears_output_before_summing_so_stale_samples_do_not_leak() {
        let mut node = BusNode::new(1);
        let input = AudioBuffer::new(1, 2);
        let mut output = [AudioBuffer::new(1, 2)];
        output[0].channel_mut(0).copy_from_slice(&[9.0, 9.0]);

        node.process(&[&input], &mut output, &transport(2));

        assert_eq!(output[0].channel(0), &[0.0, 0.0]);
    }

    #[test]
    fn num_inputs_is_never_zero() {
        let node = BusNode::new(0);
        assert_eq!(node.num_input_ports(), 1);
    }
}
