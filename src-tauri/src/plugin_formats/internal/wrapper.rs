//! Internal WASM plugin wrapper.
//!
//! Wraps a loaded WASM module as a graph-compatible `AudioNode`.

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::parameter::ParameterMap;
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{Device, DeviceFormat, DeviceInfo};

pub struct WasmWrapperNode {
    node_id: NodeId,
    device_id: String,
    info: DeviceInfo,
    params: ParameterMap,
    lifecycle: LifecycleState,
}

impl WasmWrapperNode {
    pub fn new(node_id: NodeId, device_id: impl Into<String>, name: impl Into<String>) -> Self {
        let id = device_id.into();
        WasmWrapperNode {
            node_id,
            info: DeviceInfo {
                device_id: id.clone(),
                name: name.into(),
                vendor: "Community".to_string(),
                version: "0.0".to_string(),
                format: DeviceFormat::Wasm,
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

impl AudioNode for WasmWrapperNode {
    fn node_id(&self) -> NodeId { self.node_id }
    fn label(&self) -> &str { "WasmWrapperNode" }
    fn num_input_ports(&self) -> usize { 1 }
    fn num_output_ports(&self) -> usize { 1 }
    fn prepare(&mut self, _ctx: PrepareContext) { self.lifecycle = LifecycleState::Running; }
    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], _: &TransportInfo) {
        if !inputs.is_empty() && !outputs.is_empty() {
            outputs[0].copy_from(inputs[0]);
        }
    }
    fn reset(&mut self) {}
}

impl Device for WasmWrapperNode {
    fn device_id(&self) -> &str { &self.device_id }
    fn info(&self) -> &DeviceInfo { &self.info }
    fn parameters(&self) -> &ParameterMap { &self.params }
    fn parameters_mut(&mut self) -> &mut ParameterMap { &mut self.params }
    fn save_state(&self) -> DeviceState { DeviceState::empty(&self.device_id) }
    fn load_state(&mut self, _: &DeviceState) {}
    fn lifecycle_state(&self) -> LifecycleState { self.lifecycle }
    fn suspend(&mut self) { self.lifecycle = LifecycleState::Suspended; }
    fn resume(&mut self) { self.lifecycle = LifecycleState::Running; }
    fn destroy(&mut self) { self.lifecycle = LifecycleState::Destroyed; }
}
