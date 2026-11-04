//! A VST3 effect in the sandbox (ADR-033, steps 5 and 6): the VST3 test effect through
//! the host, in the bridge process and in the Echoes process.  The mirror of the effect
//! tests of `sandbox_tests.rs`; the parameter tests at the end run each behaviour in
//! both places and require the same answer.
//!
//! The test effect's state is its gain, so a state blob makes the processing audible.
//! A crash here is the bridge killed from outside, as from the Task Manager, and a hang
//! is the bridge frozen (the plugin's own `Crash` and `Hang` parameters are used by
//! `plugin_host_bridge/tests/vst3_bridge.rs`).

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};
use crate::audio_graph::plugin_params::PluginParamControl;
use crate::plugin_formats::clap::ffi::{
    take_plugin_params_rescan, take_plugin_state_edited, CLAP_PARAM_RESCAN_INFO,
    CLAP_PARAM_RESCAN_TEXT,
};
use crate::plugin_formats::vst3::test_dll::{effect_id, test_vst3_dll};
use crate::plugin_system::crash_marker;
use crate::plugin_system::host::{PluginHost, PluginIsolation, UnavailableReason};
use crate::plugin_system::registry::PluginManifest;
use crate::plugin_system::wrapper::DeviceFormat;

use super::sandbox_tests::artifacts;

const KEY: &str = "fx0";
const FRAMES: usize = 64;
const LATENCY: u32 = 64;

fn host() -> PluginHost {
    let mut host = PluginHost::new();
    host.set_bridge_exe(artifacts().0.clone());
    host.registry_mut().add(PluginManifest {
        id: effect_id(),
        name: "Echoes Test Gain".into(),
        vendor: "Echoes Studio".into(),
        format: DeviceFormat::Vst3,
        path: Some(test_vst3_dll().clone()),
        categories: vec![],
        num_sidechain_inputs: 0,
        is_instrument: false,
    });
    host
}

fn gain_state(gain: f64) -> Vec<u8> {
    gain.to_le_bytes().to_vec()
}

fn prepared(host: &mut PluginHost, sandboxed: bool, live: bool) -> Box<dyn AudioNode> {
    let mut node = host
        .instantiate(KEY, &effect_id(), sandboxed)
        .expect("plugin starts");
    node.prepare(PrepareContext {
        sample_rate: 48_000.0,
        max_block_size: 4096,
        num_channels: 2,
        live,
    });
    node
}

fn run_block(node: &mut Box<dyn AudioNode>) -> Vec<f32> {
    let mut input = AudioBuffer::new(2, 4096);
    for channel in 0..2 {
        input.channel_mut(channel)[..FRAMES].fill(0.5);
    }
    let mut outputs = vec![AudioBuffer::new(2, 4096)];
    let transport = TransportInfo {
        block_size: FRAMES,
        ..TransportInfo::stopped(48_000.0)
    };
    node.process(&[&input], &mut outputs, &transport);
    outputs[0].channel(0)[..FRAMES].to_vec()
}

fn is_sandboxed(host: &PluginHost) -> bool {
    host.instance_by_key(KEY).unwrap().is_sandboxed()
}

fn saved_state(host: &PluginHost) -> Option<Vec<u8>> {
    host.collect_plugin_states()
        .into_iter()
        .find(|(key, _)| key == KEY)
        .map(|(_, bytes)| bytes)
}

fn bridge_process_id(host: &PluginHost) -> u32 {
    host.instance_by_key(KEY)
        .unwrap()
        .bridge()
        .unwrap()
        .process_id()
}

fn is_dead(host: &PluginHost) -> bool {
    host.instance_by_key(KEY)
        .unwrap()
        .bridge()
        .unwrap()
        .is_dead()
}

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut c_void;
    fn TerminateProcess(process: *mut c_void, exit_code: u32) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

#[link(name = "ntdll")]
extern "system" {
    fn NtSuspendProcess(process: *mut c_void) -> i32;
}

const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_SUSPEND_RESUME: u32 = 0x0800;

fn with_process(process_id: u32, access: u32, action: impl FnOnce(*mut c_void)) {
    // SAFETY: plain Win32 calls on a process this test started.
    unsafe {
        let handle = OpenProcess(access, 0, process_id);
        assert!(!handle.is_null(), "open the bridge process");
        action(handle);
        CloseHandle(handle);
    }
}

fn kill(process_id: u32) {
    with_process(process_id, PROCESS_TERMINATE, |handle| {
        // SAFETY: `handle` is open with the right to terminate.
        assert_ne!(unsafe { TerminateProcess(handle, 1) }, 0);
    });
}

