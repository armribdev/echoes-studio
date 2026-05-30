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

pub struct MasterNode {
    id: NodeId,
    num_inputs: usize,
    soft_clip_enabled: Arc<AtomicBool>,
    peak_l: f32,
    peak_r: f32,
}

impl MasterNode {
    pub fn new(id: NodeId, num_inputs: usize, soft_clip_enabled: Arc<AtomicBool>) -> Self {
        MasterNode {
            id,
            num_inputs: num_inputs.max(1),
            soft_clip_enabled,
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

    fn prepare(&mut self, _ctx: PrepareContext) {}

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
        if output.num_channels >= 2 {
            let (ch_l, ch_r) = output.two_channels_mut(0, 1);
            for i in 0..frames {
                if apply_clip {
                    ch_l[i] = Self::soft_clip(ch_l[i]);
                    ch_r[i] = Self::soft_clip(ch_r[i]);
                }
                self.peak_l = self.peak_l.max(ch_l[i].abs());
                self.peak_r = self.peak_r.max(ch_r[i].abs());
            }
        } else {
            let ch = output.channel_mut(0);
            for i in 0..frames {
                if apply_clip {
                    ch[i] = Self::soft_clip(ch[i]);
                }
                self.peak_l = self.peak_l.max(ch[i].abs());
            }
            self.peak_r = self.peak_l;
        }
    }

    fn reset(&mut self) {
        self.peak_l = 0.0;
        self.peak_r = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::node::{NodeId, TransportInfo};

    #[test]
    fn soft_clip_tanh_matches_direct() {
        let x = 0.5_f32;
        assert!((MasterNode::soft_clip(x) - x.tanh()).abs() < 1e-6);
    }

    #[test]
    fn process_linear_when_soft_clip_disabled() {
        let clip_flag = Arc::new(AtomicBool::new(false));
        let mut node = MasterNode::new(NodeId(0), 1, Arc::clone(&clip_flag));
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
        let mut input = AudioBuffer::new(1, 1);
        input.channel_mut(0)[0] = 0.5;
        let input_ref = &input;
        let mut output = AudioBuffer::new(1, 1);
        let transport = TransportInfo::stopped(48_000.0);
        let mut outputs = [output];
        node.process(&[input_ref], &mut outputs, &transport);
        assert!((outputs[0].channel(0)[0] - 0.5_f32.tanh()).abs() < 1e-6);
    }
}
