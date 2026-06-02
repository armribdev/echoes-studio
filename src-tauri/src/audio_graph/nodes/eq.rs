use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::audio_graph::oversampling::Oversample2x;
use crate::audio_graph::parameter_block::ParameterBlock;
use crate::project::EqParams;

// ── Hot-parameter block layout (ADR-009) ──────────────────────────────────────
//
// Cell order for the EQ's `ParameterBlock`.  Shared by the compiler (which
// seeds the block), the gateway (which writes cells on a knob turn via
// `eq_block_values`), and `EqNode` (which reads them per block).  Values are
// stored in natural units (Hz, dB, Q).
pub const EQ_PARAM_COUNT: usize = 7;
pub const EQ_LOW_GAIN_DB: usize = 0;
pub const EQ_LOW_FREQ: usize = 1;
pub const EQ_MID_GAIN_DB: usize = 2;
pub const EQ_MID_FREQ: usize = 3;
pub const EQ_MID_Q: usize = 4;
pub const EQ_HIGH_GAIN_DB: usize = 5;
pub const EQ_HIGH_FREQ: usize = 6;

/// Flatten `EqParams` into the `ParameterBlock` cell order above.
pub fn eq_block_values(p: &EqParams) -> [f32; EQ_PARAM_COUNT] {
    [
        p.low_gain_db,
        p.low_freq,
        p.mid_gain_db,
        p.mid_freq,
        p.mid_q,
        p.high_gain_db,
        p.high_freq,
    ]
}

/// Read the block back into an `EqParams`.
fn eq_params_from_block(block: &ParameterBlock) -> EqParams {
    EqParams {
        low_gain_db: block.get(EQ_LOW_GAIN_DB),
        low_freq: block.get(EQ_LOW_FREQ),
        mid_gain_db: block.get(EQ_MID_GAIN_DB),
        mid_freq: block.get(EQ_MID_FREQ),
        mid_q: block.get(EQ_MID_Q),
        high_gain_db: block.get(EQ_HIGH_GAIN_DB),
        high_freq: block.get(EQ_HIGH_FREQ),
    }
}

// Direct Form II Transposed biquad — no allocation per sample.
// RBJ Audio EQ Cookbook: S = 1 → steepest monotonically increasing/decreasing shelf.
const DEFAULT_SHELF_SLOPE: f32 = 1.0;

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
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

    fn reset(&mut self) {
        self.s1 = [0.0; 2];
        self.s2 = [0.0; 2];
    }

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
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: [0.0; 2],
            s2: [0.0; 2],
        }
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
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: [0.0; 2],
            s2: [0.0; 2],
        }
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
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: [0.0; 2],
            s2: [0.0; 2],
        }
    }
}

pub struct EqNode {
    id: NodeId,
    /// Cached parameter values — the coefficients currently baked into the
    /// biquads.  Compared against the hot block each block to decide whether a
    /// recompute is needed.
    params: EqParams,
    low: Biquad,
    mid: Biquad,
    high: Biquad,
    oversample: Oversample2x,

    /// Oversampled sample rate (engine SR × 2), captured at `prepare`.
    sr_os: f32,

    /// Hot-parameter block (ADR-009).  When present, `process` reads parameter
    /// values from it each block and recomputes coefficients only when they
    /// change — so a knob turn never triggers a recompile.  `None` keeps the
    /// pre-ADR-009 behaviour (coefficients frozen at `prepare`).
    block: Option<Arc<ParameterBlock>>,
}

impl EqNode {
    pub fn new(id: NodeId, params: EqParams) -> Self {
        EqNode {
            id,
            params,
            low: Biquad::default(),
            mid: Biquad::default(),
            high: Biquad::default(),
            oversample: Oversample2x::default(),
            sr_os: 96_000.0,
            block: None,
        }
    }

    /// Attach the hot-parameter block (ADR-009).  Builder-style; called by the
    /// compiler right after `new`.
    pub fn with_block(mut self, block: Arc<ParameterBlock>) -> Self {
        self.block = Some(block);
        self
    }

