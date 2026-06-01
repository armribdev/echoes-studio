//! Plugin system — unified device abstraction layer.
//!
//! ## Relationship to the audio graph
//!
//! The plugin system is a **support layer** for the audio graph, not the
//! central architecture.  It answers the question "how do we safely integrate
//! external code?", while the audio graph answers "how do we execute DSP?".
//!
//! Plugin wrappers implement [`wrapper::Device`] (which in turn implements
//! [`crate::audio_graph::node::AudioNode`]).  The graph compiler asks the
//! [`host::PluginHost`] to build a `Box<dyn AudioNode>` for each plugin node;
//! after that the graph runtime treats it like any other node.
//!
//! ## Module map
//!
//! - [`parameter`]    — `Param`, `ParameterMap`, `ParameterRange`
//! - [`wrapper`]      — `Device` trait + `PassThroughDevice` placeholder
//! - [`lifecycle`]    — `LifecycleState` + transition validator
//! - [`state`]        — `DeviceState` for project save/load
//! - [`registry`]     — `PluginManifest` + `PluginRegistry` (discovery)
//! - [`host`]         — `PluginHost` (lifecycle, parameter routing, state)
//! - [`capabilities`] — `DeviceCapabilities` introspected device metadata (Phase 1)
//! - [`instance`]     — `PluginInstance` (Phase 2a): Arc-shared native handle

pub mod capabilities;
pub mod gui;
pub mod host;
pub mod instance;
pub mod lifecycle;
pub mod param_queue;
pub mod parameter;
pub mod registry;
pub mod state;
pub mod wrapper;

pub use capabilities::{
    AudioBusInfo, DeviceCapabilities, MidiPortInfo, NativeParamId, ParameterFlags,
    ParameterInfo, SpeakerLayout,
};
pub use host::PluginHost;
pub use instance::{NativeHandle, PluginInstance};
pub use lifecycle::LifecycleState;
pub use param_queue::{ParamChange, ParamQueue};
pub use parameter::{Param, ParameterMap, ParameterRange};
pub use registry::{PluginManifest, PluginRegistry};
pub use state::DeviceState;
pub use wrapper::{Device, DeviceFormat, DeviceInfo, PassThroughDevice};
