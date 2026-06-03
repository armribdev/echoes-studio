//! VST3 wrapper — adapts a live IAudioProcessor into an AudioNode.
//!
//! ## Real-time contract
//!
//! All allocations happen in `prepare`.  `process` only moves pointers around
//! and calls IAudioProcessor::process — no heap ops, no locks.
//!
//! ## Shared instance (ADR-007, Phase 2a)
//!
//! The COM pointers no longer live in this wrapper.  They are owned by
//! [`PluginInstance`] behind an `Arc`; the wrapper holds a clone captured at
//! construction time.  Every FFI dispatch (`prepare`, `process`, `reset`)
//! reaches the handle via `instance.vst3_handles()` — a non-atomic field read
//! through the `Arc::deref`.  No `Arc::clone` and no `Arc::drop` ever runs on
//! the audio thread.
//!
//! ## Unified lifecycle (Phase 2b)
//!
//! The wrapper no longer keeps its own `LifecycleState` field.  The single
//! source of truth is the `AtomicU8` inside [`PluginInstance`].  Queries
//! (`lifecycle_state()`) load the atomic; mutations (`suspend / resume /
//! destroy`) validate via [`transition`] against the current atomic value
//! before writing the new state back.
//!
//! ## Thread safety
//!
//! `VstWrapperNode` is `Send` because:
//! - The COM object pointers (now inside the shared `PluginInstance`) are
//!   accessed serially per the audio-graph scheduler's per-step ownership
//!   invariant: `prepare` runs on the command thread, `process` runs
//!   exclusively on the audio thread.
//! - The shared `Arc<PluginInstance>` is `Send + Sync` (see `instance.rs`
//!   safety claim).

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::plugin_system::capabilities::DeviceCapabilities;
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::lifecycle::{transition, LifecycleState};
use crate::plugin_system::parameter::ParameterMap;
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{Device, DeviceInfo};

use super::ffi::{
    AudioBusBuffers, HostParameterChanges, IAudioProcessor, IComponent, ProcessData, ProcessSetup,
    K_RESULT_OK, SPEAKER_STEREO,
};

pub struct VstWrapperNode {
    node_id: NodeId,
    info: DeviceInfo,
    params: ParameterMap,

    /// Shared, refcounted handle.  Captured at construction; cloned only on
    /// the command thread.  Deref on the audio thread is a non-atomic load.
    /// Phase 2b: also holds the authoritative `lifecycle` atomic (the wrapper
    /// no longer keeps its own copy).
    instance: Arc<PluginInstance>,

    // Per-block scratch — pre-allocated in prepare, pointer-stable until next prepare.
    // Vecs are never resized on the audio thread.
    in_channel_ptrs: Vec<*mut f32>,
    out_channel_ptrs: Vec<*mut f32>,
    in_bus: AudioBusBuffers,
    out_bus: AudioBusBuffers,

    num_channels: usize,

    /// Pre-allocated host-side parameter-change list (ADR-007).  Filled each
    /// block from the instance's SPSC ring and handed to the plugin via
    /// `ProcessData::input_param_changes`.  Allocated in `prepare`; pointer
    /// stable thereafter.  `None` until prepared.
    param_changes: Option<Box<HostParameterChanges>>,

    /// Cached intrinsic latency (samples).  Read once at `prepare()` via
    /// `IAudioProcessor::getLatencySamples`; queried off-RT by the graph
    /// compiler through `latency_samples()`.
    reported_latency: u32,
}

// SAFETY: see module-level doc.  Raw pointers are reached via Arc-shared
// `PluginInstance`, which itself bears `Send + Sync` under the same
// sequential-access discipline.
unsafe impl Send for VstWrapperNode {}

impl VstWrapperNode {
    /// Construct from a shared [`PluginInstance`].  Caller must guarantee the
    /// instance was built with VST3 native handles (`PluginInstance::new_vst3`).
    pub fn new(node_id: NodeId, instance: Arc<PluginInstance>) -> Self {
        debug_assert!(
            instance.vst3_handles().is_some(),
            "VstWrapperNode requires a VST3 PluginInstance"
        );
        let info = instance.legacy_device_info();
        Self {
            node_id,
            info,
            params: ParameterMap::new(),
            instance,
            in_channel_ptrs: Vec::new(),
            out_channel_ptrs: Vec::new(),
            in_bus: unsafe { std::mem::zeroed() },
            out_bus: unsafe { std::mem::zeroed() },
            num_channels: 2,
            param_changes: None,
            reported_latency: 0,
        }
    }

    /// VST3 handles helper.  Panics if the shared instance was constructed
    /// for a different format — wired in `PluginHost::instantiate_vst3` and
    /// guarded by the constructor `debug_assert!`, so this is a programmer
    /// error rather than a runtime condition.
    #[inline(always)]
    fn handles(&self) -> (NonNull<IComponent>, NonNull<IAudioProcessor>) {
        // Non-atomic pointer-chase through Arc::deref.  Identical RT cost to a
        // struct field load.
        self.instance
            .vst3_handles()
            .expect("VstWrapperNode missing VST3 handles (programmer error)")
    }
}