fn freeze(process_id: u32) {
    with_process(process_id, PROCESS_SUSPEND_RESUME, |handle| {
        // SAFETY: `handle` is open with the right to suspend.
        assert!(unsafe { NtSuspendProcess(handle) } >= 0);
    });
}

fn run_until_dead(host: &PluginHost, node: &mut Box<dyn AudioNode>) {
    for _ in 0..40 {
        let output = run_block(node);
        assert!(
            output.iter().all(|&s| s == 0.0 || s == 0.25),
            "silence or the plugin's sound, never garbage"
        );
        if is_dead(host) {
            return;
        }
    }
    panic!("the bridge was never condemned");
}

#[test]
fn a_sandboxed_vst3_effect_processes_and_reports_latency() {
    let mut host = host();
    let mut node = prepared(&mut host, true, false);
    assert!(is_sandboxed(&host));
    assert_eq!(node.latency_samples(), LATENCY);
    assert!(
        run_block(&mut node).iter().all(|&s| s == 0.5),
        "unity gain by default"
    );
    assert!(host.unavailable_plugins().is_empty());
}

#[test]
fn the_state_is_saved_and_restored_through_the_sandbox() {
    let mut host = host();
    host.set_pending_state_blobs(HashMap::from([(KEY.to_string(), gain_state(1.5))]));
    let mut node = prepared(&mut host, true, false);
    host.clear_pending_state_blobs();

    assert!(run_block(&mut node).iter().all(|&s| s == 0.75));
    assert_eq!(saved_state(&host), Some(gain_state(1.5)));
}

#[test]
fn toggling_the_sandbox_moves_the_plugin_and_keeps_its_state() {
    let mut host = host();
    host.set_pending_state_blobs(HashMap::from([(KEY.to_string(), gain_state(0.5))]));
    let _sandboxed = prepared(&mut host, true, false);
    host.clear_pending_state_blobs();

    let mut in_process = prepared(&mut host, false, false);
    assert!(!is_sandboxed(&host));
    assert!(run_block(&mut in_process).iter().all(|&s| s == 0.25));

    let mut sandboxed_again = prepared(&mut host, true, false);
    assert!(is_sandboxed(&host));
    assert!(run_block(&mut sandboxed_again).iter().all(|&s| s == 0.25));
}

#[test]
fn a_recompile_reuses_the_running_bridge() {
    let mut host = host();
    let _first = prepared(&mut host, true, true);
    let before = host.instance_by_key(KEY).unwrap().instance_id();
    let process_id = bridge_process_id(&host);
    let mut second = prepared(&mut host, true, true);
    assert_eq!(host.instance_by_key(KEY).unwrap().instance_id(), before);
    assert_eq!(bridge_process_id(&host), process_id, "same process");
    assert!(run_block(&mut second).iter().all(|&s| s == 0.5));
}

#[test]
fn the_isolation_preference_decides_for_a_vst3_effect() {
    let mut host = host();
    prepared(&mut host, false, false);
    assert!(!is_sandboxed(&host), "Chosen: follows the effect");

    let always = HashSet::from([effect_id()]);
    assert!(host.set_isolation(PluginIsolation::Chosen, always.clone()));
    prepared(&mut host, false, false);
    assert!(is_sandboxed(&host), "always isolated");

    assert!(host.set_isolation(PluginIsolation::All, HashSet::new()));
    prepared(&mut host, false, false);
    assert!(is_sandboxed(&host), "All: isolated without being chosen");

    assert!(host.set_isolation(PluginIsolation::Off, always));
    prepared(&mut host, true, false);
    assert!(!is_sandboxed(&host), "Off: in-process even when chosen");
}

#[test]
fn a_killed_bridge_is_contained_reported_and_recovered_on_recompile() {
    let mut host = host();
    host.set_pending_state_blobs(HashMap::from([(KEY.to_string(), gain_state(0.5))]));
    let mut node = prepared(&mut host, true, true);
    host.clear_pending_state_blobs();
    assert!(run_block(&mut node).iter().all(|&s| s == 0.25));
    let crashes_before = crash_marker::crash_count();

    kill(bridge_process_id(&host));
    run_until_dead(&host, &mut node);
    assert!(crash_marker::crash_count() > crashes_before);
    let unavailable = host.unavailable_plugins();
    assert_eq!(unavailable.len(), 1);
    assert_eq!(unavailable[0].reason, UnavailableReason::Crashed);

    let mut restarted = prepared(&mut host, true, false);
    assert!(!is_dead(&host));
    assert!(
        run_block(&mut restarted).iter().all(|&s| s == 0.25),
        "restarted with its last known state"
    );
    host.retain_devices(&HashSet::from([KEY.to_string()]));
    assert!(host.unavailable_plugins().is_empty());
}

