//! Offline rendering — drives the real-time graph from the command thread.
//!
//! ## Purpose
//!
//! Mixdown export must produce **exactly** what the user hears live.  Before
//! this module existed, the frontend rendered a second graph via Web Audio's
//! `OfflineAudioContext` and shipped raw samples to `export_wav` / `export_mp3`.
//! That made Web Audio a parallel DSP engine — a silent divergence risk
//! (different gain laws, no plugins, no modulation, no PDC).
//!
//! Now exports compile the same `AudioGraph` → `GraphScheduler` pipeline as
//! live playback and call `process_block` in a tight loop until the project
//! duration is reached.  The live engine, transport, and CPAL stream are never
//! touched.
//!
//! ## Threading
//!
//! Runs entirely on the command thread.  The freshly compiled scheduler is
//! owned by this module for the duration of the render and dropped here (off
//! the audio thread), so the RT-safety contract is irrelevant: this code may
//! allocate freely.
//!
//! ## Shared state with the live engine
//!
//! The `LiveParams` registry and `PluginHost` are shared with the live engine
//! so that exports reflect current knob positions and plugin parameter state.
//! No `enqueue` is performed — the live scheduler is untouched.

use crate::audio_engine::graph::AudioGraphCompiler;
use crate::audio_graph::denormal::enable_flush_to_zero;
use crate::audio_graph::node::TransportInfo;
use crate::audio_graph::{CompileConfig, LiveParams};
use crate::plugin_system::PluginHost;
use crate::project::AppProject;

/// Block size used for the offline render loop.
///
/// Larger blocks reduce per-block overhead (mod dispatch, plan traversal) and
/// max out throughput; chosen to match the live engine's `MAX_BLOCK_FRAMES`
/// so node `prepare()` allocations are sized identically.
const OFFLINE_BLOCK_SIZE: usize = 4096;
const OFFLINE_NUM_CHANNELS: usize = 2;

/// Render the entire project to a stereo interleaved buffer pair.
///
/// Returns `(samples_left, samples_right)` of `ceil(project_duration_seconds *
/// sample_rate)` samples each.  Returns `Err` if the project has no audio
/// content (duration == 0) — callers should surface this to the user rather
/// than writing an empty file.
pub fn render_project_offline(
    project: &AppProject,
    live_params: &LiveParams,
    plugin_host: &mut PluginHost,
    sample_rate: u32,
) -> Result<(Vec<f32>, Vec<f32>), String> {
    let config = CompileConfig {
        sample_rate: sample_rate as f32,
        max_block_size: OFFLINE_BLOCK_SIZE,
        num_channels: OFFLINE_NUM_CHANNELS,
    };

    // Same denormal mitigation as the live audio thread — DSP feedback paths
    // hit sub-normals on long silences, with the same CPU-spike penalty.
    enable_flush_to_zero();

    let (mut scheduler, _cells, duration_samples) =
        AudioGraphCompiler::compile_scheduler(project, config, live_params, plugin_host);

    if duration_samples == 0 {
        return Err("Aucun clip à exporter".to_string());
    }

    let total_frames = duration_samples as usize;
    let mut left = Vec::with_capacity(total_frames);
    let mut right = Vec::with_capacity(total_frames);

    let mut mix_buf = vec![0.0_f32; OFFLINE_BLOCK_SIZE * 2];
    let bpm = project.bpm;
    let sr_f32 = sample_rate as f32;
    let mut position: u64 = 0;

    while (position as usize) < total_frames {
        let remaining = total_frames - position as usize;
        let block = remaining.min(OFFLINE_BLOCK_SIZE);

        let info = TransportInfo {
            is_playing: true,
            position_samples: position,
            position_seconds: position as f64 / sample_rate as f64,
            bpm,
            sample_rate: sr_f32,
            block_size: block,
        };

        let mix = &mut mix_buf[..block * 2];
        for s in mix.iter_mut() {
            *s = 0.0;
        }
        scheduler.process_block(&info, mix);

        for f in 0..block {
            left.push(mix[f * 2]);
            right.push(mix[f * 2 + 1]);
        }

        position += block as u64;
    }

    Ok((left, right))
}
