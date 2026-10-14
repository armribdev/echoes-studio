//! Audio graph compiler bridge.
//!
//! This module is the single integration point between `AppProject` state and
//! the new `audio_graph` subsystem.  The gateway's `recompile_if_needed`
//! function calls `ProjectGraphCompiler::compile`, which:
//!
//! 1. Builds an `AudioGraph` topology from the current project state.
//! 2. Runs the `GraphCompiler` (topological sort, node factory).
//! 3. Wraps the result in a `GraphScheduler` ready for the audio thread.
//! 4. Returns the scheduler for delivery to `AudioEngine::update_scheduler`.
//!
//! ## Why keep this bridge file?
//!
//! The `engine_gateway` module imports `AudioGraphCompiler` by name.
//! Keeping this file with the same public name ensures the gateway doesn't
//! need changes during the migration.  The implementation is now fully
//! delegated to the new `audio_graph` subsystem.

use std::collections::HashMap;

use crate::audio_graph::{
    AudioGraph, CompileConfig, GraphCompiler, GraphScheduler, LiveParams, MeterCell,
};
use crate::plugin_system::PluginHost;
use crate::project::AppProject;

/// Number of pool workers the live scheduler should use, parsed from the
/// `ECHOES_AUDIO_WORKERS` environment variable.  Defaults to `0` (purely
/// sequential level execution — unchanged behaviour).  Negative or
/// non-numeric values are treated as `0`.
///
/// A future change can swap this for a project / device setting; the env var
/// gives us an opt-in path to benchmark parallel execution without committing
/// to a default-on rollout that could shift XRun characteristics.
pub fn audio_worker_count() -> usize {
    std::env::var("ECHOES_AUDIO_WORKERS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

/// Bridges `AppProject` → `GraphScheduler` + per-track meter cells.
pub struct AudioGraphCompiler;

impl AudioGraphCompiler {
    /// Compile the current project state.
    ///
    /// Returns both the scheduler (sent to the audio thread via `RuntimeSnapshot`)
    /// and the meter cells (held by `AudioEngine` and read by the reporter thread).
    ///
    /// Called on the command thread (never the audio thread).
    pub fn compile_scheduler(
        project: &AppProject,
        config: CompileConfig,
        live_params: &LiveParams,
        plugin_host: &mut PluginHost,
    ) -> (GraphScheduler, HashMap<String, MeterCell>, u64) {
        let duration_samples = (project.timeline_end_secs() * config.sample_rate as f64) as u64;

        // Keep existing atomics intact: an in-flight hot-path command racing
        // this recompile must not be clobbered.
        live_params.prune_to_project(project);
        plugin_host.sync_track_info(project);

        let graph = AudioGraph::from_project(project);

        let num_workers = audio_worker_count();

        match GraphCompiler::compile(&graph, project, config, live_params, plugin_host) {
            Ok(compiled) => {
                let cells = compiled.meter_cells.clone();
                (
                    GraphScheduler::with_workers(compiled, num_workers),
                    cells,
                    duration_samples,
                )
            }
            Err(e) => {
                eprintln!("[audio_graph] Compile error: {} — using empty graph", e);
                (
                    GraphScheduler::empty(config.sample_rate),
                    HashMap::new(),
                    duration_samples,
                )
            }
        }
    }
}