#[test]
fn a_frozen_bridge_is_contained_and_the_audio_thread_never_waits_for_it() {
    let mut host = host();
    let mut node = prepared(&mut host, true, true);
    assert!(run_block(&mut node).iter().all(|&s| s == 0.5));

    freeze(bridge_process_id(&host));
    let started = std::time::Instant::now();
    run_until_dead(&host, &mut node);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "every block waited at most 3/4 of its period"
    );
    assert_eq!(
        host.unavailable_plugins()[0].reason,
        UnavailableReason::Crashed
    );
    kill(bridge_process_id(&host));
}

#[test]
fn a_vst3_plugin_that_cannot_start_in_the_sandbox_is_reported_unavailable() {
    let mut host = PluginHost::new();
    host.registry_mut().add(PluginManifest {
        id: effect_id(),
        name: "Echoes Test Gain".into(),
        vendor: "Echoes Studio".into(),
        format: DeviceFormat::Vst3,
        path: Some(test_vst3_dll().clone()),
        categories: vec![],
        num_sidechain_inputs: 0,
        is_instrument: false,
    });
    host.set_bridge_exe(std::path::PathBuf::from("does-not-exist.exe"));

    assert!(host.instantiate(KEY, &effect_id(), true).is_err());
    assert!(
        host.instance_by_key(KEY).is_none(),
        "never loaded in-process instead"
    );
    assert_eq!(host.unavailable_plugins().len(), 1);
}

#[repr(C)]
#[derive(Default)]
struct FileTime {
    low: u32,
    high: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetProcessTimes(
        process: *mut c_void,
        created: *mut FileTime,
        exited: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn GetThreadTimes(
        thread: *mut c_void,
        created: *mut FileTime,
        exited: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn GetCurrentThread() -> *mut c_void;
}

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

fn seconds(time: &FileTime) -> f64 {
    ((u64::from(time.high) << 32) | u64::from(time.low)) as f64 / 1e7
}

/// CPU seconds (kernel + user) of the calling thread.
fn current_thread_cpu() -> f64 {
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    // SAFETY: the pseudo-handle of the calling thread and valid out-pointers.
    unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        );
    }
    seconds(&kernel) + seconds(&user)
}

fn process_cpu(process_id: u32) -> f64 {
    let mut total = 0.0;
    with_process(process_id, PROCESS_QUERY_LIMITED_INFORMATION, |handle| {
        let (mut created, mut exited, mut kernel, mut user) = Default::default();
        // SAFETY: `handle` is open for queries and the out-pointers are valid.
        unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
        total = seconds(&kernel) + seconds(&user);
    });
    total
}

/// Round trip of one block (ADR-033 step 5): the VST3 effect in the Echoes process and
/// in the bridge, offline wait, as the CLAP measure of step 4:
/// `cargo test --bin echoes-studio measure_the_vst3_block_round_trip -- --ignored --nocapture`.
#[test]
#[ignore = "timing measure, prints its figures"]
fn measure_the_vst3_block_round_trip() {
    const WARM_UP: usize = 200;
    const BLOCKS: usize = 3000;

    let mut host = host();
    for (label, sandboxed) in [("in-process", false), ("sandboxed", true)] {
        let mut effect = prepared(&mut host, sandboxed, false);
        let mut input = AudioBuffer::new(2, 4096);
        let mut outputs = vec![AudioBuffer::new(2, 4096)];
        for frames in [64usize, 512] {
            input.channel_mut(0)[..frames].fill(0.5);
            input.channel_mut(1)[..frames].fill(0.5);
            let transport = TransportInfo {
                block_size: frames,
                ..TransportInfo::stopped(48_000.0)
            };
            let mut micros = Vec::with_capacity(BLOCKS);
            for block in 0..WARM_UP + BLOCKS {
                let start = std::time::Instant::now();
                effect.process(&[&input], &mut outputs, &transport);
                if block >= WARM_UP {
                    micros.push(start.elapsed().as_secs_f64() * 1e6);
                }
            }
            micros.sort_by(f64::total_cmp);
            let at = |share: f64| micros[((micros.len() - 1) as f64 * share) as usize];
            let mean = micros.iter().sum::<f64>() / micros.len() as f64;
            println!(
                "VST3 effect {label:<10} {frames:>3} frames: mean {mean:7.1} us   p50 {:7.1}   p99 {:7.1}   max {:8.1}",
                at(0.5),
                at(0.99),
                micros[micros.len() - 1]
            );
        }
    }
}

