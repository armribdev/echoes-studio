//! Meter node — pass-through node that records peak levels for the UI.
//!
//! Inserted after a track's gain stage.  Audio passes through unchanged;
//! the side-effect is writing the block's peak amplitude to a pair of
//! `Arc<AtomicU32>` cells (L, R) that the position-reporter thread reads
//! every ~33 ms without blocking the audio thread.

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

pub struct MeterNode {
    id: NodeId,
    /// Shared cells written by this node and read by the reporter thread.
    peak_l_out: Arc<AtomicU32>,
    peak_r_out: Arc<AtomicU32>,
}

impl MeterNode {
    /// Create a node that writes its per-block peak into the provided atomics.
    pub fn new(id: NodeId, peak_l: Arc<AtomicU32>, peak_r: Arc<AtomicU32>) -> Self {
        MeterNode {
            id,
            peak_l_out: peak_l,
            peak_r_out: peak_r,
        }
    }
}

impl AudioNode for MeterNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "MeterNode"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, _ctx: PrepareContext) {}

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
        let input = &inputs[0];
        let output = &mut outputs[0];

        output.copy_from(input);

        let frames = input.num_frames;
        let mut peak_l = 0.0_f32;
        let mut peak_r = 0.0_f32;

        if input.num_channels >= 1 {
            for &s in input.channel(0).iter().take(frames) {
                peak_l = peak_l.max(s.abs());
            }
        }
        if input.num_channels >= 2 {
            for &s in input.channel(1).iter().take(frames) {
                peak_r = peak_r.max(s.abs());
            }
        } else {
            peak_r = peak_l;
        }

        self.peak_l_out.store(peak_l.to_bits(), Ordering::Relaxed);
        self.peak_r_out.store(peak_r.to_bits(), Ordering::Relaxed);
    }

    fn reset(&mut self) {
        self.peak_l_out.store(0, Ordering::Relaxed);
        self.peak_r_out.store(0, Ordering::Relaxed);
    }
}
