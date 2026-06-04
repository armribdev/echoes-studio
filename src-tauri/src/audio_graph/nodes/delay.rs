use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::denormal::ANTI_DENORMAL;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::audio_graph::parameter_block::ParameterBlock;
use crate::project::DelayParams;

// Cell order for `ParameterBlock` (ADR-009): shared by compiler, gateway, and `DelayNode`.
// `ping_pong` bool is encoded as 0.0 / 1.0 (threshold 0.5).
pub const DELAY_PARAM_COUNT: usize = 5;
pub const DELAY_DELAY_MS: usize = 0;
pub const DELAY_FEEDBACK: usize = 1;
pub const DELAY_WET: usize = 2;
pub const DELAY_DRY: usize = 3;
pub const DELAY_PING_PONG: usize = 4;

const DEFAULT_ENGINE_SAMPLE_RATE: f32 = 48_000.0;

/// Flatten `DelayParams` into the `ParameterBlock` cell order above.
pub fn delay_block_values(p: &DelayParams) -> [f32; DELAY_PARAM_COUNT] {
    [
        p.delay_ms,
        p.feedback,
        p.wet,
        p.dry,
        if p.ping_pong { 1.0 } else { 0.0 },
    ]
}

/// Read the block back into a `DelayParams`.
fn delay_params_from_block(block: &ParameterBlock) -> DelayParams {
    DelayParams {
        delay_ms: block.get(DELAY_DELAY_MS),
        feedback: block.get(DELAY_FEEDBACK),
        wet: block.get(DELAY_WET),
        dry: block.get(DELAY_DRY),
        ping_pong: block.get(DELAY_PING_PONG) >= 0.5,
    }
}

pub struct DelayNode {
    id: NodeId,
    /// Cached parameter values — `delay_ms` is currently baked into
    /// `delay_samples`.  Compared against the hot block each block to decide
    /// whether a recompute is needed.
    params: DelayParams,
    // Interleaved stereo ring buffer: [L0, R0, L1, R1, ...]
    buf: Vec<f32>,
    write_pos: usize,
    delay_samples: usize,

    /// Engine sample rate, captured at `prepare`, used to recompute
    /// `delay_samples` from `delay_ms` on a live change.
    sample_rate: f32,

    /// Hot-parameter block (ADR-009).  When present, `process` reads parameter
    /// values from it each block and recomputes `delay_samples` only when
    /// `delay_ms` changes — without reallocating or clearing the ring, so the
    /// delay line content is preserved.  `feedback`/`wet`/`ping_pong` are read
    /// from `self.params` directly each block.  `None` keeps the pre-ADR-009
    /// behaviour (parameters frozen at `prepare`).
    block: Option<Arc<ParameterBlock>>,

    /// Per-parameter modulation deltas accumulated by the scheduler each block.
    mod_deltas: [f32; DELAY_PARAM_COUNT],
}

impl DelayNode {
    pub fn new(id: NodeId, params: DelayParams) -> Self {
        Self {
            id,
            params,
            buf: Vec::new(),
            write_pos: 0,
            delay_samples: 0,
            sample_rate: DEFAULT_ENGINE_SAMPLE_RATE,
            block: None,
            mod_deltas: [0.0; DELAY_PARAM_COUNT],
        }
    }

    /// Attach the hot-parameter block (ADR-009).  Builder-style; called by the
    /// compiler right after `new`.
    pub fn with_block(mut self, block: Arc<ParameterBlock>) -> Self {
        self.block = Some(block);
        self
    }

    /// Recompute the read offset from `self.params.delay_ms`.  Does not touch
    /// the ring buffer or `write_pos`, so the delay line content survives a
    /// `delay_ms` change.
    fn recompute_delay_samples(&mut self) {
        self.delay_samples = (self.params.delay_ms * 0.001 * self.sample_rate) as usize;
    }
}

