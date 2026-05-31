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
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::plugin_formats::clap::ffi::{ClapHost, ClapLibrary, ClapPlugin};
use crate::plugin_formats::vst3::ffi::{IAudioProcessor, IComponent, Vst3Library};
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::wrapper::{DeviceFormat, DeviceInfo};

// ── Native handle ─────────────────────────────────────────────────────────────

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
        _lib:      Arc<Vst3Library>,
    },

    /// Live CLAP plugin instance.  `_host` is heap-stable for the lifetime
    /// of `plugin` (CLAP spec: the host pointer passed to `create_plugin`
    /// must outlive the plugin).
    Clap {
        plugin: *const ClapPlugin,
        _host:  Box<ClapHost>,
        _lib:   Arc<ClapLibrary>,
    },
}

// SAFETY (see module doc): VST3/CLAP plugin access is sequenced by the host;
// no concurrent FFI invocation is possible in Phase 2a.  Raw pointers are only
// dereferenced under that discipline.
unsafe impl Send for NativeHandle {}
unsafe impl Sync for NativeHandle {}

// ── PluginInstance ────────────────────────────────────────────────────────────

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
    /// Stable project-side identifier (e.g. `"vst3://<dll>?<cid>"`).
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
}

impl PluginInstance {
    /// Construct an internal (PassThrough) instance with no FFI handle.
    pub fn new_internal(
        device_id: impl Into<String>,
        name:      impl Into<String>,
    ) -> Arc<Self> {
        Arc::new(PluginInstance {
            device_id: device_id.into(),
            name:      name.into(),
            vendor:    "Internal".to_string(),
            format:    DeviceFormat::Internal,
            handle:    NativeHandle::Internal,
            lifecycle: AtomicU8::new(encode_lifecycle(LifecycleState::Running)),
        })
    }

    /// Construct a VST3 instance from already-acquired COM pointers.
    ///
    /// The `Vst3Library` Arc is moved in; `Drop` on this instance is what
    /// eventually frees it.
    pub fn new_vst3(
        device_id: impl Into<String>,
        name:      impl Into<String>,
        component: NonNull<IComponent>,
        processor: NonNull<IAudioProcessor>,
        lib:       Arc<Vst3Library>,
    ) -> Arc<Self> {
        Arc::new(PluginInstance {
            device_id: device_id.into(),
            name:      name.into(),
            vendor:    String::new(),
            format:    DeviceFormat::Vst3,
            handle:    NativeHandle::Vst3 { component, processor, _lib: lib },
            lifecycle: AtomicU8::new(encode_lifecycle(LifecycleState::Created)),
        })
    }

    /// Construct a CLAP instance from an already-created plugin pointer.
    pub fn new_clap(
        device_id: impl Into<String>,
        name:      impl Into<String>,
        vendor:    impl Into<String>,
        plugin:    *const ClapPlugin,
        host:      Box<ClapHost>,
        lib:       Arc<ClapLibrary>,
    ) -> Arc<Self> {
        Arc::new(PluginInstance {
            device_id: device_id.into(),
            name:      name.into(),
            vendor:    vendor.into(),
            format:    DeviceFormat::Clap,
            handle:    NativeHandle::Clap { plugin, _host: host, _lib: lib },
            lifecycle: AtomicU8::new(encode_lifecycle(LifecycleState::Created)),
        })
    }

    // ── Handle access (used by wrappers / future GUI editor) ──────────────────

    /// Borrow the native handle for FFI dispatch.  Wrappers pattern-match
    /// the variant they expect; other variants indicate a routing bug at
    /// instantiation time.
    pub fn handle(&self) -> &NativeHandle {
        &self.handle
    }

