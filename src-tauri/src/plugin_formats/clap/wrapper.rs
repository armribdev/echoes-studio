//! CLAP wrapper — adapts a live ClapPlugin into an AudioNode.
//!
//! ## Real-time contract
//!
//! All allocations (Vec, Box) happen in `new` and `prepare`.  `process` only
//! updates raw pointers and calls `ClapPlugin::process` — no heap ops, no locks.
//!
//! ## Shared instance (ADR-007, Phase 2a)
//!
//! The CLAP plugin pointer and host struct no longer live in this wrapper;
//! they are owned by a shared [`PluginInstance`] behind an `Arc`.  The wrapper
//! holds a clone captured at construction; `process` reaches the plugin via
//! `instance.clap_plugin()` — a non-atomic load through `Arc::deref`.
//!
//! ## Unified lifecycle (Phase 2b)
//!
//! The local `LifecycleState` field is gone.  All queries / mutations route
//! through `instance.lifecycle()` / `instance.set_lifecycle()`; mutations
//! validate via [`transition`] against the current atomic value.
//!
//! ## Thread safety
//!
//! `ClapWrapperNode` is `Send` because:
//! - CLAP plugin instances are required by spec to be single-threaded.  The
//!   audio-graph scheduler enforces per-step exclusive ownership, and the
//!   host only touches the instance between blocks.
//! - The shared `Arc<PluginInstance>` is `Send + Sync` (see `instance.rs`
//!   safety claim).

use std::ffi::c_void;
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::audio_graph::plugin_params::{parse_plugin_param_name, PluginParamControl};
use crate::plugin_formats::clap::params::{read_clap_params, ClapParamEvents};
use crate::plugin_formats::clap::transport::clap_transport_event;
use crate::plugin_formats::crash_guard;
use crate::plugin_system::capabilities::DeviceCapabilities;
use crate::plugin_system::instance::{PluginInstance, FFI_GATE_HARVEST_BUDGET};
use crate::plugin_system::lifecycle::{transition, LifecycleState};
use crate::plugin_system::parameter::ParameterMap;
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{Device, DeviceInfo};

use super::ffi::{
    ClapAudioBuffer, ClapOutputEvents, ClapPlugin, ClapPluginLatency, ClapProcess,
    CLAP_EXT_LATENCY, CLAP_PROCESS_ERROR,
};

// Stack context passed to the SEH trampoline — no heap allocation.
struct ClapProcessCtx {
    plugin: *const ClapPlugin,
    process: *const ClapProcess,
}

unsafe extern "C" fn clap_process_trampoline(raw: *mut c_void) -> i32 {
    let ctx = &*(raw as *const ClapProcessCtx);
    ((*ctx.plugin).process)(ctx.plugin, ctx.process)
}

// Plugins can push output events; we discard them until Phase 4 MIDI output.
unsafe extern "C" fn out_try_push(
    _: *const ClapOutputEvents,
    _: *const super::ffi::ClapEventHeader,
) -> bool {
    true
}

pub struct ClapWrapperNode {
    node_id: NodeId,
    info: DeviceInfo,
    params: ParameterMap,

    /// Shared, refcounted handle to the live plugin.  Captured at
    /// construction; cloned only on the command thread.  Deref on the audio
    /// thread is a non-atomic field load (cf. `ADR-007`).
    /// Phase 2b: also holds the authoritative `lifecycle` atomic.
    instance: Arc<PluginInstance>,

    // Per-block scratch — pre-allocated in prepare, stable after that
    in_channel_ptrs: Vec<*mut f32>,
    out_channel_ptrs: Vec<*mut f32>,
    in_audio_buf: ClapAudioBuffer,
    out_audio_buf: ClapAudioBuffer,

    /// Parameter ranges, controlled parameters and the per-block input event
    /// list (ADR-007, ADR-021).  No allocation in `process`.
    param_events: ClapParamEvents,
    out_events: ClapOutputEvents,

    num_channels: usize,
    active: bool,

    /// Set to `true` after the first SEH exception from the plugin's
    /// `process()`.  All subsequent calls return immediately with silence,
    /// keeping the audio thread safe until the graph is next recompiled.
    crashed: bool,

    /// Cached intrinsic latency (samples).  Read once at `prepare()` via the
    /// `clap.latency` extension; queried by the graph compiler off the audio
    /// thread through `latency_samples()`.
    reported_latency: u32,

}

// SAFETY: see module-level doc.  CLAP pointers are reached via Arc-shared
// `PluginInstance`, which bears `Send + Sync` under the same single-threaded
// access discipline mandated by the CLAP spec.
unsafe impl Send for ClapWrapperNode {}

