//! Master output node — the final sink in every audio graph.
//!
//! The master node is a **sink** (it has output ports declared as 0 in the
//! compiler).  The scheduler treats it specially: rather than reading from
//! `output_buffers[master_step]`, the engine reads from the upstream
//! signal that feeds into the master's input ports.
//!
//! The master node itself performs:
//! - Summing all connected inputs (same as BusNode)
//! - Optional soft clip (tanh saturation) when enabled via `LiveParams`
//! - Peak metering for the UI
//!
//! Because the master is a sink, its "outputs" in the compiler have count 0.
//! However, we still allocate one slot in `output_buffers[master_step]` as a
//! working buffer that the scheduler reads after `run_steps`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::audio_graph::oversampling::Oversample2x;

pub struct MasterNode {
    id: NodeId,
    num_inputs: usize,
    soft_clip_enabled: Arc<AtomicBool>,
    oversample: Oversample2x,
    peak_l: f32,
    peak_r: f32,
}

impl MasterNode {
    pub fn new(id: NodeId, num_inputs: usize, soft_clip_enabled: Arc<AtomicBool>) -> Self {
        MasterNode {
            id,
            num_inputs: num_inputs.max(1),
            soft_clip_enabled,
            oversample: Oversample2x::default(),
            peak_l: 0.0,
            peak_r: 0.0,
        }
    }

    /// Peak levels since last reset (linear, 0.0 … 1.0+).
    pub fn peak_levels(&self) -> (f32, f32) {
        (self.peak_l, self.peak_r)
    }

    pub fn reset_peaks(&mut self) {
        self.peak_l = 0.0;
        self.peak_r = 0.0;
    }

    #[inline]
    fn soft_clip(x: f32) -> f32 {
        // Gentle tanh saturation — not linear; shapes all levels, not only peaks.
        x.tanh()
    }
}

impl AudioNode for MasterNode {
    fn node_id(&self) -> NodeId { self.id }
    fn label(&self) -> &str { "MasterNode" }
    fn num_input_ports(&self) -> usize { self.num_inputs }
    /// The compiler treats the master as a sink (num_outputs = 0).
    /// We return 1 here so the scheduler pre-allocates a working buffer
    /// for the `write_master_output` path.
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.oversample.prepare(ctx.max_block_size, ctx.sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
        let output = &mut outputs[0];
        output.clear();

        // Sum all inputs
        for input in inputs {
            let channels = output.num_channels.min(input.num_channels);
            let frames = output.num_frames.min(input.num_frames);
            for ch in 0..channels {
                let in_ch = input.channel(ch);
                let out_ch = output.channel_mut(ch);
                for i in 0..frames {
                    out_ch[i] += in_ch[i];
                }
            }
        }

        let apply_clip = self.soft_clip_enabled.load(Ordering::Relaxed);
        let frames = output.num_frames;
        if apply_clip {
            if output.num_channels >= 2 {
                let (ch_l, ch_r) = output.two_channels_mut(0, 1);
                self.oversample.process_inplace(0, ch_l, frames, Self::soft_clip);
                self.oversample.process_inplace(1, ch_r, frames, Self::soft_clip);
            } else {
                let ch = output.channel_mut(0);
                self.oversample.process_inplace(0, ch, frames, Self::soft_clip);
            }
        }
        if output.num_channels >= 2 {
            let (ch_l, ch_r) = output.two_channels_mut(0, 1);
            for i in 0..frames {
                self.peak_l = self.peak_l.max(ch_l[i].abs());
                self.peak_r = self.peak_r.max(ch_r[i].abs());
            }
        } else {
            let ch = output.channel(0);
            for i in 0..frames {
                self.peak_l = self.peak_l.max(ch[i].abs());
            }
            self.peak_r = self.peak_l;
        }
    }

    fn reset(&mut self) {
        self.peak_l = 0.0;
        self.peak_r = 0.0;
        self.oversample.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::node::{NodeId, PrepareContext, TransportInfo};

    #[test]
    fn soft_clip_tanh_matches_direct() {
        let x = 0.5_f32;
        assert!((MasterNode::soft_clip(x) - x.tanh()).abs() < 1e-6);
    }

    #[test]
    fn process_linear_when_soft_clip_disabled() {
        let clip_flag = Arc::new(AtomicBool::new(false));
        let mut node = MasterNode::new(NodeId(0), 1, Arc::clone(&clip_flag));
        node.prepare(PrepareContext { sample_rate: 48_000.0, max_block_size: 512, num_channels: 1 });
        let mut input = AudioBuffer::new(1, 4);
        input.channel_mut(0).copy_from_slice(&[0.5, -0.3, 0.9, 1.2]);
        let input_ref = &input;
        let mut output = AudioBuffer::new(1, 4);
        let transport = TransportInfo::stopped(48_000.0);
        let mut outputs = [output];
        node.process(&[input_ref], &mut outputs, &transport);
        let out = outputs[0].channel(0);
        assert!((out[0] - 0.5).abs() < 1e-6);
        assert!((out[1] - (-0.3)).abs() < 1e-6);
        assert!((out[2] - 0.9).abs() < 1e-6);
        assert!((out[3] - 1.2).abs() < 1e-6);
    }

    #[test]
    fn process_applies_tanh_when_soft_clip_enabled() {
        let clip_flag = Arc::new(AtomicBool::new(true));
        let mut node = MasterNode::new(NodeId(0), 1, Arc::clone(&clip_flag));
        node.prepare(PrepareContext { sample_rate: 48_000.0, max_block_size: 512, num_channels: 1 });
        let mut input = AudioBuffer::new(1, 8);
        input.channel_mut(0).fill(0.5);
        let input_ref = &input;
        let mut output = AudioBuffer::new(1, 8);
        let transport = TransportInfo::stopped(48_000.0);
        let mut outputs = [output];
        node.process(&[input_ref], &mut outputs, &transport);
        let last = outputs[0].channel(0)[7];
        assert!((last - 0.5_f32.tanh()).abs() < 0.05, "last sample {last}, expected ~{}", 0.5_f32.tanh());
        assert!((last - 0.5).abs() > 1e-4, "soft clip should shape the plateau");
    }
}