    /// Convenience: extract VST3 handles if this instance is VST3.
    pub fn vst3_handles(&self) -> Option<(NonNull<IComponent>, NonNull<IAudioProcessor>)> {
        match &self.handle {
            NativeHandle::Vst3 { component, processor, .. } => Some((*component, *processor)),
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

    // ── Lifecycle (atomic read/write; the wrapper still owns its own copy
    //    until Phase 2b unifies the source of truth) ──────────────────────────

    pub fn set_lifecycle(&self, state: LifecycleState) {
        self.lifecycle.store(encode_lifecycle(state), Ordering::Release);
    }

    pub fn lifecycle(&self) -> LifecycleState {
        decode_lifecycle(self.lifecycle.load(Ordering::Acquire))
    }

    /// Build a `DeviceInfo` matching the legacy wrapper shape for this
    /// instance — used while Phase 2 still relies on the per-wrapper
    /// `DeviceInfo`.  Caller may override defaults (e.g. channel counts).
    pub fn legacy_device_info(&self) -> DeviceInfo {
        DeviceInfo {
            device_id:       self.device_id.clone(),
            name:            self.name.clone(),
            vendor:          self.vendor.clone(),
            version:         String::new(),
            format:          self.format,
            num_inputs:      2,
            num_outputs:     2,
            has_midi_input:  false,
            has_midi_output: false,
        }
    }
}

// ── Teardown ──────────────────────────────────────────────────────────────────
//
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

            NativeHandle::Vst3 { component, processor, .. } => {
                let proc = processor.as_ptr();
                let comp = component.as_ptr();
                unsafe {
                    // Deactivate and terminate before releasing.
                    ((*(*proc).vtbl).set_processing)(proc, 0);
                    ((*(*comp).vtbl).set_active)(comp, 0);
                    ((*(*comp).vtbl).terminate)(comp);
                    // Release IAudioProcessor ref (often the same object as
                    // IComponent — release decrements its independent ref).
                    ((*(*proc).vtbl).release)(proc as *mut _);
                    // Release IComponent ref.
                    ((*(*comp).vtbl).release)(comp as *mut _);
                }
                // _lib Arc drops next → FreeLibrary when last consumer is gone.
            }

            NativeHandle::Clap { plugin, .. } => {
                let p = *plugin;
                unsafe {
                    // The wrapper is responsible for calling stop_processing /
                    // deactivate at suspend time; here we only need to destroy
                    // the instance.  Calling stop_processing on an inactive
                    // plugin would violate the CLAP state machine.
                    ((*p).destroy)(p);
                }
                // _host Box drops → free the host struct.
                // _lib  Arc drops → FreeLibrary when last consumer is gone.
            }
        }
    }
}

// ── Lifecycle encoding ────────────────────────────────────────────────────────

const LC_CREATED:     u8 = 0;
const LC_LOADED:      u8 = 1;
const LC_INITIALIZED: u8 = 2;
const LC_RUNNING:     u8 = 3;
const LC_SUSPENDED:   u8 = 4;
const LC_DESTROYED:   u8 = 5;
const LC_ERROR:       u8 = 6;

fn encode_lifecycle(state: LifecycleState) -> u8 {
    match state {
        LifecycleState::Created     => LC_CREATED,
        LifecycleState::Loaded      => LC_LOADED,
        LifecycleState::Initialized => LC_INITIALIZED,
        LifecycleState::Running     => LC_RUNNING,
        LifecycleState::Suspended   => LC_SUSPENDED,
        LifecycleState::Destroyed   => LC_DESTROYED,
        LifecycleState::Error       => LC_ERROR,
    }
}

fn decode_lifecycle(code: u8) -> LifecycleState {
    match code {
        LC_LOADED      => LifecycleState::Loaded,
        LC_INITIALIZED => LifecycleState::Initialized,
        LC_RUNNING     => LifecycleState::Running,
        LC_SUSPENDED   => LifecycleState::Suspended,
        LC_DESTROYED   => LifecycleState::Destroyed,
        LC_ERROR       => LifecycleState::Error,
        _              => LifecycleState::Created,
    }
}
