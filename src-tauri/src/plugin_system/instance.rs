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
#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::audio_graph::plugin_params::ValueModulatedParam;
use crate::plugin_formats::clap::audio_ports::{read_output_ports, ClapOutputPort};
use crate::plugin_formats::clap::ffi::{
    audio_thread_scope, notify_track_info_changed, read_note_names, ClapHost, ClapHostContext,
    ClapLibrary, ClapPlugin, HostTrackInfo, NoteName,
};
use crate::plugin_formats::vst3::ffi::{
    IAudioProcessor, IComponent, ProcessSetup, Vst3Library, SPEAKER_STEREO,
};
use crate::plugin_system::bridge::BridgedPlugin;
use crate::plugin_system::crash_marker::{self, MarkerGuard, PluginThread};
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::param_queue::{ParamChange, ParamQueue};
use crate::plugin_system::wrapper::DeviceFormat;

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
#[cfg(test)]
static NEXT_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

/// Hand out the next process-unique instance identity.
#[cfg(test)]
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

    /// Live CLAP plugin instance.  `host` is heap-stable for the lifetime
    /// of `plugin` (CLAP spec: the host pointer passed to `create_plugin`
    /// must outlive the plugin).
    Clap {
        plugin: *const ClapPlugin,
        host: Box<ClapHost>,
        _lib: Arc<ClapLibrary>,
    },

    /// CLAP plugin hosted in a `plugin_host_bridge` process (ADR-015).  Its
    /// off-RT calls are requests to that process, so they never touch this
    /// process's memory and need no FFI gate.
    Bridged(Box<BridgedPlugin>),
}

// SAFETY (see module doc): VST3/CLAP plugin access is sequenced by the host;
// no concurrent FFI invocation is possible in Phase 2a.  Raw pointers are only
// dereferenced under that discipline.
unsafe impl Send for NativeHandle {}
unsafe impl Sync for NativeHandle {}

// ── Activation state (ADR-016) ────────────────────────────────────────────────
//
// The plugin activation lifecycle (CLAP `init → activate → start_processing`;
// VST3 `setBusArrangements → activateBus → setActive → setupProcessing →
// setProcessing`) is owned **by the instance**, not the wrapper, so it survives a
// wrapper rebuild when a graph is recompiled and the same live instance is
// reused.  All transitions run on the command thread (graph prepare) or the
// drop-ring worker (teardown) — never the audio thread.

struct ActivationState {
    /// CLAP only: `init` must run exactly once before `activate`.  Unused (and
    /// always `false`) for VST3, whose component is initialised at creation.
    initialized: AtomicBool,
    active: AtomicBool,
    /// `start_processing` has run on the audio thread. CLAP forbids calling it
    /// from the command thread.
    processing: AtomicBool,
    /// Sample rate (`f32::to_bits`) the plugin was activated at; activation is
    /// bound to it, so a config change forces deactivate + reactivate.
    sr_bits: AtomicU32,
    /// Max block size the plugin was activated at.
    block: AtomicU32,
}

impl ActivationState {
    fn new() -> Self {
        ActivationState {
            initialized: AtomicBool::new(false),
            active: AtomicBool::new(false),
            processing: AtomicBool::new(false),
            sr_bits: AtomicU32::new(0),
            block: AtomicU32::new(0),
        }
    }
}

// ── FFI access gate (between-blocks FFI fence — step 0 foundation) ─────────────
//
// VST3/CLAP mandate single-threaded instance access.  The audio thread enters
// the gate around the wrapper's `process()` FFI call; off-RT callers (state
// save/load, GUI) acquire it before touching the same instance.  This is a
// non-blocking mutual exclusion: the RT side never waits — it skips its block
// when an off-RT holder owns the instance — while the off-RT side spins within a
// bounded budget and otherwise gives up (its caller falls back to a cached blob).
// Because the RT side never blocks, there is no priority inversion.
//
// STEP 0: primitive only — no caller yet.  Wired into the wrappers (RT side) and
// the host state paths (off-RT side) in later steps.
const GATE_IDLE: u8 = 0;
const GATE_RT: u8 = 1;
const GATE_OFFRT: u8 = 2;

/// Time an off-RT state harvest spends trying to claim a live instance's FFI gate
/// between two audio blocks before falling back to a cached blob.  Shared by the
/// project-save path and R-4 re-instantiation.  A few ms spans a block period at
/// typical buffer sizes; when stopped the gate is free, so acquisition is immediate.
pub(crate) const FFI_GATE_HARVEST_BUDGET: Duration = Duration::from_millis(3);

/// RAII release for the audio-thread side of the FFI gate.  Dropping it returns
/// the gate to `GATE_IDLE`, guaranteeing release on every exit path (including
/// the SEH-caught and early-return paths in the wrappers).
pub struct RtFfiGuard<'a> {
    gate: &'a AtomicU8,
    _marker: MarkerGuard,
}

