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
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};

#[derive(Default)]
pub struct AuWrapperNode;

impl AuWrapperNode {
    pub fn new() -> Self {
        Self
    }
}

impl AudioNode for AuWrapperNode {
    fn label(&self) -> &str {
        "AuWrapperNode"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }
    fn prepare(&mut self, _ctx: PrepareContext) {}
    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], _: &TransportInfo) {
        if !inputs.is_empty() && !outputs.is_empty() {
            outputs[0].copy_from(inputs[0]);
        }
    }
    fn reset(&mut self) {}
}
