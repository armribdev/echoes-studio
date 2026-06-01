//! Plugin host — manages the lifecycle of all device instances.
//!
//! The `PluginHost` is the single entry point for all plugin operations.
//! It lives on the **command/gateway thread**, never on the audio thread.
//!
//! ## Scan-then-instantiate flow
//!
//! 1. At startup, `scan_plugins` populates the registry by probing VST3 and
//!    CLAP search directories.
//! 2. When the graph compiler encounters `NodeKind::Plugin { device_id }`, it
//!    calls `instantiate`.  The host:
//!       * builds an `Arc<PluginInstance>` that owns the native handle,
//!       * stores its own clone of that `Arc` in `instances`,
//!       * hands a wrapper `Box<dyn AudioNode>` (holding *another* clone) to
//!         the compiler.
//!    Both clones share the same underlying handle — ADR-007 / Phase 2a.
//! 3. The wrapper carries the audio-side clone into the `GraphScheduler` on
//!    the audio thread.  When the old `CompiledGraph` retires through the
//!    snapshot drop ring, the audio-side clone drops off the RT path.  The
//!    last `Arc::drop` (whichever thread holds it) runs `PluginInstance::drop`
//!    and tears the FFI handle down.

use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::audio_graph::node::{AudioNode, NodeId};
use crate::plugin_formats::clap::ffi::{ClapLibrary, CLAP_PLUGIN_FACTORY_ID};
use crate::plugin_formats::clap::host::decode_plugin_id as clap_decode_id;
use crate::plugin_formats::clap::wrapper::ClapWrapperNode;
use crate::plugin_formats::vst3::ffi::{Vst3Library, instantiate_processor};
use crate::plugin_formats::vst3::host::decode_plugin_id as vst3_decode_id;
use crate::plugin_formats::vst3::wrapper::VstWrapperNode;
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::lifecycle::{LifecycleError, LifecycleState, transition};
use crate::plugin_system::registry::{PluginManifest, PluginRegistry};
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{DeviceFormat, PassThroughDevice};

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
            HostError::DeviceNotFound(id)     => write!(f, "device not found: {id}"),
            HostError::PluginNotFound(id)     => write!(f, "plugin not in registry: {id}"),
            HostError::LifecycleError(e)      => write!(f, "lifecycle error: {e}"),
            HostError::InstantiationFailed(e) => write!(f, "instantiation failed: {e}"),
            HostError::ParameterNotFound(k)   => write!(f, "parameter not found: {k}"),
        }
    }
}

// ── Device record ─────────────────────────────────────────────────────────────
//
// Phase 2a: long-lived ownership root for one device — carries the host-side
// `Arc<PluginInstance>` clone so the host can address the live instance.
//
// Phase 2b: the per-record `lifecycle` field is gone.  The instance's atomic
// is the single source of truth; this record only keeps the transitional
// `saved_state` blob (retired by Phase 3 / 4 once SPSC + FFI state lands).

struct DeviceRecord {
    instance:    Arc<PluginInstance>,
    saved_state: DeviceState,
}

// ── PluginHost ────────────────────────────────────────────────────────────────

pub struct PluginHost {
    registry: PluginRegistry,
    records:  HashMap<String, DeviceRecord>,
}

impl PluginHost {
    pub fn new() -> Self {
        PluginHost {
            registry: PluginRegistry::new(),
            records:  HashMap::new(),
        }
    }

    // ── Registry ───────────────────────────────────────────────────────────────

    pub fn registry(&self)     -> &PluginRegistry     { &self.registry }
    pub fn registry_mut(&mut self) -> &mut PluginRegistry { &mut self.registry }

    // ── Plugin scanning ────────────────────────────────────────────────────────

    /// Scan standard system directories for VST3 and CLAP plugins.
    ///
    /// Safe to call multiple times — already-registered plugins are skipped.
    pub fn scan_plugins(&mut self) {
        let vst3_host = crate::plugin_formats::vst3::host::Vst3Host::new();
        vst3_host.scan(&mut self.registry);

        let clap_host = crate::plugin_formats::clap::host::ClapHost::new();
        clap_host.scan(&mut self.registry);

        let total = self.registry.len();
        eprintln!("[plugin] scan complete — {total} plugins registered");
    }

