//! Internal WASM plugin wrapper.
//!
//! Wraps a loaded WASM module as a graph-compatible `AudioNode`.

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

pub struct WasmWrapperNode {
    node_id: NodeId,
}

impl WasmWrapperNode {
    pub fn new(node_id: NodeId) -> Self {
        Self { node_id }
    }
}

impl AudioNode for WasmWrapperNode {
    fn node_id(&self) -> NodeId {
        self.node_id
    }
    fn label(&self) -> &str {
        "WasmWrapperNode"
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
