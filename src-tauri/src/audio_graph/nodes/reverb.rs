use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::denormal::ANTI_DENORMAL;
use crate::audio_graph::mod_smooth::ModSmooth;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::audio_graph::parameter_block::ParameterBlock;
use crate::project::ReverbParams;

// Cell order for `ParameterBlock` (ADR-009): shared by compiler, gateway, and `ReverbNode`.
pub const REVERB_PARAM_COUNT: usize = 5;
pub const REVERB_ROOM_SIZE: usize = 0;
pub const REVERB_DAMPING: usize = 1;
pub const REVERB_WET: usize = 2;
pub const REVERB_DRY: usize = 3;
pub const REVERB_WIDTH: usize = 4;

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

    mod_smooth: ModSmooth<REVERB_PARAM_COUNT>,
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
            mod_smooth: ModSmooth::new(),
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
        self.mod_smooth.prepare(ctx.sample_rate, ctx.max_block_size);
    }

    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], t: &TransportInfo) {
        // ADR-009 + modulation: read live block values, add any modulation deltas,
        // and re-apply comb coefficients only when room_size/damping effectively changed.
        let base = self.block.as_ref().map(|b| reverb_params_from_block(b)).unwrap_or_else(|| self.params.clone());
        let deltas = self.mod_smooth.advance();
        let effective = ReverbParams {
            room_size: base.room_size + deltas[REVERB_ROOM_SIZE],
            damping:   base.damping   + deltas[REVERB_DAMPING],
            wet:       base.wet       + deltas[REVERB_WET],
            dry:       base.dry       + deltas[REVERB_DRY],
            width:     base.width     + deltas[REVERB_WIDTH],
        };
        if reverb_block_values(&effective) != reverb_block_values(&self.params) {
            let recompute = effective.room_size != self.params.room_size
                || effective.damping != self.params.damping;
            self.params = effective;
            if recompute {
                self.apply_coeffs();
            }
        }

        let input = &inputs[0];
        let output = &mut outputs[0];
        let frames = t.block_size.min(output.num_frames).min(input.num_frames);
        let channels = output.num_channels.min(input.num_channels);

        let wet = self.params.wet;
        let dry = 1.0 - wet;
        let width = self.params.width;
        let wet1 = wet * (width / 2.0 + 0.5);
        let wet2 = wet * (0.5 - width / 2.0);

        for i in 0..frames {
            let in_l = if channels > 0 {
                input.channel(0)[i]
            } else {
                0.0
            };
            let in_r = if channels > 1 {
                input.channel(1)[i]
            } else {
                in_l
            };
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
        self.mod_smooth.reset();
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
        self.mod_smooth.add_target(param_id as usize, delta);
    }

    fn reset_modulation(&mut self) {
        self.mod_smooth.reset_target();
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