impl Drop for RtFfiGuard<'_> {
    fn drop(&mut self) {
        self.gate.store(GATE_IDLE, Ordering::Release);
    }
}

/// RAII release for the off-RT (command / GUI thread) side of the FFI gate.
pub struct OffRtFfiGuard<'a> {
    gate: &'a AtomicU8,
    _marker: MarkerGuard,
}

impl Drop for OffRtFfiGuard<'_> {
    fn drop(&mut self) {
        self.gate.store(GATE_IDLE, Ordering::Release);
    }
}

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
    #[cfg(test)]
    instance_id: u64,

    /// Stable project-side identifier (e.g. `"vst3://<dll>?<cid>"`).  This is the
    /// plugin **type** id — two instances of the same plugin share it, so it is
    /// **not** a unique instance handle (use [`instance_id`](Self::instance_id)).
    pub device_id: String,

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

    /// Plugin activation lifecycle (ADR-016).  Owned here so it survives wrapper
    /// rebuilds on recompile.  One per format that supports reuse; the unused one
    /// for a given handle stays inert.
    clap_activation: ActivationState,
    vst3_activation: ActivationState,

    /// Non-blocking FFI access gate (step 0).  Serialises the audio thread's
    /// `process()` FFI against off-RT FFI (state save/load, GUI) on this same
    /// instance.  `GATE_IDLE` / `GATE_RT` / `GATE_OFFRT`.
    ffi_gate: AtomicU8,

    /// `crash_marker` tag of this plugin while it runs in-process; 0 when
    /// sandboxed (its crash cannot take Echoes down).
    crash_tag: u64,

    /// Native ids currently driven by host modulation (ADR-021).  The plugin
    /// keeps a modulation offset until told otherwise, so a recompile that drops
    /// a route must reset it.  Command thread only.
    modulated_params: Mutex<Vec<u32>>,
    /// Parameters modulated by value because the plugin cannot modulate them
    /// (ADR-021 amendment); their base is restored before a state save.
    /// Command thread only.
    value_modulated_params: Mutex<Vec<ValueModulatedParam>>,
    /// CLAP output ports in graph order (ADR-014 multi-out), read once right
    /// after `init` — before the instance can be in any graph, so the read
    /// never races `process`.
    output_ports: OnceLock<Vec<ClapOutputPort>>,
    /// The plugin's track changed and its `clap.track-info.changed` was not
    /// delivered yet (the plugin was busy in a block).  Command thread only.
    track_info_pending: AtomicBool,
}

/// Some in-process plugin still owes a `clap.track-info.changed`; the app polls
/// [`take_undelivered_track_info`] to retry.
static TRACK_INFO_UNDELIVERED: AtomicBool = AtomicBool::new(false);

/// Returns whether a track-info notification is still owed, and clears it.
pub fn take_undelivered_track_info() -> bool {
    TRACK_INFO_UNDELIVERED.swap(false, Ordering::AcqRel)
}

impl PluginInstance {
    pub fn new_internal(device_id: impl Into<String>) -> Arc<Self> {
        let device_id: String = device_id.into();
        let crash_tag = crash_marker::plugin_tag(&device_id);
        Arc::new(Self {
            #[cfg(test)]
            instance_id: next_instance_id(),
            device_id,
            format: DeviceFormat::Internal,
            handle: NativeHandle::Internal,
            lifecycle: AtomicU8::new(encode_lifecycle(LifecycleState::Running)),
            param_queue: ParamQueue::with_capacity(PARAM_QUEUE_CAPACITY),
            clap_activation: ActivationState::new(),
            vst3_activation: ActivationState::new(),
            ffi_gate: AtomicU8::new(GATE_IDLE),
            crash_tag,
            modulated_params: Mutex::new(Vec::new()),
            value_modulated_params: Mutex::new(Vec::new()),
            output_ports: OnceLock::new(),
            track_info_pending: AtomicBool::new(false),
        })
    }

    /// Construct a VST3 instance from already-acquired COM pointers.
    ///
    /// The `Vst3Library` Arc is moved in; `Drop` on this instance is what
    /// eventually frees it.
    pub fn new_vst3(
        device_id: impl Into<String>,
        component: NonNull<IComponent>,
        processor: NonNull<IAudioProcessor>,
        lib: Arc<Vst3Library>,
    ) -> Arc<Self> {
        let device_id: String = device_id.into();
        let crash_tag = crash_marker::plugin_tag(&device_id);
        Arc::new(Self {
            #[cfg(test)]
            instance_id: next_instance_id(),
            device_id,
            format: DeviceFormat::Vst3,
            handle: NativeHandle::Vst3 {
                component,
                processor,
                _lib: lib,
            },
            lifecycle: AtomicU8::new(encode_lifecycle(LifecycleState::Created)),
            param_queue: ParamQueue::with_capacity(PARAM_QUEUE_CAPACITY),
            clap_activation: ActivationState::new(),
            vst3_activation: ActivationState::new(),
            ffi_gate: AtomicU8::new(GATE_IDLE),
            crash_tag,
            modulated_params: Mutex::new(Vec::new()),
            value_modulated_params: Mutex::new(Vec::new()),
            output_ports: OnceLock::new(),
            track_info_pending: AtomicBool::new(false),
        })
    }

