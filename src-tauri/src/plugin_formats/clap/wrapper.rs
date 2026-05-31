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
//! ## Thread safety
//!
//! `ClapWrapperNode` is `Send` because:
//! - CLAP plugin instances are required by spec to be single-threaded.  The
//!   audio-graph scheduler enforces per-step exclusive ownership, and the
//!   host only touches the instance between blocks.
//! - The shared `Arc<PluginInstance>` is `Send + Sync` (see `instance.rs`
//!   safety claim).

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::plugin_system::capabilities::DeviceCapabilities;
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::parameter::ParameterMap;
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{Device, DeviceInfo};

use super::ffi::{
    ClapAudioBuffer, ClapOutputEvents, ClapPlugin, ClapPluginLatency, ClapProcess,
    FixedEventList, CLAP_EXT_LATENCY, CLAP_PROCESS_ERROR,
};

// ── No-op output events (plugins can push events out; we ignore them for now) ──
unsafe extern "C" fn out_try_push(_: *const ClapOutputEvents, _: *const super::ffi::ClapEventHeader) -> bool { true }

// ── ClapWrapperNode ────────────────────────────────────────────────────────────

pub struct ClapWrapperNode {
    node_id: NodeId,
    info:    DeviceInfo,
    params:  ParameterMap,

    /// Local lifecycle copy.  Phase 2a keeps this duplicated alongside the
    /// authoritative `instance.lifecycle` atomic; Phase 2b will retire it.
    lifecycle: LifecycleState,

    /// Shared, refcounted handle to the live plugin.  Captured at
    /// construction; cloned only on the command thread.  Deref on the audio
    /// thread is a non-atomic field load (cf. `ADR-007`).
    instance: Arc<PluginInstance>,

    // Per-block scratch — pre-allocated in prepare, stable after that
    in_channel_ptrs:  Vec<*mut f32>,
    out_channel_ptrs: Vec<*mut f32>,
    in_audio_buf:  ClapAudioBuffer,
    out_audio_buf: ClapAudioBuffer,

    // Fixed-size event list (no alloc in process)
    in_events:  Box<FixedEventList>,
    out_events: ClapOutputEvents,

    num_channels: usize,
    active: bool,

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
        ClapWrapperNode {
            node_id,
            info,
            params:    ParameterMap::new(),
            lifecycle: LifecycleState::Created,
            instance,
            in_channel_ptrs:  Vec::new(),
            out_channel_ptrs: Vec::new(),
            in_audio_buf:  unsafe { std::mem::zeroed() },
            out_audio_buf: unsafe { std::mem::zeroed() },
            in_events:  FixedEventList::new(),
            out_events: ClapOutputEvents {
                ctx: std::ptr::null_mut(),
                try_push: out_try_push,
            },
            num_channels: 2,
            active: false,
            reported_latency: 0,
        }
    }

    /// CLAP plugin-pointer helper.  Panics if the shared instance was built
    /// for a different format — wired in `PluginHost::instantiate_clap` and
    /// guarded by the constructor `debug_assert!`.
    #[inline(always)]
    fn plugin_ptr(&self) -> *const ClapPlugin {
        // Non-atomic pointer-chase through Arc::deref.
        self.instance
            .clap_plugin()
            .expect("ClapWrapperNode missing CLAP plugin pointer (programmer error)")
    }
}

impl AudioNode for ClapWrapperNode {
    fn node_id(&self) -> NodeId { self.node_id }
    fn label(&self) -> &str { "ClapWrapperNode" }
    fn num_input_ports(&self)  -> usize { 1 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.num_channels = ctx.num_channels;
        self.in_channel_ptrs  = vec![std::ptr::null_mut(); ctx.num_channels];
        self.out_channel_ptrs = vec![std::ptr::null_mut(); ctx.num_channels];

        let p = self.plugin_ptr();
        unsafe {
            // Initialize the plugin instance if not already done
            ((*p).init)(p);

            // Activate (sets sample rate + block size bounds)
            let ok = ((*p).activate)(p, ctx.sample_rate as f64, 1, ctx.max_block_size as u32);
            self.active = ok;

            if ok {
                // Query `clap.latency` *after* activate (spec: main thread only,
                // value may not be valid before activation).  Off-RT — invoked
                // by the command thread during graph prepare.
                let ext_ptr = ((*p).get_extension)(p, CLAP_EXT_LATENCY.as_ptr() as *const _);
                if !ext_ptr.is_null() {
                    let ext = ext_ptr as *const ClapPluginLatency;
                    self.reported_latency = ((*ext).get)(p);
                }

                ((*p).start_processing)(p);
            }
        }

        self.lifecycle = LifecycleState::Running;
        self.instance.set_lifecycle(LifecycleState::Running);
    }