    /// Return all discovered plugins as serialisable descriptors.
    pub fn list_plugins(&self) -> Vec<PluginInfo> {
        self.registry
            .all()
            .filter(|m| {
                matches!(m.format, DeviceFormat::Vst3 | DeviceFormat::Clap)
            })
            .map(|m| PluginInfo {
                id:     m.id.clone(),
                name:   m.name.clone(),
                vendor: m.vendor.clone(),
                format: format_tag(m.format),
                categories: m.categories.clone(),
            })
            .collect()
    }

    // ── Instantiation ─────────────────────────────────────────────────────────

    /// Instantiate a plugin and return the audio-side `Box<dyn AudioNode>`
    /// the graph compiler embeds in `CompiledGraph.processors`.
    ///
    /// The host keeps its own `Arc<PluginInstance>` clone in `instances` so
    /// later lifecycle / parameter / state calls can address the live
    /// instance directly (ADR-007).
    pub fn instantiate(
        &mut self,
        node_id:   NodeId,
        plugin_id: &str,
    ) -> Result<Box<dyn AudioNode>, HostError> {
        let manifest = self.registry.get(plugin_id)
            .ok_or_else(|| HostError::PluginNotFound(plugin_id.to_string()))?
            .clone();

        let (instance, device): (Arc<PluginInstance>, Box<dyn AudioNode>) = match manifest.format {
            DeviceFormat::Internal => {
                let inst = PluginInstance::new_internal(plugin_id, &manifest.name);
                let node = Box::new(PassThroughDevice::new(node_id, plugin_id, &manifest.name));
                (inst, node)
            }

            DeviceFormat::Vst3 => {
                self.instantiate_vst3(node_id, plugin_id, &manifest)?
            }

            DeviceFormat::Clap => {
                self.instantiate_clap(node_id, plugin_id, &manifest)?
            }

            DeviceFormat::Au => {
                eprintln!("[host] AU not yet implemented for '{plugin_id}' — pass-through");
                let inst = PluginInstance::new_internal(plugin_id, &manifest.name);
                let node = Box::new(PassThroughDevice::new(node_id, plugin_id, &manifest.name));
                (inst, node)
            }

            DeviceFormat::Wasm => {
                eprintln!("[host] WASM not yet implemented for '{plugin_id}' — pass-through");
                let inst = PluginInstance::new_internal(plugin_id, &manifest.name);
                let node = Box::new(PassThroughDevice::new(node_id, plugin_id, &manifest.name));
                (inst, node)
            }
        };

        // Phase 2b: lifecycle now lives exclusively on the instance.  The
        // legacy host-side state machine jumped straight to `Running` after
        // instantiation; we preserve that observable behaviour by stamping
        // the atomic here (the wrapper's `prepare` will later re-stamp it,
        // which is idempotent).  Phase 4 will introduce the proper
        // Created → Loaded → Initialized → Running sequence once state load
        // / save lands on the host API.
        instance.set_lifecycle(LifecycleState::Running);

        self.records.insert(plugin_id.to_string(), DeviceRecord {
            instance,
            saved_state: DeviceState::empty(plugin_id),
        });

        Ok(device)
    }

    // ── VST3 instantiation ────────────────────────────────────────────────────

    fn instantiate_vst3(
        &self,
        node_id:   NodeId,
        plugin_id: &str,
        manifest:  &PluginManifest,
    ) -> Result<(Arc<PluginInstance>, Box<dyn AudioNode>), HostError> {
        let cid = vst3_decode_id(plugin_id)
            .ok_or_else(|| HostError::InstantiationFailed("invalid VST3 plugin_id".into()))?;

        let dll = manifest.path.as_ref()
            .ok_or_else(|| HostError::InstantiationFailed("no DLL path in manifest".into()))?;

        let (lib, factory) = Vst3Library::load(dll)
            .map_err(HostError::InstantiationFailed)?;

        let (comp, proc) = unsafe {
            instantiate_processor(factory, &cid)
                .map_err(HostError::InstantiationFailed)?
        };

        // Release factory ref — we no longer need to enumerate classes.
        unsafe {
            ((*(*factory).vtbl).release)(factory as *mut _);
        }

        let comp_nn = NonNull::new(comp)
            .ok_or_else(|| HostError::InstantiationFailed("null IComponent".into()))?;
        let proc_nn = NonNull::new(proc)
            .ok_or_else(|| HostError::InstantiationFailed("null IAudioProcessor".into()))?;

        let instance = PluginInstance::new_vst3(
            plugin_id, &manifest.name, comp_nn, proc_nn, lib,
        );

        let node: Box<dyn AudioNode> = Box::new(VstWrapperNode::new(
            node_id, Arc::clone(&instance),
        ));

        Ok((instance, node))
    }

