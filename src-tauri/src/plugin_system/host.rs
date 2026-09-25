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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::Arc;

use serde::Serialize;

use crate::audio_graph::node::{AudioNode, NodeId};
use crate::audio_graph::nodes::clap_instrument::ClapInstrument;
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::plugin_formats::clap::ffi::{ClapLibrary, CLAP_PLUGIN_FACTORY_ID};
use crate::plugin_formats::clap::host::decode_plugin_id as clap_decode_id;
use crate::plugin_formats::clap::params::read_clap_params;
use crate::plugin_formats::clap::wrapper::ClapWrapperNode;
use crate::plugin_formats::vst3::ffi::{instantiate_processor, Vst3Library};
use crate::plugin_formats::vst3::host::decode_plugin_id as vst3_decode_id;
use crate::plugin_formats::vst3::wrapper::VstWrapperNode;
use crate::plugin_system::bridge::{locate_bridge_exe, BridgedPlugin};
use crate::plugin_system::bridged_node::BridgedNode;
use crate::plugin_system::crash_marker::{self, plugin_tag, PluginThread};
use crate::plugin_system::instance::{PluginInstance, FFI_GATE_HARVEST_BUDGET};
use crate::plugin_system::lifecycle::{transition, LifecycleError, LifecycleState};
use crate::plugin_system::parameter::PluginParamInfo;
use crate::plugin_system::registry::{PluginManifest, PluginRegistry};
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{DeviceFormat, PassThroughDevice};

#[derive(Debug)]
pub enum HostError {
    DeviceNotFound(String),
    PluginNotFound(String),
    LifecycleError(LifecycleError),
    InstantiationFailed(String),
    UnsupportedFormat(String),
    ParameterNotFound(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::DeviceNotFound(id) => write!(f, "device not found: {id}"),
            HostError::PluginNotFound(id) => write!(f, "plugin not in registry: {id}"),
            HostError::LifecycleError(e) => write!(f, "lifecycle error: {e}"),
            HostError::InstantiationFailed(e) => write!(f, "instantiation failed: {e}"),
            HostError::UnsupportedFormat(e) => write!(f, "unsupported format: {e}"),
            HostError::ParameterNotFound(k) => write!(f, "parameter not found: {k}"),
        }
    }
}

/// Which plugins run in a sandbox process (ADR-015).  An app preference, not a
/// project setting: it describes how much this machine trusts its plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginIsolation {
    /// Every plugin in-process, whatever the effects ask for.
    Off,
    /// Only the effects the user isolated (`PluginParams.sandboxed`) and the
    /// plugins marked "always isolate".
    #[default]
    Chosen,
    /// Every plugin that can be isolated.
    All,
}

/// Why a placement's plugin could not be loaded; shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    NotInstalled,
    UnsupportedFormat,
    LoadFailed,
    /// A sandboxed plugin crashed or stopped answering; it is silent until the
    /// project is recompiled, which restarts it.
    Crashed,
}