impl AudioNode for VstWrapperNode {
    fn node_id(&self) -> NodeId {
        self.node_id
    }
    fn label(&self) -> &str {
        "VstWrapperNode"
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

        // Pre-allocate the parameter-change list (ADR-007) — heap, off-RT.
        self.param_changes = Some(HostParameterChanges::boxed());

        let (comp_nn, proc_nn) = self.handles();
        let proc = proc_nn.as_ptr();
        let comp = comp_nn.as_ptr();

        let mut in_arr = SPEAKER_STEREO;
        let mut out_arr = SPEAKER_STEREO;
        unsafe {
            ((*(*proc).vtbl).set_bus_arrangements)(proc, &mut in_arr, 1, &mut out_arr, 1);
        }

        unsafe {
            ((*(*comp).vtbl).activate_bus)(comp, 0, 0, 0, 1);
            ((*(*comp).vtbl).activate_bus)(comp, 0, 1, 0, 1);
            ((*(*comp).vtbl).set_active)(comp, 1);
        }

        let setup = ProcessSetup {
            process_mode: 0,         // realtime
            symbolic_sample_size: 0, // float32
            max_samples_per_block: ctx.max_block_size as i32,
            sample_rate: ctx.sample_rate as f64,
        };
        unsafe {
            ((*(*proc).vtbl).setup_processing)(proc, &setup);
            ((*(*proc).vtbl).set_processing)(proc, 1);

            // Query intrinsic latency after setup so the value reflects the
            // configured sample rate / block size.  VST3 spec: callable from
            // the main thread; the host must re-query if `restart` is fired
            // with `kLatencyChanged` (handled when Phase 6 wires restart).
            self.reported_latency = ((*(*proc).vtbl).get_latency_samples)(proc);
        }

        self.instance.set_lifecycle(LifecycleState::Running);
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        if outputs.is_empty() {
            return;
        }

        let nc = self.num_channels;
        let nf = transport.block_size; // real block, not MAX_BLOCK_FRAMES capacity

        // ADR-007: drain pending parameter changes.
        // VST3 `IParameterChanges` carries normalised values directly, so the
        // queued `normalised` field maps 1:1.  No allocation: the list lives in
        // `self.param_changes`, reset and refilled in place each block.
        let param_changes_ptr: *mut c_void = if let Some(pc) = self.param_changes.as_mut() {
            pc.reset();
            self.instance.drain_params(|change| {
                pc.push(
                    change.native_id,
                    change.sample_offset as i32,
                    change.normalised as f64,
                );
            });
            if pc.count > 0 {
                pc.as_mut() as *mut HostParameterChanges as *mut c_void
            } else {
                std::ptr::null_mut()
            }
        } else {
            std::ptr::null_mut()
        };

        // SAFETY: VST3 spec — plugins must not write input buffers.
        for c in 0..nc {
            self.in_channel_ptrs[c] = if !inputs.is_empty() {
                inputs[0].channel(c).as_ptr() as *mut f32
            } else {
                std::ptr::null_mut()
            };
        }

        for c in 0..nc {
            self.out_channel_ptrs[c] = outputs[0].channel_mut(c).as_mut_ptr();
        }

        self.in_bus = AudioBusBuffers {
            num_channels: nc as i32,
            silence_flags: if inputs.is_empty() { u64::MAX } else { 0 },
            channel_buffers32: self.in_channel_ptrs.as_mut_ptr(),
        };
        self.out_bus = AudioBusBuffers {
            num_channels: nc as i32,
            silence_flags: 0,
            channel_buffers32: self.out_channel_ptrs.as_mut_ptr(),
        };

        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: nf as i32,
            num_inputs: 1,
            num_outputs: 1,
            inputs: &mut self.in_bus,
            outputs: &mut self.out_bus,
            input_param_changes: param_changes_ptr,
            output_param_changes: std::ptr::null_mut(),
            input_events: std::ptr::null_mut(),
            output_events: std::ptr::null_mut(),
            process_context: std::ptr::null_mut(),
        };

        // Non-atomic load through Arc::deref → identical cost to a field read.
        let (_, proc_nn) = self.handles();
        let proc = proc_nn.as_ptr();
        let r = unsafe { ((*(*proc).vtbl).process)(proc, &mut data) };

        if r != K_RESULT_OK && !inputs.is_empty() {
            outputs[0].copy_from(inputs[0]);
        }
    }

    fn reset(&mut self) {
        // VST3 setProcessing(false) then setProcessing(true) resets internal state.
        let (_, proc_nn) = self.handles();
        let proc = proc_nn.as_ptr();
        unsafe {
            ((*(*proc).vtbl).set_processing)(proc, 0);
            ((*(*proc).vtbl).set_processing)(proc, 1);
        }
    }

    /// Plain field read — populated once in `prepare()`.  Off-RT consumer
    /// (graph compiler); never invoked from `process`.
    fn latency_samples(&self) -> u32 {
        self.reported_latency
    }
}

// NOTE: no `Drop` impl here.  The COM pointers are owned by `PluginInstance`,
// which holds the only authoritative `Drop` for the FFI handles (see
// `instance.rs`).  When the snapshot drop ring releases this wrapper, the
// embedded `Arc<PluginInstance>` decrements the refcount; the host's own
// clone usually still holds the instance alive at that moment.  The handle
// is freed when the **last** `Arc` is released, on whichever off-RT thread
// performs that release.

impl Device for VstWrapperNode {
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
    fn load_state(&mut self, _state: &DeviceState) {}

    // Mutations validate the requested transition against the current atomic
    // value via `lifecycle::transition`.  Invalid transitions are silently
    // ignored at this layer — the host-side API is what surfaces errors to
    // callers (`PluginHost::{suspend,resume}_device`).  No FFI side effect
    // here in Phase 2b: suspend/resume on VST3 happens via setProcessing
    // toggles already exercised by `reset()`, and the lifecycle field has
    // historically been advisory only.  Phase 4 will wire it to setActive /
    // setProcessing once state save/load lands.

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

    /// Phase 1: legacy shape (stereo, no params, no state) plus the
    /// freshly-introspected latency.  Phase 2 will replace this with full
    /// bus / parameter introspection via `IComponent::getBusInfo` and
    /// `IEditController::getParameterInfo`.
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::from_legacy_info(&self.info);
        caps.reported_latency_samples = self.reported_latency;
        caps
    }
}
