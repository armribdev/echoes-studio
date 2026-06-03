//! Shared plugin instance — Phase 2a foundation (ADR-007).
//!
//! ## Why this exists
//!
//! Before Phase 2a the wrapper node owned the FFI handles (`IComponent`,
//! `IAudioProcessor`, `ClapPlugin`) directly, while the `PluginHost` kept a
//! parallel `DeviceRecord` with **no link** to the live instance.  As a
//! consequence:
//!   * `PluginHost::set_parameter_normalised` had no addressee — it could only
//!     stash a JSON blob the audio thread never read.
//!   * The GUI editor opened a **second** IComponent / ClapPlugin (A1 of the
//!     plugin-GUI audit) because there was no shareable handle to the audio-side
//!     instance.
//!
//! ## What this file provides
//!
//! [`PluginInstance`] owns the format-specific native handles **once**, behind
//! an `Arc`.  Every other actor (the audio-graph wrapper node, the
//! `PluginHost`, eventually the GUI editor in Phase 2c) holds a **clone of the
//! same `Arc`** — there is one, and only one, live plugin per `device_id`.
//!
//! ## Real-time contract
//!
//! `PluginInstance` is constructed and destroyed off the audio thread.  The
//! audio thread only ever performs:
//!   1. A non-atomic pointer chase through `Arc::deref` to reach the native
//!      handle.  Identical cost to a struct field read.
//!   2. FFI calls on that handle, sequenced exclusively by the scheduler's
//!      per-step ownership invariant.
//!
//! `Arc::clone` happens at instantiation time (off-RT).  `Arc::drop` happens
//! when the old `CompiledGraph` is retired through the snapshot drop ring, on
//! a dedicated worker thread — never on the audio thread.
//!
//! ## Send / Sync
//!
//! VST3 and CLAP both mandate single-threaded plugin access.  The host
//! enforces it (audio thread holds exclusive access during `process_block`;
//! the command thread only touches the instance between blocks).  Under that
//! discipline the raw pointers inside `NativeHandle` are safe to expose as
//! `Send + Sync`.  In Phase 2a no off-RT caller invokes any FFI on the
//! instance — only the wrapper does, from the audio thread.  Phase 2c will
//! introduce GUI calls and must coordinate via the lifecycle state machine.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::Arc;

use crate::plugin_formats::clap::ffi::{ClapHost, ClapLibrary, ClapPlugin};
use crate::plugin_formats::vst3::ffi::{IAudioProcessor, IComponent, Vst3Library};
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::param_queue::{ParamChange, ParamQueue};
use crate::plugin_system::wrapper::{DeviceFormat, DeviceInfo};

/// Capacity of the per-instance parameter SPSC ring (ADR-007).  1024 slots ≈
/// 17 s of edits at a 60 Hz UI — far more than the drain interval needs.
const PARAM_QUEUE_CAPACITY: usize = 1024;

// Every `PluginInstance` is stamped at construction with a process-unique,
// monotonically increasing `instance_id`.  Unlike `device_id` — which is the
// project-side *plugin type* identifier (e.g. the VST3 FUID / CLAP id, shared by
// every use of that plugin) — `instance_id` identifies *this one live instance*:
//
//   * **Unique:** `fetch_add` on a global atomic never hands out the same value
//     twice within a process run, so two instances of the *same* plugin type
//     (and therefore the same `device_id`) still carry distinct identities.
//   * **Stable & invariant:** assigned once in the constructor, never mutated and
//     never recycled — the field has no setter.
//
// This is the foundation for addressing a specific instance instead of a plugin
// *type*; the project/graph/gateway layers must carry it end-to-end before the
// `device_id` collision (R-2) can be fully closed (see module note below).
static NEXT_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

