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
//! live playback and call `process_block` in a tight loop.  The live engine,
//! transport, and CPAL stream are never touched.
//!
//! ## Export length and alignment
//!
//! [`render_project_offline`] (the export path) compensates the master latency
//! (the first `N` frames are skipped, so the file lines up with the timeline)
//! and keeps the tail after the last clip until the sound dies out or hits a
//! cap.  [`render_project_in_blocks`] (the C-2 harness path) renders exactly the
//! timeline length, uncompensated, so the golden hashes stay untouched.
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
use crate::audio_graph::node::{TimeSignature, TransportInfo};
use crate::audio_graph::{CompileConfig, GraphScheduler, LiveParams};
use crate::plugin_system::PluginHost;
use crate::project::AppProject;

/// Block size used for the offline render loop.
///
/// Larger blocks reduce per-block overhead (mod dispatch, plan traversal) and
/// max out throughput; chosen to match the live engine's `MAX_BLOCK_FRAMES`
/// so node `prepare()` allocations are sized identically.
const OFFLINE_BLOCK_SIZE: usize = 4096;
const OFFLINE_NUM_CHANNELS: usize = 2;

/// −80 dBFS: under the 16-bit noise floor, so nothing audible is cut.
const TAIL_SILENCE_THRESHOLD: f32 = 1.0e-4;
/// Longer than the internal delay's maximum time (2 s), so a gap between two
/// echoes is not taken for the end of the sound.
const TAIL_SILENCE_HOLD_SECONDS: f64 = 2.5;
const TAIL_MAX_SECONDS: f64 = 10.0;

/// Render the project for export to a stereo buffer pair.
///
/// The result starts at timeline zero (master latency compensated) and ends at
/// the later of the last clip's end and the last audible sample, at most
/// [`TAIL_MAX_SECONDS`] after the last clip.  Returns `Err` if the project has
/// no audio content (duration == 0) — callers should surface this to the user
/// rather than writing an empty file.
pub fn render_project_offline(
    project: &AppProject,
    live_params: &LiveParams,
    plugin_host: &mut PluginHost,
    sample_rate: u32,
) -> Result<(Vec<f32>, Vec<f32>), String> {
    let (scheduler, clip_end_frames) =
        compile_for_render(project, live_params, plugin_host, sample_rate)?;
    let latency_frames = scheduler.latency_samples() as usize;
    let mut renderer = BlockRenderer::new(scheduler, project, sample_rate, OFFLINE_BLOCK_SIZE);

    let mut left = Vec::with_capacity(clip_end_frames + latency_frames);
    let mut right = Vec::with_capacity(clip_end_frames + latency_frames);
    renderer.render_into(clip_end_frames + latency_frames, &mut left, &mut right);
    left.drain(..latency_frames);
    right.drain(..latency_frames);

    append_tail(&mut renderer, &mut left, &mut right, clip_end_frames);
    Ok((left, right))
}

/// Renders exactly the timeline length, uncompensated, processing `block_size`
/// frames per `process_block` call (at most `OFFLINE_BLOCK_SIZE`), the way the
/// live engine receives device-sized buffers.  Nodes are still prepared for
/// `OFFLINE_BLOCK_SIZE`, so a determinism check only varies the block size.
pub(crate) fn render_project_in_blocks(
    project: &AppProject,
    live_params: &LiveParams,
    plugin_host: &mut PluginHost,
    sample_rate: u32,
    block_size: usize,
) -> Result<(Vec<f32>, Vec<f32>), String> {
    assert!(
        (1..=OFFLINE_BLOCK_SIZE).contains(&block_size),
        "block_size must be in 1..={OFFLINE_BLOCK_SIZE}"
    );
    let (scheduler, total_frames) =
        compile_for_render(project, live_params, plugin_host, sample_rate)?;
    let mut renderer = BlockRenderer::new(scheduler, project, sample_rate, block_size);

    let mut left = Vec::with_capacity(total_frames);
    let mut right = Vec::with_capacity(total_frames);
    renderer.render_into(total_frames, &mut left, &mut right);
    Ok((left, right))
}

