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
//! The wrapper keeps no lifecycle state.  The single source of truth is the
//! `AtomicU8` inside [`PluginInstance`], driven by `PluginHost`.
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
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};
use crate::audio_graph::plugin_params::{parse_plugin_param_name, PluginParamControl, PluginParamControls};
use crate::plugin_formats::crash_guard;
use crate::plugin_system::instance::{PluginInstance, FFI_GATE_HARVEST_BUDGET};
use crate::plugin_system::lifecycle::LifecycleState;

use super::ffi::{
    AudioBusBuffers, HostParameterChanges, IAudioProcessor, IComponent, ProcessData, K_RESULT_OK,
};

// Stack context passed to the SEH trampoline — no heap allocation.
struct Vst3ProcessCtx {
    proc_ptr: *mut IAudioProcessor,
    data_ptr: *mut ProcessData,
}

unsafe extern "C" fn vst3_process_trampoline(raw: *mut c_void) -> i32 {
    let ctx = &*(raw as *const Vst3ProcessCtx);
    ((*(*ctx.proc_ptr).vtbl).process)(ctx.proc_ptr, ctx.data_ptr)
}

pub struct VstWrapperNode {

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

    /// Set to `true` after the first SEH exception from the plugin's
    /// `process()`.  All subsequent calls return immediately with silence,
    /// keeping the audio thread safe until the graph is next recompiled.
    crashed: bool,

    /// Parameters targeted by modulation or automation (ADR-021), injected at
    /// compile time; pre-allocated, iterated in `process()` without allocation.
    param_controls: PluginParamControls,
}

// SAFETY: see module-level doc.  Raw pointers are reached via Arc-shared
// `PluginInstance`, which itself bears `Send + Sync` under the same
// sequential-access discipline.
unsafe impl Send for VstWrapperNode {}

impl VstWrapperNode {
    /// Construct from a shared [`PluginInstance`].  Caller must guarantee the
    /// instance was built with VST3 native handles (`PluginInstance::new_vst3`).
    pub fn new(instance: Arc<PluginInstance>) -> Self {
        debug_assert!(
            instance.vst3_handles().is_some(),
            "VstWrapperNode requires a VST3 PluginInstance"
        );
        Self {
            instance,
            in_channel_ptrs: Vec::new(),
            out_channel_ptrs: Vec::new(),
            in_bus: unsafe { std::mem::zeroed() },
            out_bus: unsafe { std::mem::zeroed() },
            num_channels: 2,
            param_changes: None,
            reported_latency: 0,
            crashed: false,
            param_controls: PluginParamControls::default(),
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

        // Activation lifecycle is owned by the shared instance (ADR-016) so it
        // survives a wrapper rebuild when the live instance is reused across a
        // recompile.  Idempotent: an already-active instance is left untouched
        // (no double `setActive`).
        self.instance
            .vst3_ensure_active(ctx.sample_rate, ctx.max_block_size as u32);

        // Query intrinsic latency after setup so the value reflects the
        // configured sample rate / block size.  VST3 spec: callable from the
        // main thread; the host must re-query if `restart` fires `kLatencyChanged`
        // (handled when Phase 6 wires restart).
        //
        // On a recompile that reuses a live instance (ADR-016), this runs while
        // the outgoing graph may still `process()` it.  Fence the getter with the
        // instance FFI gate so it can't race `process()`; if a block is in flight
        // and the gate can't be claimed in time, keep the current latency (a
        // transient — corrected on a later recompile).
        if let Some(_gate) = self.instance.offrt_try_acquire(FFI_GATE_HARVEST_BUDGET) {
            let (_, proc_nn) = self.handles();
            let proc = proc_nn.as_ptr();
            unsafe {
                self.reported_latency = ((*(*proc).vtbl).get_latency_samples)(proc);
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
        if self.crashed || outputs.is_empty() {
            return;
        }

        // FFI access gate (step 1): claim exclusive access to the shared instance
        // for this block.  If an off-RT caller (state save/load, GUI) currently
        // holds it, skip the plugin FFI and bypass the audio so the two never run
        // concurrently — the VST3 single-threaded contract.  Non-blocking: the
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
            // ADR-021: VST3 has no host modulation, so modulation is added to
            // the tracked base value.  Automation sets the value outright and,
            // on the same parameter, takes precedence over modulation.
            for control in self.param_controls.iter() {
                if control.automation.is_empty() {
                    for (offset, value) in control.modulation.iter() {
                        let effective = (control.base_value() + value).clamp(0.0, 1.0);
                        pc.push(control.native_id, offset as i32, effective as f64);
                    }
                } else {
                    for (offset, value) in control.automation.iter() {
                        pc.push(control.native_id, offset as i32, value.clamp(0.0, 1.0) as f64);
                    }
                }
            }
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
        let mut ctx = Vst3ProcessCtx { proc_ptr: proc, data_ptr: &mut data };
        let r = unsafe {
            crash_guard::guarded(
                vst3_process_trampoline,
                &mut ctx as *mut Vst3ProcessCtx as *mut c_void,
            )
        };

        if r == crash_guard::CAUGHT {
            // SEH exception from the plugin: mute permanently until next recompile.
            self.crashed = true;
            // Mark the shared instance as Error so reuse_existing won't hand the
            // corrupted handle to the next graph compile; forces a fresh FFI instantiation.
            self.instance.set_lifecycle(LifecycleState::Error);
            crate::plugin_system::crash_marker::note_crash();
            return;
        }
        if r != K_RESULT_OK && !inputs.is_empty() {
            outputs[0].copy_from(inputs[0]);
        }
    }

    fn param_id(&self, name: &str) -> Option<u32> {
        parse_plugin_param_name(name)
    }

    fn reset_modulation(&mut self) {
        self.param_controls.clear_block();
    }

    fn apply_modulation(&mut self, param_id: u32, delta: f32) {
        self.param_controls.modulate(param_id, delta);
    }

    fn apply_modulation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        self.param_controls.modulate_at(param_id, sample_offset, value);
    }

    fn apply_automation(&mut self, param_id: u32, value: f32) {
        self.param_controls.automate(param_id, value);
    }

    fn apply_automation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        self.param_controls.automate_at(param_id, sample_offset, value);
    }

    fn inject_plugin_param_controls(&mut self, controls: Vec<PluginParamControl>) {
        self.param_controls = PluginParamControls::new(controls);
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