/// Hand out the next process-unique instance identity.
fn next_instance_id() -> u64 {
    // Relaxed is sufficient: we only need uniqueness, not ordering against other
    // memory — and this runs off the audio thread at construction time.
    NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Format-specific live handles for one plugin instance.
///
/// Variants own every resource the format needs to keep alive for the lifetime
/// of the instance.  `Drop` on the enclosing [`PluginInstance`] is responsible
/// for ordered teardown (FFI release → DLL drop).
pub enum NativeHandle {
    /// In-process internal device — no FFI handle to manage.
    Internal,

    /// Live VST3 IComponent + IAudioProcessor (often the same COM object).
    /// The `Vst3Library` Arc keeps the DLL mapped until the last consumer
    /// drops its reference.
    Vst3 {
        component: NonNull<IComponent>,
        processor: NonNull<IAudioProcessor>,
        _lib: Arc<Vst3Library>,
    },

    /// Live CLAP plugin instance.  `_host` is heap-stable for the lifetime
    /// of `plugin` (CLAP spec: the host pointer passed to `create_plugin`
    /// must outlive the plugin).
    Clap {
        plugin: *const ClapPlugin,
        _host: Box<ClapHost>,
        _lib: Arc<ClapLibrary>,
    },
}

// SAFETY (see module doc): VST3/CLAP plugin access is sequenced by the host;
// no concurrent FFI invocation is possible in Phase 2a.  Raw pointers are only
// dereferenced under that discipline.
unsafe impl Send for NativeHandle {}
unsafe impl Sync for NativeHandle {}

/// Shared, refcounted handle to one live plugin instance.
///
/// Held by:
///   * `PluginHost.instances` — the long-lived ownership root.
///   * The wrapper node inside `CompiledGraph.processors` — captured at
///     compile time, dropped through the snapshot drop ring.
///   * (Phase 2c) the GUI editor window.
///
/// All fields except `lifecycle` are immutable after construction.
pub struct PluginInstance {
    /// Process-unique, stable identity for **this live instance** (see the
    /// "Instance identity" note above).  Distinct from `device_id`, which names
    /// the plugin *type* and is shared across every use of the same plugin.
    /// Assigned once at construction; immutable and never recycled.
    instance_id: u64,

    /// Stable project-side identifier (e.g. `"vst3://<dll>?<cid>"`).  This is the
    /// plugin **type** id — two instances of the same plugin share it, so it is
    /// **not** a unique instance handle (use [`instance_id`](Self::instance_id)).
    pub device_id: String,

    /// Human-readable name (from the plugin manifest).
    pub name: String,

    /// Vendor string (from the plugin manifest; may be empty).
    pub vendor: String,

    /// Host-visible plugin format.
    pub format: DeviceFormat,

    /// Format-specific native handle.  Held in the same struct so dropping
    /// the last `Arc` tears the handle down in [`Drop`].
    handle: NativeHandle,

    /// Atomic lifecycle marker.  Phase 2a leaves the wrapper free to keep its
    /// own copy as well (full unification is Phase 2b); this field exists so
    /// the host can read state without going through the wrapper.
    lifecycle: AtomicU8,

    /// Lock-free SPSC ring of pending parameter changes (ADR-007).  The
    /// command thread (via `PluginHost`) is the sole producer; the audio-graph
    /// wrapper drains it at the top of `process` — the sole consumer.
    param_queue: ParamQueue,
}

impl PluginInstance {
    pub fn new_internal(device_id: impl Into<String>, name: impl Into<String>) -> Arc<Self> {
        Arc::new(PluginInstance {
            instance_id: next_instance_id(),
            device_id: device_id.into(),
            name: name.into(),
            vendor: "Internal".to_string(),
            format: DeviceFormat::Internal,
            handle: NativeHandle::Internal,
            lifecycle: AtomicU8::new(encode_lifecycle(LifecycleState::Running)),
            param_queue: ParamQueue::with_capacity(PARAM_QUEUE_CAPACITY),
        })
    }

    /// Construct a VST3 instance from already-acquired COM pointers.
    ///
    /// The `Vst3Library` Arc is moved in; `Drop` on this instance is what
    /// eventually frees it.
    pub fn new_vst3(
        device_id: impl Into<String>,
        name: impl Into<String>,
        component: NonNull<IComponent>,
        processor: NonNull<IAudioProcessor>,
        lib: Arc<Vst3Library>,
    ) -> Arc<Self> {
        Arc::new(PluginInstance {
            instance_id: next_instance_id(),
            device_id: device_id.into(),
            name: name.into(),
            vendor: String::new(),
            format: DeviceFormat::Vst3,
            handle: NativeHandle::Vst3 {
                component,
                processor,
                _lib: lib,
            },
            lifecycle: AtomicU8::new(encode_lifecycle(LifecycleState::Created)),
            param_queue: ParamQueue::with_capacity(PARAM_QUEUE_CAPACITY),
        })
    }

    pub fn new_clap(
        device_id: impl Into<String>,
        name: impl Into<String>,
        vendor: impl Into<String>,
        plugin: *const ClapPlugin,
        host: Box<ClapHost>,
        lib: Arc<ClapLibrary>,
    ) -> Arc<Self> {
        Arc::new(PluginInstance {
            instance_id: next_instance_id(),
            device_id: device_id.into(),
            name: name.into(),
            vendor: vendor.into(),
            format: DeviceFormat::Clap,
            handle: NativeHandle::Clap {
                plugin,
                _host: host,
                _lib: lib,
            },
            lifecycle: AtomicU8::new(encode_lifecycle(LifecycleState::Created)),
            param_queue: ParamQueue::with_capacity(PARAM_QUEUE_CAPACITY),
        })
    }

    /// Process-unique, stable identity for this live instance.  Two instances of
    /// the same plugin type (same [`device_id`](Self::device_id)) return
    /// **different** values; a given instance returns the **same** value for its
    /// whole lifetime.  Use this — not `device_id` — whenever a caller must point
    /// at one specific instance rather than a plugin type.
    pub fn instance_id(&self) -> u64 {
        self.instance_id
    }

    /// Push a normalised parameter change into the SPSC ring.  Called on the
    /// command thread (sole producer).  Returns `false` if the ring is full.
    pub fn push_param(&self, native_id: u32, normalised: f32) -> bool {
        self.param_queue.push(ParamChange {
            native_id,
            normalised,
            sample_offset: 0,
        })
    }

    /// Drain all pending parameter changes (audio thread, sole consumer).
    pub fn drain_params(&self, f: impl FnMut(ParamChange)) {
        self.param_queue.drain(f);
    }

    // VST3 (`IComponent::get/setState`) and CLAP (`clap.state.save/load`) both
    // carry their native handle here.  These run on the command thread; per the
    // module header they are NOT sequenced against the audio thread's `process`
    // on the same instance.  In practice save/load happen while the transport is
    // stopped (explicit project save / project open), where no `process` is in
    // flight for this instance — that is the operating invariant until a proper
    // between-blocks barrier lands.  Internal devices (and AU/WASM) return
    // `None` / `false`.

    /// Serialise the live plugin's processor state.  `None` when the format has
    /// no FFI state path yet or the plugin reports no state.
    pub fn save_state_blob(&self) -> Option<Vec<u8>> {
        match &self.handle {
            NativeHandle::Vst3 { component, .. } => unsafe {
                crate::plugin_formats::vst3::ffi::read_component_state(component.as_ptr())
            },
            NativeHandle::Clap { plugin, .. } => unsafe {
                crate::plugin_formats::clap::ffi::read_plugin_state(*plugin)
            },
            _ => None,
        }
    }

    /// Restore previously-saved processor state into the live plugin.  Returns
    /// `false` if unsupported for this format or the plugin rejected the blob.
    pub fn load_state_blob(&self, bytes: &[u8]) -> bool {
        match &self.handle {
            NativeHandle::Vst3 { component, .. } => unsafe {
                crate::plugin_formats::vst3::ffi::write_component_state(component.as_ptr(), bytes)
            },
            NativeHandle::Clap { plugin, .. } => unsafe {
                crate::plugin_formats::clap::ffi::write_plugin_state(*plugin, bytes)
            },
            _ => false,
        }
    }

    /// Borrow the native handle for FFI dispatch.  Wrappers pattern-match
    /// the variant they expect; other variants indicate a routing bug at
    /// instantiation time.
    pub fn handle(&self) -> &NativeHandle {
        &self.handle
    }

    /// Convenience: extract VST3 handles if this instance is VST3.
    pub fn vst3_handles(&self) -> Option<(NonNull<IComponent>, NonNull<IAudioProcessor>)> {
        match &self.handle {
            NativeHandle::Vst3 {
                component,
                processor,
                ..
            } => Some((*component, *processor)),
            _ => None,
        }
    }

    /// Convenience: extract the CLAP plugin pointer if this instance is CLAP.
    pub fn clap_plugin(&self) -> Option<*const ClapPlugin> {
        match &self.handle {
            NativeHandle::Clap { plugin, .. } => Some(*plugin),
            _ => None,
        }
    }

    pub fn set_lifecycle(&self, state: LifecycleState) {
        self.lifecycle
            .store(encode_lifecycle(state), Ordering::Release);
    }

    pub fn lifecycle(&self) -> LifecycleState {
        decode_lifecycle(self.lifecycle.load(Ordering::Acquire))
    }

    pub fn legacy_device_info(&self) -> DeviceInfo {
        DeviceInfo {
            device_id: self.device_id.clone(),
            name: self.name.clone(),
            vendor: self.vendor.clone(),
            version: String::new(),
            format: self.format,
            num_inputs: 2,
            num_outputs: 2,
            has_midi_input: false,
            has_midi_output: false,
        }
    }
}

// Runs on whichever thread releases the last `Arc<PluginInstance>`:
//   * Most commonly the snapshot drop-ring worker (when the old
//     `CompiledGraph` retires).
//   * Occasionally the command thread (when `PluginHost::destroy_device`
//     removes the host-side clone after the audio side is gone).
//
// **Never the audio thread**: the audio thread releases its grip by swapping
// the snapshot, after which the old graph travels to the drop ring.

impl Drop for PluginInstance {
    fn drop(&mut self) {
        match &self.handle {
            NativeHandle::Internal => {}

            NativeHandle::Vst3 {
                component,
                processor,
                ..
            } => {
                let proc = processor.as_ptr();
                let comp = component.as_ptr();
                unsafe {
                    ((*(*proc).vtbl).set_processing)(proc, 0);
                    ((*(*comp).vtbl).set_active)(comp, 0);
                    ((*(*comp).vtbl).terminate)(comp);
                    // IAudioProcessor and IComponent may be the same COM object;
                    // release each independently.
                    ((*(*proc).vtbl).release)(proc as *mut _);
                    ((*(*comp).vtbl).release)(comp as *mut _);
                }
            }

            NativeHandle::Clap { plugin, .. } => {
                let p = *plugin;
                unsafe {
                    ((*p).destroy)(p);
                }
            }
        }
    }
}

const LC_CREATED: u8 = 0;
const LC_LOADED: u8 = 1;
const LC_INITIALIZED: u8 = 2;
const LC_RUNNING: u8 = 3;
const LC_SUSPENDED: u8 = 4;
const LC_DESTROYED: u8 = 5;
const LC_ERROR: u8 = 6;

fn encode_lifecycle(state: LifecycleState) -> u8 {
    match state {
        LifecycleState::Created => LC_CREATED,
        LifecycleState::Loaded => LC_LOADED,
        LifecycleState::Initialized => LC_INITIALIZED,
        LifecycleState::Running => LC_RUNNING,
        LifecycleState::Suspended => LC_SUSPENDED,
        LifecycleState::Destroyed => LC_DESTROYED,
        LifecycleState::Error => LC_ERROR,
    }
}

fn decode_lifecycle(code: u8) -> LifecycleState {
    match code {
        LC_LOADED => LifecycleState::Loaded,
        LC_INITIALIZED => LifecycleState::Initialized,
        LC_RUNNING => LifecycleState::Running,
        LC_SUSPENDED => LifecycleState::Suspended,
        LC_DESTROYED => LifecycleState::Destroyed,
        LC_ERROR => LifecycleState::Error,
        _ => LifecycleState::Created,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_queue_push_then_drain_roundtrips() {
        // An internal instance carries the same SPSC ring as any plugin.
        let inst = PluginInstance::new_internal("dev://x", "X");
        assert!(inst.push_param(7, 0.25));
        assert!(inst.push_param(9, 0.75));

        let mut got = Vec::new();
        inst.drain_params(|c| got.push((c.native_id, c.normalised)));
        assert_eq!(got, vec![(7, 0.25), (9, 0.75)]);

        let mut n = 0;
        inst.drain_params(|_| n += 1);
        assert_eq!(n, 0);
    }

    #[test]
    fn instance_id_is_unique_even_for_the_same_device_id() {
        // Two instances of the *same* plugin type (identical `device_id`) — the
        // exact R-2 collision scenario (same plugin on two tracks).  Their
        // instance identities must differ.
        let a = PluginInstance::new_internal("echoes.gain", "Gain");
        let b = PluginInstance::new_internal("echoes.gain", "Gain");

        assert_eq!(a.device_id, b.device_id, "same plugin type → same device_id");
        assert_ne!(
            a.instance_id(),
            b.instance_id(),
            "same device_id must NOT collapse the instance identity"
        );
    }

    #[test]
    fn instance_id_is_stable_for_an_instances_lifetime() {
        let inst = PluginInstance::new_internal("echoes.gain", "Gain");
        let first = inst.instance_id();
        inst.set_lifecycle(LifecycleState::Suspended);
        assert_eq!(inst.instance_id(), first);
        assert_eq!(inst.instance_id(), first);
    }
}