    // ── CLAP instantiation ────────────────────────────────────────────────────

    fn instantiate_clap(
        &self,
        node_id:   NodeId,
        plugin_id: &str,
        manifest:  &PluginManifest,
    ) -> Result<(Arc<PluginInstance>, Box<dyn AudioNode>), HostError> {
        let clap_id = clap_decode_id(plugin_id)
            .ok_or_else(|| HostError::InstantiationFailed("invalid CLAP plugin_id".into()))?;

        let dll = manifest.path.as_ref()
            .ok_or_else(|| HostError::InstantiationFailed("no DLL path in manifest".into()))?;

        let (lib, entry) = ClapLibrary::load(dll)
            .map_err(HostError::InstantiationFailed)?;

        // Get factory
        let factory_ptr = unsafe {
            ((*entry).get_factory)(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const std::ffi::c_char)
        };
        if factory_ptr.is_null() {
            return Err(HostError::InstantiationFailed("clap_entry.get_factory returned null".into()));
        }
        let factory = factory_ptr as *const crate::plugin_formats::clap::ffi::ClapPluginFactory;

        // Build host struct (heap-stable for plugin lifetime)
        let host = crate::plugin_formats::clap::ffi::make_host();

        // Create plugin instance
        let clap_id_cstr: Vec<u8> = clap_id.bytes().chain(std::iter::once(0)).collect();
        let plugin = unsafe {
            ((*factory).create_plugin)(factory, host.as_ref(), clap_id_cstr.as_ptr() as *const std::ffi::c_char)
        };
        if plugin.is_null() {
            return Err(HostError::InstantiationFailed(
                format!("CLAP create_plugin returned null for '{clap_id}'"),
            ));
        }

        let instance = PluginInstance::new_clap(
            plugin_id,
            &manifest.name,
            &manifest.vendor,
            plugin,
            host,
            lib,
        );

        let node: Box<dyn AudioNode> = Box::new(ClapWrapperNode::new(
            node_id, Arc::clone(&instance),
        ));

        Ok((instance, node))
    }

    // ── Parameter routing ─────────────────────────────────────────────────────

    /// Push a normalised parameter change to a live plugin (ADR-007).
    ///
    /// `native_id` is the format-native parameter identifier (VST3 `ParamID`,
    /// CLAP `clap_id`) — the same `u32` the frontend already stores in
    /// `PluginParams.params`.  The value is enqueued on the instance's SPSC
    /// ring; the audio-graph wrapper drains it at the top of `process` and
    /// converts it to the native parameter event.  No recompile, no lock.
    ///
    /// Returns `Ok(false)` if the ring was full (newest dropped — the UI will
    /// resend on its next tick).  Errors only if the device is unknown.
    pub fn push_plugin_param(
        &self,
        device_id: &str,
        native_id: u32,
        normalised: f32,
    ) -> Result<bool, HostError> {
        let record = self.records.get(device_id)
            .ok_or_else(|| HostError::DeviceNotFound(device_id.to_string()))?;
        Ok(record.instance.push_param(native_id, normalised))
    }

    // ── Lifecycle management ───────────────────────────────────────────────────
    //
    // Phase 2b: the instance atomic is the only state store.  Each command
    // reads the current value, validates the target via `transition`, then
    // stores the new value back.  The read-validate-store sequence is safe
    // because lifecycle mutations are command-thread only — the audio thread
    // never writes to `instance.lifecycle`, it only reads it (and even that
    // is not yet wired on the hot path).

    pub fn suspend_device(&mut self, device_id: &str) -> Result<(), HostError> {
        let record = self.records.get(device_id)
            .ok_or_else(|| HostError::DeviceNotFound(device_id.to_string()))?;
        let next = transition(record.instance.lifecycle(), LifecycleState::Suspended)
            .map_err(HostError::LifecycleError)?;
        record.instance.set_lifecycle(next);
        Ok(())
    }