/// CPU of several VST3 effects at the pace of the live callback (ADR-033, "Real-time
/// risks": more cores in the live wait): the audio thread's CPU and the bridges' CPU,
/// as a share of one core, in the Echoes process and in bridges.
/// `cargo test --bin echoes-studio measure_eight_vst3_effects -- --ignored --nocapture`.
#[test]
#[ignore = "timing measure, prints its figures"]
fn measure_eight_vst3_effects() {
    const FRAMES_PER_BLOCK: usize = 512;
    const BLOCK_COUNT: usize = 500;
    const EFFECTS: usize = 8;

    for sandboxed in [false, true] {
        let mut host = host();
        let mut nodes: Vec<Box<dyn AudioNode>> = (0..EFFECTS)
            .map(|index| {
                let key = format!("fx{index}");
                let mut node = host
                    .instantiate(&key, &effect_id(), sandboxed)
                    .expect("plugin starts");
                node.prepare(PrepareContext {
                    sample_rate: 48_000.0,
                    max_block_size: 4096,
                    num_channels: 2,
                    live: true,
                });
                node
            })
            .collect();
        let process_ids: Vec<u32> = (0..EFFECTS)
            .filter(|_| sandboxed)
            .map(|index| {
                host.instance_by_key(&format!("fx{index}"))
                    .unwrap()
                    .bridge()
                    .unwrap()
                    .process_id()
            })
            .collect();

        let mut input = AudioBuffer::new(2, 4096);
        input.channel_mut(0)[..FRAMES_PER_BLOCK].fill(0.5);
        input.channel_mut(1)[..FRAMES_PER_BLOCK].fill(0.5);
        let mut outputs = vec![AudioBuffer::new(2, 4096)];
        let transport = TransportInfo {
            block_size: FRAMES_PER_BLOCK,
            ..TransportInfo::stopped(48_000.0)
        };
        let period = std::time::Duration::from_secs_f64(FRAMES_PER_BLOCK as f64 / 48_000.0);
        let bridges_before: f64 = process_ids.iter().map(|id| process_cpu(*id)).sum();
        let thread_before = current_thread_cpu();
        let started = std::time::Instant::now();
        let mut late = 0;
        for block in 0..BLOCK_COUNT {
            let deadline = started + period * (block as u32 + 1);
            for node in &mut nodes {
                node.process(&[&input], &mut outputs, &transport);
            }
            if std::time::Instant::now() > deadline {
                late += 1;
            }
            while std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
        }
        let wall = started.elapsed().as_secs_f64();
        let thread = (current_thread_cpu() - thread_before) / wall * 100.0;
        let bridges = (process_ids.iter().map(|id| process_cpu(*id)).sum::<f64>() - bridges_before)
            / wall
            * 100.0;
        println!(
            "{EFFECTS} VST3 effects {:<10}: audio thread {thread:5.1} % of a core, bridges {bridges:5.1} % of a core, {late} late block(s) of {BLOCK_COUNT}",
            if sandboxed { "sandboxed" } else { "in-process" }
        );
        assert!(!sandboxed || process_ids.iter().all(|id| *id != 0));
        assert!(host.unavailable_plugins().is_empty());
    }
}

/// ADR-033 step 5, open question 2: a real VST3 effect (TDR Nova, when installed;
/// `ECHOES_TEST_VST3` names another) loads in the bridge, whose main thread is an OLE
/// apartment and whose audio thread has none, and renders the samples it renders in the
/// Echoes process.
/// The first installed VST3 effect (`ECHOES_TEST_VST3` points at one), or `None`.
fn installed_vst3_effect() -> Option<PluginManifest> {
    use crate::plugin_formats::vst3::host::Vst3Host;
    use crate::plugin_system::ffi_lifecycle_tests::discover_all;
    use crate::plugin_system::registry::PluginRegistry;

    let mut registry = PluginRegistry::new();
    for path in discover_all("ECHOES_TEST_VST3", "vst3") {
        Vst3Host::new().register_bundle(&path, &mut registry);
    }
    let manifest = registry
        .all()
        .find(|m| m.format == DeviceFormat::Vst3 && !m.is_instrument)
        .cloned();
    manifest
}

