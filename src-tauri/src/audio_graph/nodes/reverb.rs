use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::denormal::ANTI_DENORMAL;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo, MAX_CONTROL_POINTS};
use crate::audio_graph::nodes::modulated;
use crate::audio_graph::parameter_block::ParameterBlock;
use crate::project::ReverbParams;

// Cell order for `ParameterBlock` (ADR-009): shared by compiler, gateway, and `ReverbNode`.
pub const REVERB_PARAM_COUNT: usize = 5;
pub const REVERB_ROOM_SIZE: usize = 0;
pub const REVERB_DAMPING: usize = 1;
pub const REVERB_WET: usize = 2;
pub const REVERB_DRY: usize = 3;
pub const REVERB_WIDTH: usize = 4;

/// Bounds a modulated value may reach, in cell order (mirrored by the UI's modulation targets).
pub const REVERB_RANGES: [(f32, f32); REVERB_PARAM_COUNT] = [(0.0, 1.0); REVERB_PARAM_COUNT];

/// Flatten `ReverbParams` into the `ParameterBlock` cell order above.
pub fn reverb_block_values(p: &ReverbParams) -> [f32; REVERB_PARAM_COUNT] {
    [p.room_size, p.damping, p.wet, p.dry, p.width]
}

/// Read the block back into a `ReverbParams`.
fn reverb_params_from_block(block: &ParameterBlock) -> ReverbParams {
    ReverbParams {
        room_size: block.get(REVERB_ROOM_SIZE),
        damping: block.get(REVERB_DAMPING),
        wet: block.get(REVERB_WET),
        dry: block.get(REVERB_DRY),
        width: block.get(REVERB_WIDTH),
    }
}

struct CombFilter {
    buf: Vec<f32>,
    pos: usize,
    filterstore: f32,
    damp1: f32,
    damp2: f32,
    feedback: f32,
}

impl CombFilter {
    fn new(size: usize) -> Self {
        CombFilter {
            buf: vec![0.0; size],
            pos: 0,
            filterstore: 0.0,
            damp1: 0.5,
            damp2: 0.5,
            feedback: 0.5,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.buf[self.pos];
        // ANTI_DENORMAL keeps the LPF + comb state out of the sub-normal range
        // when input has been silent for several seconds.
        self.filterstore = output * self.damp2 + self.filterstore * self.damp1 + ANTI_DENORMAL;
        self.buf[self.pos] = input + self.filterstore * self.feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        output
    }
}

struct AllpassFilter {
    buf: Vec<f32>,
    pos: usize,
}

impl AllpassFilter {
    fn new(size: usize) -> Self {
        AllpassFilter {
            buf: vec![0.0; size],
            pos: 0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let bufout = self.buf[self.pos];
        let output = -input + bufout;
        self.buf[self.pos] = input + bufout * 0.5 + ANTI_DENORMAL;
        self.pos = (self.pos + 1) % self.buf.len();
        output
    }
}

// Freeverb tunings at 44100 Hz
const COMB_TUNING_L: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING_L: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;

pub struct ReverbNode {
    id: NodeId,
    /// Cached parameter values — `room_size` / `damping` are currently baked
    /// into the comb feedback/damping coefficients.  Compared against the hot
    /// block each block to decide whether a recompute is needed.
    params: ReverbParams,
    combs_l: Vec<CombFilter>,
    combs_r: Vec<CombFilter>,
    allpass_l: Vec<AllpassFilter>,
    allpass_r: Vec<AllpassFilter>,

    /// Hot-parameter block (ADR-009).  When present, `process` reads parameter
    /// values from it each block and re-applies the comb coefficients only when
    /// `room_size`/`damping` change — without reallocating the comb/allpass
    /// buffers, so the reverb tail is preserved.  `wet`/`dry`/`width` are read
    /// from `self.params` directly each block.  `None` keeps the pre-ADR-009
    /// behaviour (coefficients frozen at `prepare`).
    block: Option<Arc<ParameterBlock>>,

    /// Sub-block modulation on the absolute control grid (ADR-019 Tranche 3).
    /// `base_delta` is the block-start (offset 0) param-delta; `seg_*` hold the
    /// interior grid points used to re-apply coeffs / wet-dry mix mid-block.
    /// Reset each block.
    base_delta: [f32; REVERB_PARAM_COUNT],
    seg_offset: [u32; MAX_CONTROL_POINTS],
    seg_delta: [[f32; REVERB_PARAM_COUNT]; MAX_CONTROL_POINTS],
    seg_count: usize,
}

impl ReverbNode {
    pub fn new(id: NodeId, params: ReverbParams) -> Self {
        Self {
            id,
            params,
            combs_l: Vec::new(),
            combs_r: Vec::new(),
            allpass_l: Vec::new(),
            allpass_r: Vec::new(),
            block: None,
            base_delta: [0.0; REVERB_PARAM_COUNT],
            seg_offset: [0; MAX_CONTROL_POINTS],
            seg_delta: [[0.0; REVERB_PARAM_COUNT]; MAX_CONTROL_POINTS],
            seg_count: 0,
        }
    }

