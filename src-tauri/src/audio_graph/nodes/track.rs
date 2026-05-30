//! Track node — reads timeline clips and outputs audio for one track.
//!
//! ## Role in the graph
//!
//! `TrackNode` is a **source** (0 inputs, 1 output).  It reads pre-decoded
//! f32 samples from its `clips` list, applies no gain (gain is handled by
//! the downstream `GainNode`), and writes the summed audio to output port 0.
//!
//! ## Real-time contract
//!
//! `process` never allocates.  All clip data is held in `Arc<Vec<f32>>`
//! shared from the project snapshot; no copies are made.

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

// ── Clip data ─────────────────────────────────────────────────────────────────

/// Pre-decoded audio clip data, ready for real-time playback.
pub struct ClipData {
    /// Clip start in the project timeline, in samples at the engine sample rate.
    pub start_sample: u64,
    /// Clip end in the project timeline, in samples at the engine sample rate.
    pub end_sample: u64,
    /// Decoded mono f32 samples (−1.0 … +1.0).
    /// Shared via `Arc` so multiple snapshots can reference the same data.
    pub samples: Arc<Vec<f32>>,
    /// Sample rate of the source audio.  Used for pitch-correct playback
    /// when the engine sample rate differs (future: SRC resampling).
    pub native_sample_rate: u32,
}

// ── TrackNode ─────────────────────────────────────────────────────────────────

pub struct TrackNode {
    id: NodeId,
    clips: Vec<ClipData>,
    engine_sample_rate: f32,
    num_channels: usize,
}

impl TrackNode {
    pub fn new(id: NodeId, clips: Vec<ClipData>) -> Self {
        TrackNode {
            id,
            clips,
            engine_sample_rate: 48_000.0,
            num_channels: 2,
        }
    }
}

impl AudioNode for TrackNode {
    fn node_id(&self) -> NodeId { self.id }
    fn label(&self) -> &str { "TrackNode" }
    fn num_input_ports(&self) -> usize { 0 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.engine_sample_rate = ctx.sample_rate;
        self.num_channels = ctx.num_channels;
    }

    /// Render active clips into the output buffer.
    ///
    /// Algorithm:
    ///   For each clip active in `[pos, pos+block_size)`:
    ///     Compute `[start_frame, end_frame)` = intersection with the block once,
    ///     then accumulate samples only in that range (overlapping clips supported).
    ///
    /// No allocation.  Mono source is duplicated to all output channels.
    fn process(
        &mut self,
        _inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        let output = &mut outputs[0];
        output.clear();

        if !transport.is_playing {
            return;
        }

        let pos = transport.position_samples;
        let block = transport.block_size;

        for clip in &self.clips {
            // Fast reject: clip entirely outside this block
            if pos + block as u64 <= clip.start_sample || pos >= clip.end_sample {
                continue;
            }

            // Compute the source-position ratio for linear-interpolated pitch
            // correction when native SR ≠ engine SR.
            let sr_ratio = if clip.native_sample_rate > 0 {
                clip.native_sample_rate as f64 / self.engine_sample_rate as f64
            } else {
                1.0
            };

            // Intersection of block [pos, pos+block) with clip [start, end), as frame indices.
            let overlap_start = pos.max(clip.start_sample);
            let overlap_end = (pos + block as u64).min(clip.end_sample);
            let start_frame = (overlap_start - pos) as usize;
            let end_frame = (overlap_end - pos) as usize;

            // Write channel 0 (mono source) — linear interpolation between
            // adjacent source samples when native SR ≠ engine SR.
            let out_ch0 = output.channel_mut(0);
            for frame in start_frame..end_frame {
                let local_offset = overlap_start - clip.start_sample + (frame - start_frame) as u64;
                let src_pos = local_offset as f64 * sr_ratio;
                let idx0 = src_pos as usize;
                if idx0 < clip.samples.len() {
                    let s0 = clip.samples[idx0];
                    let s1 = if idx0 + 1 < clip.samples.len() {
                        clip.samples[idx0 + 1]
                    } else {
                        s0
                    };
                    let frac = (src_pos - idx0 as f64) as f32;
                    out_ch0[frame] += s0 + frac * (s1 - s0);
                }
            }
        }

        // Duplicate ch0 to remaining channels (mono → stereo / multichannel)
        output.mono_to_all_channels();
    }

    fn reset(&mut self) {
        // TrackNode has no stateful DSP; transport position is external.
    }
}
