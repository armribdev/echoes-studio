use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo, MAX_CONTROL_POINTS};
use crate::audio_graph::oversampling::{Oversample2x, OVERSAMPLE_FACTOR};
use crate::audio_graph::nodes::modulated;
use crate::audio_graph::parameter_block::ParameterBlock;
use crate::project::EqParams;

// Cell order for `ParameterBlock` (ADR-009): shared by compiler, gateway, and `EqNode`.
// Values in natural units (Hz, dB, Q).
pub const EQ_PARAM_COUNT: usize = 7;
pub const EQ_LOW_GAIN_DB: usize = 0;
pub const EQ_LOW_FREQ: usize = 1;
pub const EQ_MID_GAIN_DB: usize = 2;
pub const EQ_MID_FREQ: usize = 3;
pub const EQ_MID_Q: usize = 4;
pub const EQ_HIGH_GAIN_DB: usize = 5;
pub const EQ_HIGH_FREQ: usize = 6;

// Modulation-only targets: the delta is in octaves around the frequency cell
// (ADR-019 amendment); `low_freq` etc. keep their additive Hz delta.
pub const EQ_LOW_FREQ_OCTAVES: usize = 7;
pub const EQ_MID_FREQ_OCTAVES: usize = 8;
pub const EQ_HIGH_FREQ_OCTAVES: usize = 9;
const EQ_MODULATION_SLOTS: usize = 10;

/// Bounds a modulated value may reach, in cell order (mirrored by the UI's modulation targets).
pub const EQ_RANGES: [(f32, f32); EQ_PARAM_COUNT] = [
    (-12.0, 12.0),
    (20.0, 20_000.0),
    (-12.0, 12.0),
    (20.0, 20_000.0),
    (0.1, 10.0),
    (-12.0, 12.0),
    (20.0, 20_000.0),
];

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

