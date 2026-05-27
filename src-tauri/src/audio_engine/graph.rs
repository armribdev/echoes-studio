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

use crate::audio_graph::{
    AudioGraph,
    GraphCompiler,
    GraphScheduler,
    CompileConfig,
};
use crate::project::AppProject;

/// Bridges `AppProject` → `GraphScheduler`.
///
/// Replaces the previous flat snapshot compiler.
pub struct AudioGraphCompiler;

impl AudioGraphCompiler {
    /// Compile the current project state into a `GraphScheduler`.
    ///
    /// Called on the command thread (never the audio thread).
    /// May allocate freely.
    pub fn compile_scheduler(
        project: &AppProject,
        config: CompileConfig,
    ) -> GraphScheduler {
        let graph = AudioGraph::from_project(project);

        match GraphCompiler::compile(&graph, project, config) {
            Ok(compiled) => GraphScheduler::new(compiled),
            Err(e) => {
                eprintln!("[audio_graph] Compile error: {} — using empty graph", e);
                GraphScheduler::empty(config.sample_rate)
            }
        }
    }
}
