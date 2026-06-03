//! Audio Unit wrapper node — adapts an AU instrument/effect into `AudioNode`.
//!
//! Status: **stub — macOS implementation pending**.
//!
//! ## Implementation plan
//!
//! 1. `AudioComponentInstanceNew` → `AudioUnit` handle
//! 2. `AudioUnitSetProperty(kAudioUnitProperty_StreamFormat, ...)` for SR/channels
//! 3. `AudioUnitInitialize`
//! 4. On process: use `AudioUnitRenderActionFlags` + `AudioBufferList` bridging
//!    to map between the scheduler's `AudioBuffer` slices and CoreAudio's layout.
//! 5. Parameter control: `AudioUnitSetParameter` with parameter IDs.
//! 6. State: `AudioUnitGetProperty(kAudioUnitProperty_ClassInfo, ...)` → CFDictionary → JSON.

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::parameter::ParameterMap;
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{Device, DeviceFormat, DeviceInfo};

pub struct AuWrapperNode {
    node_id: NodeId,
    device_id: String,
    info: DeviceInfo,
    params: ParameterMap,
    lifecycle: LifecycleState,
}

impl AuWrapperNode {
    pub fn new(node_id: NodeId, device_id: impl Into<String>, name: impl Into<String>) -> Self {
        let id = device_id.into();
        Self {
            node_id,
            info: DeviceInfo {
                device_id: id.clone(),
                name: name.into(),
                vendor: "Unknown".to_string(),
                version: "0.0".to_string(),
                format: DeviceFormat::Au,
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

impl AudioNode for AuWrapperNode {
    fn node_id(&self) -> NodeId {
        self.node_id
    }
    fn label(&self) -> &str {
        "AuWrapperNode"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }
    fn prepare(&mut self, _ctx: PrepareContext) {
        self.lifecycle = LifecycleState::Running;
    }
    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], _: &TransportInfo) {
        if !inputs.is_empty() && !outputs.is_empty() {
            outputs[0].copy_from(inputs[0]);
        }
    }
    fn reset(&mut self) {}
}

impl Device for AuWrapperNode {
    fn device_id(&self) -> &str {
        &self.device_id
    }
    fn info(&self) -> &DeviceInfo {
        &self.info
    }
    fn parameters(&self) -> &ParameterMap {
        &self.params
    }
    fn parameters_mut(&mut self) -> &mut ParameterMap {
        &mut self.params
    }
    fn save_state(&self) -> DeviceState {
        DeviceState::empty(&self.device_id)
    }
    fn load_state(&mut self, _state: &DeviceState) {}
    fn lifecycle_state(&self) -> LifecycleState {
        self.lifecycle
    }
    fn suspend(&mut self) {
        self.lifecycle = LifecycleState::Suspended;
    }
    fn resume(&mut self) {
        self.lifecycle = LifecycleState::Running;
    }
    fn destroy(&mut self) {
        self.lifecycle = LifecycleState::Destroyed;
    }
}
