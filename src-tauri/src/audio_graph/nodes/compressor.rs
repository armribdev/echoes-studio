use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::project::CompressorParams;

pub struct CompressorNode {
    id: NodeId,
    params: CompressorParams,
    envelope: f32,
    attack_coeff: f32,
    release_coeff: f32,
    makeup_linear: f32,
}

impl CompressorNode {
    pub fn new(id: NodeId, params: CompressorParams) -> Self {
        CompressorNode {
            id, params,
            envelope: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            makeup_linear: 1.0,
        }
    }

    fn compute_gain_db(&self, input_db: f32) -> f32 {
        let thr = self.params.threshold_db;
        let ratio = self.params.ratio;
        let knee = self.params.knee_db;
        let half_knee = knee / 2.0;
        if input_db <= thr - half_knee {
            0.0
        } else if input_db < thr + half_knee && knee > 0.0 {
            let x = input_db - thr + half_knee;
            (1.0 / ratio - 1.0) * x * x / (2.0 * knee)
        } else {
            thr + (input_db - thr) / ratio - input_db
        }
    }
}

impl AudioNode for CompressorNode {
    fn node_id(&self) -> NodeId { self.id }
    fn label(&self) -> &str { "CompressorNode" }
    fn num_input_ports(&self) -> usize { 1 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, ctx: PrepareContext) {
        let sr = ctx.sample_rate;
        self.attack_coeff  = (-1.0 / (self.params.attack_ms  * 0.001 * sr)).exp();
        self.release_coeff = (-1.0 / (self.params.release_ms * 0.001 * sr)).exp();
        self.makeup_linear = 10.0_f32.powf(self.params.makeup_db / 20.0);
    }

    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], t: &TransportInfo) {
        let input = &inputs[0];
        let output = &mut outputs[0];
        let frames = t.block_size.min(output.num_frames).min(input.num_frames);
        let channels = output.num_channels.min(input.num_channels);

        for i in 0..frames {
            // Peak detection across channels
            let mut peak = 0.0_f32;
            for ch in 0..channels {
                peak = peak.max(input.channel(ch)[i].abs());
            }
            let peak_db = if peak > 1e-6 { 20.0 * peak.log10() } else { -120.0 };
            let gain_db = self.compute_gain_db(peak_db);
            let gain_linear = 10.0_f32.powf(gain_db / 20.0);

            // RC envelope on gain
            let coeff = if gain_linear < self.envelope { self.attack_coeff } else { self.release_coeff };
            self.envelope = coeff * self.envelope + (1.0 - coeff) * gain_linear;

            let total_gain = self.envelope * self.makeup_linear;
            for ch in 0..channels {
                output.channel_mut(ch)[i] = input.channel(ch)[i] * total_gain;
            }
        }
    }

    fn reset(&mut self) { self.envelope = 0.0; }
}