impl AudioNode for DelayNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "DelayNode"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        // Allocate 2 seconds of headroom at 2 channels.  The buffer size depends
        // only on the sample rate (structural), never on the hot params.
        let max_samples = (ctx.sample_rate * 2.0) as usize;
        self.buf = vec![0.0; max_samples * 2];
        self.sample_rate = ctx.sample_rate;
        // If a hot block is attached, seed the cached params from it so the read
        // offset reflects any edits made before this (re)compile.
        if let Some(block) = &self.block {
            self.params = delay_params_from_block(block);
        }
        self.recompute_delay_samples();
        self.write_pos = 0;
    }

    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], t: &TransportInfo) {
        // ADR-009 + modulation: read live block values, add any modulation deltas,
        // and recompute the read offset only when delay_ms effectively changed.
        // ping_pong is a bool encoded as float (< 0.5 = false, >= 0.5 = true).
        let base = self.block.as_ref().map(|b| delay_params_from_block(b)).unwrap_or_else(|| self.params.clone());
        let ping_pong_float = (if base.ping_pong { 1.0_f32 } else { 0.0 }) + self.mod_deltas[DELAY_PING_PONG];
        let effective = DelayParams {
            delay_ms:  base.delay_ms  + self.mod_deltas[DELAY_DELAY_MS],
            feedback:  base.feedback  + self.mod_deltas[DELAY_FEEDBACK],
            wet:       base.wet       + self.mod_deltas[DELAY_WET],
            dry:       base.dry       + self.mod_deltas[DELAY_DRY],
            ping_pong: ping_pong_float >= 0.5,
        };
        if delay_block_values(&effective) != delay_block_values(&self.params) {
            let recompute = effective.delay_ms != self.params.delay_ms;
            self.params = effective;
            if recompute {
                self.recompute_delay_samples();
            }
        }

        let input = &inputs[0];
        let output = &mut outputs[0];
        let frames = t.block_size.min(output.num_frames).min(input.num_frames);
        let channels = output.num_channels.min(input.num_channels);
        let cap = self.buf.len() / 2; // capacity in stereo frames
        if cap == 0 {
            return;
        }

        let wet = self.params.wet;
        let dry = 1.0 - wet;
        let feedback = self.params.feedback;
        let ping_pong = self.params.ping_pong;

        for i in 0..frames {
            let read_pos = (self.write_pos + cap - self.delay_samples.min(cap - 1)) % cap;
            let del_l = self.buf[read_pos * 2];
            let del_r = self.buf[read_pos * 2 + 1];

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

            // Write to delay buffer: feedback from delay out, cross-channel if ping-pong.
            // ANTI_DENORMAL on each write keeps the recursive state out of the
            // sub-normal range during long silences (otherwise: x86 CPU spike).
            if ping_pong {
                self.buf[self.write_pos * 2] = in_l + del_r * feedback + ANTI_DENORMAL;
                self.buf[self.write_pos * 2 + 1] = in_r + del_l * feedback + ANTI_DENORMAL;
            } else {
                self.buf[self.write_pos * 2] = in_l + del_l * feedback + ANTI_DENORMAL;
                self.buf[self.write_pos * 2 + 1] = in_r + del_r * feedback + ANTI_DENORMAL;
            }

            self.write_pos = (self.write_pos + 1) % cap;

            if channels > 0 {
                output.channel_mut(0)[i] = in_l * dry + del_l * wet;
            }
            if channels > 1 {
                output.channel_mut(1)[i] = in_r * dry + del_r * wet;
            }
        }
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write_pos = 0;
    }

    fn param_id(&self, name: &str) -> Option<u32> {
        match name {
            "delay_ms"  => Some(DELAY_DELAY_MS as u32),
            "feedback"  => Some(DELAY_FEEDBACK as u32),
            "wet"       => Some(DELAY_WET as u32),
            "dry"       => Some(DELAY_DRY as u32),
            "ping_pong" => Some(DELAY_PING_PONG as u32),
            _ => None,
        }
    }

    fn apply_modulation(&mut self, param_id: u32, delta: f32) {
        let idx = param_id as usize;
        if idx < DELAY_PARAM_COUNT {
            self.mod_deltas[idx] += delta;
        }
    }

    fn reset_modulation(&mut self) {
        self.mod_deltas = [0.0; DELAY_PARAM_COUNT];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulation_delta_applied_to_params() {
        use crate::audio_graph::node::AudioNode;

        let params = DelayParams::default();
        let block = ParameterBlock::new(&delay_block_values(&params));
        let mut node = DelayNode::new(NodeId(1), params.clone()).with_block(Arc::clone(&block));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
        });

        node.apply_modulation(DELAY_FEEDBACK as u32, 0.2);

        let input = AudioBuffer::new(2, 64);
        let mut output = AudioBuffer::new(2, 64);
        let t = TransportInfo::stopped(48_000.0);
        node.process(&[&input], std::slice::from_mut(&mut output), &t);

        assert!((node.params.feedback - (params.feedback + 0.2)).abs() < 1e-6);

        node.reset_modulation();
        node.process(&[&input], std::slice::from_mut(&mut output), &t);
        assert!((node.params.feedback - params.feedback).abs() < 1e-6, "delta must clear after reset");
    }

    #[test]
    fn hot_block_changes_delay_without_prepare() {
        let params = DelayParams::default();
        let block = ParameterBlock::new(&delay_block_values(&params));
        let mut node = DelayNode::new(NodeId(1), params).with_block(Arc::clone(&block));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
        });

        let before = node.delay_samples;

        block.set(DELAY_DELAY_MS, 125.0);

        let input = AudioBuffer::new(2, 64);
        let mut output = AudioBuffer::new(2, 64);
        let t = TransportInfo::stopped(48_000.0);
        node.process(&[&input], std::slice::from_mut(&mut output), &t);

        assert_ne!(
            node.delay_samples, before,
            "delay must recompute its read offset from the hot block on a live change"
        );
        assert_eq!(
            node.params.delay_ms, 125.0,
            "cached params follow the block"
        );
    }
}
