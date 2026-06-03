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
use std::ptr::NonNull;
use std::sync::Arc;

use crate::audio_graph::node::{AudioNode, NodeId};
use crate::audio_graph::nodes::clap_instrument::ClapInstrument;
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::plugin_formats::clap::ffi::{ClapLibrary, CLAP_PLUGIN_FACTORY_ID};
use crate::plugin_formats::clap::host::decode_plugin_id as clap_decode_id;
use crate::plugin_formats::clap::wrapper::ClapWrapperNode;
use crate::plugin_formats::vst3::ffi::{instantiate_processor, Vst3Library};
use crate::plugin_formats::vst3::host::decode_plugin_id as vst3_decode_id;
use crate::plugin_formats::vst3::wrapper::VstWrapperNode;
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::lifecycle::{transition, LifecycleError, LifecycleState};
use crate::plugin_system::registry::{PluginManifest, PluginRegistry};
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{DeviceFormat, PassThroughDevice};

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
            HostError::DeviceNotFound(id) => write!(f, "device not found: {id}"),
            HostError::PluginNotFound(id) => write!(f, "plugin not in registry: {id}"),
            HostError::LifecycleError(e) => write!(f, "lifecycle error: {e}"),
            HostError::InstantiationFailed(e) => write!(f, "instantiation failed: {e}"),
            HostError::ParameterNotFound(k) => write!(f, "parameter not found: {k}"),
        }
    }
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
}

impl PluginHost {
    pub fn new() -> Self {
        Self {
            registry: PluginRegistry::new(),
            records: HashMap::new(),
            pending_state_blobs: HashMap::new(),
        }
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

    /// `instance_key` = per-placement id (ADR-007); host keeps `Arc` in `records`.
    pub fn instantiate(
        &mut self,
        node_id: NodeId,
        instance_key: &str,
        plugin_id: &str,
        sandboxed: bool,
    ) -> Result<Box<dyn AudioNode>, HostError> {
        // Inert until ADR-015 step 6d wires the out-of-process bridge.
        let _ = sandboxed;

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

        instance.set_lifecycle(LifecycleState::Running);

        // ADR-010 race-free state restore: if the project-load path staged a blob
        // for this device, apply it now — the instance is brand-new and not yet in
        // any snapshot, so `setState` cannot race the audio thread's `process()`.
        // The blob is also cached in `saved_state` so formats with no FFI state
        // path (internal/AU/WASM) still round-trip it on the next save.
        let saved_state = match self.pending_state_blobs.get(plugin_id) {
            Some(bytes) => {
                let _ = instance.load_state_blob(bytes);
                DeviceState::from_binary(plugin_id, bytes.clone())
            }
            None => DeviceState::empty(plugin_id),
        };

        self.records.insert(
            instance_key.to_string(),
            DeviceRecord {
                instance,
                saved_state,
            },
        );

        Ok(device)
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
        let manifest = self
            .registry
            .get(plugin_id)
            .ok_or_else(|| HostError::PluginNotFound(plugin_id.to_string()))?
            .clone();

        match manifest.format {
            DeviceFormat::Clap => {
                let instance = self.create_clap_instance(plugin_id, &manifest)?;
                instance.set_lifecycle(LifecycleState::Running);
                self.records.insert(
                    instance_key.to_string(),
                    DeviceRecord {
                        instance: Arc::clone(&instance),
                        saved_state: DeviceState::empty(plugin_id),
                    },
                );
                Ok(Box::new(ClapInstrument::new(instance)))
            }
            other => Err(HostError::InstantiationFailed(format!(
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
        let record = self
            .record_by_device_id(device_id)
            .ok_or_else(|| HostError::DeviceNotFound(device_id.to_string()))?;
        Ok(record.instance.push_param(native_id, normalised))
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
        // Records are keyed by `instance_key`; `active` is the set of
        // `device_id`s (plugin types) the new graph still references, so match
        // on the instance's type rather than on the map key.
        self.records.retain(|_instance_key, record| {
            let keep = active.contains(&record.instance.device_id);
            if !keep {
                // Mark terminal; the actual FFI teardown happens in
                // `PluginInstance::drop` once the last `Arc` is released off-RT.
                record.instance.set_lifecycle(LifecycleState::Destroyed);
            }
            keep
        });
    }

    /// Serialise a device's live state (Phase 4b / ADR-010).
    ///
    /// VST3 devices return their processor state via FFI
    /// (`IComponent::getState`).  Formats without an FFI state path yet (CLAP =
    /// Phase 4c, internal) fall back to the transitional `saved_state` cache so
    /// the returned `DeviceState` is never lost.
    pub fn save_device_state(&self, device_id: &str) -> Option<DeviceState> {
        let record = self.record_by_device_id(device_id)?;
        match record.instance.save_state_blob() {
            Some(bytes) => Some(DeviceState::from_binary(device_id, bytes)),
            None => Some(record.saved_state.clone()),
        }
    }

    /// Iterate the device (plugin **type**) IDs of all currently-instantiated
    /// plugins.  Used by the project-save path to harvest live plugin state,
    /// which is keyed by `device_id` in the project file (unchanged here).  May
    /// yield the same `device_id` more than once when a plugin type is used on
    /// several tracks; the consumer de-duplicates via its `device_id`-keyed map.
    pub fn device_ids(&self) -> impl Iterator<Item = &str> {
        self.records.values().map(|r| r.instance.device_id.as_str())
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
}