    pub fn new_clap(
        device_id: impl Into<String>,
        plugin: *const ClapPlugin,
        host: Box<ClapHost>,
        lib: Arc<ClapLibrary>,
    ) -> Arc<Self> {
        let device_id: String = device_id.into();
        let crash_tag = crash_marker::plugin_tag(&device_id);
        Arc::new(Self {
            #[cfg(test)]
            instance_id: next_instance_id(),
            device_id,
            format: DeviceFormat::Clap,
            handle: NativeHandle::Clap {
                plugin,
                host,
                _lib: lib,
            },
            lifecycle: AtomicU8::new(encode_lifecycle(LifecycleState::Created)),
            param_queue: ParamQueue::with_capacity(PARAM_QUEUE_CAPACITY),
            clap_activation: ActivationState::new(),
            vst3_activation: ActivationState::new(),
            ffi_gate: AtomicU8::new(GATE_IDLE),
            crash_tag,
            modulated_params: Mutex::new(Vec::new()),
            value_modulated_params: Mutex::new(Vec::new()),
            output_ports: OnceLock::new(),
            track_info_pending: AtomicBool::new(false),
        })
    }

    /// A CLAP plugin running in a sandbox process (ADR-015).
    pub fn new_bridged(device_id: impl Into<String>, bridged: BridgedPlugin) -> Arc<Self> {
        Arc::new(Self {
            #[cfg(test)]
            instance_id: next_instance_id(),
            device_id: device_id.into(),
            format: DeviceFormat::Clap,
            handle: NativeHandle::Bridged(Box::new(bridged)),
            lifecycle: AtomicU8::new(encode_lifecycle(LifecycleState::Created)),
            param_queue: ParamQueue::with_capacity(PARAM_QUEUE_CAPACITY),
            clap_activation: ActivationState::new(),
            vst3_activation: ActivationState::new(),
            ffi_gate: AtomicU8::new(GATE_IDLE),
            crash_tag: 0,
            modulated_params: Mutex::new(Vec::new()),
            value_modulated_params: Mutex::new(Vec::new()),
            output_ports: OnceLock::new(),
            track_info_pending: AtomicBool::new(false),
        })
    }

    /// The sandbox process of a sandboxed instance.
    pub fn bridge(&self) -> Option<&BridgedPlugin> {
        match &self.handle {
            NativeHandle::Bridged(bridged) => Some(bridged),
            _ => None,
        }
    }

    pub fn is_sandboxed(&self) -> bool {
        self.bridge().is_some()
    }

    /// The plugin crashed — caught in-process, or its sandbox was condemned —
    /// and stays silent until a recompile rebuilds it.
    pub fn has_crashed(&self) -> bool {
        self.lifecycle() == LifecycleState::Error
            || self.bridge().is_some_and(BridgedPlugin::is_dead)
    }

    /// Records which parameters are now modulated and returns those that no
    /// longer are, so the caller resets their modulation.  Command thread only.
    pub fn replace_modulated_params(&self, now: &[u32]) -> Vec<u32> {
        let mut modulated = self
            .modulated_params
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let released = modulated
            .iter()
            .copied()
            .filter(|id| !now.contains(id))
            .collect();
        *modulated = now.to_vec();
        released
    }

    /// Records the value-modulated parameters and returns the previous ones.
    /// Command thread only.
    pub fn replace_value_modulated_params(
        &self,
        now: Vec<ValueModulatedParam>,
    ) -> Vec<ValueModulatedParam> {
        let mut params = self
            .value_modulated_params
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::mem::replace(&mut *params, now)
    }

    /// Current plain value of a CLAP parameter, read under the FFI gate.
    /// `None` for other formats, or when a block is in flight past the budget.
    pub fn clap_param_value(&self, native_id: u32) -> Option<f64> {
        if let Some(bridged) = self.bridge() {
            return bridged.param_value(native_id);
        }
        let NativeHandle::Clap { plugin, .. } = &self.handle else {
            return None;
        };
        let _gate = self.offrt_try_acquire(FFI_GATE_HARVEST_BUDGET)?;
        // SAFETY: live CLAP plugin owned by this instance; FFI gate held.
        unsafe { crate::plugin_formats::clap::params::read_clap_param_value(*plugin, native_id) }
    }

