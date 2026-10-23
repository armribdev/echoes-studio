//! Plugin system — unified device abstraction layer.
//!
//! ## Relationship to the audio graph
//!
//! The plugin system is a **support layer** for the audio graph, not the
//! central architecture.  It answers the question "how do we safely integrate
//! external code?", while the audio graph answers "how do we execute DSP?".
//!
//! Plugin wrappers implement [`crate::audio_graph::node::AudioNode`] around a
//! shared [`instance::PluginInstance`].  The graph compiler asks the
//! [`host::PluginHost`] to build a `Box<dyn AudioNode>` for each plugin node;
//! after that the graph runtime treats it like any other node.
//!
//! ## Module map
//!
//! - [`parameter`]    — `PluginParamInfo` (parameter metadata)
//! - [`wrapper`]      — `DeviceFormat` + `PassThroughDevice` placeholder
//! - [`lifecycle`]    — `LifecycleState` + transition validator
//! - [`state`]        — `DeviceState` for project save/load
//! - [`registry`]     — `PluginManifest` + `PluginRegistry` (discovery)
//! - [`host`]         — `PluginHost` (lifecycle, parameter routing, state)
//! - [`instance`]     — `PluginInstance` (Phase 2a): Arc-shared native handle
//! - [`track_info`]   — what each placement is told about its track (`clap.track-info`)
//! - [`bridge`]       — out-of-process sandbox transport primitives (Phase 6c-i,
//!   not yet wired; see ADR-015)

pub mod bridge;
pub mod bridged_node;
pub mod crash_marker;
pub mod gui;
pub mod host;
pub mod instance;
pub mod lifecycle;
pub mod param_queue;
pub mod parameter;
pub mod presets;
pub mod registry;
pub mod state;
pub mod track_info;
pub mod wrapper;

#[cfg(test)]
mod ffi_lifecycle_tests;
#[cfg(all(test, windows))]
mod sandbox_tests;
#[cfg(all(test, windows))]
mod vst3_preset_file_tests;
#[cfg(all(test, windows))]
mod vst3_mirror_tests;
#[cfg(all(test, windows))]
mod vst3_output_bus_tests;
#[cfg(all(test, windows))]
mod vst3_modulated_state_tests;
#[cfg(all(test, windows))]
mod vst3_program_tests;
#[cfg(all(test, windows))]
mod vst3_sidechain_tests;
#[cfg(all(test, windows))]
mod vst3_track_info_tests;

pub use host::PluginHost;
