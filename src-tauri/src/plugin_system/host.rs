//! Plugin host — manages the lifecycle of all device instances.
//!
//! The `PluginHost` is the single entry point for all plugin operations:
//! loading, instantiation, parameter sync, state management, and teardown.
//!
//! ## Responsibilities
//!
//! - Load plugin manifests from the registry
//! - Instantiate device wrappers (`Device + AudioNode`)
//! - Manage lifecycle state transitions
//! - Route parameter changes from the UI to the correct device
//! - Serialise/deserialise device state for project save/load
//! - Provide `AudioNode` handles to the graph compiler
//!
//! ## Thread safety
//!
//! `PluginHost` runs on the **command/gateway thread**, never on the audio
//! thread.  Plugin instances are moved into `GraphScheduler` (audio thread)
//! when a graph is compiled, after which the host no longer has direct access
//! to those instances.  State changes arrive via the command pipeline.

use std::collections::HashMap;

use crate::audio_graph::node::{AudioNode, NodeId};
use crate::plugin_system::lifecycle::{LifecycleState, LifecycleError, transition};
use crate::plugin_system::parameter::ParamKey;
use crate::plugin_system::registry::{PluginManifest, PluginRegistry};
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{Device, DeviceInfo, DeviceFormat, PassThroughDevice};

// ── Host error ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum HostError {
    DeviceNotFound(String),
    PluginNotFound(String),
    LifecycleError(LifecycleError),
    InstantiationFailed(String),
    ParameterNotFound(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::DeviceNotFound(id)      => write!(f, "device not found: {}", id),
            HostError::PluginNotFound(id)      => write!(f, "plugin not in registry: {}", id),
            HostError::LifecycleError(e)       => write!(f, "lifecycle error: {}", e),
            HostError::InstantiationFailed(e)  => write!(f, "instantiation failed: {}", e),
            HostError::ParameterNotFound(k)    => write!(f, "parameter not found: {}", k),
        }
    }
}

// ── Device record ─────────────────────────────────────────────────────────────

/// The host's view of a live device instance.
struct DeviceRecord {
    /// Opaque device ID (matches the graph node's `device_id`).
    id: String,
    /// Current lifecycle state (mirrored from the device; updated on transitions).
    lifecycle: LifecycleState,
    /// Saved state (updated after each UI parameter change for undo support).
    saved_state: DeviceState,
}

// ── PluginHost ────────────────────────────────────────────────────────────────

pub struct PluginHost {
    registry: PluginRegistry,
    /// Metadata records for every device the host knows about.
    records: HashMap<String, DeviceRecord>,
}

impl PluginHost {
    pub fn new() -> Self {
        PluginHost {
            registry: PluginRegistry::new(),
            records: HashMap::new(),
        }
    }

    // ── Registry ───────────────────────────────────────────────────────────────

    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut PluginRegistry {
        &mut self.registry
    }

    // ── Instantiation ─────────────────────────────────────────────────────────