/// Frequency moved by `octaves` and `hz`, held in `range`.  With no octave
/// delta this is exactly `modulated(base, hz, range)`, so Hz routes stay bit-identical.
fn modulated_frequency(base: f32, hz: f32, octaves: f32, range: (f32, f32)) -> f32 {
    modulated(base, base * (octaves.exp2() - 1.0) + hz, range)
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
const DEFAULT_ENGINE_SAMPLE_RATE: f32 = 48_000.0;

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
        Self {
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
        Self {
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
        Self {
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

    /// Sub-block modulation on the absolute control grid (ADR-019 Tranche 3).
    /// `base_delta` is the block-start (offset 0) param-delta; `seg_*` hold the
    /// interior grid points used to recompute the biquads mid-block.  Reset each block.
    base_delta: [f32; EQ_MODULATION_SLOTS],
    seg_offset: [u32; MAX_CONTROL_POINTS],
    seg_delta: [[f32; EQ_MODULATION_SLOTS]; MAX_CONTROL_POINTS],
    seg_count: usize,
}

impl EqNode {
    pub fn new(id: NodeId, params: EqParams) -> Self {
        Self {
            id,
            params,
            low: Biquad::default(),
            mid: Biquad::default(),
            high: Biquad::default(),
            oversample: Oversample2x::default(),
            sr_os: DEFAULT_ENGINE_SAMPLE_RATE * OVERSAMPLE_FACTOR as f32,
            block: None,
            base_delta: [0.0; EQ_MODULATION_SLOTS],
            seg_offset: [0; MAX_CONTROL_POINTS],
            seg_delta: [[0.0; EQ_MODULATION_SLOTS]; MAX_CONTROL_POINTS],
            seg_count: 0,
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
        // ADR-009 + ADR-019 Tranche 3: read live block values, then apply
        // modulation per control segment on the absolute grid, recomputing the
        // biquads per segment only when the effective params change.  Consecutive
        // `process_channel` calls on sub-slices are seamless — the oversampler's
        // per-channel state lives in `self`, and `recompute_coeffs` preserves the
        // biquad state, so an unmodulated block (one segment) is bit-identical.
        let base = self.block.as_ref().map(|b| eq_params_from_block(b)).unwrap_or_else(|| self.params.clone());

        let input = &inputs[0];
        let output = &mut outputs[0];
        let channels = output.num_channels.min(input.num_channels).min(2);
        let frames = t.block_size.min(output.num_frames).min(input.num_frames);

        let mut frame = 0usize;
        for k in 0..=self.seg_count {
            let seg_end = if k < self.seg_count {
                (self.seg_offset[k] as usize).min(frames)
            } else {
                frames
            };
            if seg_end <= frame {
                continue;
            }
            let delta = if k == 0 { self.base_delta } else { self.seg_delta[k - 1] };
            let effective = EqParams {
                low_gain_db:  modulated(base.low_gain_db,  delta[EQ_LOW_GAIN_DB],  EQ_RANGES[EQ_LOW_GAIN_DB]),
                low_freq:     modulated_frequency(base.low_freq, delta[EQ_LOW_FREQ], delta[EQ_LOW_FREQ_OCTAVES], EQ_RANGES[EQ_LOW_FREQ]),
                mid_gain_db:  modulated(base.mid_gain_db,  delta[EQ_MID_GAIN_DB],  EQ_RANGES[EQ_MID_GAIN_DB]),
                mid_freq:     modulated_frequency(base.mid_freq, delta[EQ_MID_FREQ], delta[EQ_MID_FREQ_OCTAVES], EQ_RANGES[EQ_MID_FREQ]),
                mid_q:        modulated(base.mid_q,        delta[EQ_MID_Q],        EQ_RANGES[EQ_MID_Q]),
                high_gain_db: modulated(base.high_gain_db, delta[EQ_HIGH_GAIN_DB], EQ_RANGES[EQ_HIGH_GAIN_DB]),
                high_freq:    modulated_frequency(base.high_freq, delta[EQ_HIGH_FREQ], delta[EQ_HIGH_FREQ_OCTAVES], EQ_RANGES[EQ_HIGH_FREQ]),
            };
            if eq_block_values(&effective) != eq_block_values(&self.params) {
                self.params = effective;
                self.recompute_coeffs();
            }

            let len = seg_end - frame;
            let Self {
                low,
                mid,
                high,
                oversample,
                ..
            } = &mut *self;
            for ch in 0..channels {
                let in_ch = &input.channel(ch)[frame..seg_end];
                let out_ch = &mut output.channel_mut(ch)[frame..seg_end];
                oversample.process_channel(ch, in_ch, out_ch, len, |x| {
                    let s = low.process(x, ch);
                    let s = mid.process(s, ch);
                    high.process(s, ch)
                });
            }
            frame = seg_end;
        }
    }

    fn reset(&mut self) {
        self.low.reset();
        self.mid.reset();
        self.high.reset();
        self.oversample.reset();
        self.base_delta = [0.0; EQ_MODULATION_SLOTS];
        self.seg_count = 0;
    }

    fn param_id(&self, name: &str) -> Option<u32> {
        match name {
            "low_gain_db"  => Some(EQ_LOW_GAIN_DB as u32),
            "low_freq"     => Some(EQ_LOW_FREQ as u32),
            "mid_gain_db"  => Some(EQ_MID_GAIN_DB as u32),
            "mid_freq"     => Some(EQ_MID_FREQ as u32),
            "mid_q"        => Some(EQ_MID_Q as u32),
            "high_gain_db" => Some(EQ_HIGH_GAIN_DB as u32),
            "high_freq"    => Some(EQ_HIGH_FREQ as u32),
            "low_freq_octaves"  => Some(EQ_LOW_FREQ_OCTAVES as u32),
            "mid_freq_octaves"  => Some(EQ_MID_FREQ_OCTAVES as u32),
            "high_freq_octaves" => Some(EQ_HIGH_FREQ_OCTAVES as u32),
            _ => None,
        }
    }

    fn apply_modulation(&mut self, param_id: u32, delta: f32) {
        let p = param_id as usize;
        if p < EQ_MODULATION_SLOTS {
            self.base_delta[p] += delta;
        }
    }

    fn apply_modulation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        let p = param_id as usize;
        if p >= EQ_MODULATION_SLOTS {
            return;
        }
        // Grid offsets arrive ascending and are deduplicated, so `seg_offset`
        // stays sorted — the segment loop relies on that.
        for k in 0..self.seg_count {
            if self.seg_offset[k] == sample_offset {
                self.seg_delta[k][p] += value;
                return;
            }
        }
        if self.seg_count < MAX_CONTROL_POINTS {
            let k = self.seg_count;
            self.seg_offset[k] = sample_offset;
            self.seg_delta[k] = [0.0; EQ_MODULATION_SLOTS];
            self.seg_delta[k][p] = value;
            self.seg_count += 1;
        }
    }

    fn reset_modulation(&mut self) {
        self.base_delta = [0.0; EQ_MODULATION_SLOTS];
        self.seg_count = 0;
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
    fn interior_grid_point_recomputes_biquads_mid_block() {
        use crate::audio_graph::node::AudioNode;

        let params = EqParams::default();
        let block = ParameterBlock::new(&eq_block_values(&params));
        let mut node = EqNode::new(NodeId(1), params.clone()).with_block(Arc::clone(&block));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
        });
        let b0_base = node.low.b0;

        let input = AudioBuffer::new(2, 64);
        let mut output = AudioBuffer::new(2, 64);
        let t = TransportInfo::stopped(48_000.0);

        node.reset_modulation();
        node.apply_modulation_at(EQ_LOW_GAIN_DB as u32, 32, 6.0); // low shelf +6 dB from frame 32
        node.process(&[&input], std::slice::from_mut(&mut output), &t);

        assert!(
            (node.params.low_gain_db - (params.low_gain_db + 6.0)).abs() < 1e-4,
            "interior grid point must reach the cached params, got {}",
            node.params.low_gain_db
        );
        assert_ne!(
            node.low.b0, b0_base,
            "low-shelf biquad must be recomputed for the interior segment"
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

        let before = node.low.b0;

        block.set(EQ_LOW_GAIN_DB, 12.0);

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
    fn modulation_delta_applied_to_params() {
        use crate::audio_graph::node::AudioNode;

        let params = EqParams::default();
        let block = ParameterBlock::new(&eq_block_values(&params));
        let mut node = EqNode::new(NodeId(1), params.clone()).with_block(Arc::clone(&block));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
        });

        let input = AudioBuffer::new(2, 64);
        let mut output = AudioBuffer::new(2, 64);
        let t = TransportInfo::stopped(48_000.0);

        // Run until the smoothed delta converges to the target.
        for _ in 0..200 {
            node.reset_modulation();
            node.apply_modulation(EQ_LOW_GAIN_DB as u32, 6.0);
            node.process(&[&input], std::slice::from_mut(&mut output), &t);
        }
        assert!(
            (node.params.low_gain_db - (params.low_gain_db + 6.0)).abs() < 1e-3,
            "smoothed delta must converge to target"
        );

        // Remove modulation — smoothed must decay back to zero.
        for _ in 0..200 {
            node.reset_modulation();
            node.process(&[&input], std::slice::from_mut(&mut output), &t);
        }
        assert!(
            (node.params.low_gain_db - params.low_gain_db).abs() < 1e-3,
            "smoothed delta must decay to zero when modulation is removed"
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

    #[test]
    fn frequency_modulation_in_octaves_doubles_and_halves() {
        let range = EQ_RANGES[EQ_MID_FREQ];
        assert_eq!(modulated_frequency(1000.0, 0.0, 1.0, range), 2000.0);
        assert_eq!(modulated_frequency(1000.0, 0.0, -1.0, range), 500.0);
        assert_eq!(modulated_frequency(1000.0, 0.0, 20.0, range), 20_000.0);
        assert_eq!(modulated_frequency(1000.0, 0.0, -20.0, range), 20.0);
    }

    #[test]
    fn frequency_modulation_without_octaves_matches_hz_routes_bit_for_bit() {
        let range = EQ_RANGES[EQ_MID_FREQ];
        for (base, hz) in [(1000.0, 600.0), (1000.0, -0.3), (437.5, 12.25), (20.0, -5000.0)] {
            assert_eq!(modulated_frequency(base, hz, 0.0, range).to_bits(), modulated(base, hz, range).to_bits());
        }
    }

    #[test]
    fn octave_targets_have_their_own_param_ids() {
        let node = EqNode::new(NodeId(1), EqParams::default());
        assert_eq!(node.param_id("mid_freq_octaves"), Some(EQ_MID_FREQ_OCTAVES as u32));
        assert_eq!(node.param_id("mid_freq"), Some(EQ_MID_FREQ as u32));
    }
}
