//! Device wrapper — the bridge between external plugin formats and the graph.
//!
//! ## Design principle (Rule 2)
//!
//! The audio graph runtime is **plugin-format agnostic**.  It only understands
//! `AudioNode` — buffers, ports, process calls.  VST3, AU, WASM, and internal
//! devices each provide a wrapper that implements both `AudioNode` (for the
//! graph) and `Device` (for the plugin host / UI layer).
//!
//! ```text
//! External plugin (VST3 / AU / WASM / internal)
//!       │
//!       ▼
//! DeviceWrapper  implements AudioNode
//!       │
//!       ▼
//! AudioGraph  (format-agnostic)
//! ```
//!
//! ## Format isolation
//!
//! The graph runtime never sees VST3 `IComponent` pointers, AU `AudioUnit`
//! handles, or WASM module instances.  All format-specific code lives inside
//! the corresponding wrapper.

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::plugin_system::parameter::ParameterMap;
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::lifecycle::LifecycleState;

// ── Device info ───────────────────────────────────────────────────────────────

/// Static metadata about a plugin device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub format: DeviceFormat,
    pub num_inputs: usize,
    pub num_outputs: usize,
    pub has_midi_input: bool,
    pub has_midi_output: bool,
}

/// The hosting format of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFormat {
    Internal,
    Vst3,
    Au,
    Clap,
    Wasm,
}

// ── Device trait ──────────────────────────────────────────────────────────────

/// Unifies all plugin formats behind a single interface.
///
/// A `Device` is always wrapped in a struct that also implements `AudioNode`.
/// The graph runtime interacts exclusively via `AudioNode::process`.  The
/// `Device` trait is used by the `PluginHost` and UI layer only.
pub trait Device: AudioNode {
    fn device_id(&self) -> &str;
    fn info(&self) -> &DeviceInfo;

    // ── Parameter access ───────────────────────────────────────────────────────

    fn parameters(&self) -> &ParameterMap;
    fn parameters_mut(&mut self) -> &mut ParameterMap;

    fn set_parameter_normalised(&mut self, key: &str, value: f32) {
        use crate::plugin_system::parameter::ParamKey;
        let k = ParamKey::new(key);
        self.parameters_mut().set_normalised(&k, value);
    }

    // ── State serialisation ────────────────────────────────────────────────────

    /// Serialise the complete device state for project save / undo.
    fn save_state(&self) -> DeviceState;

    /// Restore device state from a previously saved snapshot.
    fn load_state(&mut self, state: &DeviceState);

    // ── Lifecycle ──────────────────────────────────────────────────────────────

    fn lifecycle_state(&self) -> LifecycleState;

    /// Move the device to `SUSPENDED` state (pauses DSP, keeps resources).
    fn suspend(&mut self);

    /// Resume from `SUSPENDED` to `RUNNING`.
    fn resume(&mut self);

    /// Permanently tear down the device (releases all resources).
    fn destroy(&mut self);

    // ── Transport / sync ──────────────────────────────────────────────────────

    /// Called when the project BPM or time signature changes.
    fn on_tempo_changed(&mut self, _bpm: f32, _numerator: u8, _denominator: u8) {}

    /// Reported processing latency in samples.
    fn reported_latency(&self) -> u32 {
        0
    }
}

// ── Pass-through wrapper (used as placeholder) ────────────────────────────────

/// A minimal no-op device used while a real plugin is loading or as a
/// compile-time placeholder.
pub struct PassThroughDevice {
    node_id: NodeId,
    info: DeviceInfo,
    params: ParameterMap,
    lifecycle: LifecycleState,
    num_channels: usize,
}

impl PassThroughDevice {
    pub fn new(node_id: NodeId, device_id: impl Into<String>, name: impl Into<String>) -> Self {
        let device_id_str = device_id.into();
        PassThroughDevice {
            node_id,
            info: DeviceInfo {
                device_id: device_id_str,
                name: name.into(),
                vendor: "Internal".to_string(),
                version: "0.1".to_string(),
                format: DeviceFormat::Internal,
                num_inputs: 2,
                num_outputs: 2,
                has_midi_input: false,
                has_midi_output: false,
            },
            params: ParameterMap::new(),
            lifecycle: LifecycleState::Running,
            num_channels: 2,
        }
    }
}

impl AudioNode for PassThroughDevice {
    fn node_id(&self) -> NodeId { self.node_id }
    fn label(&self) -> &str { "PassThrough" }
    fn num_input_ports(&self) -> usize { 1 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.num_channels = ctx.num_channels;
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
        outputs[0].copy_from(inputs[0]);
    }

    fn reset(&mut self) {}
}

impl Device for PassThroughDevice {
    fn device_id(&self) -> &str { &self.info.device_id }
    fn info(&self) -> &DeviceInfo { &self.info }
    fn parameters(&self) -> &ParameterMap { &self.params }
    fn parameters_mut(&mut self) -> &mut ParameterMap { &mut self.params }

    fn save_state(&self) -> DeviceState {
        DeviceState::empty(&self.info.device_id)
    }

    fn load_state(&mut self, _state: &DeviceState) {}

    fn lifecycle_state(&self) -> LifecycleState { self.lifecycle }
    fn suspend(&mut self) { self.lifecycle = LifecycleState::Suspended; }
    fn resume(&mut self) { self.lifecycle = LifecycleState::Running; }
    fn destroy(&mut self) { self.lifecycle = LifecycleState::Destroyed; }
}