    /// The plugin's own text for a plain parameter value (`-6.0 dB`), read under
    /// the FFI gate.  `None` for other formats, when the plugin cannot render
    /// it, or when a block is in flight past the budget.
    pub fn clap_param_text(&self, native_id: u32, value: f64) -> Option<String> {
        if let Some(bridged) = self.bridge() {
            return bridged.param_text(native_id, value);
        }
        let NativeHandle::Clap { plugin, .. } = &self.handle else {
            return None;
        };
        let _gate = self.offrt_try_acquire(FFI_GATE_HARVEST_BUDGET)?;
        // SAFETY: live CLAP plugin owned by this instance; FFI gate held.
        unsafe { crate::plugin_formats::clap::ffi::read_param_text(*plugin, native_id, value) }
    }

    /// The plugin's note names (`clap.note-name`), read under the FFI gate.  `None`
    /// for other formats, or when a block is in flight past the budget.
    pub fn clap_note_names(&self) -> Option<Vec<NoteName>> {
        if let Some(bridged) = self.bridge() {
            return bridged.note_names();
        }
        let NativeHandle::Clap { plugin, .. } = &self.handle else {
            return None;
        };
        let _gate = self.offrt_try_acquire(FFI_GATE_HARVEST_BUDGET)?;
        // SAFETY: live CLAP plugin owned by this instance; FFI gate held.
        Some(unsafe { read_note_names(*plugin) })
    }

    /// Gives a CLAP plugin the info of its track, then tells it
    /// (`clap.track-info.changed`) when it differs from the last one.  Command
    /// thread.  Other formats ignore it.
    pub fn set_track_info(&self, info: &HostTrackInfo) {
        if let Some(bridged) = self.bridge() {
            bridged.set_track_info(info);
            return;
        }
        let Some(context) = self.clap_host_context() else {
            return;
        };
        if context.set_track_info(info) {
            self.track_info_pending.store(true, Ordering::Release);
            self.notify_track_info_changed();
        }
    }

    /// Delivers a pending `clap.track-info.changed` under the FFI gate, off the
    /// audio thread.  A plugin busy in a block keeps it pending for a retry; a
    /// crashed one never hears it.  Command thread.
    pub fn notify_track_info_changed(&self) {
        if !self.track_info_pending.load(Ordering::Acquire) {
            return;
        }
        let Some(plugin) = self.clap_plugin() else {
            return;
        };
        if self.has_crashed() {
            self.track_info_pending.store(false, Ordering::Release);
            return;
        }
        let Some(_gate) = self.offrt_try_acquire(FFI_GATE_HARVEST_BUDGET) else {
            TRACK_INFO_UNDELIVERED.store(true, Ordering::Release);
            return;
        };
        self.track_info_pending.store(false, Ordering::Release);
        // SAFETY: live CLAP plugin owned by this instance; FFI gate held.
        unsafe { notify_track_info_changed(plugin) };
    }

    /// Puts value-modulated parameters back on their base so a state save
    /// captures the user's setting, not a point of the modulation.
    ///
    /// # Safety
    /// `plugin` must be this instance's live CLAP plugin and the caller must hold the FFI gate.
    unsafe fn restore_value_modulated_bases(&self, plugin: *const ClapPlugin) {
        let bases = self.value_modulated_bases();
        if !bases.is_empty() {
            crate::plugin_formats::clap::params::flush_clap_param_values(plugin, &bases);
        }
    }