#[test]
fn a_real_vst3_effect_renders_in_the_bridge_like_in_the_echoes_process() {
    let Some(manifest) = installed_vst3_effect() else {
        eprintln!("[sandbox] SKIP a real VST3 effect in the bridge: none installed");
        return;
    };

    let render = |sandboxed: bool| -> Vec<f32> {
        let mut host = PluginHost::new();
        host.set_bridge_exe(artifacts().0.clone());
        host.registry_mut().add(manifest.clone());
        let mut node = host
            .instantiate(KEY, &manifest.id, sandboxed)
            .expect("the effect starts");
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 512,
            num_channels: 2,
            live: false,
        });
        assert_eq!(host.instance_by_key(KEY).unwrap().is_sandboxed(), sandboxed);
        let transport = TransportInfo {
            block_size: 512,
            ..TransportInfo::stopped(48_000.0)
        };
        let mut rendered = Vec::new();
        for block in 0..64 {
            let mut input = AudioBuffer::new(2, 512);
            for channel in 0..2 {
                for (index, sample) in input.channel_mut(channel).iter_mut().enumerate() {
                    *sample = 0.5 * ((block * 512 + index) as f32 * 0.05).sin();
                }
            }
            let mut outputs = vec![AudioBuffer::new(2, 512)];
            node.process(&[&input], &mut outputs, &transport);
            rendered.extend_from_slice(outputs[0].channel(0));
        }
        assert!(host.unavailable_plugins().is_empty());
        rendered
    };

    let in_process = render(false);
    let sandboxed = render(true);
    eprintln!(
        "[sandbox] {}: {} samples, in the bridge {}",
        manifest.name,
        in_process.len(),
        if in_process == sandboxed {
            "identical"
        } else {
            "DIFFERENT"
        }
    );
    assert!(in_process.iter().any(|s| s.abs() > 1e-3), "sound came out");
    assert_eq!(in_process, sandboxed);
}

// ── Parameters (ADR-033, step 6) ────────────────────────────────────────────
//
// Each test runs the plugin in the Echoes process and in the bridge and requires
// the same answer: the sandbox must not change what a parameter does.

const GAIN: u32 = 0;
const SELF_VALUE: u32 = 7;
const WRITE_OUTPUT: u32 = 8;
const HOLD_EDIT: u32 = 9;
const LOOKAHEAD: u32 = 10;
const RENAME: u32 = 11;
const PROCESSOR_SELF: u32 = 12;
const PARAMETER_COUNT: usize = 13;
const HELD_VALUE: f64 = 0.25;

fn plugin_value(host: &PluginHost, native_id: u32) -> f64 {
    host.instance_by_key(KEY)
        .unwrap()
        .param_value(native_id)
        .expect("the plugin reports the value")
}

fn modulated_control(native_id: u32, base: &Arc<AtomicU32>) -> PluginParamControl {
    let mut control = PluginParamControl::new(native_id, Arc::clone(base));
    control.modulated = true;
    control
}

fn base_of(base: &Arc<AtomicU32>) -> f32 {
    f32::from_bits(base.load(Ordering::Relaxed))
}

/// A block with `drive` applied to the node's modulation and automation first, as
/// the scheduler does.
fn run_driven_block(
    node: &mut Box<dyn AudioNode>,
    drive: impl FnOnce(&mut Box<dyn AudioNode>),
) -> Vec<f32> {
    node.reset_modulation();
    drive(node);
    run_block(node)
}

#[test]
fn the_parameter_list_is_the_same_in_process_and_sandboxed() {
    let mut in_process_host = host();
    let _in_process = prepared(&mut in_process_host, false, false);
    let mut sandboxed_host = host();
    let _sandboxed = prepared(&mut sandboxed_host, true, false);

    let in_process = in_process_host.plugin_params(KEY).unwrap();
    assert_eq!(in_process.len(), PARAMETER_COUNT);
    assert_eq!(sandboxed_host.plugin_params(KEY).unwrap(), in_process);
}

fn the_plugin_renders_its_own_text(sandboxed: bool) {
    let mut host = host();
    let _node = prepared(&mut host, sandboxed, false);
    assert_eq!(
        host.plugin_param_text(KEY, GAIN, 0.25).as_deref(),
        Some("0.50")
    );
    assert_eq!(
        host.plugin_param_text(KEY, GAIN, 1.0).as_deref(),
        Some("2.00")
    );
    assert_eq!(host.plugin_param_text(KEY, 999, 0.5), None);
    assert_eq!(host.plugin_param_text("unknown", GAIN, 0.5), None);
}