    pub fn resume_device(&mut self, device_id: &str) -> Result<(), HostError> {
        let record = self.records.get(device_id)
            .ok_or_else(|| HostError::DeviceNotFound(device_id.to_string()))?;
        let next = transition(record.instance.lifecycle(), LifecycleState::Running)
            .map_err(HostError::LifecycleError)?;
        record.instance.set_lifecycle(next);
        Ok(())
    }

    pub fn destroy_device(&mut self, device_id: &str) {
        if let Some(record) = self.records.get(device_id) {
            // `Destroyed` is reachable from any non-terminal state via
            // `transition`'s catch-all rule for `Error`; we use unconditional
            // store here because destroy is a forced teardown, not a
            // negotiable transition.
            record.instance.set_lifecycle(LifecycleState::Destroyed);
        }
        // Dropping the record releases the host-side `Arc<PluginInstance>`
        // clone.  If the audio side has already retired its clone, the last
        // `Arc::drop` here runs `PluginInstance::drop` and tears the FFI down.
        self.records.remove(device_id);
    }

    // ── State serialisation ───────────────────────────────────────────────────

    /// Serialise a device's live state (Phase 4b / ADR-010).
    ///
    /// VST3 devices return their processor state via FFI
    /// (`IComponent::getState`).  Formats without an FFI state path yet (CLAP =
    /// Phase 4c, internal) fall back to the transitional `saved_state` cache so
    /// the returned `DeviceState` is never lost.
    pub fn save_device_state(&self, device_id: &str) -> Option<DeviceState> {
        let record = self.records.get(device_id)?;
        match record.instance.save_state_blob() {
            Some(bytes) => Some(DeviceState::from_binary(device_id, bytes)),
            None        => Some(record.saved_state.clone()),
        }
    }

    /// Restore a device's state.  For VST3 the blob is pushed into the live
    /// plugin via `IComponent::setState`; the blob is also cached so formats
    /// not yet wired to FFI keep their previous behaviour.
    pub fn load_device_state(&mut self, state: DeviceState) {
        let id = state.device_id.clone();
        if let Some(record) = self.records.get_mut(&id) {
            if let Some(bytes) = state.as_binary() {
                if !record.instance.load_state_blob(bytes) {
                    eprintln!("[plugin] setState rejected/ignored for '{id}'");
                }
            }
            record.saved_state = state;
        }
    }

    /// Iterate the device IDs of all currently-instantiated plugins.  Used by
    /// the project-save path to harvest live plugin state.
    pub fn device_ids(&self) -> impl Iterator<Item = &str> {
        self.records.keys().map(String::as_str)
    }

    // ── Introspection ─────────────────────────────────────────────────────────

    pub fn device_count(&self) -> usize { self.records.len() }

    pub fn lifecycle_of(&self, device_id: &str) -> Option<LifecycleState> {
        // Phase 2b: read the authoritative atomic.
        self.records.get(device_id).map(|r| r.instance.lifecycle())
    }

    /// Borrow the shared instance for an already-instantiated device.
    ///
    /// Returns a clone of the host-side `Arc` so callers (Phase 2c GUI editor,
    /// future tests) can address the live plugin without duplicating it.
    pub fn instance(&self, device_id: &str) -> Option<Arc<PluginInstance>> {
        self.records.get(device_id).map(|r| Arc::clone(&r.instance))
    }
}

impl Default for PluginHost {
    fn default() -> Self { Self::new() }
}

// ── Serialisable plugin info (for the frontend) ───────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginInfo {
    pub id:         String,
    pub name:       String,
    pub vendor:     String,
    pub format:     String,
    pub categories: Vec<String>,
}

fn format_tag(fmt: DeviceFormat) -> String {
    match fmt {
        DeviceFormat::Vst3     => "VST3".into(),
        DeviceFormat::Clap     => "CLAP".into(),
        DeviceFormat::Au       => "AU".into(),
        DeviceFormat::Internal => "Internal".into(),
        DeviceFormat::Wasm     => "WASM".into(),
    }
}
