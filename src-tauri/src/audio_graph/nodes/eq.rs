use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::project::EqParams;

// Direct Form II Transposed biquad — no allocation per sample.
// RBJ Audio EQ Cookbook: S = 1 → steepest monotonically increasing/decreasing shelf.
const DEFAULT_SHELF_SLOPE: f32 = 1.0;

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32, b1: f32, b2: f32,
    a1: f32, a2: f32,
    s1: [f32; 2],
    s2: [f32; 2],
}

impl Biquad {
    fn process(&mut self, x: f32, ch: usize) -> f32 {
        let y = self.b0 * x + self.s1[ch];
        self.s1[ch] = self.b1 * x - self.a1 * y + self.s2[ch];
        self.s2[ch] = self.b2 * x - self.a2 * y;
        y
    }

    fn reset(&mut self) { self.s1 = [0.0; 2]; self.s2 = [0.0; 2]; }

    /// RBJ shelving alpha from shelf slope S (see Audio EQ Cookbook, case S).
    fn shelf_alpha(w: f32, a: f32, shelf_slope: f32) -> f32 {
        let inv_s = 1.0 / shelf_slope;
        w.sin() / 2.0 * ((a + 1.0 / a) * (inv_s - 1.0) + 2.0).sqrt()
    }

    fn low_shelf(freq: f32, gain_db: f32, sr: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w = 2.0 * std::f32::consts::PI * freq / sr;
        let cos_w = w.cos();
        let alpha = Self::shelf_alpha(w, a, DEFAULT_SHELF_SLOPE);
        let a_sqrt = a.sqrt();
        let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w + 2.0 * a_sqrt * alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w - 2.0 * a_sqrt * alpha);
        let a0 = (a + 1.0) + (a - 1.0) * cos_w + 2.0 * a_sqrt * alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w);
        let a2 = (a + 1.0) + (a - 1.0) * cos_w - 2.0 * a_sqrt * alpha;
        Biquad { b0: b0/a0, b1: b1/a0, b2: b2/a0, a1: a1/a0, a2: a2/a0, s1: [0.0;2], s2: [0.0;2] }
    }

    fn peaking(freq: f32, gain_db: f32, q: f32, sr: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w = 2.0 * std::f32::consts::PI * freq / sr;
        let alpha = w.sin() / (2.0 * q);
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * w.cos();
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * w.cos();
        let a2 = 1.0 - alpha / a;
        Biquad { b0: b0/a0, b1: b1/a0, b2: b2/a0, a1: a1/a0, a2: a2/a0, s1: [0.0;2], s2: [0.0;2] }
    }

    fn high_shelf(freq: f32, gain_db: f32, sr: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w = 2.0 * std::f32::consts::PI * freq / sr;
        let cos_w = w.cos();
        let alpha = Self::shelf_alpha(w, a, DEFAULT_SHELF_SLOPE);
        let a_sqrt = a.sqrt();
        let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w + 2.0 * a_sqrt * alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w - 2.0 * a_sqrt * alpha);
        let a0 = (a + 1.0) - (a - 1.0) * cos_w + 2.0 * a_sqrt * alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w);
        let a2 = (a + 1.0) - (a - 1.0) * cos_w - 2.0 * a_sqrt * alpha;
        Biquad { b0: b0/a0, b1: b1/a0, b2: b2/a0, a1: a1/a0, a2: a2/a0, s1: [0.0;2], s2: [0.0;2] }
    }
}

pub struct EqNode {
    id: NodeId,
    params: EqParams,
    low: Biquad,
    mid: Biquad,
    high: Biquad,
}

impl EqNode {
    pub fn new(id: NodeId, params: EqParams) -> Self {
        EqNode { id, params, low: Biquad::default(), mid: Biquad::default(), high: Biquad::default() }
    }
}

impl AudioNode for EqNode {
    fn node_id(&self) -> NodeId { self.id }
    fn label(&self) -> &str { "EqNode" }
    fn num_input_ports(&self) -> usize { 1 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, ctx: PrepareContext) {
        let sr = ctx.sample_rate;
        self.low  = Biquad::low_shelf(self.params.low_freq, self.params.low_gain_db, sr);
        self.mid  = Biquad::peaking(self.params.mid_freq, self.params.mid_gain_db, self.params.mid_q, sr);
        self.high = Biquad::high_shelf(self.params.high_freq, self.params.high_gain_db, sr);
    }

    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], t: &TransportInfo) {
        let input = &inputs[0];
        let output = &mut outputs[0];
        let channels = output.num_channels.min(input.num_channels).min(2);
        let frames = t.block_size.min(output.num_frames).min(input.num_frames);
        for ch in 0..channels {
            let in_ch = input.channel(ch);
            let out_ch = output.channel_mut(ch);
            for i in 0..frames {
                let s = self.low.process(in_ch[i], ch);
                let s = self.mid.process(s, ch);
                out_ch[i] = self.high.process(s, ch);
            }
        }
    }

    fn reset(&mut self) { self.low.reset(); self.mid.reset(); self.high.reset(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn biquad_mag_at_hz(b: &Biquad, freq_hz: f32, sr: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * freq_hz / sr;
        let cos_w = w.cos();
        let sin_w = w.sin();
        let num_re = b.b0 + b.b1 * cos_w + b.b2 * (2.0 * cos_w * cos_w - 1.0);
        let num_im = -b.b1 * sin_w - b.b2 * 2.0 * cos_w * sin_w;
        let den_re = 1.0 + b.a1 * cos_w + b.a2 * (2.0 * cos_w * cos_w - 1.0);
        let den_im = -b.a1 * sin_w - b.a2 * 2.0 * cos_w * sin_w;
        (num_re * num_re + num_im * num_im).sqrt() / (den_re * den_re + den_im * den_im).sqrt()
    }

    fn db(x: f32) -> f32 { 20.0 * x.log10() }

    #[test]
    fn shelf_alpha_depends_on_slope() {
        let w = 0.5;
        let a = 2.0;
        let alpha_s1 = Biquad::shelf_alpha(w, a, 1.0);
        let alpha_s2 = Biquad::shelf_alpha(w, a, 2.0);
        assert!((alpha_s1 - alpha_s2).abs() > 1e-6, "S must affect alpha");
    }

    #[test]
    fn low_shelf_dc_gain_matches_rbj() {
        let sr = 48_000.0;
        let gain_db = 12.0;
        let b = Biquad::low_shelf(250.0, gain_db, sr);
        let mag_db = db(biquad_mag_at_hz(&b, 0.0, sr));
        assert!((mag_db - gain_db).abs() < 0.5, "DC gain {mag_db} dB, expected {gain_db} dB");
    }

    #[test]
    fn low_shelf_half_gain_at_corner_frequency() {
        let sr = 48_000.0;
        let gain_db = 12.0;
        let freq = 250.0;
        let b = Biquad::low_shelf(freq, gain_db, sr);
        let mag_db = db(biquad_mag_at_hz(&b, freq, sr));
        assert!(
            (mag_db - gain_db / 2.0).abs() < 0.5,
            "corner gain {mag_db} dB, expected {} dB",
            gain_db / 2.0
        );
    }
    #[test]
    fn high_shelf_nyquist_gain_matches_rbj() {
        let sr = 48_000.0;
        let gain_db = 12.0;
        let b = Biquad::high_shelf(4000.0, gain_db, sr);
        let mag_db = db(biquad_mag_at_hz(&b, sr * 0.5, sr));
        assert!((mag_db - gain_db).abs() < 0.5, "Nyquist gain {mag_db} dB, expected {gain_db} dB");
    }
}
