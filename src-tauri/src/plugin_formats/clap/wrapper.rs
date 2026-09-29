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
//! The wrapper keeps no lifecycle state.  The single source of truth is the
//! atomic inside [`PluginInstance`], driven by `PluginHost`.
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
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};
use crate::audio_graph::plugin_params::{parse_plugin_param_name, PluginParamControl};
use crate::plugin_formats::clap::audio_ports::ClapOutputBuses;
use crate::plugin_formats::clap::params::{
    observe_output_event, read_clap_params, ClapParamEvents,
};
use crate::plugin_formats::clap::transport::clap_transport_event;
use crate::plugin_formats::crash_guard;
use crate::plugin_system::instance::{PluginInstance, FFI_GATE_HARVEST_BUDGET};
use crate::plugin_system::lifecycle::LifecycleState;

use super::ffi::{
    audio_thread_scope, ClapAudioBuffer, ClapOutputEvents, ClapPlugin, ClapPluginLatency,
    ClapProcess, CLAP_EXT_LATENCY, CLAP_PROCESS_ERROR,
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

pub struct ClapWrapperNode {
    /// Shared, refcounted handle to the live plugin.  Captured at
    /// construction; cloned only on the command thread.  Deref on the audio
    /// thread is a non-atomic field load (cf. `ADR-007`).
    /// Phase 2b: also holds the authoritative `lifecycle` atomic.
    instance: Arc<PluginInstance>,

    /// Auxiliary (sidechain) input port count, from the scan-time
    /// `clap.audio-ports` probe (ADR-014, `PluginManifest.num_sidechain_inputs`).
    /// `0` for every plugin that doesn't declare an aux bus — unchanged from
    /// before this field existed. Fixed for the wrapper's lifetime: a plugin
    /// swap goes through a fresh `ClapWrapperNode`, not a mutation of this one.
    num_aux_inputs: usize,

    // Per-block scratch — pre-allocated in prepare, stable after that.
    // Port 0 is the main bus; ports `1..=num_aux_inputs` are aux buses, each
    // sized like the main bus (the graph has no per-port channel model — every
    // `AudioBuffer` in it carries the engine's channel count uniformly).
    in_port_channel_ptrs: Vec<Vec<*mut f32>>,
    in_audio_bufs: Vec<ClapAudioBuffer>,
    /// Every declared output bus (ADR-014 multi-out); rebuilt in `prepare`,
    /// once the plugin has been `init`ed and has reported its ports.
    outputs: ClapOutputBuses,

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
    steady_time: i64,
}

// SAFETY: see module-level doc.  CLAP pointers are reached via Arc-shared
// `PluginInstance`, which bears `Send + Sync` under the same single-threaded
// access discipline mandated by the CLAP spec.
unsafe impl Send for ClapWrapperNode {}

impl ClapWrapperNode {
    /// Construct from a shared [`PluginInstance`].  Caller must guarantee the
    /// instance was built with a CLAP native handle (`PluginInstance::new_clap`).
    ///
    /// `num_aux_inputs` comes from the plugin's scanned manifest
    /// (`PluginHost::sidechain_input_ports`, ADR-014) — `0` for every plugin
    /// without a declared aux bus, so callers that don't care can pass `0`.
    pub fn new(instance: Arc<PluginInstance>, num_aux_inputs: usize) -> Self {
        debug_assert!(
            instance.clap_plugin().is_some(),
            "ClapWrapperNode requires a CLAP PluginInstance"
        );
        Self {
            instance,
            num_aux_inputs,
            in_port_channel_ptrs: Vec::new(),
            in_audio_bufs: Vec::new(),
            outputs: ClapOutputBuses::new(&[]),
            param_events: ClapParamEvents::new(),
            out_events: ClapOutputEvents {
                ctx: std::ptr::null_mut(),
                try_push: observe_output_event,
            },
            num_channels: 2,
            active: false,
            crashed: false,
            steady_time: 0,
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
    fn label(&self) -> &str {
        "ClapWrapperNode"
    }
    fn num_input_ports(&self) -> usize {
        1 + self.num_aux_inputs
    }
    fn num_output_ports(&self) -> usize {
        self.outputs.num_ports()
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.num_channels = ctx.num_channels;
        self.in_port_channel_ptrs = (0..1 + self.num_aux_inputs)
            .map(|_| vec![std::ptr::null_mut(); ctx.num_channels])
            .collect();
        self.in_audio_bufs = (0..1 + self.num_aux_inputs)
            .map(|_| unsafe { std::mem::zeroed() })
            .collect();

        // Activation lifecycle is owned by the shared instance (ADR-016) so it
        // survives a wrapper rebuild when the graph is recompiled and the live
        // instance is reused.  Idempotent: an already-active instance is left
        // untouched (the CLAP spec forbids a second `activate`).
        self.active = self
            .instance
            .clap_ensure_active(ctx.sample_rate, ctx.max_block_size as u32);

        self.outputs = ClapOutputBuses::new(self.instance.output_ports());
        self.outputs.prepare(ctx.num_channels, ctx.max_block_size);

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
        let _audio_thread = audio_thread_scope();

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
                self.steady_time = self.steady_time.wrapping_add(transport.block_size as i64);
                return;
            }
        };
        self.instance.clap_start_processing();

        let nc = self.num_channels;
        let nf = transport.block_size; // real block, not MAX_BLOCK_FRAMES capacity

        self.param_events.fill(&self.instance);
        self.param_events.buffer.sort();
        self.out_events.ctx = self.param_events.output_context();

        // Port 0 = main bus (from the upstream connection); ports 1.. = aux
        // buses (sidechain — silence until something is routed into them, cf.
        // ADR-014 follow-ups). `inputs` always has one `&AudioBuffer` per
        // declared port — the scheduler substitutes a shared silence buffer for
        // an unconnected one, it never shortens the slice.
        for port in 0..1 + self.num_aux_inputs {
            let src = inputs.get(port).copied();
            for c in 0..nc {
                self.in_port_channel_ptrs[port][c] = match src {
                    // CLAP spec: hosts may pass const data as the input; safe cast.
                    Some(buf) => buf.channel(c).as_ptr() as *mut f32,
                    None => std::ptr::null_mut(),
                };
            }
            self.in_audio_bufs[port] = ClapAudioBuffer {
                data32: self.in_port_channel_ptrs[port].as_mut_ptr(),
                data64: std::ptr::null_mut(),
                channel_count: nc as u32,
                latency: 0,
                constant_mask: 0,
            };
        }
        let (audio_outputs, audio_outputs_count) = self.outputs.bind(outputs);

        let transport_event = clap_transport_event(transport);
        let process = ClapProcess {
            steady_time: self.steady_time,
            frames_count: nf as u32,
            transport: &transport_event,
            audio_inputs: self.in_audio_bufs.as_ptr(),
            audio_outputs,
            audio_inputs_count: if inputs.is_empty() {
                0
            } else {
                self.in_audio_bufs.len() as u32
            },
            audio_outputs_count,
            in_events: &self.param_events.buffer.input_events_vtable,
            out_events: &self.out_events,
        };

        let p = self.plugin_ptr();
        let mut ctx = ClapProcessCtx {
            plugin: p,
            process: &process,
        };
        let status = unsafe {
            crash_guard::guarded(
                clap_process_trampoline,
                &mut ctx as *mut ClapProcessCtx as *mut c_void,
            )
        };

        self.steady_time = self.steady_time.wrapping_add(nf as i64);
        if status == crash_guard::CAUGHT {
            // SEH exception from the plugin: mute permanently until next recompile.
            self.crashed = true;
            // Mark the shared instance as Error so reuse_existing won't hand the
            // corrupted handle to the next graph compile; forces a fresh FFI instantiation.
            self.instance.set_lifecycle(LifecycleState::Error);
            crate::plugin_system::crash_marker::note_crash();
            return;
        }
        if status == CLAP_PROCESS_ERROR && !inputs.is_empty() {
            outputs[0].copy_from(inputs[0]);
        }
        self.outputs.finish(outputs);
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
        self.param_events
            .controls_mut()
            .modulate_at(param_id, sample_offset, value);
    }

    fn apply_automation(&mut self, param_id: u32, value: f32) {
        self.param_events.controls_mut().automate(param_id, value);
    }

    fn apply_automation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        self.param_events
            .controls_mut()
            .automate_at(param_id, sample_offset, value);
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

/// Real-FFI proof that a declared aux (sidechain) input port actually reaches
/// the plugin's `process` call (ADR-014 follow-up: "wrapper aux feed"), and
/// that a declared aux output comes back out of it (ADR-014 multi-out).
/// Companion to the scan-time introspection test in
/// `plugin_formats/clap/host.rs`, which proves the *count* is read correctly;
/// this proves the *audio* is routed correctly once a wrapper is told about it.
#[cfg(all(test, windows))]
mod aux_feed_tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::OnceLock;

    use crate::plugin_system::host::PluginHost;
    use crate::plugin_system::registry::PluginManifest;
    use crate::plugin_system::wrapper::DeviceFormat;

    use super::*;

    const PLUGIN_ID: &str = "clap:echoes.test.gain";

    /// Built once per test run — mirrors `plugin_system::sandbox_tests::artifacts`.
    fn test_plugin_dll() -> &'static PathBuf {
        static DLL: OnceLock<PathBuf> = OnceLock::new();
        DLL.get_or_init(|| {
            let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
            let target = workspace.join("target").join("clap-aux-feed-test");
            let status = Command::new(env!("CARGO"))
                .args(["build", "--quiet", "-p", "test_clap_plugin", "--target-dir"])
                .arg(&target)
                .current_dir(workspace)
                .status()
                .expect("run cargo");
            assert!(status.success(), "building test_clap_plugin failed");
            target.join("debug").join("test_clap_plugin.dll")
        })
    }

    fn manifest(dll: &Path, num_sidechain_inputs: usize) -> PluginManifest {
        PluginManifest {
            id: PLUGIN_ID.into(),
            name: "Echoes Test Gain".into(),
            vendor: "Echoes Studio".into(),
            format: DeviceFormat::Clap,
            path: Some(dll.to_path_buf()),
            categories: vec![],
            num_sidechain_inputs,
            is_instrument: false,
        }
    }

    /// A wrapper told about one aux input (as `PluginHost::instantiate` would,
    /// once the scanner reports `num_sidechain_inputs: 1` for a real sidechain
    /// plugin) must deliver `inputs[1]` to the plugin as its second CLAP audio
    /// port — not silence, not a duplicate of the main bus.
    #[test]
    fn aux_port_signal_reaches_the_plugin_process_call() {
        let dll = test_plugin_dll();
        let mut host = PluginHost::new();
        host.registry_mut().add(manifest(dll, 1));

        let mut node = host
            .instantiate("fx0", PLUGIN_ID, false)
            .expect("real CLAP instantiation must succeed");
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
            live: true,
        });
        assert_eq!(node.num_input_ports(), 2, "main + one declared aux port");

        let frames = 32;
        let main = AudioBuffer::new(2, frames); // silent
        let mut aux = AudioBuffer::new(2, frames);
        for ch in 0..2 {
            aux.channel_mut(ch).fill(0.3);
        }
        let mut outputs = vec![AudioBuffer::new(2, frames)];
        let transport = TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: frames,
        };

        node.process(&[&main, &aux], &mut outputs, &transport);

        // Test plugin: out = main*gain + aux. main is silent and default gain is
        // 1.0, so a non-silent output can only have come from the aux buffer.
        for ch in 0..2 {
            for &s in outputs[0].channel(ch) {
                assert!(
                    (s - 0.3).abs() < 1e-6,
                    "expected the aux port's 0.3 to reach the plugin verbatim, got {s}"
                );
            }
        }
    }

    /// A wrapper built for a plugin with no declared aux bus keeps the legacy
    /// single-port shape — no behaviour change for the overwhelming majority of
    /// plugins that don't have a sidechain input.
    #[test]
    fn no_aux_ports_declared_keeps_the_single_port_shape() {
        let dll = test_plugin_dll();
        let mut host = PluginHost::new();
        host.registry_mut().add(manifest(dll, 0));

        let node = host
            .instantiate("fx0", PLUGIN_ID, false)
            .expect("real CLAP instantiation must succeed");
        assert_eq!(node.num_input_ports(), 1, "no aux bus declared");
    }

    fn prepare_context() -> PrepareContext {
        PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 64,
            num_channels: 2,
            live: true,
        }
    }

    fn transport(frames: usize) -> TransportInfo {
        TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: frames,
        }
    }

    fn assert_all(buffer: &AudioBuffer, frames: usize, expected: f32, what: &str) {
        for ch in 0..2 {
            for &s in &buffer.channel(ch)[..frames] {
                assert!(
                    (s - expected).abs() < 1e-6,
                    "{what}, channel {ch}: expected {expected}, got {s}"
                );
            }
        }
    }

    /// The test plugin declares a mono "Aux Out" carrying half its main output:
    /// the effect node must expose it as graph output 1, spread over both
    /// channels, while output 0 stays the main bus.
    #[test]
    fn an_effect_aux_output_reaches_graph_output_one() {
        let dll = test_plugin_dll();
        let mut host = PluginHost::new();
        host.registry_mut().add(manifest(dll, 0));

        let mut node = host
            .instantiate("fx0", PLUGIN_ID, false)
            .expect("real CLAP instantiation must succeed");
        node.prepare(prepare_context());
        assert_eq!(node.num_output_ports(), 2, "main + the declared aux output");

        let frames = 32;
        let mut main = AudioBuffer::new(2, frames);
        for ch in 0..2 {
            main.channel_mut(ch).fill(0.4);
        }
        let mut outputs = vec![AudioBuffer::new(2, frames), AudioBuffer::new(2, frames)];

        node.process(&[&main], &mut outputs, &transport(frames));

        assert_all(&outputs[0], frames, 0.4, "main output");
        assert_all(&outputs[1], frames, 0.2, "aux output");
    }

    /// What the track input picker lists: aux outputs only, named by the plugin.
    #[test]
    fn the_host_lists_a_loaded_plugin_aux_outputs() {
        use crate::plugin_system::host::PluginOutputInfo;

        let dll = test_plugin_dll();
        let mut host = PluginHost::new();
        host.registry_mut().add(manifest(dll, 0));
        host.instantiate("fx0", PLUGIN_ID, false)
            .expect("real CLAP instantiation must succeed");

        assert_eq!(
            host.plugin_outputs("fx0"),
            vec![PluginOutputInfo {
                port: 1,
                name: "Aux Out".into(),
                channel_count: 1
            }]
        );
        assert!(host.plugin_outputs("not-loaded").is_empty());
    }

    /// Same plugin hosted as a track instrument: its aux output must come out
    /// of the instrument node too — the drum-machine case.
    #[test]
    fn an_instrument_aux_output_reaches_graph_output_one() {
        use crate::audio_graph::midi::{MidiEvent, MidiEventKind, MidiEventList};

        let dll = test_plugin_dll();
        let mut host = PluginHost::new();
        host.registry_mut().add(manifest(dll, 0));

        let mut device = host
            .instantiate_instrument("instrument:t1", PLUGIN_ID)
            .expect("real CLAP instrument instantiation must succeed");
        device.prepare(prepare_context());
        assert_eq!(
            device.num_output_ports(),
            2,
            "main + the declared aux output"
        );

        let frames = 32;
        let mut events = MidiEventList::new();
        events.push(MidiEvent {
            offset: 0,
            kind: MidiEventKind::NoteOn {
                pitch: 60,
                velocity: 127,
            },
        });
        let mut outputs = vec![AudioBuffer::new(2, frames), AudioBuffer::new(2, frames)];

        device.process_midi(&events, &mut outputs, &transport(frames));

        assert_all(&outputs[0], frames, 1.0, "main output");
        assert_all(&outputs[1], frames, 0.5, "aux output");
    }
}
