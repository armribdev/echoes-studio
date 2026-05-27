//! Meter node — pass-through node that records peak/RMS levels for the UI.
//!
//! The node is inserted after a track's gain stage.  Audio passes through
//! unchanged; the side-effect is writing level data to a lock-free ring
//! buffer that the UI polls via a Tauri command.
//!
//! Current implementation: pass-through only.  Level data is computed but
//! not yet routed to the UI (future: write to `Arc<AtomicU32>` meter cells).

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

pub struct MeterNode {
    id: NodeId,
    meter_id: String,
    peak_l: f32,
    peak_r: f32,
    rms_acc_l: f32,
    rms_acc_r: f32,
    frame_count: usize,
}

impl MeterNode {
    pub fn new(id: NodeId, meter_id: String) -> Self {
        MeterNode {
            id,
            meter_id,
            peak_l: 0.0,
            peak_r: 0.0,
            rms_acc_l: 0.0,
            rms_acc_r: 0.0,
            frame_count: 0,
        }
    }

    pub fn meter_id(&self) -> &str {
        &self.meter_id
    }

    pub fn peak(&self) -> (f32, f32) {
        (self.peak_l, self.peak_r)
    }

    pub fn rms(&self) -> (f32, f32) {
        if self.frame_count == 0 {
            return (0.0, 0.0);
        }
        let rms_l = (self.rms_acc_l / self.frame_count as f32).sqrt();
        let rms_r = (self.rms_acc_r / self.frame_count as f32).sqrt();
        (rms_l, rms_r)
    }
}

impl AudioNode for MeterNode {
    fn node_id(&self) -> NodeId { self.id }
    fn label(&self) -> &str { "MeterNode" }
    fn num_input_ports(&self) -> usize { 1 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, _ctx: PrepareContext) {}

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
        let input = &inputs[0];
        let output = &mut outputs[0];

        // Pass through
        output.copy_from(input);

        // Measure
        let frames = input.num_frames;
        if input.num_channels >= 1 {
            let ch_l = input.channel(0);
            for &s in ch_l.iter().take(frames) {
                self.peak_l = self.peak_l.max(s.abs());
                self.rms_acc_l += s * s;
            }
        }
        if input.num_channels >= 2 {
            let ch_r = input.channel(1);
            for &s in ch_r.iter().take(frames) {
                self.peak_r = self.peak_r.max(s.abs());
                self.rms_acc_r += s * s;
            }
        } else {
            self.peak_r = self.peak_l;
            self.rms_acc_r = self.rms_acc_l;
        }
        self.frame_count += frames;
    }

    fn reset(&mut self) {
        self.peak_l = 0.0;
        self.peak_r = 0.0;
        self.rms_acc_l = 0.0;
        self.rms_acc_r = 0.0;
        self.frame_count = 0;
    }
}