    /// Recompute the three biquad coefficient sets from `self.params`.
    /// Preserves the filter state (`s1`/`s2`) so a parameter change does not
    /// reset the running filters — only the coefficients move.
    fn recompute_coeffs(&mut self) {
        let s_low = self.low.s1;
        let s_low2 = self.low.s2;
        let s_mid = self.mid.s1;
        let s_mid2 = self.mid.s2;
        let s_high = self.high.s1;
        let s_high2 = self.high.s2;

        self.low = Biquad::low_shelf(self.params.low_freq, self.params.low_gain_db, self.sr_os);
        self.mid = Biquad::peaking(
            self.params.mid_freq,
            self.params.mid_gain_db,
            self.params.mid_q,
            self.sr_os,
        );
        self.high = Biquad::high_shelf(self.params.high_freq, self.params.high_gain_db, self.sr_os);

        self.low.s1 = s_low;
        self.low.s2 = s_low2;
        self.mid.s1 = s_mid;
        self.mid.s2 = s_mid2;
        self.high.s1 = s_high;
        self.high.s2 = s_high2;
    }
}

impl AudioNode for EqNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "EqNode"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.sr_os = ctx.sample_rate * 2.0;
        // If a hot block is attached, seed the cached params from it so the
        // coefficients reflect any edits made before this (re)compile.
        if let Some(block) = &self.block {
            self.params = eq_params_from_block(block);
        }
        self.recompute_coeffs();
        self.oversample.prepare(ctx.max_block_size, ctx.sample_rate);
    }

    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], t: &TransportInfo) {
        // ADR-009: pull live parameter values from the hot block and recompute
        // the biquads only when they actually changed.  No allocation, no lock:
        // `ParameterBlock::get` is one relaxed atomic load per parameter.
        let next = self.block.as_ref().map(|b| eq_params_from_block(b));
        if let Some(next) = next {
            if eq_block_values(&next) != eq_block_values(&self.params) {
                self.params = next;
                self.recompute_coeffs();
            }
        }

        let input = &inputs[0];
        let output = &mut outputs[0];
        let channels = output.num_channels.min(input.num_channels).min(2);
        let frames = t.block_size.min(output.num_frames).min(input.num_frames);
        let Self {
            low,
            mid,
            high,
            oversample,
            ..
        } = self;
        for ch in 0..channels {
            let in_ch = input.channel(ch);
            let out_ch = output.channel_mut(ch);
            oversample.process_channel(ch, in_ch, out_ch, frames, |x| {
                let s = low.process(x, ch);
                let s = mid.process(s, ch);
                high.process(s, ch)
            });
        }
    }

    fn reset(&mut self) {
        self.low.reset();
        self.mid.reset();
        self.high.reset();
        self.oversample.reset();
    }
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

    fn db(x: f32) -> f32 {
        20.0 * x.log10()
    }

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
        assert!(
            (mag_db - gain_db).abs() < 0.5,
            "DC gain {mag_db} dB, expected {gain_db} dB"
        );
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
    fn hot_block_changes_coefficients_without_prepare() {
        use crate::audio_graph::node::{AudioNode, PrepareContext};
        use crate::audio_graph::parameter_block::ParameterBlock;
        use std::sync::Arc;

        let params = EqParams::default();
        let block = ParameterBlock::new(&eq_block_values(&params));
        let mut node = EqNode::new(NodeId(1), params).with_block(Arc::clone(&block));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
        });

        // Baseline: flat EQ → low-shelf is unity-ish.
        let before = node.low.b0;

        // Write a big low-shelf boost into the block (cell order from constants).
        block.set(EQ_LOW_GAIN_DB, 12.0);

        // Run a block: the node must detect the change and recompute.
        let input = AudioBuffer::new(2, 64);
        let mut output = AudioBuffer::new(2, 64);
        let t = TransportInfo::stopped(48_000.0);
        node.process(&[&input], std::slice::from_mut(&mut output), &t);

        assert_ne!(
            node.low.b0, before,
            "EQ must recompute coefficients from the hot block on a live change"
        );
        assert_eq!(
            node.params.low_gain_db, 12.0,
            "cached params follow the block"
        );
    }

    #[test]
    fn high_shelf_nyquist_gain_matches_rbj() {
        let sr = 48_000.0;
        let gain_db = 12.0;
        let b = Biquad::high_shelf(4000.0, gain_db, sr);
        let mag_db = db(biquad_mag_at_hz(&b, sr * 0.5, sr));
        assert!(
            (mag_db - gain_db).abs() < 0.5,
            "Nyquist gain {mag_db} dB, expected {gain_db} dB"
        );
    }
}