#[test]
fn an_in_process_vst3_plugin_renders_its_own_text() {
    the_plugin_renders_its_own_text(false);
}

#[test]
fn a_sandboxed_vst3_plugin_renders_its_own_text() {
    the_plugin_renders_its_own_text(true);
}

fn a_pushed_value_reaches_the_plugin(sandboxed: bool) {
    let mut host = host();
    let mut node = prepared(&mut host, sandboxed, false);
    assert!(run_block(&mut node).iter().all(|&s| s == 0.5));
    // Normalised 0.25 of the 0..2 range is a gain of 0.5.
    assert!(host.push_plugin_param(KEY, GAIN, 0.25).unwrap());
    assert!(run_block(&mut node).iter().all(|&s| s == 0.25));
    assert_eq!(plugin_value(&host, GAIN), 0.25);
}

#[test]
fn a_value_pushed_to_an_in_process_vst3_plugin_is_delivered() {
    a_pushed_value_reaches_the_plugin(false);
}

#[test]
fn a_value_pushed_to_a_sandboxed_vst3_plugin_is_delivered() {
    a_pushed_value_reaches_the_plugin(true);
}

fn automation_and_modulation_move_the_plugin(sandboxed: bool) {
    let mut host = host();
    let mut node = prepared(&mut host, sandboxed, false);
    let base = Arc::new(AtomicU32::new(0.9f32.to_bits()));
    node.inject_plugin_param_controls(vec![modulated_control(GAIN, &base)]);
    assert_eq!(
        base_of(&base),
        0.5,
        "the base starts at the value of the plugin"
    );

    // Modulation moves around the base: 0.5 + 0.25 of 0..2 is a gain of 1.5.
    let modulated = run_driven_block(&mut node, |node| node.apply_modulation_at(GAIN, 0, 0.25));
    assert!(modulated.iter().all(|&s| s == 0.75));
    // Automation sets the value outright and wins on the same parameter.
    let automated = run_driven_block(&mut node, |node| {
        node.apply_modulation_at(GAIN, 0, 0.25);
        node.apply_automation_at(GAIN, 0, 0.25);
    });
    assert!(automated.iter().all(|&s| s == 0.25));

    // A point inside the block applies at its sample offset.
    let mut input = AudioBuffer::new(2, 4096);
    for channel in 0..2 {
        input.channel_mut(channel)[..FRAMES].fill(0.5);
    }
    node.reset_modulation();
    node.apply_automation_at(GAIN, 0, 0.25);
    node.apply_automation_at(GAIN, 32, 1.0);
    let mut outputs = vec![AudioBuffer::new(2, 4096)];
    let transport = TransportInfo {
        block_size: FRAMES,
        ..TransportInfo::stopped(48_000.0)
    };
    node.process(&[&input], &mut outputs, &transport);
    let output = &outputs[0].channel(0)[..FRAMES];
    assert!(output[..32].iter().all(|&s| s == 0.25));
    assert!(output[32..].iter().all(|&s| s == 1.0));
}

#[test]
fn automation_and_modulation_move_an_in_process_vst3_plugin() {
    automation_and_modulation_move_the_plugin(false);
}

#[test]
fn automation_and_modulation_move_a_sandboxed_vst3_plugin_the_same_way() {
    automation_and_modulation_move_the_plugin(true);
}

fn the_base_is_put_back_before_the_state_is_saved(sandboxed: bool) {
    let mut host = host();
    let mut node = prepared(&mut host, sandboxed, false);
    let base = Arc::new(AtomicU32::new(0));
    node.inject_plugin_param_controls(vec![modulated_control(GAIN, &base)]);

    run_driven_block(&mut node, |node| node.apply_modulation_at(GAIN, 0, 0.25));
    assert_eq!(
        plugin_value(&host, GAIN),
        0.75,
        "the plugin sits on a point of the modulation"
    );
    assert_eq!(
        saved_state(&host),
        Some(gain_state(1.0)),
        "the state holds the setting of the user: a normalised 0.5 of 0..2"
    );
}

#[test]
fn an_in_process_vst3_state_is_saved_on_the_base_of_a_modulated_parameter() {
    the_base_is_put_back_before_the_state_is_saved(false);
}

#[test]
fn a_sandboxed_vst3_state_is_saved_on_the_base_of_a_modulated_parameter() {
    the_base_is_put_back_before_the_state_is_saved(true);
}