/// Compiles the project like live playback does; returns the scheduler and the
/// timeline length in frames.
fn compile_for_render(
    project: &AppProject,
    live_params: &LiveParams,
    plugin_host: &mut PluginHost,
    sample_rate: u32,
) -> Result<(GraphScheduler, usize), String> {
    let config = CompileConfig {
        sample_rate: sample_rate as f32,
        max_block_size: OFFLINE_BLOCK_SIZE,
        num_channels: OFFLINE_NUM_CHANNELS,
        live: false,
    };

    // Same denormal mitigation as the live audio thread — DSP feedback paths
    // hit sub-normals on long silences, with the same CPU-spike penalty.
    enable_flush_to_zero();

    let (scheduler, _cells, duration_samples) =
        AudioGraphCompiler::compile_scheduler(project, config, live_params, plugin_host);

    if duration_samples == 0 {
        return Err("Aucun clip à exporter".to_string());
    }
    Ok((scheduler, duration_samples as usize))
}

/// Keeps rendering after the last clip until the sound has been quiet for
/// [`TAIL_SILENCE_HOLD_SECONDS`] or the tail reaches [`TAIL_MAX_SECONDS`], then
/// drops the trailing silence (never going under `clip_end_frames`).
fn append_tail(
    renderer: &mut BlockRenderer,
    left: &mut Vec<f32>,
    right: &mut Vec<f32>,
    clip_end_frames: usize,
) {
    let sample_rate = renderer.sample_rate as f64;
    let hold_frames = (TAIL_SILENCE_HOLD_SECONDS * sample_rate) as usize;
    let limit_frames = clip_end_frames + (TAIL_MAX_SECONDS * sample_rate) as usize;

    let mut audible_end = last_audible_frame(left, right).map_or(0, |frame| frame + 1);
    while left.len() - audible_end < hold_frames && left.len() < limit_frames {
        let start = left.len();
        let frames = OFFLINE_BLOCK_SIZE.min(limit_frames - start);
        renderer.render_into(frames, left, right);
        if let Some(frame) = last_audible_frame(&left[start..], &right[start..]) {
            audible_end = start + frame + 1;
        }
    }

    let end = audible_end.max(clip_end_frames);
    left.truncate(end);
    right.truncate(end);
}

fn last_audible_frame(left: &[f32], right: &[f32]) -> Option<usize> {
    left.iter()
        .zip(right)
        .rposition(|(l, r)| l.abs() > TAIL_SILENCE_THRESHOLD || r.abs() > TAIL_SILENCE_THRESHOLD)
}

/// Pulls consecutive blocks out of a scheduler, keeping the transport position
/// between calls.
struct BlockRenderer<'a> {
    scheduler: GraphScheduler,
    project: &'a AppProject,
    time_signature: TimeSignature,
    sample_rate: u32,
    block_size: usize,
    mix_buffer: Vec<f32>,
    position: u64,
}

impl<'a> BlockRenderer<'a> {
    fn new(
        scheduler: GraphScheduler,
        project: &'a AppProject,
        sample_rate: u32,
        block_size: usize,
    ) -> Self {
        Self {
            scheduler,
            project,
            time_signature: TimeSignature::new(project.numerator, project.denominator),
            sample_rate,
            block_size,
            mix_buffer: vec![0.0_f32; OFFLINE_BLOCK_SIZE * 2],
            position: 0,
        }
    }

    fn render_into(&mut self, frames: usize, left: &mut Vec<f32>, right: &mut Vec<f32>) {
        let mut rendered = 0;
        while rendered < frames {
            let block = (frames - rendered).min(self.block_size);

            let info = TransportInfo {
                is_playing: true,
                position_samples: self.position,
                position_seconds: self.position as f64 / self.sample_rate as f64,
                bpm: self.project.bpm,
                time_signature: self.time_signature,
                sample_rate: self.sample_rate as f32,
                block_size: block,
            };

            let mix = &mut self.mix_buffer[..block * 2];
            mix.fill(0.0);
            self.scheduler.process_block(&info, mix);

            for frame in mix.chunks_exact(2) {
                left.push(frame[0]);
                right.push(frame[1]);
            }

            self.position += block as u64;
            rendered += block;
        }
    }
}