    /// Attach the hot-parameter block (ADR-009).  Builder-style; called by the
    /// compiler right after `new`.
    pub fn with_block(mut self, block: Arc<ParameterBlock>) -> Self {
        self.block = Some(block);
        self
    }

    /// Re-apply the comb feedback/damping coefficients from `self.params`.  Does
    /// **not** resize or clear any buffer, so the running reverb tail survives a
    /// `room_size`/`damping` change — only the coefficients move.
    fn apply_coeffs(&mut self) {
        let feedback = 0.84 * self.params.room_size;
        let damp = self.params.damping * 0.4;
        for c in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            c.feedback = feedback;
            c.damp1 = damp;
            c.damp2 = 1.0 - damp;
        }
    }
}

impl AudioNode for ReverbNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "ReverbNode"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        let scale = ctx.sample_rate / 44100.0;

        // Comb/allpass buffer sizes depend only on the sample rate (structural),
        // never on the hot params — so they are allocated here and reused.
        self.combs_l = COMB_TUNING_L
            .iter()
            .map(|&t| CombFilter::new((t as f32 * scale) as usize))
            .collect();
        self.combs_r = COMB_TUNING_L
            .iter()
            .map(|&t| CombFilter::new(((t + STEREO_SPREAD) as f32 * scale) as usize))
            .collect();
        self.allpass_l = ALLPASS_TUNING_L
            .iter()
            .map(|&t| AllpassFilter::new((t as f32 * scale) as usize))
            .collect();
        self.allpass_r = ALLPASS_TUNING_L
            .iter()
            .map(|&t| AllpassFilter::new(((t + STEREO_SPREAD) as f32 * scale) as usize))
            .collect();