fn a_restart_request_is_applied_by_the_next_recompile(sandboxed: bool) {
    let mut host = host();
    let mut node = prepared(&mut host, sandboxed, true);
    let instance_id = host.instance_by_key(KEY).unwrap().instance_id();
    assert_eq!(node.latency_samples(), LATENCY);

    assert!(host.push_plugin_param(KEY, LOOKAHEAD, 1.0).unwrap());
    run_block(&mut node);

    let mut recompiled = prepared(&mut host, sandboxed, true);
    assert_eq!(
        recompiled.latency_samples(),
        128,
        "the latency is read again"
    );
    assert_eq!(
        host.instance_by_key(KEY).unwrap().instance_id(),
        instance_id,
        "same instance, reactivated"
    );
    assert!(run_block(&mut recompiled).iter().all(|&s| s == 0.5));
}

#[test]
fn an_in_process_vst3_latency_change_is_applied_by_the_next_recompile() {
    a_restart_request_is_applied_by_the_next_recompile(false);
}

#[test]
fn a_sandboxed_vst3_latency_change_is_applied_by_the_next_recompile() {
    a_restart_request_is_applied_by_the_next_recompile(true);
}

/// One test for both modes: the rescan flag is process-wide.
#[test]
fn new_parameter_titles_reach_the_app_in_process_and_sandboxed() {
    for sandboxed in [false, true] {
        let mut host = host();
        let mut node = prepared(&mut host, sandboxed, false);
        let title = |host: &PluginHost| {
            host.plugin_params(KEY)
                .unwrap()
                .into_iter()
                .find(|param| param.native_id == SELF_VALUE)
                .unwrap()
                .name
        };
        assert_eq!(title(&host), "Self value");
        take_plugin_params_rescan();

        assert!(host.push_plugin_param(KEY, RENAME, 1.0).unwrap());
        run_block(&mut node);
        assert_eq!(
            take_plugin_params_rescan(),
            CLAP_PARAM_RESCAN_TEXT | CLAP_PARAM_RESCAN_INFO,
            "sandboxed: {sandboxed}"
        );
        assert_eq!(take_plugin_params_rescan(), 0, "reported once");
        assert_eq!(title(&host), "Renamed value");
    }
}

/// One test for both modes: the unsaved flag is process-wide.
#[test]
fn an_edit_in_the_plugin_window_marks_the_project_modified_and_moves_the_base() {
    for sandboxed in [false, true] {
        let mut host = host();
        let mut node = prepared(&mut host, sandboxed, false);
        let base = Arc::new(AtomicU32::new(0));
        node.inject_plugin_param_controls(vec![modulated_control(SELF_VALUE, &base)]);

        take_plugin_state_edited();
        assert!(host.push_plugin_param(KEY, WRITE_OUTPUT, 1.0).unwrap());
        run_block(&mut node);
        run_block(&mut node);
        assert!(
            !take_plugin_state_edited(),
            "a value the processor writes by itself is not an edit (sandboxed: {sandboxed})"
        );

        assert!(host.push_plugin_param(KEY, HOLD_EDIT, 1.0).unwrap());
        run_block(&mut node);
        run_block(&mut node);
        assert!(
            take_plugin_state_edited(),
            "the project reads as modified (sandboxed: {sandboxed})"
        );
        assert_eq!(
            f64::from(base_of(&base)),
            HELD_VALUE,
            "the base follows the edit"
        );
    }
}

fn modulation_pauses_while_the_plugin_window_drags(sandboxed: bool) {
    let mut host = host();
    let mut node = prepared(&mut host, sandboxed, false);
    let base = Arc::new(AtomicU32::new(0));
    node.inject_plugin_param_controls(vec![modulated_control(SELF_VALUE, &base)]);
    let modulate = |node: &mut Box<dyn AudioNode>| {
        run_driven_block(node, |node| node.apply_modulation_at(SELF_VALUE, 0, 0.2));
    };

    assert!(host.push_plugin_param(KEY, HOLD_EDIT, 1.0).unwrap());
    run_block(&mut node); // the plugin starts to drag
    run_block(&mut node); // the engine hears of it
    modulate(&mut node);
    assert_eq!(plugin_value(&host, PROCESSOR_SELF), HELD_VALUE, "paused");

    assert!(host.push_plugin_param(KEY, HOLD_EDIT, 0.0).unwrap());
    run_block(&mut node); // the plugin lets go
    run_block(&mut node); // the engine hears of it
    modulate(&mut node);
    let resumed = plugin_value(&host, PROCESSOR_SELF);
    assert!(
        (resumed - 0.45).abs() < 1e-6,
        "base 0.25 + 0.2, got {resumed}"
    );
}