impl ClapWrapperNode {
    /// Construct from a shared [`PluginInstance`].  Caller must guarantee the
    /// instance was built with a CLAP native handle (`PluginInstance::new_clap`).
    pub fn new(node_id: NodeId, instance: Arc<PluginInstance>) -> Self {
        debug_assert!(
            instance.clap_plugin().is_some(),
            "ClapWrapperNode requires a CLAP PluginInstance"
        );
        let info = instance.legacy_device_info();
        Self {
            node_id,
            info,
            params: ParameterMap::new(),
            instance,
            in_channel_ptrs: Vec::new(),
            out_channel_ptrs: Vec::new(),
            in_audio_buf: unsafe { std::mem::zeroed() },
            out_audio_buf: unsafe { std::mem::zeroed() },
            param_events: ClapParamEvents::new(),
            out_events: ClapOutputEvents {
                ctx: std::ptr::null_mut(),
                try_push: out_try_push,
            },
            num_channels: 2,
            active: false,
            crashed: false,
            reported_latency: 0,
        }
    }

    /// CLAP plugin-pointer helper.  Panics if the shared instance was built
    /// for a different format — wired in `PluginHost::instantiate_clap` and
    /// guarded by the constructor `debug_assert!`.
    #[inline(always)]
    fn plugin_ptr(&self) -> *const ClapPlugin {
        self.instance
            .clap_plugin()
            .expect("ClapWrapperNode missing CLAP plugin pointer (programmer error)")
    }
}

impl AudioNode for ClapWrapperNode {
    fn node_id(&self) -> NodeId {
        self.node_id
    }
    fn label(&self) -> &str {
        "ClapWrapperNode"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.num_channels = ctx.num_channels;
        self.in_channel_ptrs = vec![std::ptr::null_mut(); ctx.num_channels];
        self.out_channel_ptrs = vec![std::ptr::null_mut(); ctx.num_channels];

        // Activation lifecycle is owned by the shared instance (ADR-016) so it
        // survives a wrapper rebuild when the graph is recompiled and the live
        // instance is reused.  Idempotent: an already-active instance is left
        // untouched (the CLAP spec forbids a second `activate`).
        self.active = self
            .instance
            .clap_ensure_active(ctx.sample_rate, ctx.max_block_size as u32);

        // On a recompile that reuses a live instance (ADR-016), this introspection
        // (`[main-thread]` getters) runs while the outgoing graph may still
        // `process()` it.  Fence the whole block with the instance FFI gate so it
        // can't race `process()`; if a block is in flight and the gate can't be
        // claimed in time, skip introspection this compile (latency/ranges keep
        // their current values — a transient corrected on a later recompile).
        if self.active {
            if let Some(_gate) = self.instance.offrt_try_acquire(FFI_GATE_HARVEST_BUDGET) {
                let p = self.plugin_ptr();
                unsafe {
                    // `clap.latency` is valid only after activate (main thread / prepare).
                    let ext_ptr = ((*p).get_extension)(p, CLAP_EXT_LATENCY.as_ptr() as *const _);
                    if !ext_ptr.is_null() {
                        let ext = ext_ptr as *const ClapPluginLatency;
                        self.reported_latency = ((*ext).get)(p);
                    }

                    self.param_events.set_ranges(&read_clap_params(p));
                }
            }
        }

        self.instance.set_lifecycle(LifecycleState::Running);
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        if self.crashed || outputs.is_empty() || !self.active {
            return;
        }

        // FFI access gate (step 1): claim exclusive access to the shared instance
        // for this block.  If an off-RT caller (state save/load, GUI) currently
        // holds it, skip the plugin FFI and bypass the audio so the two never run
        // concurrently — the CLAP single-threaded contract.  Non-blocking: the
        // audio thread never waits.  Held until the end of `process` so it is
        // released on every exit path, including the SEH-caught path below.
        // Until an off-RT caller exists (step 2+) this always succeeds, so the
        // bypass branch is unreachable and behaviour is unchanged.
        let _ffi_gate = match self.instance.rt_try_enter() {
            Some(guard) => guard,
            None => {
                if !inputs.is_empty() {
                    outputs[0].copy_from(inputs[0]);
                }
                return;
            }
        };

        let nc = self.num_channels;
        let nf = transport.block_size; // real block, not MAX_BLOCK_FRAMES capacity

        self.param_events.fill(&self.instance);
        self.param_events.buffer.sort();

        for c in 0..nc {
            self.in_channel_ptrs[c] = if !inputs.is_empty() {
                // CLAP spec: hosts may pass const data as the input; safe cast.
                inputs[0].channel(c).as_ptr() as *mut f32
            } else {
                std::ptr::null_mut()
            };
            self.out_channel_ptrs[c] = outputs[0].channel_mut(c).as_mut_ptr();
        }

        self.in_audio_buf = ClapAudioBuffer {
            data32: self.in_channel_ptrs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: nc as u32,
            latency: 0,
            constant_mask: 0,
        };
        self.out_audio_buf = ClapAudioBuffer {
            data32: self.out_channel_ptrs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: nc as u32,
            latency: 0,
            constant_mask: 0,
        };

        let transport_event = clap_transport_event(transport);
        let process = ClapProcess {
            steady_time: -1,
            frames_count: nf as u32,
            transport: &transport_event,
            audio_inputs: &self.in_audio_buf,
            audio_outputs: &mut self.out_audio_buf,
            audio_inputs_count: if inputs.is_empty() { 0 } else { 1 },
            audio_outputs_count: 1,
            in_events: &self.param_events.buffer.input_events_vtable,
            out_events: &self.out_events,
        };

        let p = self.plugin_ptr();
        let mut ctx = ClapProcessCtx { plugin: p, process: &process };
        let status = unsafe {
            crash_guard::guarded(
                clap_process_trampoline,
                &mut ctx as *mut ClapProcessCtx as *mut c_void,
            )
        };

        if status == crash_guard::CAUGHT {
            // SEH exception from the plugin: mute permanently until next recompile.
            self.crashed = true;
            // Mark the shared instance as Error so reuse_existing won't hand the
            // corrupted handle to the next graph compile; forces a fresh FFI instantiation.
            self.instance.set_lifecycle(LifecycleState::Error);
            return;
        }
        if status == CLAP_PROCESS_ERROR && !inputs.is_empty() {
            outputs[0].copy_from(inputs[0]);
        }
    }

