//! VST3 wrapper — adapts a live IAudioProcessor into an AudioNode.
//!
//! ## Real-time contract
//!
//! All allocations happen in `prepare`.  `process` only moves pointers around
//! and calls IAudioProcessor::process — no heap ops, no locks.
//!
//! ## Thread safety
//!
//! `VstWrapperNode` is `Send` because:
//! - The COM object pointers are only dereferenced on one thread at a time:
//!   `prepare` on the command thread, `process` exclusively on the audio thread.
//! - `Arc<Vst3Library>` is `Send + Sync` (FreeLibrary is safe from any thread).

use std::ptr::NonNull;
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::parameter::ParameterMap;
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{Device, DeviceFormat, DeviceInfo};

use super::ffi::{
    AudioBusBuffers, IAudioProcessor, IComponent, ProcessData, ProcessSetup,
    Vst3Library, K_RESULT_OK, SPEAKER_STEREO,
};

// ── VstWrapperNode ─────────────────────────────────────────────────────────────

pub struct VstWrapperNode {
    node_id: NodeId,
    info:    DeviceInfo,
    params:  ParameterMap,
    lifecycle: LifecycleState,

    // COM pointers (never null after successful construction)
    component: NonNull<IComponent>,
    processor: NonNull<IAudioProcessor>,

    // Keeps the DLL loaded for the lifetime of this node.
    _lib: Arc<Vst3Library>,

    // Per-block scratch — pre-allocated in prepare, pointer-stable until next prepare.
    // Vecs are never resized on the audio thread.
    in_channel_ptrs:  Vec<*mut f32>,
    out_channel_ptrs: Vec<*mut f32>,
    in_bus:  AudioBusBuffers,
    out_bus: AudioBusBuffers,

    num_channels: usize,
}

// SAFETY: see module-level doc
unsafe impl Send for VstWrapperNode {}

impl VstWrapperNode {
    /// Construct from a live IComponent + IAudioProcessor pair.
    pub fn new(
        node_id: NodeId,
        plugin_id: impl Into<String>,
        name: impl Into<String>,
        component: NonNull<IComponent>,
        processor: NonNull<IAudioProcessor>,
        lib: Arc<Vst3Library>,
    ) -> Self {
        let plugin_id = plugin_id.into();
        VstWrapperNode {
            node_id,
            info: DeviceInfo {
                device_id:   plugin_id,
                name:        name.into(),
                vendor:      String::new(),
                version:     String::new(),
                format:      DeviceFormat::Vst3,
                num_inputs:  2,
                num_outputs: 2,
                has_midi_input:  false,
                has_midi_output: false,
            },
            params:    ParameterMap::new(),
            lifecycle: LifecycleState::Created,
            component,
            processor,
            _lib: lib,
            in_channel_ptrs:  Vec::new(),
            out_channel_ptrs: Vec::new(),
            in_bus:  unsafe { std::mem::zeroed() },
            out_bus: unsafe { std::mem::zeroed() },
            num_channels: 2,
        }
    }
}

impl AudioNode for VstWrapperNode {
    fn node_id(&self) -> NodeId { self.node_id }
    fn label(&self) -> &str { "VstWrapperNode" }
    fn num_input_ports(&self)  -> usize { 1 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.num_channels = ctx.num_channels;

        // Pre-allocate pointer scratch (stable for the life of this prepared node)
        self.in_channel_ptrs  = vec![std::ptr::null_mut(); ctx.num_channels];
        self.out_channel_ptrs = vec![std::ptr::null_mut(); ctx.num_channels];

        let proc = self.processor.as_ptr();

        // Inform plugin of bus arrangement (stereo in + stereo out)
        let mut in_arr  = SPEAKER_STEREO;
        let mut out_arr = SPEAKER_STEREO;
        unsafe {
            ((*(*proc).vtbl).set_bus_arrangements)(proc, &mut in_arr, 1, &mut out_arr, 1);
        }

        // Activate main I/O buses on the component side
        let comp = self.component.as_ptr();
        unsafe {
            // kAudio=0, kInput=0, index 0, activate=true(1)
            ((*(*comp).vtbl).activate_bus)(comp, 0, 0, 0, 1);
            // kAudio=0, kOutput=1, index 0, activate=true(1)
            ((*(*comp).vtbl).activate_bus)(comp, 0, 1, 0, 1);
            ((*(*comp).vtbl).set_active)(comp, 1);
        }

        // Setup processing parameters
        let setup = ProcessSetup {
            process_mode:          0, // realtime
            symbolic_sample_size:  0, // float32
            max_samples_per_block: ctx.max_block_size as i32,
            sample_rate:           ctx.sample_rate as f64,
        };
        unsafe {
            ((*(*proc).vtbl).setup_processing)(proc, &setup);
            ((*(*proc).vtbl).set_processing)(proc, 1);
        }

        self.lifecycle = LifecycleState::Running;
    }

