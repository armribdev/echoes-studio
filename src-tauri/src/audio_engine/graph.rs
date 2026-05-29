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
    AudioGraph,
    GraphCompiler,
    GraphScheduler,
    CompileConfig,
    LiveParams,
    MeterCell,
};
use crate::project::AppProject;

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
    ) -> (GraphScheduler, HashMap<String, MeterCell>, u64) {
        let duration_secs = project.tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(|c| c.end)
            .fold(0.0_f64, f64::max);
        let duration_samples = (duration_secs * config.sample_rate as f64) as u64;

        // Drop registry entries for tracks/modulators/routes that no longer
        // exist in the project; refresh the global `any_solo` flag.  Existing
        // atomics are kept intact so an in-flight hot-path command racing with
        // this recompile cannot be clobbered.
        live_params.prune_to_project(project);

        let graph = AudioGraph::from_project(project);

        match GraphCompiler::compile(&graph, project, config, live_params) {
            Ok(compiled) => {
                let cells = compiled.meter_cells.clone();
                (GraphScheduler::new(compiled), cells, duration_samples)
            }
            Err(e) => {
                eprintln!("[audio_graph] Compile error: {} — using empty graph", e);
                (GraphScheduler::empty(config.sample_rate), HashMap::new(), duration_samples)
            }
        }
    }
}