    fn value_modulated_bases(&self) -> Vec<(u32, f64)> {
        self.value_modulated_params
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|param| (param.native_id, param.base_plain() as f64))
            .collect()
    }

    /// Process-unique, stable identity for this live instance.  Two instances of
    /// the same plugin type (same [`device_id`](Self::device_id)) return
    /// **different** values; a given instance returns the **same** value for its
    /// whole lifetime.  Use this — not `device_id` — whenever a caller must point
    /// at one specific instance rather than a plugin type.
    #[cfg(test)]
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
    // carry their native handle here.  These run on the command thread and are
    // NOT sequenced against the audio thread's `process` on the same instance:
    // the **caller** must hold the FFI gate (`offrt_try_acquire`) around them
    // whenever the instance may be live, so they cannot race `process`.  Callers
    // that omit the gate must guarantee no block is in flight (e.g. a brand-new
    // instance not yet in any snapshot).  Internal devices (and AU/WASM) return
    // `None` / `false`.

    /// Serialise the live plugin's processor state.  `None` when the format has
    /// no FFI state path yet or the plugin reports no state.
    pub fn save_state_blob(&self) -> Option<Vec<u8>> {
        match &self.handle {
            NativeHandle::Vst3 { component, .. } => unsafe {
                crate::plugin_formats::vst3::ffi::read_component_state(component.as_ptr())
            },
            NativeHandle::Clap { plugin, .. } => unsafe {
                self.restore_value_modulated_bases(*plugin);
                crate::plugin_formats::clap::ffi::read_plugin_state(*plugin)
            },
            NativeHandle::Bridged(bridged) => {
                let bases = self.value_modulated_bases();
                if !bases.is_empty() {
                    bridged.flush_values(&bases);
                }
                bridged.save_state()
            }
            NativeHandle::Internal => None,
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
            NativeHandle::Bridged(bridged) => bridged.load_state(bytes),
            NativeHandle::Internal => false,
        }
    }

    /// Carry persisted state from a `previous` instance of the **same plugin
    /// type** into this freshly-created one (R-4 — state must survive a
    /// controlled re-instantiation, not only a project reload).
    ///
    /// The `previous` instance's *live* FFI state is preferred (it reflects edits
    /// made since load); `cached` is the host's last-serialised blob, used as a
    /// fallback for formats with no live FFI state path (internal/AU/WASM).
    /// Returns the bytes actually carried so the host can re-cache them, or `None`
    /// when the types differ or there is nothing to carry — never carries state
    /// across plugin types.
    ///
    /// Runs on the command thread at instantiation, before this instance enters
    /// any snapshot, so applying it to *this* instance cannot race the audio
    /// thread's `process()` (same race-free property as ADR-010 restore).  The
    /// read of `previous`'s live state *can* race — `previous` may still be live
    /// in the outgoing graph — so it is fenced by `previous`'s FFI gate; if a
    /// block is in flight and the gate cannot be claimed in time, we fall back to
    /// the cached blob rather than read concurrently with `process()`.
    pub fn restore_from(
        &self,
        previous: &PluginInstance,
        cached: Option<&[u8]>,
    ) -> Option<Vec<u8>> {
        if self.device_id != previous.device_id {
            return None;
        }
        // A crashed plugin's memory may be corrupt: calling into it again could
        // take Echoes down.  Its last saved state is the safe choice.
        let live = (!previous.has_crashed())
            .then(|| previous.offrt_try_acquire(FFI_GATE_HARVEST_BUDGET))
            .flatten()
            .and_then(|_gate| previous.save_state_blob());
        let bytes = live
            .filter(|b| !b.is_empty())
            .or_else(|| cached.filter(|c| !c.is_empty()).map(<[u8]>::to_vec))?;
        let _ = self.load_state_blob(&bytes);
        Some(bytes)
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

    /// Clone the VST3 library Arc if this instance is VST3.  Used by the editor
    /// to re-obtain the factory for dual-component plugins (audit V4); the DLL
    /// stays mapped as long as the returned Arc lives.
    pub fn vst3_library(&self) -> Option<Arc<Vst3Library>> {
        match &self.handle {
            NativeHandle::Vst3 { _lib, .. } => Some(Arc::clone(_lib)),
            _ => None,
        }
    }

    /// `init` once. Required before `clap.state` or `activate`. Command thread.
    pub fn clap_ensure_initialized(&self) -> bool {
        let _running = self.mark(PluginThread::Control);
        let plugin = match &self.handle {
            NativeHandle::Clap { plugin, .. } => *plugin,
            _ => return false,
        };
        let act = &self.clap_activation;
        if act.initialized.load(Ordering::Acquire) {
            return true;
        }
        unsafe {
            if !((*plugin).init)(plugin) {
                return false;
            }
            let _ = self.output_ports.set(read_output_ports(plugin));
        }
        act.initialized.store(true, Ordering::Release);
        true
    }

    /// CLAP output ports in graph order (main first).  Empty for other formats,
    /// a sandboxed plugin, or a plugin without `clap.audio-ports`: those keep
    /// the single main output.
    pub fn output_ports(&self) -> &[ClapOutputPort] {
        self.output_ports.get().map(Vec::as_slice).unwrap_or(&[])
    }

    /// Idempotently bring the CLAP plugin to the active state
    /// (`init → activate`), owned by the instance so it survives wrapper
    /// rebuilds on recompile (ADR-016 reuse). `start_processing` is deferred to
    /// the audio thread (`clap_start_processing`).
    ///
    /// - Already active at the same `(sample_rate, max_block)` → no-op, unless
    ///   the plugin requested a restart (see [`Self::clap_restart_if_requested`]).
    /// - Already active at a different config → `stop_processing + deactivate`,
    ///   then reactivate (CLAP binds these bounds at `activate`).
    /// - Not active → `init` (once) + `activate`.
    ///
    /// No-op (`false`) for non-CLAP handles.  Runs on the command thread during
    /// graph prepare — never the audio thread.  Returns whether the plugin is
    /// active.
    pub fn clap_ensure_active(&self, sample_rate: f32, max_block: u32) -> bool {
        let _running = self.mark(PluginThread::Control);
        let plugin = match &self.handle {
            NativeHandle::Clap { plugin, .. } => *plugin,
            _ => return false,
        };
        let act = &self.clap_activation;

        if act.active.load(Ordering::Acquire) {
            if act.sr_bits.load(Ordering::Relaxed) == sample_rate.to_bits()
                && act.block.load(Ordering::Relaxed) == max_block
            {
                return self.clap_restart_if_requested(plugin, sample_rate, max_block);
            }
            // Config changed — reactivate with the new bounds, fenced from the
            // outgoing graph's `process` when the gate can be claimed in time.
            let _gate = self.offrt_try_acquire(FFI_GATE_HARVEST_BUDGET);
            self.clap_deactivate(plugin);
            return self.clap_activate(plugin, sample_rate, max_block);
        }

        if !self.clap_ensure_initialized() {
            return false;
        }
        self.clap_activate(plugin, sample_rate, max_block)
    }

    /// Honours the plugin's `request_restart`: an active plugin may only change
    /// its latency (or ports) during `activate`, so it is deactivated and
    /// reactivated, and the wrapper then reads the new latency.  Returns whether
    /// the plugin is active.
    fn clap_restart_if_requested(
        &self,
        plugin: *const ClapPlugin,
        sample_rate: f32,
        max_block: u32,
    ) -> bool {
        let Some(context) = self.clap_host_context() else {
            return true;
        };
        if !context.take_restart_request() {
            return true;
        }
        // The outgoing graph may still be processing this reused instance: while
        // the gate is held its blocks pass through instead.
        let Some(_gate) = self.offrt_try_acquire(FFI_GATE_HARVEST_BUDGET) else {
            context.request_restart();
            return true;
        };
        self.clap_deactivate(plugin);
        self.clap_activate(plugin, sample_rate, max_block)
    }

    fn clap_deactivate(&self, plugin: *const ClapPlugin) {
        let act = &self.clap_activation;
        if act.processing.swap(false, Ordering::AcqRel) {
            clap_stop_processing(plugin);
        }
        unsafe { ((*plugin).deactivate)(plugin) };
        act.active.store(false, Ordering::Release);
    }

    fn clap_activate(&self, plugin: *const ClapPlugin, sample_rate: f32, max_block: u32) -> bool {
        let act = &self.clap_activation;
        let ok = unsafe { ((*plugin).activate)(plugin, sample_rate as f64, 1, max_block) };
        if ok {
            act.sr_bits.store(sample_rate.to_bits(), Ordering::Relaxed);
            act.block.store(max_block, Ordering::Relaxed);
            act.active.store(true, Ordering::Release);
        }
        // This activation already applies whatever a pending restart asked for.
        if let Some(context) = self.clap_host_context() {
            context.take_restart_request();
        }
        ok
    }

    /// `start_processing` once, on the audio thread, before the first `process`.
    pub fn clap_start_processing(&self) {
        let plugin = match &self.handle {
            NativeHandle::Clap { plugin, .. } => *plugin,
            _ => return,
        };
        let act = &self.clap_activation;
        if !act.active.load(Ordering::Acquire) || act.processing.swap(true, Ordering::AcqRel) {
            return;
        }
        unsafe {
            if !((*plugin).start_processing)(plugin) {
                act.processing.store(false, Ordering::Release);
            }
        }
    }

    /// Idempotently bring the VST3 component/processor to the active, processing
    /// state, owned by the instance so it survives wrapper rebuilds on recompile
    /// (ADR-016 reuse).  Mirrors [`clap_ensure_active`]:
    ///
    /// - Already active at the same `(sample_rate, max_block)` → no-op.
    /// - Already active at a different config → `setProcessing(0)` +
    ///   `setActive(0)`, then re-setup.
    /// - Not active → `setBusArrangements` + `activateBus` ×2 + `setActive(1)` +
    ///   `setupProcessing` + `setProcessing(1)`.
    ///
    /// No-op (`false`) for non-VST3 handles.  Runs on the command thread during
    /// graph prepare — never the audio thread.
    pub fn vst3_ensure_active(&self, sample_rate: f32, max_block: u32) -> bool {
        let _running = self.mark(PluginThread::Control);
        let (comp, proc) = match &self.handle {
            NativeHandle::Vst3 {
                component,
                processor,
                ..
            } => (component.as_ptr(), processor.as_ptr()),
            _ => return false,
        };
        let sr_bits = sample_rate.to_bits();
        let act = &self.vst3_activation;

        if act.active.load(Ordering::Acquire) {
            if act.sr_bits.load(Ordering::Relaxed) == sr_bits
                && act.block.load(Ordering::Relaxed) == max_block
            {
                return true;
            }
            unsafe {
                ((*(*proc).vtbl).set_processing)(proc, 0);
                ((*(*comp).vtbl).set_active)(comp, 0);
            }
            act.active.store(false, Ordering::Release);
        }

        unsafe {
            let mut in_arr = SPEAKER_STEREO;
            let mut out_arr = SPEAKER_STEREO;
            ((*(*proc).vtbl).set_bus_arrangements)(proc, &mut in_arr, 1, &mut out_arr, 1);
            // kAudio=0; input bus (dir=0) and output bus (dir=1), index 0, on.
            ((*(*comp).vtbl).activate_bus)(comp, 0, 0, 0, 1);
            ((*(*comp).vtbl).activate_bus)(comp, 0, 1, 0, 1);
            ((*(*comp).vtbl).set_active)(comp, 1);
            let setup = ProcessSetup {
                process_mode: 0,
                symbolic_sample_size: 0,
                max_samples_per_block: max_block as i32,
                sample_rate: sample_rate as f64,
            };
            ((*(*proc).vtbl).setup_processing)(proc, &setup);
            ((*(*proc).vtbl).set_processing)(proc, 1);
        }
        act.sr_bits.store(sr_bits, Ordering::Relaxed);
        act.block.store(max_block, Ordering::Relaxed);
        act.active.store(true, Ordering::Release);
        true
    }

    /// Convenience: extract the CLAP plugin pointer if this instance is CLAP.
    pub fn clap_plugin(&self) -> Option<*const ClapPlugin> {
        match &self.handle {
            NativeHandle::Clap { plugin, .. } => Some(*plugin),
            _ => None,
        }
    }

    /// Host state the CLAP plugin's callbacks see (editor window, drain flag).
    pub fn clap_host_context(&self) -> Option<&ClapHostContext> {
        match &self.handle {
            NativeHandle::Clap { host, .. } => host.context(),
            _ => None,
        }
    }

    /// Records that `thread` is about to run this plugin's code, until the
    /// guard drops, so a crash can be attributed at the next start.
    pub fn mark(&self, thread: PluginThread) -> MarkerGuard {
        crash_marker::enter(thread, self.crash_tag)
    }

    pub fn set_lifecycle(&self, state: LifecycleState) {
        self.lifecycle
            .store(encode_lifecycle(state), Ordering::Release);
    }

    pub fn lifecycle(&self) -> LifecycleState {
        decode_lifecycle(self.lifecycle.load(Ordering::Acquire))
    }

    /// Audio-thread side of the FFI gate (step 0).  Non-blocking: returns
    /// `Some(guard)` when the instance is idle (the common case), `None` when an
    /// off-RT caller currently holds it — in which case the wrapper must skip its
    /// FFI for this block.  Never waits, so it can never invert priority.
    pub fn rt_try_enter(&self) -> Option<RtFfiGuard<'_>> {
        match self.ffi_gate.compare_exchange(
            GATE_IDLE,
            GATE_RT,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => Some(RtFfiGuard {
                gate: &self.ffi_gate,
                _marker: self.mark(PluginThread::Audio),
            }),
            Err(_) => None,
        }
    }

    /// Off-RT side of the FFI gate (step 0; command / GUI thread).  Spins up to
    /// `budget` to claim exclusive FFI access between two audio blocks; returns
    /// `None` on timeout so the caller can fall back to a cached blob.  Only the
    /// off-RT side ever waits — the audio thread is never made to block.
    pub fn offrt_try_acquire(&self, budget: Duration) -> Option<OffRtFfiGuard<'_>> {
        let deadline = Instant::now() + budget;
        loop {
            if self
                .ffi_gate
                .compare_exchange(GATE_IDLE, GATE_OFFRT, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Some(OffRtFfiGuard {
                    gate: &self.ffi_gate,
                    _marker: self.mark(PluginThread::Control),
                });
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::hint::spin_loop();
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
        let _running = self.mark(PluginThread::Control);
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
                    // Only stop/deactivate if the plugin was activated: calling
                    // setProcessing(0)/setActive(0) on an unactivated component
                    // violates the VST3 spec and crashes strict implementations.
                    if self.vst3_activation.active.load(Ordering::Acquire) {
                        ((*(*proc).vtbl).set_processing)(proc, 0);
                        ((*(*comp).vtbl).set_active)(comp, 0);
                    }
                    ((*(*comp).vtbl).terminate)(comp);
                    // IAudioProcessor and IComponent may be the same COM object;
                    // release each independently.
                    ((*(*proc).vtbl).release)(proc as *mut _);
                    ((*(*comp).vtbl).release)(comp as *mut _);
                }
            }

            // The bridge process owns the plugin; dropping the handle stops it.
            NativeHandle::Bridged(_) => {}

            NativeHandle::Clap { plugin, .. } => {
                let p = *plugin;
                unsafe {
                    // The activation lifecycle is owned here (ADR-016): the
                    // wrappers no longer stop/deactivate in their own `Drop`,
                    // so the instance does it once, before `destroy`.
                    if self.clap_activation.active.load(Ordering::Acquire) {
                        if self.clap_activation.processing.load(Ordering::Acquire) {
                            clap_stop_processing(p);
                        }
                        ((*p).deactivate)(p);
                    }
                    ((*p).destroy)(p);
                }
            }
        }
    }
}