    fn reset(&mut self) {
        if self.active {
            let p = self.plugin_ptr();
            unsafe {
                ((*p).reset)(p);
            }
        }
    }

    fn param_id(&self, name: &str) -> Option<u32> {
        parse_plugin_param_name(name)
    }

    fn reset_modulation(&mut self) {
        self.param_events.controls_mut().clear_block();
    }

    fn apply_modulation(&mut self, param_id: u32, delta: f32) {
        self.param_events.controls_mut().modulate(param_id, delta);
    }

    fn apply_modulation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        self.param_events.controls_mut().modulate_at(param_id, sample_offset, value);
    }

    fn apply_automation(&mut self, param_id: u32, value: f32) {
        self.param_events.controls_mut().automate(param_id, value);
    }

    fn apply_automation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        self.param_events.controls_mut().automate_at(param_id, sample_offset, value);
    }

    fn inject_plugin_param_controls(&mut self, controls: Vec<PluginParamControl>) {
        self.param_events.set_controls(controls, &self.instance);
    }

    /// Plain field read; populated once in `prepare()` via the `clap.latency`
    /// extension.  Called by the graph compiler — never on the audio thread.
    fn latency_samples(&self) -> u32 {
        self.reported_latency
    }
}

// NOTE: no `Drop` impl here (ADR-016).  The CLAP `stop_processing → deactivate
// → destroy` sequence is owned by `PluginInstance::Drop`, which runs once when
// the **last** `Arc<PluginInstance>` is released.  A wrapper that is displaced by
// a recompile must NOT deactivate the plugin, because the new graph may share
// (reuse) the same live instance — deactivating it would corrupt the instance
// the new graph is still processing.

impl Device for ClapWrapperNode {
    fn device_id(&self) -> &str {
        &self.info.device_id
    }
    fn info(&self) -> &DeviceInfo {
        &self.info
    }
    fn parameters(&self) -> &ParameterMap {
        &self.params
    }
    fn parameters_mut(&mut self) -> &mut ParameterMap {
        &mut self.params
    }

    fn save_state(&self) -> DeviceState {
        DeviceState::empty(&self.info.device_id)
    }
    fn load_state(&mut self, _: &DeviceState) {}

    // Invalid transitions are silently ignored at this layer — the host-side
    // API surfaces errors to callers.  No FFI side effect in Phase 2b: CLAP
    // suspend/resume would require start_processing/stop_processing toggles,
    // which Phase 4 will wire properly once the host owns the lifecycle.

    fn lifecycle_state(&self) -> LifecycleState {
        self.instance.lifecycle()
    }

    fn suspend(&mut self) {
        if let Ok(next) = transition(self.instance.lifecycle(), LifecycleState::Suspended) {
            self.instance.set_lifecycle(next);
        }
    }
    fn resume(&mut self) {
        if let Ok(next) = transition(self.instance.lifecycle(), LifecycleState::Running) {
            self.instance.set_lifecycle(next);
        }
    }
    fn destroy(&mut self) {
        if let Ok(next) = transition(self.instance.lifecycle(), LifecycleState::Destroyed) {
            self.instance.set_lifecycle(next);
        }
    }

    fn reported_latency(&self) -> u32 {
        self.reported_latency
    }

    /// Phase 1: legacy shape (stereo, no params) plus the freshly-introspected
    /// latency.  Phase 2 replaces this with full audio-port / param introspection.
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::from_legacy_info(&self.info);
        caps.reported_latency_samples = self.reported_latency;
        caps
    }
}
