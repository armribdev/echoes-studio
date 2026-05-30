use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::denormal::ANTI_DENORMAL;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::project::DelayParams;

pub struct DelayNode {
    id: NodeId,
    params: DelayParams,
    // Interleaved stereo ring buffer: [L0, R0, L1, R1, ...]
    buf: Vec<f32>,
    write_pos: usize,
    delay_samples: usize,
}

impl DelayNode {
    pub fn new(id: NodeId, params: DelayParams) -> Self {
        DelayNode { id, params, buf: Vec::new(), write_pos: 0, delay_samples: 0 }
    }
}

impl AudioNode for DelayNode {
    fn node_id(&self) -> NodeId { self.id }
    fn label(&self) -> &str { "DelayNode" }
    fn num_input_ports(&self) -> usize { 1 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, ctx: PrepareContext) {
        // Allocate 2 seconds of headroom at 2 channels
        let max_samples = (ctx.sample_rate * 2.0) as usize;
        self.buf = vec![0.0; max_samples * 2];
        self.delay_samples = (self.params.delay_ms * 0.001 * ctx.sample_rate) as usize;
        self.write_pos = 0;
    }

    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], t: &TransportInfo) {
        let input = &inputs[0];
        let output = &mut outputs[0];
        let frames = t.block_size.min(output.num_frames).min(input.num_frames);
        let channels = output.num_channels.min(input.num_channels);
        let cap = self.buf.len() / 2; // capacity in stereo frames
        if cap == 0 { return; }

        let wet = self.params.wet;
        let dry = 1.0 - wet;
        let feedback = self.params.feedback;
        let ping_pong = self.params.ping_pong;

        for i in 0..frames {
            // Read from delay line
            let read_pos = (self.write_pos + cap - self.delay_samples.min(cap - 1)) % cap;
            let del_l = self.buf[read_pos * 2];
            let del_r = self.buf[read_pos * 2 + 1];

            let in_l = if channels > 0 { input.channel(0)[i] } else { 0.0 };
            let in_r = if channels > 1 { input.channel(1)[i] } else { in_l };

            // Write to delay buffer: feedback from delay out, cross-channel if ping-pong.
            // ANTI_DENORMAL on each write keeps the recursive state out of the
            // sub-normal range during long silences (otherwise: x86 CPU spike).
            if ping_pong {
                self.buf[self.write_pos * 2]     = in_l + del_r * feedback + ANTI_DENORMAL;
                self.buf[self.write_pos * 2 + 1] = in_r + del_l * feedback + ANTI_DENORMAL;
            } else {
                self.buf[self.write_pos * 2]     = in_l + del_l * feedback + ANTI_DENORMAL;
                self.buf[self.write_pos * 2 + 1] = in_r + del_r * feedback + ANTI_DENORMAL;
            }

            self.write_pos = (self.write_pos + 1) % cap;

            if channels > 0 { output.channel_mut(0)[i] = in_l * dry + del_l * wet; }
            if channels > 1 { output.channel_mut(1)[i] = in_r * dry + del_r * wet; }
        }
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write_pos = 0;
    }
}
