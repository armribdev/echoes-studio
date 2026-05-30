//! CLAP wrapper — adapts a live ClapPlugin into an AudioNode.
//!
//! ## Real-time contract
//!
//! All allocations (Vec, Box) happen in `new` and `prepare`.  `process` only
//! updates raw pointers and calls `ClapPlugin::process` — no heap ops, no locks.
//!
//! ## Thread safety
//!
//! `ClapWrapperNode` is `Send` because:
//! - CLAP plugin instances are required by spec to be single-threaded: all
//!   audio-thread calls (activate, process) happen on one thread at a time.
//! - The `Arc<ClapLibrary>` keeps the DLL loaded and is `Send + Sync`.

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::parameter::ParameterMap;
use crate::plugin_system::state::DeviceState;
use crate::plugin_system::wrapper::{Device, DeviceFormat, DeviceInfo};

use super::ffi::{
    ClapAudioBuffer, ClapHost, ClapLibrary, ClapOutputEvents, ClapProcess, ClapPlugin,
    FixedEventList, CLAP_PROCESS_CONTINUE, CLAP_PROCESS_ERROR,
};

// ── No-op output events (plugins can push events out; we ignore them for now) ──
unsafe extern "C" fn out_try_push(_: *const ClapOutputEvents, _: *const super::ffi::ClapEventHeader) -> bool { true }

// ── ClapWrapperNode ────────────────────────────────────────────────────────────

pub struct ClapWrapperNode {
    node_id: NodeId,
    info:    DeviceInfo,
    params:  ParameterMap,
    lifecycle: LifecycleState,

    // CLAP plugin instance pointer (never null after successful construction)
    plugin: *const ClapPlugin,

    // Host struct — must outlive the plugin instance
    _host: Box<ClapHost>,

    // Keeps the DLL loaded
    _lib: Arc<ClapLibrary>,

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
}

// SAFETY: see module-level doc
unsafe impl Send for ClapWrapperNode {}

impl ClapWrapperNode {
    pub fn new(
        node_id: NodeId,
        plugin_id: impl Into<String>,
        name:      impl Into<String>,
        vendor:    impl Into<String>,
        plugin:    *const ClapPlugin,
        host:      Box<ClapHost>,
        lib:       Arc<ClapLibrary>,
    ) -> Self {
        let plugin_id = plugin_id.into();
        ClapWrapperNode {
            node_id,
            info: DeviceInfo {
                device_id:   plugin_id,
                name:        name.into(),
                vendor:      vendor.into(),
                version:     String::new(),
                format:      DeviceFormat::Clap,
                num_inputs:  2,
                num_outputs: 2,
                has_midi_input:  false,
                has_midi_output: false,
            },
            params:    ParameterMap::new(),
            lifecycle: LifecycleState::Created,
            plugin,
            _host: host,
            _lib:  lib,
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
        }
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

        let p = self.plugin;
        unsafe {
            // Initialize the plugin instance if not already done
            ((*p).init)(p);

            // Activate (sets sample rate + block size bounds)
            let ok = ((*p).activate)(p, ctx.sample_rate as f64, 1, ctx.max_block_size as u32);
            self.active = ok;

            if ok {
                ((*p).start_processing)(p);
            }
        }

        self.lifecycle = LifecycleState::Running;
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

        let p = self.plugin;
        let status = unsafe { ((*p).process)(p, &process) };

        // On error, pass through
        if status == CLAP_PROCESS_ERROR && !inputs.is_empty() {
            outputs[0].copy_from(inputs[0]);
        }
    }

    fn reset(&mut self) {
        if self.active {
            let p = self.plugin;
            unsafe { ((*p).reset)(p); }
        }
    }
}

impl Drop for ClapWrapperNode {
    fn drop(&mut self) {
        let p = self.plugin;
        unsafe {
            if self.active {
                ((*p).stop_processing)(p);
                ((*p).deactivate)(p);
            }
            ((*p).destroy)(p);
        }
        // _lib Arc drops → FreeLibrary when no other nodes use this DLL
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
    fn suspend(&mut self)  { self.lifecycle = LifecycleState::Suspended; }
    fn resume(&mut self)   { self.lifecycle = LifecycleState::Running; }
    fn destroy(&mut self)  { self.lifecycle = LifecycleState::Destroyed; }
}
