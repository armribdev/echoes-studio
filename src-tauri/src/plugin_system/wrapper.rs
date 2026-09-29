//! Device wrapper — the bridge between external plugin formats and the graph.
//!
//! ## Design principle (Rule 2)
//!
//! The audio graph runtime is **plugin-format agnostic**.  It only understands
//! `AudioNode` — buffers, ports, process calls.  Each format provides a wrapper
//! that implements `AudioNode` around a shared `Arc<PluginInstance>` (ADR-007);
//! host-side concerns (lifecycle, state, parameters) live on `PluginHost` and
//! `PluginInstance`, not on the wrapper.
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

/// The hosting format of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFormat {
    Internal,
    Vst3,
    #[cfg(feature = "au")]
    Au,
    Clap,
    #[cfg(feature = "wasm")]
    Wasm,
}

/// Stereo pass-through node standing in for an internal-format device.
pub struct PassThroughDevice {
    node_id: NodeId,
}

impl PassThroughDevice {
    pub fn new(node_id: NodeId) -> Self {
        Self { node_id }
    }
}

impl AudioNode for PassThroughDevice {
    fn node_id(&self) -> NodeId {
        self.node_id
    }
    fn label(&self) -> &str {
        "PassThrough"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, _ctx: PrepareContext) {}

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
