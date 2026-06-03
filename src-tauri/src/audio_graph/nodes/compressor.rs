use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::audio_graph::oversampling::Oversample2x;
use crate::audio_graph::parameter_block::ParameterBlock;
use crate::project::CompressorParams;

// Cell order for `ParameterBlock` (ADR-009): shared by compiler, gateway, and `CompressorNode`.
// Values in natural units (dB, ratio, ms).
pub const COMP_PARAM_COUNT: usize = 6;
pub const COMP_THRESHOLD_DB: usize = 0;
pub const COMP_RATIO: usize = 1;
pub const COMP_ATTACK_MS: usize = 2;
pub const COMP_RELEASE_MS: usize = 3;
pub const COMP_MAKEUP_DB: usize = 4;
pub const COMP_KNEE_DB: usize = 5;

/// Flatten `CompressorParams` into the `ParameterBlock` cell order above.
pub fn compressor_block_values(p: &CompressorParams) -> [f32; COMP_PARAM_COUNT] {
    [
        p.threshold_db,
        p.ratio,
        p.attack_ms,
        p.release_ms,
        p.makeup_db,
        p.knee_db,
    ]
}

/// Read the block back into a `CompressorParams`.
fn compressor_params_from_block(block: &ParameterBlock) -> CompressorParams {
    CompressorParams {
        threshold_db: block.get(COMP_THRESHOLD_DB),
        ratio: block.get(COMP_RATIO),
        attack_ms: block.get(COMP_ATTACK_MS),
        release_ms: block.get(COMP_RELEASE_MS),
        makeup_db: block.get(COMP_MAKEUP_DB),
        knee_db: block.get(COMP_KNEE_DB),
    }
}

pub struct CompressorNode {
    id: NodeId,
    /// Cached parameter values — the time-constants currently baked into
    /// `attack_coeff` / `release_coeff` / `makeup_linear`.  Compared against the
    /// hot block each block to decide whether a recompute is needed.
    params: CompressorParams,
    envelope: f32,
    attack_coeff: f32,
    release_coeff: f32,
    makeup_linear: f32,
    oversample: Oversample2x,
    /// Per-block envelope gains (base rate), pre-allocated in `prepare`.
    env_scratch: Vec<f32>,

    /// Oversampled sample rate (engine SR × 2), captured at `prepare`.
    sr_os: f32,

    /// Hot-parameter block (ADR-009).  When present, `process` reads parameter
    /// values from it each block and recomputes the time-constants only when
    /// they change — so a knob turn never triggers a recompile.  `None` keeps
    /// the pre-ADR-009 behaviour (coefficients frozen at `prepare`).
    block: Option<Arc<ParameterBlock>>,
}

impl CompressorNode {
    pub fn new(id: NodeId, params: CompressorParams) -> Self {
        CompressorNode {
            id,
            params,
            envelope: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            makeup_linear: 1.0,
            oversample: Oversample2x::default(),
            env_scratch: Vec::new(),
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

    /// Recompute the time-constant coefficients from `self.params`.  Does not
    /// touch `self.envelope`, so a parameter change does not reset the running
    /// detector — only the smoothing/makeup gain move.
    fn recompute_coeffs(&mut self) {
        self.attack_coeff = (-1.0 / (self.params.attack_ms * 0.001 * self.sr_os)).exp();
        self.release_coeff = (-1.0 / (self.params.release_ms * 0.001 * self.sr_os)).exp();
        self.makeup_linear = 10.0_f32.powf(self.params.makeup_db / 20.0);
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
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "CompressorNode"
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
            self.params = compressor_params_from_block(block);
        }
        self.recompute_coeffs();
        self.oversample.prepare(ctx.max_block_size, ctx.sample_rate);
        self.env_scratch.resize(ctx.max_block_size, 0.0);
    }

    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], t: &TransportInfo) {
        // ADR-009: pull live parameter values from the hot block and recompute
        // the time-constants only when they actually changed.  No allocation,
        // no lock: `ParameterBlock::get` is one relaxed atomic load per param.
        let next = self.block.as_ref().map(|b| compressor_params_from_block(b));
        if let Some(next) = next {
            if compressor_block_values(&next) != compressor_block_values(&self.params) {
                self.params = next;
                self.recompute_coeffs();
            }
        }

        let input = &inputs[0];
        let output = &mut outputs[0];
        let frames = t.block_size.min(output.num_frames).min(input.num_frames);
        let channels = output.num_channels.min(input.num_channels).min(2);

        for i in 0..frames {
            let mut peak = 0.0_f32;
            for ch in 0..channels {
                peak = peak.max(input.channel(ch)[i].abs());
            }
            let peak_db = if peak > 1e-6 {
                20.0 * peak.log10()
            } else {
                -120.0
            };
            let gain_db = self.compute_gain_db(peak_db);
            let gain_linear = 10.0_f32.powf(gain_db / 20.0);
            let coeff = if gain_linear < self.envelope {
                self.attack_coeff
            } else {
                self.release_coeff
            };
            self.envelope = coeff * self.envelope + (1.0 - coeff) * gain_linear;
            self.env_scratch[i] = self.envelope * self.makeup_linear;
        }

        for ch in 0..channels {
            let in_ch = input.channel(ch);
            let out_ch = output.channel_mut(ch);
            let mut os_idx = 0usize;
            self.oversample
                .process_channel(ch, in_ch, out_ch, frames, |x| {
                    let g = self.env_scratch[os_idx / 2];
                    os_idx += 1;
                    x * g
                });
        }
    }

    fn reset(&mut self) {
        self.envelope = 0.0;
        self.oversample.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_block_changes_coeffs_without_prepare() {
        let params = CompressorParams::default();
        let block = ParameterBlock::new(&compressor_block_values(&params));
        let mut node = CompressorNode::new(NodeId(1), params).with_block(Arc::clone(&block));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
        });

        let before_makeup = node.makeup_linear;

        block.set(COMP_MAKEUP_DB, 6.0);

        let input = AudioBuffer::new(2, 64);
        let mut output = AudioBuffer::new(2, 64);
        let t = TransportInfo::stopped(48_000.0);
        node.process(&[&input], std::slice::from_mut(&mut output), &t);

        assert_ne!(
            node.makeup_linear, before_makeup,
            "compressor must recompute coefficients from the hot block on a live change"
        );
        assert_eq!(node.params.makeup_db, 6.0, "cached params follow the block");
    }
}