    fn process(
        &mut self,
        inputs:  &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
        if outputs.is_empty() { return; }

        let nc = self.num_channels;
        let nf = outputs[0].num_frames;

        // Populate input channel pointer array.
        // SAFETY: VST3 spec guarantees plugins do not write to input buffers.
        for c in 0..nc {
            self.in_channel_ptrs[c] = if !inputs.is_empty() {
                inputs[0].channel(c).as_ptr() as *mut f32
            } else {
                std::ptr::null_mut()
            };
        }

        // Populate output channel pointer array.
        for c in 0..nc {
            self.out_channel_ptrs[c] = outputs[0].channel_mut(c).as_mut_ptr();
        }

        // Build AudioBusBuffers (no allocation — just setting fields).
        self.in_bus = AudioBusBuffers {
            num_channels:      nc as i32,
            silence_flags:     if inputs.is_empty() { u64::MAX } else { 0 },
            channel_buffers32: self.in_channel_ptrs.as_mut_ptr(),
        };
        self.out_bus = AudioBusBuffers {
            num_channels:      nc as i32,
            silence_flags:     0,
            channel_buffers32: self.out_channel_ptrs.as_mut_ptr(),
        };

        let mut data = ProcessData {
            process_mode:         0,
            symbolic_sample_size: 0,
            num_samples:          nf as i32,
            num_inputs:           1,
            num_outputs:          1,
            inputs:               &mut self.in_bus,
            outputs:              &mut self.out_bus,
            input_param_changes:  std::ptr::null_mut(),
            output_param_changes: std::ptr::null_mut(),
            input_events:         std::ptr::null_mut(),
            output_events:        std::ptr::null_mut(),
            process_context:      std::ptr::null_mut(),
        };

        let proc = self.processor.as_ptr();
        let r = unsafe { ((*(*proc).vtbl).process)(proc, &mut data) };

        // On plugin error, fall back to pass-through.
        if r != K_RESULT_OK && !inputs.is_empty() {
            outputs[0].copy_from(inputs[0]);
        }
    }

    fn reset(&mut self) {
        // VST3 setProcessing(false) then setProcessing(true) resets internal state.
        let proc = self.processor.as_ptr();
        unsafe {
            ((*(*proc).vtbl).set_processing)(proc, 0);
            ((*(*proc).vtbl).set_processing)(proc, 1);
        }
    }
}

impl Drop for VstWrapperNode {
    fn drop(&mut self) {
        let proc = self.processor.as_ptr();
        let comp = self.component.as_ptr();
        unsafe {
            // Deactivate and terminate before releasing.
            ((*(*proc).vtbl).set_processing)(proc, 0);
            ((*(*comp).vtbl).set_active)(comp, 0);
            ((*(*comp).vtbl).terminate)(comp);
            // Release IAudioProcessor ref (may be same object as IComponent).
            ((*(*proc).vtbl).release)(proc as *mut _);
            // Release IComponent ref.
            ((*(*comp).vtbl).release)(comp as *mut _);
        }
        // _lib Arc drops here, FreeLibrary when last node using this DLL is gone.
    }
}

impl Device for VstWrapperNode {
    fn device_id(&self) -> &str { &self.info.device_id }
    fn info(&self)       -> &DeviceInfo  { &self.info }
    fn parameters(&self) -> &ParameterMap { &self.params }
    fn parameters_mut(&mut self) -> &mut ParameterMap { &mut self.params }

    fn save_state(&self) -> DeviceState {
        DeviceState::empty(&self.info.device_id)
    }
    fn load_state(&mut self, _state: &DeviceState) {}

    fn lifecycle_state(&self)  -> LifecycleState { self.lifecycle }
    fn suspend(&mut self)  { self.lifecycle = LifecycleState::Suspended; }
    fn resume(&mut self)   { self.lifecycle = LifecycleState::Running; }
    fn destroy(&mut self)  { self.lifecycle = LifecycleState::Destroyed; }
}