    fn process(
        &mut self,
        inputs:  &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
        if outputs.is_empty() || !self.active { return; }

        let nc = self.num_channels;
        let nf = outputs[0].num_frames;

        // Populate channel pointer arrays (no allocation)
        for c in 0..nc {
            self.in_channel_ptrs[c] = if !inputs.is_empty() {
                // CLAP spec: hosts may pass const data as the input; safe cast.
                inputs[0].channel(c).as_ptr() as *mut f32
            } else {
                std::ptr::null_mut()
            };
            self.out_channel_ptrs[c] = outputs[0].channel_mut(c).as_mut_ptr();
        }

        // Build ClapAudioBuffer structs (stack, no alloc)
        self.in_audio_buf = ClapAudioBuffer {
            data32:        self.in_channel_ptrs.as_mut_ptr(),
            data64:        std::ptr::null_mut(),
            channel_count: nc as u32,
            latency:       0,
            constant_mask: 0,
        };
        self.out_audio_buf = ClapAudioBuffer {
            data32:        self.out_channel_ptrs.as_mut_ptr(),
            data64:        std::ptr::null_mut(),
            channel_count: nc as u32,
            latency:       0,
            constant_mask: 0,
        };

        let process = ClapProcess {
            steady_time:         -1,
            frames_count:        nf as u32,
            transport:           std::ptr::null(),
            audio_inputs:        &self.in_audio_buf,
            audio_outputs:       &mut self.out_audio_buf,
            audio_inputs_count:  if inputs.is_empty() { 0 } else { 1 },
            audio_outputs_count: 1,
            in_events:  &self.in_events.input_events_vtable,
            out_events: &self.out_events,
        };

        // Non-atomic load through Arc::deref.
        let p = self.plugin_ptr();
        let status = unsafe { ((*p).process)(p, &process) };

        // On error, pass through
        if status == CLAP_PROCESS_ERROR && !inputs.is_empty() {
            outputs[0].copy_from(inputs[0]);
        }
    }

    fn reset(&mut self) {
        if self.active {
            let p = self.plugin_ptr();
            unsafe { ((*p).reset)(p); }
        }
    }

    /// Plain field read; populated once in `prepare()` via the `clap.latency`
    /// extension.  Called by the graph compiler — never on the audio thread.
    fn latency_samples(&self) -> u32 {
        self.reported_latency
    }
}

impl Drop for ClapWrapperNode {
    fn drop(&mut self) {
        // CLAP lifecycle requires stop_processing + deactivate before
        // destroy, but only if start_processing was called (the spec
        // forbids stop on an inactive plugin).  We perform those two steps
        // here, then release the `Arc<PluginInstance>` clone; the final
        // `destroy()` happens in `PluginInstance::drop` once the last `Arc`
        // is released (host clone + this clone — see `instance.rs`).
        //
        // Runs off-RT: the snapshot drop ring releases the old `CompiledGraph`
        // on a dedicated worker thread, never on the audio thread.
        if self.active {
            let p = self.plugin_ptr();
            unsafe {
                ((*p).stop_processing)(p);
                ((*p).deactivate)(p);
            }
            self.active = false;
        }
        // `self.instance` Arc drops next.  If this was the last clone the
        // PluginInstance::Drop fires destroy() + frees the DLL.
    }
}

impl Device for ClapWrapperNode {
    fn device_id(&self) -> &str { &self.info.device_id }
    fn info(&self)       -> &DeviceInfo  { &self.info }
    fn parameters(&self) -> &ParameterMap { &self.params }
    fn parameters_mut(&mut self) -> &mut ParameterMap { &mut self.params }

    fn save_state(&self) -> DeviceState { DeviceState::empty(&self.info.device_id) }
    fn load_state(&mut self, _: &DeviceState) {}

    fn lifecycle_state(&self) -> LifecycleState { self.lifecycle }
    fn suspend(&mut self)  {
        self.lifecycle = LifecycleState::Suspended;
        self.instance.set_lifecycle(LifecycleState::Suspended);
    }
    fn resume(&mut self)   {
        self.lifecycle = LifecycleState::Running;
        self.instance.set_lifecycle(LifecycleState::Running);
    }
    fn destroy(&mut self)  {
        self.lifecycle = LifecycleState::Destroyed;
        self.instance.set_lifecycle(LifecycleState::Destroyed);
    }

    fn reported_latency(&self) -> u32 { self.reported_latency }

    /// Phase 1: legacy shape (stereo, no params) plus the freshly-introspected
    /// latency.  Phase 2 replaces this with full audio-port / param introspection.
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::from_legacy_info(&self.info);
        caps.reported_latency_samples = self.reported_latency;
        caps
    }
}