#[test]
fn a_drag_in_an_in_process_vst3_window_pauses_the_modulation() {
    modulation_pauses_while_the_plugin_window_drags(false);
}

#[test]
fn a_drag_in_a_sandboxed_vst3_window_pauses_the_modulation() {
    modulation_pauses_while_the_plugin_window_drags(true);
}

/// What a real effect shows of its parameters, in the Echoes process or in the bridge:
/// the list, the text of a value, and the samples it renders while `automated` follows
/// a ramp.
struct RealEffectRun {
    params: Vec<crate::plugin_system::parameter::PluginParamInfo>,
    text: Option<String>,
    rendered: Vec<f32>,
}

fn run_real_effect(
    manifest: &PluginManifest,
    sandboxed: bool,
    automated: Option<u32>,
    text_of: Option<u32>,
) -> RealEffectRun {
    const BLOCK: usize = 512;
    let mut host = PluginHost::new();
    host.set_bridge_exe(artifacts().0.clone());
    host.registry_mut().add(manifest.clone());
    let mut node = host
        .instantiate(KEY, &manifest.id, sandboxed)
        .expect("the effect starts");
    node.prepare(PrepareContext {
        sample_rate: 48_000.0,
        max_block_size: BLOCK,
        num_channels: 2,
        live: false,
    });
    assert_eq!(host.instance_by_key(KEY).unwrap().is_sandboxed(), sandboxed);
    if let Some(native_id) = automated {
        let base = Arc::new(AtomicU32::new(0.5f32.to_bits()));
        node.inject_plugin_param_controls(vec![PluginParamControl::new(native_id, base)]);
    }
    let transport = TransportInfo {
        block_size: BLOCK,
        ..TransportInfo::stopped(48_000.0)
    };
    let mut rendered = Vec::new();
    for block in 0..64 {
        let mut input = AudioBuffer::new(2, BLOCK);
        for channel in 0..2 {
            for (index, sample) in input.channel_mut(channel).iter_mut().enumerate() {
                *sample = 0.5 * ((block * BLOCK + index) as f32 * 0.05).sin();
            }
        }
        node.reset_modulation();
        if let Some(native_id) = automated {
            node.apply_automation_at(native_id, 0, block as f32 / 63.0);
        }
        let mut outputs = vec![AudioBuffer::new(2, BLOCK)];
        node.process(&[&input], &mut outputs, &transport);
        rendered.extend_from_slice(outputs[0].channel(0));
    }
    assert!(host.unavailable_plugins().is_empty());
    RealEffectRun {
        params: host.plugin_params(KEY).expect("the parameter list"),
        text: text_of.and_then(|native_id| host.plugin_param_text(KEY, native_id, 0.5)),
        rendered,
    }
}

/// ADR-033 step 6 with a real plugin (skipped when none is installed): the parameter
/// list, the text of a value and the sound of an automated parameter are the same in
/// the bridge as in the Echoes process.
#[test]
fn a_real_vst3_effect_lists_and_follows_its_parameters_in_the_bridge_like_in_the_echoes_process() {
    let Some(manifest) = installed_vst3_effect() else {
        eprintln!(
            "[sandbox] SKIP the parameters of a real VST3 effect in the bridge: none installed"
        );
        return;
    };
    let plain = run_real_effect(&manifest, false, None, None);
    let candidates: Vec<u32> = plain
        .params
        .iter()
        .filter(|param| param.automatable && !param.hidden)
        .map(|param| param.native_id)
        .take(24)
        .collect();
    let Some((native_id, in_process)) = candidates.iter().find_map(|&native_id| {
        let run = run_real_effect(&manifest, false, Some(native_id), Some(native_id));
        (run.rendered != plain.rendered).then_some((native_id, run))
    }) else {
        eprintln!(
            "[sandbox] SKIP {}: no automated parameter among {} changed the sound",
            manifest.name,
            candidates.len()
        );
        return;
    };

    let sandboxed = run_real_effect(&manifest, true, Some(native_id), Some(native_id));
    eprintln!(
        "[sandbox] {}: {} parameters, parameter {native_id} automated, text {:?}",
        manifest.name,
        sandboxed.params.len(),
        sandboxed.text
    );
    assert_eq!(sandboxed.params, in_process.params, "the same list");
    assert_eq!(sandboxed.text, in_process.text, "the same text");
    assert!(
        in_process.rendered == sandboxed.rendered,
        "the automated parameter must sound the same in the bridge"
    );
}