/// `stop_processing` belongs to the audio thread. Callers run it off the audio
/// callback only when no `process` can run concurrently (last reference
/// dropped, or FFI gate held), so this thread may take the audio role for it.
fn clap_stop_processing(plugin: *const ClapPlugin) {
    let _audio_thread = audio_thread_scope();
    unsafe { ((*plugin).stop_processing)(plugin) };
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
    fn ffi_gate_offrt_holder_blocks_rt_until_released() {
        let inst = PluginInstance::new_internal("echoes.gain");
        let held = inst
            .offrt_try_acquire(Duration::from_millis(1))
            .expect("idle gate must be acquirable");
        // While off-RT holds the gate, the audio thread must be denied (it skips).
        assert!(
            inst.rt_try_enter().is_none(),
            "RT must not enter while an off-RT caller holds the instance"
        );
        drop(held);
        // Once released, the audio thread acquires immediately.
        assert!(
            inst.rt_try_enter().is_some(),
            "RT must acquire once the gate is released"
        );
    }

    #[test]
    fn ffi_gate_rt_holder_makes_offrt_time_out() {
        let inst = PluginInstance::new_internal("echoes.gain");
        let held = inst.rt_try_enter().expect("idle gate must be acquirable");
        // The audio thread holds it: off-RT must give up within its budget so its
        // caller can fall back to a cached blob (never the other way around).
        assert!(
            inst.offrt_try_acquire(Duration::from_millis(2)).is_none(),
            "off-RT must time out while the audio thread holds the gate"
        );
        drop(held);
        assert!(
            inst.offrt_try_acquire(Duration::from_millis(2)).is_some(),
            "off-RT must acquire once the audio thread releases"
        );
    }

    #[test]
    fn ffi_gate_never_grants_concurrent_access_under_contention() {
        use std::sync::atomic::AtomicUsize;
        use std::thread;

        let inst = PluginInstance::new_internal("echoes.gain");
        // Counts threads currently inside the gate; must never exceed 1.
        let inside = Arc::new(AtomicUsize::new(0));
        let violations = Arc::new(AtomicUsize::new(0));

        let inst_off = Arc::clone(&inst);
        let inside_off = Arc::clone(&inside);
        let viol_off = Arc::clone(&violations);
        let off = thread::spawn(move || {
            for _ in 0..200_000 {
                if let Some(_g) = inst_off.offrt_try_acquire(Duration::from_millis(5)) {
                    if inside_off.fetch_add(1, Ordering::AcqRel) != 0 {
                        viol_off.fetch_add(1, Ordering::Relaxed);
                    }
                    inside_off.fetch_sub(1, Ordering::AcqRel);
                }
            }
        });

        // RT-like hammering: never blocks, skips on contention.
        for _ in 0..200_000 {
            if let Some(_g) = inst.rt_try_enter() {
                if inside.fetch_add(1, Ordering::AcqRel) != 0 {
                    violations.fetch_add(1, Ordering::Relaxed);
                }
                inside.fetch_sub(1, Ordering::AcqRel);
            }
        }
        off.join().unwrap();

        assert_eq!(
            violations.load(Ordering::Relaxed),
            0,
            "RT and off-RT were inside the FFI gate at the same time"
        );
    }

    #[test]
    fn param_queue_push_then_drain_roundtrips() {
        // An internal instance carries the same SPSC ring as any plugin.
        let inst = PluginInstance::new_internal("dev://x");
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
        let a = PluginInstance::new_internal("echoes.gain");
        let b = PluginInstance::new_internal("echoes.gain");

        assert_eq!(
            a.device_id, b.device_id,
            "same plugin type → same device_id"
        );
        assert_ne!(
            a.instance_id(),
            b.instance_id(),
            "same device_id must NOT collapse the instance identity"
        );
    }

    #[test]
    fn restore_from_carries_cached_state_for_the_same_type() {
        // A controlled replacement: a fresh instance pulls the previous
        // instance's state. Internal devices have no live FFI state, so the
        // host-cached blob is what gets carried.
        let previous = PluginInstance::new_internal("echoes.gain");
        let next = PluginInstance::new_internal("echoes.gain");
        let carried = next.restore_from(&previous, Some(&[7u8, 8, 9]));
        assert_eq!(
            carried,
            Some(vec![7, 8, 9]),
            "same-type re-instantiation must carry the previous state forward"
        );
    }

    #[test]
    fn restore_from_refuses_across_plugin_types() {
        let previous = PluginInstance::new_internal("echoes.gain");
        let next = PluginInstance::new_internal("echoes.pan");
        assert_eq!(
            next.restore_from(&previous, Some(&[1u8, 2, 3])),
            None,
            "state must never cross plugin types"
        );
    }

    #[test]
    fn restore_from_returns_none_with_nothing_to_carry() {
        let previous = PluginInstance::new_internal("echoes.gain");
        let next = PluginInstance::new_internal("echoes.gain");
        assert_eq!(next.restore_from(&previous, None), None);
        assert_eq!(next.restore_from(&previous, Some(&[])), None);
    }

    #[test]
    fn instance_id_is_stable_for_an_instances_lifetime() {
        let inst = PluginInstance::new_internal("echoes.gain");
        let first = inst.instance_id();
        inst.set_lifecycle(LifecycleState::Suspended);
        assert_eq!(inst.instance_id(), first);
        assert_eq!(inst.instance_id(), first);
    }
}