        // If a hot block is attached, seed the cached params from it so the
        // coefficients reflect any edits made before this (re)compile.
        if let Some(block) = &self.block {
            self.params = reverb_params_from_block(block);
        }
        self.apply_coeffs();
    }

    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], t: &TransportInfo) {
        // ADR-009 + ADR-019 Tranche 3: read live block values, then apply
        // modulation per control segment on the absolute grid, re-applying comb
        // coefficients per segment only when room_size/damping change.
        let base = self.block.as_ref().map(|b| reverb_params_from_block(b)).unwrap_or_else(|| self.params.clone());

        let input = &inputs[0];
        let output = &mut outputs[0];
        let frames = t.block_size.min(output.num_frames).min(input.num_frames);
        let channels = output.num_channels.min(input.num_channels);

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
            let effective = ReverbParams {
                room_size: modulated(base.room_size, delta[REVERB_ROOM_SIZE], REVERB_RANGES[REVERB_ROOM_SIZE]),
                damping:   modulated(base.damping,   delta[REVERB_DAMPING],   REVERB_RANGES[REVERB_DAMPING]),
                wet:       modulated(base.wet,       delta[REVERB_WET],       REVERB_RANGES[REVERB_WET]),
                dry:       modulated(base.dry,       delta[REVERB_DRY],       REVERB_RANGES[REVERB_DRY]),
                width:     modulated(base.width,     delta[REVERB_WIDTH],     REVERB_RANGES[REVERB_WIDTH]),
            };
            if reverb_block_values(&effective) != reverb_block_values(&self.params) {
                let recompute = effective.room_size != self.params.room_size
                    || effective.damping != self.params.damping;
                self.params = effective;
                if recompute {
                    self.apply_coeffs();
                }
            }

            let wet = self.params.wet;
            let dry = 1.0 - wet;
            let width = self.params.width;
            let wet1 = wet * (width / 2.0 + 0.5);
            let wet2 = wet * (0.5 - width / 2.0);

            for i in frame..seg_end {
                let in_l = if channels > 0 { input.channel(0)[i] } else { 0.0 };
                let in_r = if channels > 1 { input.channel(1)[i] } else { in_l };
                let mono = (in_l + in_r) * 0.015; // Freeverb input gain

                let mut out_l = 0.0_f32;
                let mut out_r = 0.0_f32;

                for c in &mut self.combs_l {
                    out_l += c.process(mono);
                }
                for c in &mut self.combs_r {
                    out_r += c.process(mono);
                }
                for a in &mut self.allpass_l {
                    out_l = a.process(out_l);
                }
                for a in &mut self.allpass_r {
                    out_r = a.process(out_r);
                }

                if channels > 0 {
                    output.channel_mut(0)[i] = out_l * wet1 + out_r * wet2 + in_l * dry;
                }
                if channels > 1 {
                    output.channel_mut(1)[i] = out_r * wet1 + out_l * wet2 + in_r * dry;
                }
            }
            frame = seg_end;
        }
    }

    fn reset(&mut self) {
        for c in &mut self.combs_l {
            c.filterstore = 0.0;
            c.buf.fill(0.0);
        }
        for c in &mut self.combs_r {
            c.filterstore = 0.0;
            c.buf.fill(0.0);
        }
        for a in &mut self.allpass_l {
            a.buf.fill(0.0);
        }
        for a in &mut self.allpass_r {
            a.buf.fill(0.0);
        }
        self.base_delta = [0.0; REVERB_PARAM_COUNT];
        self.seg_count = 0;
    }

    fn param_id(&self, name: &str) -> Option<u32> {
        match name {
            "room_size" => Some(REVERB_ROOM_SIZE as u32),
            "damping"   => Some(REVERB_DAMPING as u32),
            "wet"       => Some(REVERB_WET as u32),
            "dry"       => Some(REVERB_DRY as u32),
            "width"     => Some(REVERB_WIDTH as u32),
            _ => None,
        }
    }

    fn apply_modulation(&mut self, param_id: u32, delta: f32) {
        let p = param_id as usize;
        if p < REVERB_PARAM_COUNT {
            self.base_delta[p] += delta;
        }
    }

    fn apply_modulation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        let p = param_id as usize;
        if p >= REVERB_PARAM_COUNT {
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
            self.seg_delta[k] = [0.0; REVERB_PARAM_COUNT];
            self.seg_delta[k][p] = value;
            self.seg_count += 1;
        }
    }

    fn reset_modulation(&mut self) {
        self.base_delta = [0.0; REVERB_PARAM_COUNT];
        self.seg_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulation_delta_applied_to_params() {
        use crate::audio_graph::node::AudioNode;

        let params = ReverbParams::default();
        let block = ParameterBlock::new(&reverb_block_values(&params));
        let mut node = ReverbNode::new(NodeId(1), params.clone()).with_block(Arc::clone(&block));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
        });

        let input = AudioBuffer::new(2, 64);
        let mut output = AudioBuffer::new(2, 64);
        let t = TransportInfo::stopped(48_000.0);

        for _ in 0..200 {
            node.reset_modulation();
            node.apply_modulation(REVERB_WET as u32, 0.3);
            node.process(&[&input], std::slice::from_mut(&mut output), &t);
        }
        assert!(
            (node.params.wet - (params.wet + 0.3)).abs() < 1e-3,
            "smoothed delta must converge to target"
        );

        for _ in 0..200 {
            node.reset_modulation();
            node.process(&[&input], std::slice::from_mut(&mut output), &t);
        }
        assert!(
            (node.params.wet - params.wet).abs() < 1e-3,
            "smoothed delta must decay to zero when modulation is removed"
        );
    }

    #[test]
    fn interior_grid_point_reapplies_coeffs_mid_block() {
        let params = ReverbParams::default();
        let block = ParameterBlock::new(&reverb_block_values(&params));
        let mut node = ReverbNode::new(NodeId(1), params.clone()).with_block(Arc::clone(&block));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
        });

        let input = AudioBuffer::new(2, 64);
        let mut output = AudioBuffer::new(2, 64);
        let t = TransportInfo::stopped(48_000.0);

        node.reset_modulation();
        node.apply_modulation_at(REVERB_ROOM_SIZE as u32, 32, 0.5); // room_size +0.5 from frame 32
        node.process(&[&input], std::slice::from_mut(&mut output), &t);

        // The last segment (frames 32..64) ran with room_size +0.5, so the comb
        // feedback reflects the interior grid point.
        let expected = 0.84 * (params.room_size + 0.5);
        assert!(
            (node.combs_l[0].feedback - expected).abs() < 1e-4,
            "comb feedback must follow the interior grid point, got {}",
            node.combs_l[0].feedback
        );
        assert!((node.params.room_size - (params.room_size + 0.5)).abs() < 1e-4);
    }

    #[test]
    fn hot_block_changes_coeffs_without_prepare() {
        let params = ReverbParams::default();
        let block = ParameterBlock::new(&reverb_block_values(&params));
        let mut node = ReverbNode::new(NodeId(1), params).with_block(Arc::clone(&block));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
        });

        let before_feedback = node.combs_l[0].feedback;

        block.set(REVERB_ROOM_SIZE, 1.0);

        let input = AudioBuffer::new(2, 64);
        let mut output = AudioBuffer::new(2, 64);
        let t = TransportInfo::stopped(48_000.0);
        node.process(&[&input], std::slice::from_mut(&mut output), &t);

        assert_ne!(
            node.combs_l[0].feedback, before_feedback,
            "reverb must re-apply comb coefficients from the hot block on a live change"
        );
        assert_eq!(node.params.room_size, 1.0, "cached params follow the block");
    }
}
