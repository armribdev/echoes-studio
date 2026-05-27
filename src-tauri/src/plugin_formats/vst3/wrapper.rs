//! VST3 wrapper — adapts a VST3 `IAudioProcessor` into an `AudioNode`.
//!
//! ## Implementation plan (not yet active)
//!
//! 1. Load the plugin binary via `ffi::Vst3Ffi`.
//! 2. Call `IPluginFactory::createInstance` to get `IComponent`.
//! 3. Query `IAudioProcessor` interface.
//! 4. On `prepare`: call `setupProcessing` with the engine's sample rate
//!    and block size.
//! 5. On `process`: build a `ProcessData` struct with the pre-allocated
//!    buffer slices and call `IAudioProcessor::process`.
//! 6. Mirror parameter changes: translate `ParamKey` → VST3 parameter ID
//!    and call `IEditController::setParamNormalized`.
//!
//! ## Buffer translation
//!
//! VST3 uses channel pointer arrays (`float** inputs`, `float** outputs`).
//! We map from the scheduler's `AudioBuffer` slices to these arrays using
//! a pre-allocated `Vec<*mut f32>` that is filled at each block boundary.
//! No heap allocation occurs in `process`.
//!
//! ## State serialisation
//!
//! VST3 state is an `IBStream` byte stream.  We wrap it in `DeviceState::Binary`.
//!
//! ## Crash isolation
//!
//! VST3 plugins run in-process.  Best-effort crash isolation can be achieved
//! with platform signal handlers or by running plugins in a subprocess
//! (future work).

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::parameter::ParameterMap;
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{Device, DeviceFormat, DeviceInfo};

/// VST3 wrapper node.
///
/// Placeholder until the VST3 FFI is wired in.  Outputs silence.
pub struct VstWrapperNode {
    node_id: NodeId,
    device_id: String,
    info: DeviceInfo,
    params: ParameterMap,
    lifecycle: LifecycleState,
}

impl VstWrapperNode {
    pub fn new(node_id: NodeId, device_id: impl Into<String>, name: impl Into<String>) -> Self {
        let id = device_id.into();
        VstWrapperNode {
            node_id,
            info: DeviceInfo {
                device_id: id.clone(),
                name: name.into(),
                vendor: "Unknown".to_string(),
                version: "0.0".to_string(),
                format: DeviceFormat::Vst3,
                num_inputs: 2,
                num_outputs: 2,
                has_midi_input: false,
                has_midi_output: false,
            },
            device_id: id,
            params: ParameterMap::new(),
            lifecycle: LifecycleState::Created,
        }
    }
}

impl AudioNode for VstWrapperNode {
    fn node_id(&self) -> NodeId { self.node_id }
    fn label(&self) -> &str { "VstWrapperNode" }
    fn num_input_ports(&self) -> usize { 1 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, _ctx: PrepareContext) {
        // TODO: call IAudioProcessor::setupProcessing
        self.lifecycle = LifecycleState::Running;
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
        // TODO: build ProcessData, call IAudioProcessor::process
        // For now: pass through
        if !inputs.is_empty() && !outputs.is_empty() {
            outputs[0].copy_from(inputs[0]);
        }
    }

    fn reset(&mut self) {}
}

impl Device for VstWrapperNode {
    fn device_id(&self) -> &str { &self.device_id }
    fn info(&self) -> &DeviceInfo { &self.info }
    fn parameters(&self) -> &ParameterMap { &self.params }
    fn parameters_mut(&mut self) -> &mut ParameterMap { &mut self.params }

    fn save_state(&self) -> DeviceState {
        // TODO: call IComponent::getState → IBStream → Vec<u8>
        DeviceState::empty(&self.device_id)
    }

    fn load_state(&mut self, _state: &DeviceState) {
        // TODO: call IComponent::setState
    }

    fn lifecycle_state(&self) -> LifecycleState { self.lifecycle }
    fn suspend(&mut self) { self.lifecycle = LifecycleState::Suspended; }
    fn resume(&mut self) { self.lifecycle = LifecycleState::Running; }
    fn destroy(&mut self) { self.lifecycle = LifecycleState::Destroyed; }
}
