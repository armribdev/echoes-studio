use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::denormal::ANTI_DENORMAL;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo, MAX_CONTROL_POINTS};
use crate::audio_graph::nodes::modulated;
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

/// Bounds a modulated value may reach, in cell order (mirrored by the UI's modulation targets).
pub const DELAY_RANGES: [(f32, f32); DELAY_PARAM_COUNT] = [
    (10.0, 2000.0),
    (0.0, 0.95),
    (0.0, 1.0),
    (0.0, 1.0),
    (0.0, 1.0),
];

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

    /// Sub-block modulation on the absolute control grid (ADR-019 Tranche 3).
    /// `base_delta` is the block-start (offset 0) param-delta; `seg_*` hold the
    /// interior grid points: `seg_offset[k]` is the frame offset, `seg_delta[k]`
    /// the accumulated param-deltas applied from that offset onward.  All reset
    /// each block by `reset_modulation`.
    base_delta: [f32; DELAY_PARAM_COUNT],
    seg_offset: [u32; MAX_CONTROL_POINTS],
    seg_delta: [[f32; DELAY_PARAM_COUNT]; MAX_CONTROL_POINTS],
    seg_count: usize,
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
            base_delta: [0.0; DELAY_PARAM_COUNT],
            seg_offset: [0; MAX_CONTROL_POINTS],
            seg_delta: [[0.0; DELAY_PARAM_COUNT]; MAX_CONTROL_POINTS],
            seg_count: 0,
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
        // ADR-009 + ADR-019 Tranche 3: read live block values, then apply
        // modulation per control segment on the absolute grid.  The read offset
        // is recomputed only when delay_ms effectively changes (preserves the
        // ring).  ping_pong is a bool encoded as float (< 0.5 = false).
        let base = self.block.as_ref().map(|b| delay_params_from_block(b)).unwrap_or_else(|| self.params.clone());

        let input = &inputs[0];
        let output = &mut outputs[0];
        let frames = t.block_size.min(output.num_frames).min(input.num_frames);
        let channels = output.num_channels.min(input.num_channels);
        let cap = self.buf.len() / 2; // capacity in stereo frames
        if cap == 0 {
            return;
        }

        // Segment k spans [start_k, end_k): start_0 = 0 with `base_delta`, then
        // each interior grid point at `seg_offset[k-1]` switches to `seg_delta[k-1]`.
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

            let ping_pong_float = (if base.ping_pong { 1.0_f32 } else { 0.0 }) + delta[DELAY_PING_PONG];
            let effective = DelayParams {
                delay_ms:  modulated(base.delay_ms, delta[DELAY_DELAY_MS], DELAY_RANGES[DELAY_DELAY_MS]),
                feedback:  modulated(base.feedback, delta[DELAY_FEEDBACK], DELAY_RANGES[DELAY_FEEDBACK]),
                wet:       modulated(base.wet,      delta[DELAY_WET],      DELAY_RANGES[DELAY_WET]),
                dry:       modulated(base.dry,      delta[DELAY_DRY],      DELAY_RANGES[DELAY_DRY]),
                ping_pong: ping_pong_float >= 0.5,
            };
            if delay_block_values(&effective) != delay_block_values(&self.params) {
                let recompute = effective.delay_ms != self.params.delay_ms;
                self.params = effective;
                if recompute {
                    self.recompute_delay_samples();
                }
            }

            let wet = self.params.wet;
            let dry = 1.0 - wet;
            let feedback = self.params.feedback;
            let ping_pong = self.params.ping_pong;

            for i in frame..seg_end {
                let read_pos = (self.write_pos + cap - self.delay_samples.min(cap - 1)) % cap;
                let del_l = self.buf[read_pos * 2];
                let del_r = self.buf[read_pos * 2 + 1];

                let in_l = if channels > 0 { input.channel(0)[i] } else { 0.0 };
                let in_r = if channels > 1 { input.channel(1)[i] } else { in_l };

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
            frame = seg_end;
        }
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write_pos = 0;
        self.base_delta = [0.0; DELAY_PARAM_COUNT];
        self.seg_count = 0;
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
        let p = param_id as usize;
        if p < DELAY_PARAM_COUNT {
            self.base_delta[p] += delta;
        }
    }

    fn apply_modulation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        let p = param_id as usize;
        if p >= DELAY_PARAM_COUNT {
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
            self.seg_delta[k] = [0.0; DELAY_PARAM_COUNT];
            self.seg_delta[k][p] = value;
            self.seg_count += 1;
        }
    }

    fn reset_modulation(&mut self) {
        self.base_delta = [0.0; DELAY_PARAM_COUNT];
        self.seg_count = 0;
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
            live: true,
        });

        let input = AudioBuffer::new(2, 64);
        let mut output = AudioBuffer::new(2, 64);
        let t = TransportInfo::stopped(48_000.0);

        for _ in 0..200 {
            node.reset_modulation();
            node.apply_modulation(DELAY_FEEDBACK as u32, 0.2);
            node.process(&[&input], std::slice::from_mut(&mut output), &t);
        }
        assert!(
            (node.params.feedback - (params.feedback + 0.2)).abs() < 1e-3,
            "smoothed delta must converge to target"
        );

        for _ in 0..200 {
            node.reset_modulation();
            node.process(&[&input], std::slice::from_mut(&mut output), &t);
        }
        assert!(
            (node.params.feedback - params.feedback).abs() < 1e-3,
            "smoothed delta must decay to zero when modulation is removed"
        );
    }

    #[test]
    fn interior_grid_point_changes_param_mid_block() {
        // wet 0.5 for [0,32), then 0.9 from frame 32; an empty 10 ms delay line
        // means out = in * (1 - wet), so the wet step is directly observable.
        let params = DelayParams {
            delay_ms: 10.0,
            feedback: 0.0,
            wet: 0.5,
            ..DelayParams::default()
        };
        let block = ParameterBlock::new(&delay_block_values(&params));
        let mut node = DelayNode::new(NodeId(1), params).with_block(Arc::clone(&block));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
            live: true,
        });

        let mut input = AudioBuffer::new(2, 64);
        input.channel_mut(0).fill(1.0);
        input.channel_mut(1).fill(1.0);
        let mut output = AudioBuffer::new(2, 64);
        let t = TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: 64,
        };

        node.reset_modulation();
        node.apply_modulation_at(DELAY_WET as u32, 32, 0.4); // wet → 0.9 from frame 32
        node.process(&[&input], std::slice::from_mut(&mut output), &t);

        let out = output.channel(0);
        assert!((out[0] - 0.5).abs() < 1e-4, "segment 0 wet 0.5 → dry 0.5, got {}", out[0]);
        assert!((out[40] - 0.1).abs() < 1e-4, "segment 1 wet 0.9 → dry 0.1, got {}", out[40]);
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
            live: true,
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
