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
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};

pub struct MeterNode {
    /// Shared cells written by this node and read by the reporter thread.
    peak_l_out: Arc<AtomicU32>,
    peak_r_out: Arc<AtomicU32>,
}

impl MeterNode {
    /// Create a node that writes its per-block peak into the provided atomics.
    pub fn new(peak_l: Arc<AtomicU32>, peak_r: Arc<AtomicU32>) -> Self {
        Self {
            peak_l_out: peak_l,
            peak_r_out: peak_r,
        }
    }
}

impl AudioNode for MeterNode {
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

        let mut peak_l = 0.0_f32;
        let mut peak_r = 0.0_f32;

        if input.num_channels >= 1 {
            for &s in input.channel(0) {
                peak_l = peak_l.max(s.abs());
            }
        }
        if input.num_channels >= 2 {
            for &s in input.channel(1) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::node::TimeSignature;

    fn transport(frames: usize) -> TransportInfo {
        TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: frames,
        }
    }

    fn f32_from_bits(cell: &AtomicU32) -> f32 {
        f32::from_bits(cell.load(Ordering::Relaxed))
    }

    #[test]
    fn passes_audio_through_unchanged() {
        let peak_l = Arc::new(AtomicU32::new(0));
        let peak_r = Arc::new(AtomicU32::new(0));
        let mut node = MeterNode::new(Arc::clone(&peak_l), Arc::clone(&peak_r));
        let mut input = AudioBuffer::new(1, 3);
        input.channel_mut(0).copy_from_slice(&[0.1, -0.2, 0.3]);
        let mut output = [AudioBuffer::new(1, 3)];

        node.process(&[&input], &mut output, &transport(3));

        assert_eq!(output[0].channel(0), input.channel(0));
    }

    #[test]
    fn records_per_channel_absolute_peak() {
        let peak_l = Arc::new(AtomicU32::new(0));
        let peak_r = Arc::new(AtomicU32::new(0));
        let mut node = MeterNode::new(Arc::clone(&peak_l), Arc::clone(&peak_r));
        let mut input = AudioBuffer::new(2, 3);
        input.channel_mut(0).copy_from_slice(&[0.1, -0.9, 0.3]);
        input.channel_mut(1).copy_from_slice(&[0.2, 0.2, -0.5]);
        let mut output = [AudioBuffer::new(2, 3)];

        node.process(&[&input], &mut output, &transport(3));

        assert!((f32_from_bits(&peak_l) - 0.9).abs() < 1e-6);
        assert!((f32_from_bits(&peak_r) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn mono_input_mirrors_left_peak_into_right() {
        let peak_l = Arc::new(AtomicU32::new(0));
        let peak_r = Arc::new(AtomicU32::new(0));
        let mut node = MeterNode::new(Arc::clone(&peak_l), Arc::clone(&peak_r));
        let mut input = AudioBuffer::new(1, 2);
        input.channel_mut(0).copy_from_slice(&[0.4, -0.6]);
        let mut output = [AudioBuffer::new(1, 2)];

        node.process(&[&input], &mut output, &transport(2));

        assert!((f32_from_bits(&peak_l) - 0.6).abs() < 1e-6);
        assert!((f32_from_bits(&peak_r) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn reset_zeroes_both_meter_cells() {
        let peak_l = Arc::new(AtomicU32::new(f32::to_bits(0.8)));
        let peak_r = Arc::new(AtomicU32::new(f32::to_bits(0.8)));
        let mut node = MeterNode::new(Arc::clone(&peak_l), Arc::clone(&peak_r));

        node.reset();

        assert_eq!(peak_l.load(Ordering::Relaxed), 0);
        assert_eq!(peak_r.load(Ordering::Relaxed), 0);
    }
}