    /// Create a new device instance for the given plugin ID.
    ///
    /// Returns a `Box<dyn AudioNode>` that can be placed in the audio graph.
    /// The host also registers the instance so it can receive parameter changes.
    ///
    /// Currently all external plugins fall back to a `PassThroughDevice`.
    /// Real format-specific constructors will be wired in per format module.
    pub fn instantiate(
        &mut self,
        node_id: NodeId,
        plugin_id: &str,
    ) -> Result<Box<dyn AudioNode>, HostError> {
        let manifest = self.registry.get(plugin_id)
            .ok_or_else(|| HostError::PluginNotFound(plugin_id.to_string()))?
            .clone();

        let device: Box<dyn AudioNode> = match manifest.format {
            DeviceFormat::Internal => {
                // Built-in devices are constructed directly
                Box::new(PassThroughDevice::new(node_id, plugin_id, &manifest.name))
            }
            DeviceFormat::Vst3 => {
                // TODO: call plugin_formats::vst3::VstWrapper::new(...)
                eprintln!("[host] VST3 not yet implemented for '{}' — using pass-through", plugin_id);
                Box::new(PassThroughDevice::new(node_id, plugin_id, &manifest.name))
            }
            DeviceFormat::Au => {
                // TODO: call plugin_formats::au::AuWrapper::new(...)
                eprintln!("[host] AU not yet implemented for '{}' — using pass-through", plugin_id);
                Box::new(PassThroughDevice::new(node_id, plugin_id, &manifest.name))
            }
            DeviceFormat::Wasm => {
                // TODO: call plugin_formats::internal::WasmWrapper::new(...)
                eprintln!("[host] WASM not yet implemented for '{}' — using pass-through", plugin_id);
                Box::new(PassThroughDevice::new(node_id, plugin_id, &manifest.name))
            }
            DeviceFormat::Clap => {
                eprintln!("[host] CLAP not yet implemented for '{}' — using pass-through", plugin_id);
                Box::new(PassThroughDevice::new(node_id, plugin_id, &manifest.name))
            }
        };

        // Register record
        self.records.insert(plugin_id.to_string(), DeviceRecord {
            id: plugin_id.to_string(),
            lifecycle: LifecycleState::Running,
            saved_state: DeviceState::empty(plugin_id),
        });

        Ok(device)
    }

    // ── Parameter routing ─────────────────────────────────────────────────────

    /// Route a normalised parameter change from the UI.
    ///
    /// The host records the change for undo/redo support.  The actual device
    /// update happens via the graph recompile (the compiler reads the host's
    /// saved state and constructs the node with updated parameters).
    ///
    /// In the future, sample-accurate parameter automation will patch the
    /// `ParameterBlock` directly without a full recompile.
    pub fn set_parameter_normalised(
        &mut self,
        device_id: &str,
        param_key: &str,
        value: f32,
    ) -> Result<(), HostError> {
        if !self.records.contains_key(device_id) {
            return Err(HostError::DeviceNotFound(device_id.to_string()));
        }
        // Record the change so the next graph compile picks it up
        let record = self.records.get_mut(device_id).unwrap();
        record.saved_state = DeviceState::from_json(
            device_id,
            serde_json::json!({ "param": param_key, "value": value }),
        );
        Ok(())
    }

    // ── Lifecycle management ───────────────────────────────────────────────────

    pub fn suspend_device(&mut self, device_id: &str) -> Result<(), HostError> {
        let record = self.records.get_mut(device_id)
            .ok_or_else(|| HostError::DeviceNotFound(device_id.to_string()))?;
        record.lifecycle = transition(record.lifecycle, LifecycleState::Suspended)
            .map_err(HostError::LifecycleError)?;
        Ok(())
    }

    pub fn resume_device(&mut self, device_id: &str) -> Result<(), HostError> {
        let record = self.records.get_mut(device_id)
            .ok_or_else(|| HostError::DeviceNotFound(device_id.to_string()))?;
        record.lifecycle = transition(record.lifecycle, LifecycleState::Running)
            .map_err(HostError::LifecycleError)?;
        Ok(())
    }

    pub fn destroy_device(&mut self, device_id: &str) {
        if let Some(record) = self.records.get_mut(device_id) {
            record.lifecycle = LifecycleState::Destroyed;
        }
        self.records.remove(device_id);
    }

    // ── State serialisation ───────────────────────────────────────────────────

    pub fn save_device_state(&self, device_id: &str) -> Option<DeviceState> {
        self.records.get(device_id).map(|r| r.saved_state.clone())
    }

    pub fn load_device_state(&mut self, state: DeviceState) {
        let id = state.device_id.clone();
        if let Some(record) = self.records.get_mut(&id) {
            record.saved_state = state;
        }
    }

    // ── Introspection ─────────────────────────────────────────────────────────

    pub fn device_count(&self) -> usize {
        self.records.len()
    }

    pub fn lifecycle_of(&self, device_id: &str) -> Option<LifecycleState> {
        self.records.get(device_id).map(|r| r.lifecycle)
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}