impl UnavailableReason {
    fn from_error(error: &HostError) -> Self {
        match error {
            HostError::PluginNotFound(_) => Self::NotInstalled,
            HostError::UnsupportedFormat(_) => Self::UnsupportedFormat,
            _ => Self::LoadFailed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnavailablePlugin {
    pub instance_key: String,
    pub plugin_id: String,
    pub reason: UnavailableReason,
}

/// A placement whose plugin failed to load.  `state` is kept verbatim so a save
/// does not erase it and a later successful load restores it.
struct UnavailablePlacement {
    plugin_id: String,
    reason: UnavailableReason,
    state: Option<DeviceState>,
}

// Phase 2b: per-record `lifecycle` removed; instance atomic is the single source of truth.
// `saved_state` blob is transitional — retired by Phase 3/4 once SPSC + FFI state lands.
struct DeviceRecord {
    instance: Arc<PluginInstance>,
    saved_state: DeviceState,
}

pub struct PluginHost {
    registry: PluginRegistry,
    records: HashMap<String, DeviceRecord>,
    /// ADR-010: blobs applied at instantiate before snapshot (project-load only).
    pending_state_blobs: HashMap<String, Vec<u8>>,
    unavailable: HashMap<String, UnavailablePlacement>,
    /// `plugin_host_bridge` executable hosting sandboxed plugins (ADR-015).
    bridge_exe: Option<PathBuf>,
    isolation: PluginIsolation,
    /// Plugin ids isolated wherever they are used, in `Chosen` mode.
    always_isolated: HashSet<String>,
}

impl PluginHost {
    pub fn new() -> Self {
        Self {
            registry: PluginRegistry::new(),
            records: HashMap::new(),
            pending_state_blobs: HashMap::new(),
            unavailable: HashMap::new(),
            bridge_exe: locate_bridge_exe(),
            isolation: PluginIsolation::default(),
            always_isolated: HashSet::new(),
        }
    }

    /// Returns whether anything changed (the caller then recompiles).
    pub fn set_isolation(&mut self, isolation: PluginIsolation, always_isolated: HashSet<String>) -> bool {
        let changed = self.isolation != isolation || self.always_isolated != always_isolated;
        self.isolation = isolation;
        self.always_isolated = always_isolated;
        changed
    }

    /// Whether `plugin_id` runs sandboxed, given the effect's own choice.
    fn isolates(&self, plugin_id: &str, chosen: bool) -> bool {
        match self.isolation {
            PluginIsolation::Off => false,
            PluginIsolation::Chosen => chosen || self.always_isolated.contains(plugin_id),
            PluginIsolation::All => true,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_bridge_exe(&mut self, path: PathBuf) {
        self.bridge_exe = Some(path);
    }

    pub fn set_pending_state_blobs(&mut self, blobs: HashMap<String, Vec<u8>>) {
        self.pending_state_blobs = blobs;
    }

    pub fn clear_pending_state_blobs(&mut self) {
        self.pending_state_blobs.clear();
    }

    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }
    pub fn registry_mut(&mut self) -> &mut PluginRegistry {
        &mut self.registry
    }

    /// Sidechain aux inputs from scan manifest (Phase 5d); 0 if unknown.
    pub fn sidechain_input_ports(&self, device_id: &str) -> usize {
        self.registry
            .get(device_id)
            .map(|m| m.num_sidechain_inputs)
            .unwrap_or(0)
    }

    /// Idempotent — skips plugins already registered.
    pub fn scan_plugins(&mut self) {
        let vst3_host = crate::plugin_formats::vst3::host::Vst3Host::new();
        vst3_host.scan(&mut self.registry);

        let clap_host = crate::plugin_formats::clap::host::ClapHost::new();
        clap_host.scan(&mut self.registry);

        let total = self.registry.len();
        eprintln!("[plugin] scan complete — {total} plugins registered");
    }

    pub fn list_plugins(&self) -> Vec<PluginInfo> {
        self.registry
            .all()
            .filter(|m| matches!(m.format, DeviceFormat::Vst3 | DeviceFormat::Clap))
            .map(|m| PluginInfo {
                id: m.id.clone(),
                name: m.name.clone(),
                vendor: m.vendor.clone(),
                format: format_tag(m.format),
                categories: m.categories.clone(),
                is_instrument: m.is_instrument,
            })
            .collect()
    }

    /// Reuse the live instance already held for this `instance_key` instead of
    /// rebuilding it, when the placement still hosts the same plugin type and the
    /// format's activation lifecycle is reuse-safe (ADR-016). Returns a fresh
    /// wrapper around the shared `Arc<PluginInstance>` — **no new native handle**,
    /// so the plugin's internal DSP state is preserved across the recompile.
    ///
    /// `None` (→ caller does a full re-instantiate) when:
    /// - state blobs are staged (project load wants a fresh instance to apply
    ///   them at instantiation — ADR-010 race-free restore),
    /// - no record exists for the key, or the slot now hosts a different type,
    /// - the instance is terminal (`Destroyed`/`Error`), or
    /// - the format isn't reuse-safe yet (AU/WASM — ADR-016 follow-up).
    ///   CLAP, VST3 and internal are reuse-safe.
    fn reuse_existing(
        &self,
        node_id: NodeId,
        instance_key: &str,
        plugin_id: &str,
        sandboxed: bool,
    ) -> Option<Box<dyn AudioNode>> {
        if !self.pending_state_blobs.is_empty() {
            return None;
        }
        let inst = &self.records.get(instance_key)?.instance;
        if inst.device_id != plugin_id || inst.is_sandboxed() != runs_sandboxed(inst.format, sandboxed) {
            return None;
        }
        if matches!(
            inst.lifecycle(),
            LifecycleState::Destroyed | LifecycleState::Error
        ) {
            return None;
        }
        match inst.format {
            DeviceFormat::Internal => Some(Box::new(PassThroughDevice::new(
                node_id,
                &inst.device_id,
                &inst.name,
            ))),
            DeviceFormat::Clap if inst.is_sandboxed() => {
                Some(Box::new(BridgedNode::new(node_id, Arc::clone(inst))))
            }
            DeviceFormat::Clap => Some(Box::new(ClapWrapperNode::new(node_id, Arc::clone(inst)))),
            DeviceFormat::Vst3 => Some(Box::new(VstWrapperNode::new(node_id, Arc::clone(inst)))),
            _ => None,
        }
    }

    /// `instance_key` = per-placement id (ADR-007); host keeps `Arc` in `records`.
    pub fn instantiate(
        &mut self,
        node_id: NodeId,
        instance_key: &str,
        plugin_id: &str,
        sandboxed: bool,
    ) -> Result<Box<dyn AudioNode>, HostError> {
        let sandboxed = self.isolates(plugin_id, sandboxed);
        let _running = (!sandboxed).then(|| crash_marker::enter(PluginThread::Control, plugin_tag(plugin_id)));
        let result = self.instantiate_placement(node_id, instance_key, plugin_id, sandboxed);
        match &result {
            Ok(_) => self.mark_available(instance_key),
            Err(error) => self.mark_unavailable(instance_key, plugin_id, UnavailableReason::from_error(error)),
        }
        result
    }

    fn instantiate_placement(
        &mut self,
        node_id: NodeId,
        instance_key: &str,
        plugin_id: &str,
        sandboxed: bool,
    ) -> Result<Box<dyn AudioNode>, HostError> {
        // ADR-016: reuse a live instance for this placement instead of rebuilding
        // it (no FFI churn, preserves the plugin's internal DSP state).
        if let Some(node) = self.reuse_existing(node_id, instance_key, plugin_id, sandboxed) {
            return Ok(node);
        }

        let manifest = self
            .registry
            .get(plugin_id)
            .ok_or_else(|| HostError::PluginNotFound(plugin_id.to_string()))?
            .clone();

        let (instance, device): (Arc<PluginInstance>, Box<dyn AudioNode>) = match manifest.format {
            DeviceFormat::Internal => {
                let inst = PluginInstance::new_internal(plugin_id, &manifest.name);
                let node = Box::new(PassThroughDevice::new(node_id, plugin_id, &manifest.name));
                (inst, node)
            }

            DeviceFormat::Vst3 => self.instantiate_vst3(node_id, plugin_id, &manifest)?,

            DeviceFormat::Clap if sandboxed => self.instantiate_bridged(node_id, plugin_id, &manifest)?,

            DeviceFormat::Clap => self.instantiate_clap(node_id, plugin_id, &manifest)?,

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

        // Advance to Running through the validated state machine.
        // VST3/CLAP start at Created; Internal/AU/WASM already at Running — invalid
        // transitions (already past that step) are silently skipped; AlreadyDestroyed is fatal.
        let mut state = instance.lifecycle();
        for target in [LifecycleState::Loaded, LifecycleState::Initialized, LifecycleState::Running] {
            match transition(state, target) {
                Ok(next) => { instance.set_lifecycle(next); state = next; }
                Err(LifecycleError::AlreadyDestroyed) => {
                    return Err(HostError::LifecycleError(LifecycleError::AlreadyDestroyed));
                }
                Err(LifecycleError::InvalidTransition { .. }) => {}
            }
        }

        let saved_state = self.restore_state_for(instance_key, plugin_id, &instance);

        self.records.insert(
            instance_key.to_string(),
            DeviceRecord {
                instance,
                saved_state,
            },
        );

        Ok(device)
    }

    /// Keeps the placement's last known state while its plugin is missing: the
    /// project-load blob, or the state kept by an earlier failed attempt.  A live
    /// record, if any, is still harvested by `collect_plugin_states`.
    fn mark_unavailable(&mut self, instance_key: &str, plugin_id: &str, reason: UnavailableReason) {
        let staged = self
            .pending_state_blobs
            .get(instance_key)
            .map(|bytes| DeviceState::from_binary(plugin_id, bytes.clone()));
        let kept = self
            .kept_state(instance_key, plugin_id)
            .cloned();
        self.unavailable.insert(
            instance_key.to_string(),
            UnavailablePlacement {
                plugin_id: plugin_id.to_string(),
                reason,
                state: staged.or(kept),
            },
        );
    }

    fn mark_available(&mut self, instance_key: &str) {
        self.unavailable.remove(instance_key);
    }

    fn kept_state(&self, instance_key: &str, plugin_id: &str) -> Option<&DeviceState> {
        self.unavailable
            .get(instance_key)
            .filter(|placement| placement.plugin_id == plugin_id)?
            .state
            .as_ref()
    }

    /// Placements whose plugin is not loaded or has crashed in its sandbox,
    /// sorted by `instance_key`.
    pub fn unavailable_plugins(&self) -> Vec<UnavailablePlugin> {
        let crashed = self
            .records
            .iter()
            .filter(|(_, record)| record.instance.has_crashed())
            .map(|(instance_key, record)| UnavailablePlugin {
                instance_key: instance_key.clone(),
                plugin_id: record.instance.device_id.clone(),
                reason: UnavailableReason::Crashed,
            });
        let mut list: Vec<UnavailablePlugin> = self
            .unavailable
            .iter()
            .map(|(instance_key, placement)| UnavailablePlugin {
                instance_key: instance_key.clone(),
                plugin_id: placement.plugin_id.clone(),
                reason: placement.reason,
            })
            .chain(crashed)
            .collect();
        list.sort_by(|a, b| a.instance_key.cmp(&b.instance_key));
        list
    }

    /// Decide and apply the state a freshly-created instance for `instance_key`
    /// must start from, and return the `DeviceState` to cache on its record.
    ///
    /// Priority (R-4):
    /// 1. **Project-load staged blob** (`pending_state_blobs`, keyed by
    ///    `instance_key` per ADR-017): applied now — the instance is brand-new and
    ///    not yet in any snapshot, so `setState` cannot race the audio thread
    ///    (ADR-010 race-free restore).
    /// 2. **Controlled re-instantiation:** carry the previous instance's state
    ///    forward when this placement still hosts the same plugin type, so state
    ///    survives a rebuild (terminal instance, non-reuse-safe format) instead of
    ///    reverting to the default preset. The previous record is about to be
    ///    overwritten by the caller, so no state is duplicated.
    /// 3. Otherwise empty.
    fn restore_state_for(
        &self,
        instance_key: &str,
        plugin_id: &str,
        instance: &PluginInstance,
    ) -> DeviceState {
        let _ = instance.clap_ensure_initialized();
        if let Some(bytes) = self.pending_state_blobs.get(instance_key) {
            let _ = instance.load_state_blob(bytes);
            return DeviceState::from_binary(plugin_id, bytes.clone());
        }
        if let Some(state) = self.kept_state(instance_key, plugin_id) {
            if let Some(bytes) = state.as_binary() {
                let _ = instance.load_state_blob(bytes);
                return state.clone();
            }
        }
        if let Some(previous) = self.records.get(instance_key) {
            let cached = previous.saved_state.as_binary();
            if let Some(bytes) = instance.restore_from(&previous.instance, cached) {
                return DeviceState::from_binary(plugin_id, bytes);
            }
        }
        DeviceState::empty(plugin_id)
    }

    fn instantiate_vst3(
        &self,
        node_id: NodeId,
        plugin_id: &str,
        manifest: &PluginManifest,
    ) -> Result<(Arc<PluginInstance>, Box<dyn AudioNode>), HostError> {
        let cid = vst3_decode_id(plugin_id)
            .ok_or_else(|| HostError::InstantiationFailed("invalid VST3 plugin_id".into()))?;

        let dll = manifest
            .path
            .as_ref()
            .ok_or_else(|| HostError::InstantiationFailed("no DLL path in manifest".into()))?;

        let (lib, factory) = Vst3Library::load(dll).map_err(HostError::InstantiationFailed)?;

        let (comp, proc) = unsafe {
            instantiate_processor(factory, &cid).map_err(HostError::InstantiationFailed)?
        };

        unsafe {
            ((*(*factory).vtbl).release)(factory as *mut _);
        }

        let comp_nn = NonNull::new(comp)
            .ok_or_else(|| HostError::InstantiationFailed("null IComponent".into()))?;
        let proc_nn = NonNull::new(proc)
            .ok_or_else(|| HostError::InstantiationFailed("null IAudioProcessor".into()))?;

        let instance = PluginInstance::new_vst3(plugin_id, &manifest.name, comp_nn, proc_nn, lib);

        let node: Box<dyn AudioNode> =
            Box::new(VstWrapperNode::new(node_id, Arc::clone(&instance)));

        Ok((instance, node))
    }

    fn instantiate_clap(
        &self,
        node_id: NodeId,
        plugin_id: &str,
        manifest: &PluginManifest,
    ) -> Result<(Arc<PluginInstance>, Box<dyn AudioNode>), HostError> {
        let instance = self.create_clap_instance(plugin_id, manifest)?;
        let node: Box<dyn AudioNode> =
            Box::new(ClapWrapperNode::new(node_id, Arc::clone(&instance)));
        Ok((instance, node))
    }

    /// Start a bridge process hosting the CLAP plugin (ADR-015).  A plugin that
    /// cannot start sandboxed is reported unavailable, never loaded in-process
    /// instead: the user sandboxed it because it may crash the app.
    fn instantiate_bridged(
        &self,
        node_id: NodeId,
        plugin_id: &str,
        manifest: &PluginManifest,
    ) -> Result<(Arc<PluginInstance>, Box<dyn AudioNode>), HostError> {
        let bridge_exe = self
            .bridge_exe
            .as_ref()
            .ok_or_else(|| HostError::InstantiationFailed("plugin_host_bridge not found".into()))?;
        let clap_id = clap_decode_id(plugin_id)
            .ok_or_else(|| HostError::InstantiationFailed("invalid CLAP plugin_id".into()))?;
        let dll = manifest
            .path
            .as_ref()
            .ok_or_else(|| HostError::InstantiationFailed("no DLL path in manifest".into()))?;
        let bridged = BridgedPlugin::spawn(bridge_exe, dll, clap_id)
            .map_err(|error| HostError::InstantiationFailed(format!("sandbox: {error}")))?;
        let instance = PluginInstance::new_bridged(plugin_id, &manifest.name, &manifest.vendor, bridged);
        let node: Box<dyn AudioNode> = Box::new(BridgedNode::new(node_id, Arc::clone(&instance)));
        Ok((instance, node))
    }

    /// Load the DLL, create the CLAP plugin, and build the shared
    /// `PluginInstance` — **without** building an audio wrapper.  Shared by the
    /// effect path ([`instantiate_clap`]) and the instrument path
    /// ([`instantiate_instrument`]) so the FFI create sequence lives in one
    /// place.
    fn create_clap_instance(
        &self,
        plugin_id: &str,
        manifest: &PluginManifest,
    ) -> Result<Arc<PluginInstance>, HostError> {
        let clap_id = clap_decode_id(plugin_id)
            .ok_or_else(|| HostError::InstantiationFailed("invalid CLAP plugin_id".into()))?;

        let dll = manifest
            .path
            .as_ref()
            .ok_or_else(|| HostError::InstantiationFailed("no DLL path in manifest".into()))?;

        let (lib, entry) = ClapLibrary::load(dll).map_err(HostError::InstantiationFailed)?;

        let factory_ptr = unsafe {
            ((*entry).get_factory)(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const std::ffi::c_char)
        };
        if factory_ptr.is_null() {
            return Err(HostError::InstantiationFailed(
                "clap_entry.get_factory returned null".into(),
            ));
        }
        let factory = factory_ptr as *const crate::plugin_formats::clap::ffi::ClapPluginFactory;

        let host = crate::plugin_formats::clap::ffi::make_host();

        let clap_id_cstr: Vec<u8> = clap_id.bytes().chain(std::iter::once(0)).collect();
        let plugin = unsafe {
            ((*factory).create_plugin)(
                factory,
                host.as_ref(),
                clap_id_cstr.as_ptr() as *const std::ffi::c_char,
            )
        };
        if plugin.is_null() {
            return Err(HostError::InstantiationFailed(format!(
                "CLAP create_plugin returned null for '{clap_id}'"
            )));
        }

        Ok(PluginInstance::new_clap(
            plugin_id,
            &manifest.name,
            &manifest.vendor,
            plugin,
            host,
            lib,
        ))
    }

    /// Instantiate a plugin as a MIDI **instrument** generator, returning a
    /// `Box<dyn InstrumentDevice>` the compiler embeds as the inner device of
    /// an `InstrumentNode`.  The host registers the shared `PluginInstance`
    /// (same as [`instantiate`]) so parameter / state / lifecycle calls address
    /// the live instance.
    ///
    /// Only CLAP is wired in this phase (C-3 variant-by-variant).  Other formats
    /// return `Err` so the caller falls back to the built-in `PolySynth`.
    pub fn instantiate_instrument(
        &mut self,
        instance_key: &str,
        plugin_id: &str,
    ) -> Result<Box<dyn InstrumentDevice>, HostError> {
        let _running = crash_marker::enter(PluginThread::Control, plugin_tag(plugin_id));
        let result = self.instantiate_instrument_placement(instance_key, plugin_id);
        match &result {
            Ok(_) => self.mark_available(instance_key),
            Err(error) => self.mark_unavailable(instance_key, plugin_id, UnavailableReason::from_error(error)),
        }
        result
    }

    fn instantiate_instrument_placement(
        &mut self,
        instance_key: &str,
        plugin_id: &str,
    ) -> Result<Box<dyn InstrumentDevice>, HostError> {
        // ADR-016: reuse the live CLAP instrument for this track instead of
        // rebuilding it, preserving its internal voice/DSP state across recompiles.
        if self.pending_state_blobs.is_empty() {
            if let Some(record) = self.records.get(instance_key) {
                let inst = &record.instance;
                if inst.device_id == plugin_id
                    && inst.format == DeviceFormat::Clap
                    && !matches!(
                        inst.lifecycle(),
                        LifecycleState::Destroyed | LifecycleState::Error
                    )
                {
                    return Ok(Box::new(ClapInstrument::new(Arc::clone(inst))));
                }
            }
        }

        let manifest = self
            .registry
            .get(plugin_id)
            .ok_or_else(|| HostError::PluginNotFound(plugin_id.to_string()))?
            .clone();

        match manifest.format {
            DeviceFormat::Clap => {
                let instance = self.create_clap_instance(plugin_id, &manifest)?;
                let mut state = instance.lifecycle();
                for target in [LifecycleState::Loaded, LifecycleState::Initialized, LifecycleState::Running] {
                    state = transition(state, target).map_err(HostError::LifecycleError)?;
                    instance.set_lifecycle(state);
                }
                let saved_state = self.restore_state_for(instance_key, plugin_id, &instance);
                self.records.insert(
                    instance_key.to_string(),
                    DeviceRecord {
                        instance: Arc::clone(&instance),
                        saved_state,
                    },
                );
                Ok(Box::new(ClapInstrument::new(instance)))
            }
            other => Err(HostError::UnsupportedFormat(format!(
                "instrument format {other:?} not supported yet (CLAP only in Phase 5c)"
            ))),
        }
    }

    /// Resolve a device record by plugin **type** id (`device_id`).
    ///
    /// Records are now keyed by the per-placement `instance_key`, but the
    /// UI/IPC surface still addresses plugins by `device_id` (audit point 2,
    /// step 4 migrates that).  With one instance per type (the only case that
    /// worked before this change) the match is unambiguous and preserves the
    /// previous `records.get(device_id)` semantics; with duplicates it returns
    /// one matching instance — no worse than the prior last-wins behaviour.
    fn record_by_device_id(&self, device_id: &str) -> Option<&DeviceRecord> {
        self.records
            .values()
            .find(|r| r.instance.device_id == device_id)
    }

    /// Push a normalised parameter change to a live plugin instance (ADR-007,
    /// ADR-017).
    ///
    /// Addressed by `instance_key` (the per-placement id = project `effect.id`),
    /// so two instances of the same plugin type on different tracks receive their
    /// own parameter changes instead of colliding on `device_id`.
    ///
    /// `native_id` is the format-native parameter identifier (VST3 `ParamID`,
    /// CLAP `clap_id`).  The value is enqueued on the instance's SPSC ring; the
    /// audio-graph wrapper drains it at the top of `process`.  No recompile, no
    /// lock.  Returns `Ok(false)` if the ring was full (newest dropped — the UI
    /// resends on its next tick).  Errors only if the placement is unknown.
    pub fn push_plugin_param(
        &self,
        instance_key: &str,
        native_id: u32,
        normalised: f32,
    ) -> Result<bool, HostError> {
        let record = self
            .records
            .get(instance_key)
            .ok_or_else(|| HostError::DeviceNotFound(instance_key.to_string()))?;
        Ok(record.instance.push_param(native_id, normalised))
    }

    /// Parameters of a loaded placement, as the plugin reports them (ADR-021).
    /// Only CLAP is wired so far; VST3 reports none yet.  Fails if the plugin
    /// is busy in a block that outlasts the harvest budget — the caller retries.
    pub fn plugin_params(&self, instance_key: &str) -> Result<Vec<PluginParamInfo>, HostError> {
        let instance = &self
            .records
            .get(instance_key)
            .ok_or_else(|| HostError::DeviceNotFound(instance_key.to_string()))?
            .instance;
        if let Some(bridged) = instance.bridge() {
            return Ok(bridged.params());
        }
        let Some(plugin) = instance.clap_plugin() else {
            return Ok(Vec::new());
        };
        let _gate = instance
            .offrt_try_acquire(FFI_GATE_HARVEST_BUDGET)
            .ok_or_else(|| HostError::InstantiationFailed("plugin busy, retry".into()))?;
        // SAFETY: live CLAP instance held by its record; FFI gate held above.
        Ok(unsafe { read_clap_params(plugin) })
    }

    pub fn suspend_device(&mut self, device_id: &str) -> Result<(), HostError> {
        let record = self
            .record_by_device_id(device_id)
            .ok_or_else(|| HostError::DeviceNotFound(device_id.to_string()))?;
        let next = transition(record.instance.lifecycle(), LifecycleState::Suspended)
            .map_err(HostError::LifecycleError)?;
        record.instance.set_lifecycle(next);
        Ok(())
    }

    pub fn resume_device(&mut self, device_id: &str) -> Result<(), HostError> {
        let record = self
            .record_by_device_id(device_id)
            .ok_or_else(|| HostError::DeviceNotFound(device_id.to_string()))?;
        let next = transition(record.instance.lifecycle(), LifecycleState::Running)
            .map_err(HostError::LifecycleError)?;
        record.instance.set_lifecycle(next);
        Ok(())
    }

    pub fn destroy_device(&mut self, device_id: &str) {
        // Records are keyed by `instance_key`; remove every record whose
        // instance is of this plugin type.
        let keys: Vec<String> = self
            .records
            .iter()
            .filter(|(_, r)| r.instance.device_id == device_id)
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            if let Some(record) = self.records.get(&key) {
                record.instance.set_lifecycle(LifecycleState::Destroyed);
            }
            self.records.remove(&key);
        }
    }

    /// Release host-side records for devices that are no longer present in the
    /// freshly compiled graph (`active` = the set of `device_id`s the new graph
    /// still references).
    ///
    /// Called on the command thread at the end of every successful compile.
    /// This is the runtime wiring for the "user removed the plugin entirely"
    /// path described in ADR-007: previously `destroy_device` had **no caller**,
    /// so the host kept its `Arc<PluginInstance>` clone for removed devices for
    /// the whole process lifetime — the FFI handle was never torn down and the
    /// `Destroyed` lifecycle transition was never reached.
    ///
    /// Dropping the host clone here does **not** tear the plugin down on the
    /// audio thread: the retiring `CompiledGraph` still holds the audio-side
    /// clone, so `PluginInstance::drop` runs only when that graph is freed
    /// through the snapshot drop ring (off-RT).  Devices still in `active` keep
    /// their existing record untouched, so a plugin that survives a recompile is
    /// never disturbed (no FFI churn, bit-identical).
    pub fn retain_devices(&mut self, active: &HashSet<String>) {
        // Match on instance_key (the map key), not device_id: comparing by device_id
        // would keep orphaned records when two placements of the same plugin type exist
        // and only one is removed — the shared device_id would still appear in `active`
        // via the surviving placement, preventing the removed placement's FFI teardown.
        self.records.retain(|instance_key, record| {
            let keep = active.contains(instance_key);
            if !keep {
                record.instance.set_lifecycle(LifecycleState::Destroyed);
            }
            keep
        });
        self.unavailable
            .retain(|instance_key, _| active.contains(instance_key));
    }

    /// Serialise a device's live state (Phase 4b / ADR-010).
    ///
    /// VST3 devices return their processor state via FFI
    /// (`IComponent::getState`).  Formats without an FFI state path yet (CLAP =
    /// Phase 4c, internal) fall back to the transitional `saved_state` cache so
    /// the returned `DeviceState` is never lost.
    pub fn save_device_state(&self, device_id: &str) -> Option<DeviceState> {
        let record = self.record_by_device_id(device_id)?;
        // Fence the live getState with the instance FFI gate (ADR-010) so it never
        // races the audio thread's process(); fall back to the cached blob if a
        // block is in flight and the gate cannot be claimed in time.
        let live = record
            .instance
            .offrt_try_acquire(FFI_GATE_HARVEST_BUDGET)
            .and_then(|_gate| record.instance.save_state_blob());
        match live {
            Some(bytes) => Some(DeviceState::from_binary(device_id, bytes)),
            None => Some(record.saved_state.clone()),
        }
    }

    /// Harvest live plugin state for every instantiated placement, keyed by
    /// `instance_key` (ADR-017), so duplicates of the same plugin type are saved
    /// separately.  Used by the project-save path; only non-empty blobs are
    /// returned (VST3/CLAP via FFI `getState`; other formats via the cached
    /// `saved_state`).
    ///
    /// Safe to call during playback: each live `getState` is fenced by the
    /// instance's FFI gate (ADR-010 follow-up), falling back to the cached blob
    /// when a block is in flight and the gate cannot be claimed in time.
    pub fn collect_plugin_states(&self) -> Vec<(String, Vec<u8>)> {
        // ADR-010: the getState FFI must not run concurrently with the audio
        // thread's process() on the same instance (single-threaded VST3/CLAP
        // contract). We claim each instance's FFI gate before the live getState;
        // when stopped the gate is free and acquisition is immediate, so the live
        // path is taken (behaviour unchanged). When a block is in flight and we
        // cannot claim the gate within the budget, we fall back to the record's
        // cached blob rather than race process().
        self.records
            .iter()
            .filter_map(|(key, record)| {
                let live = record
                    .instance
                    .offrt_try_acquire(FFI_GATE_HARVEST_BUDGET)
                    .and_then(|_gate| record.instance.save_state_blob());
                let ds = match live {
                    Some(bytes) => DeviceState::from_binary(&record.instance.device_id, bytes),
                    None => record.saved_state.clone(),
                };
                let bytes = ds.as_binary()?;
                if bytes.is_empty() {
                    None
                } else {
                    Some((key.clone(), bytes.to_vec()))
                }
            })
            .chain(
                self.unavailable
                    .iter()
                    .filter(|(key, _)| !self.records.contains_key(*key))
                    .filter_map(|(key, placement)| {
                        Some((key.clone(), placement.state.as_ref()?.as_binary()?.to_vec()))
                    }),
            )
            .collect()
    }

    pub fn device_count(&self) -> usize {
        self.records.len()
    }

    pub fn lifecycle_of(&self, device_id: &str) -> Option<LifecycleState> {
        self.record_by_device_id(device_id)
            .map(|r| r.instance.lifecycle())
    }

    /// Borrow the shared instance for an already-instantiated device.
    pub fn instance(&self, device_id: &str) -> Option<Arc<PluginInstance>> {
        self.record_by_device_id(device_id)
            .map(|r| Arc::clone(&r.instance))
    }

    /// Borrow by graph placement key (`effect.id` or `instrument:{track_id}`).
    pub fn instance_by_key(&self, instance_key: &str) -> Option<Arc<PluginInstance>> {
        self.records
            .get(instance_key)
            .map(|r| Arc::clone(&r.instance))
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub format: String,
    pub categories: Vec<String>,
    pub is_instrument: bool,
}

/// Only CLAP plugins can run sandboxed so far (ADR-015); for other formats the
/// flag is ignored and the plugin stays in-process.
fn runs_sandboxed(format: DeviceFormat, sandboxed: bool) -> bool {
    sandboxed && format == DeviceFormat::Clap
}

fn format_tag(fmt: DeviceFormat) -> String {
    match fmt {
        DeviceFormat::Vst3 => "VST3".into(),
        DeviceFormat::Clap => "CLAP".into(),
        DeviceFormat::Au => "AU".into(),
        DeviceFormat::Internal => "Internal".into(),
        DeviceFormat::Wasm => "WASM".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_system::registry::PluginManifest;

    fn manifest_with_sidechain(id: &str, aux: usize) -> PluginManifest {
        PluginManifest {
            id: id.to_string(),
            name: "Comp".into(),
            vendor: "Test".into(),
            version: String::new(),
            format: DeviceFormat::Clap,
            path: None,
            categories: vec![],
            num_audio_inputs: 2,
            num_audio_outputs: 2,
            has_midi_input: false,
            has_midi_output: false,
            num_sidechain_inputs: aux,
            is_instrument: false,
        }
    }

    #[test]
    fn sidechain_ports_read_from_manifest() {
        let mut host = PluginHost::new();
        host.registry_mut()
            .add(manifest_with_sidechain("clap:ducker", 1));
        assert_eq!(host.sidechain_input_ports("clap:ducker"), 1);
    }

    #[test]
    fn sidechain_ports_zero_for_unknown_or_plain_device() {
        let host = PluginHost::new();
        // Unknown device → 0 (compiler sizes node as before).
        assert_eq!(host.sidechain_input_ports("clap:does-not-exist"), 0);
        // Built-in device has no aux bus → 0 (bit-identical).
        assert_eq!(host.sidechain_input_ports("echoes.gain"), 0);
    }

    #[test]
    fn instantiated_internal_plugin_node_processes_audio() {
        use crate::audio_graph::buffer::AudioBuffer;
        use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};

        let mut host = PluginHost::new();

        let mut node = host
            .instantiate(NodeId(1), "echoes.gain", "echoes.gain", false)
            .expect("instantiation must succeed for a registered device");

        assert_eq!(host.device_count(), 1);
        assert!(host.instance("echoes.gain").is_some());

        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 16,
            num_channels: 2,
            live: true,
        });

        let mut input = AudioBuffer::new(2, 4);
        input.channel_mut(0).copy_from_slice(&[1.0, 2.0, 3.0, 4.0]);
        input.channel_mut(1).copy_from_slice(&[5.0, 6.0, 7.0, 8.0]);
        let inputs = [&input];
        let mut outputs = vec![AudioBuffer::new(2, 4)];
        node.process(&inputs, &mut outputs, &TransportInfo::stopped(48_000.0));

        assert_eq!(outputs[0].channel(0), &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(outputs[0].channel(1), &[5.0, 6.0, 7.0, 8.0]);
    }

    /// `retain_devices` drops records absent from the active set (the teardown
    /// path that was previously dead) and leaves surviving devices untouched.
    #[test]
    fn retain_devices_prunes_removed_records() {
        let mut host = PluginHost::new();
        host.instantiate(NodeId(1), "echoes.gain", "echoes.gain", false)
            .unwrap();
        host.instantiate(NodeId(2), "echoes.pan", "echoes.pan", false)
            .unwrap();
        assert_eq!(host.device_count(), 2);

        let mut active = HashSet::new();
        active.insert("echoes.gain".to_string());
        host.retain_devices(&active);

        assert_eq!(host.device_count(), 1);
        assert!(host.instance("echoes.gain").is_some());
        assert!(host.instance("echoes.pan").is_none());
    }

    /// Audit point 2 (host-map level): two instances of the **same** plugin
    /// type used on two tracks no longer overwrite each other — they carry
    /// distinct `instance_key`s, so both records survive.  Previously the map was
    /// keyed by `device_id`, collapsing them to one.
    #[test]
    fn same_device_id_on_two_tracks_keeps_both_records() {
        let mut host = PluginHost::new();
        host.instantiate(NodeId(1), "track-a:fx0", "echoes.gain", false)
            .unwrap();
        host.instantiate(NodeId(2), "track-b:fx0", "echoes.gain", false)
            .unwrap();

        assert_eq!(host.device_count(), 2, "two placements → two records");
        // The legacy device_id-addressed path still resolves to one of them.
        assert!(host.instance("echoes.gain").is_some());
    }

    /// Audit point 1 / ADR-016: instantiating the **same placement**
    /// (`instance_key`) again reuses the live instance instead of rebuilding it —
    /// the underlying `PluginInstance` (its `instance_id`) is preserved and the
    /// record count does not grow. Exercised on the reuse-safe internal format.
    #[test]
    fn same_instance_key_reuses_the_live_instance() {
        let mut host = PluginHost::new();
        host.instantiate(NodeId(1), "fx0", "echoes.gain", false)
            .unwrap();
        let first_id = host.instance("echoes.gain").unwrap().instance_id();

        // Recompile re-instantiates the same placement.
        host.instantiate(NodeId(2), "fx0", "echoes.gain", false)
            .unwrap();
        let second_id = host.instance("echoes.gain").unwrap().instance_id();

        assert_eq!(
            first_id, second_id,
            "same instance_key must reuse the live instance, not rebuild it"
        );
        assert_eq!(host.device_count(), 1, "reuse must not add a record");
    }

    /// ADR-017: `push_plugin_param` addresses by `instance_key`, so two instances
    /// of the same plugin type on different placements receive their own changes
    /// instead of colliding.
    #[test]
    fn push_plugin_param_targets_the_addressed_instance() {
        let mut host = PluginHost::new();
        host.instantiate(NodeId(1), "track-a:fx0", "echoes.gain", false)
            .unwrap();
        host.instantiate(NodeId(2), "track-b:fx0", "echoes.gain", false)
            .unwrap();

        assert!(host.push_plugin_param("track-b:fx0", 42, 0.5).unwrap());

        // The change landed on B's ring, not A's.
        let mut a = Vec::new();
        host.instance_by_key("track-a:fx0")
            .unwrap()
            .drain_params(|c| a.push(c.native_id));
        let mut b = Vec::new();
        host.instance_by_key("track-b:fx0")
            .unwrap()
            .drain_params(|c| b.push(c.native_id));

        assert!(a.is_empty(), "untargeted instance must receive nothing");
        assert_eq!(b, vec![42], "targeted instance receives the change");
    }

    /// ADR-016: a staged state blob (project load) forces a fresh instance so the
    /// blob is applied at instantiation (ADR-010) — reuse must not short-circuit it.
    #[test]
    fn staged_state_forces_fresh_instance_over_reuse() {
        let mut host = PluginHost::new();
        host.instantiate(NodeId(1), "fx0", "echoes.gain", false)
            .unwrap();
        let first_id = host.instance("echoes.gain").unwrap().instance_id();

        let mut staged = HashMap::new();
        staged.insert("echoes.gain".to_string(), vec![1u8, 2, 3]);
        host.set_pending_state_blobs(staged);
        host.instantiate(NodeId(2), "fx0", "echoes.gain", false)
            .unwrap();
        let second_id = host.instance("echoes.gain").unwrap().instance_id();

        assert_ne!(
            first_id, second_id,
            "a staged blob must rebuild the instance, not reuse the live one"
        );
    }

    /// ADR-010 race-free restore: a blob staged before the compile is applied to
    /// the instance *at instantiation* (before it could enter a snapshot) and is
    /// retained so a later save round-trips it.  Exercised on the internal path
    /// (no FFI), where the blob lands in the record cache.
    #[test]
    fn staged_state_is_applied_at_instantiation() {
        let mut host = PluginHost::new();
        let blob = vec![1u8, 2, 3, 4];

        let mut staged = HashMap::new();
        staged.insert("echoes.gain".to_string(), blob.clone());
        host.set_pending_state_blobs(staged);

        host.instantiate(NodeId(1), "echoes.gain", "echoes.gain", false)
            .unwrap();

        let restored = host
            .save_device_state("echoes.gain")
            .expect("device is instantiated");
        assert_eq!(
            restored.as_binary(),
            Some(blob.as_slice()),
            "staged blob must be applied/retained at instantiation"
        );
    }

    /// R-4: state must survive a controlled re-instantiation, not only a project
    /// reload. A staged blob seeds the first instance; once the staged blobs are
    /// cleared, forcing a rebuild that bypasses ADR-016 reuse (terminal instance)
    /// must carry that state into the new instance instead of reverting to default.
    #[test]
    fn state_survives_controlled_reinstantiation() {
        let mut host = PluginHost::new();
        let blob = vec![1u8, 2, 3, 4];

        // Project load: stage and apply the blob to the first instance, then clear.
        let mut staged = HashMap::new();
        staged.insert("fx0".to_string(), blob.clone());
        host.set_pending_state_blobs(staged);
        host.instantiate(NodeId(1), "fx0", "echoes.gain", false)
            .unwrap();
        host.clear_pending_state_blobs();
        let first_id = host.instance_by_key("fx0").unwrap().instance_id();

        // Force a rebuild that reuse cannot short-circuit (terminal instance).
        host.instance_by_key("fx0")
            .unwrap()
            .set_lifecycle(LifecycleState::Destroyed);
        host.instantiate(NodeId(2), "fx0", "echoes.gain", false)
            .unwrap();
        let second_id = host.instance_by_key("fx0").unwrap().instance_id();

        assert_ne!(
            first_id, second_id,
            "a terminal instance must be rebuilt, not reused"
        );
        assert_eq!(
            host.save_device_state("echoes.gain").unwrap().as_binary(),
            Some(blob.as_slice()),
            "state must survive re-instantiation, not just project load"
        );
    }

    /// A controlled re-instantiation where the placement now hosts a *different*
    /// plugin type must NOT carry the old state across types.
    #[test]
    fn reinstantiation_does_not_carry_state_across_types() {
        let mut host = PluginHost::new();
        let mut staged = HashMap::new();
        staged.insert("fx0".to_string(), vec![5u8, 6, 7]);
        host.set_pending_state_blobs(staged);
        host.instantiate(NodeId(1), "fx0", "echoes.gain", false)
            .unwrap();
        host.clear_pending_state_blobs();

        // Same placement key, different plugin type → no carry.
        host.instantiate(NodeId(2), "fx0", "echoes.pan", false)
            .unwrap();

        assert_eq!(
            host.save_device_state("echoes.pan").unwrap().as_binary(),
            None,
            "state must not leak across plugin types on the same placement"
        );
    }

    /// After the staged blobs are cleared, a fresh instantiation must NOT re-apply
    /// stale loaded state — the guard that keeps a later structural recompile from
    /// stamping old loaded state over the instance's live state.
    #[test]
    fn cleared_staged_state_is_not_applied() {
        let mut host = PluginHost::new();
        let mut staged = HashMap::new();
        staged.insert("echoes.gain".to_string(), vec![9u8, 9, 9]);
        host.set_pending_state_blobs(staged);
        host.clear_pending_state_blobs();

        host.instantiate(NodeId(1), "echoes.gain", "echoes.gain", false)
            .unwrap();

        let restored = host.save_device_state("echoes.gain").unwrap();
        assert_eq!(
            restored.as_binary(),
            None,
            "no staged blob → no state applied (empty record)"
        );
    }

    fn internal_manifest(id: &str) -> PluginManifest {
        PluginManifest {
            format: DeviceFormat::Internal,
            ..manifest_with_sidechain(id, 0)
        }
    }

    fn stage(host: &mut PluginHost, instance_key: &str, blob: &[u8]) {
        let mut staged = HashMap::new();
        staged.insert(instance_key.to_string(), blob.to_vec());
        host.set_pending_state_blobs(staged);
    }

    fn saved_blob(host: &PluginHost, instance_key: &str) -> Option<Vec<u8>> {
        host.collect_plugin_states()
            .into_iter()
            .find(|(key, _)| key == instance_key)
            .map(|(_, bytes)| bytes)
    }

    #[test]
    fn a_crashed_plugin_is_reported_and_rebuilt_from_its_saved_state() {
        let mut host = PluginHost::new();
        stage(&mut host, "fx0", &[3, 1, 4]);
        host.instantiate(NodeId(1), "fx0", "echoes.gain", false).unwrap();
        host.clear_pending_state_blobs();
        let crashed = host.instance_by_key("fx0").unwrap();
        crashed.set_lifecycle(LifecycleState::Error);

        let unavailable = host.unavailable_plugins();
        assert_eq!(unavailable.len(), 1);
        assert_eq!(unavailable[0].reason, UnavailableReason::Crashed);

        host.instantiate(NodeId(2), "fx0", "echoes.gain", false).unwrap();
        assert_ne!(host.instance_by_key("fx0").unwrap().instance_id(), crashed.instance_id());
        assert!(host.unavailable_plugins().is_empty(), "the reload clears the crash");
        assert_eq!(saved_blob(&host, "fx0"), Some(vec![3, 1, 4]));
    }

    #[test]
    fn missing_effect_keeps_its_state_for_the_next_save() {
        let mut host = PluginHost::new();
        stage(&mut host, "fx-missing", &[7, 7, 7]);
        assert!(host.instantiate(NodeId(1), "fx-missing", "clap:not-installed", false).is_err());
        host.clear_pending_state_blobs();

        assert_eq!(saved_blob(&host, "fx-missing"), Some(vec![7, 7, 7]));
    }

    #[test]
    fn missing_instrument_keeps_its_state_across_recompiles() {
        let mut host = PluginHost::new();
        stage(&mut host, "instrument:t1", &[1, 2, 3]);
        assert!(host.instantiate_instrument("instrument:t1", "clap:not-installed").is_err());
        host.clear_pending_state_blobs();
        assert!(host.instantiate_instrument("instrument:t1", "clap:not-installed").is_err());

        assert_eq!(saved_blob(&host, "instrument:t1"), Some(vec![1, 2, 3]));
    }

    #[test]
    fn removed_placement_drops_the_kept_state() {
        let mut host = PluginHost::new();
        stage(&mut host, "fx-missing", &[7]);
        let _ = host.instantiate(NodeId(1), "fx-missing", "clap:not-installed", false);
        host.clear_pending_state_blobs();

        host.retain_devices(&HashSet::new());
        assert_eq!(saved_blob(&host, "fx-missing"), None);
    }

    #[test]
    fn switching_to_another_missing_plugin_drops_the_old_state() {
        let mut host = PluginHost::new();
        stage(&mut host, "instrument:t1", &[1, 2, 3]);
        let _ = host.instantiate_instrument("instrument:t1", "clap:first");
        host.clear_pending_state_blobs();

        let _ = host.instantiate_instrument("instrument:t1", "clap:second");
        assert_eq!(saved_blob(&host, "instrument:t1"), None);
    }

    #[test]
    fn plugin_found_later_gets_its_kept_state_back() {
        let mut host = PluginHost::new();
        stage(&mut host, "fx0", &[4, 5, 6]);
        assert!(host.instantiate(NodeId(1), "fx0", "test:late", false).is_err());
        host.clear_pending_state_blobs();

        host.registry_mut().add(internal_manifest("test:late"));
        host.instantiate(NodeId(2), "fx0", "test:late", false)
            .expect("plugin is now available");

        let restored = host.save_device_state("test:late").expect("device is instantiated");
        assert_eq!(restored.as_binary(), Some([4u8, 5, 6].as_slice()));
        assert_eq!(saved_blob(&host, "fx0"), Some(vec![4, 5, 6]), "saved once, from the live record");
        assert_eq!(host.collect_plugin_states().len(), 1);
    }
}
