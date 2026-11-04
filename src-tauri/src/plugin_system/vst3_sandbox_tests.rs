//! A VST3 effect in the sandbox (ADR-033, step 5): the VST3 test effect through the
//! host, in the bridge process and in the Echoes process.  The mirror of the effect
//! tests of `sandbox_tests.rs`.
//!
//! The test effect's state is its gain, so a state blob makes the processing audible.
//! The plugin cannot be crashed or hung through a parameter yet (parameters of a
//! sandboxed VST3 plugin come with step 6): a crash is the bridge killed from outside,
//! as from the Task Manager, and a hang is the bridge frozen.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};
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
#[test]
fn a_real_vst3_effect_renders_in_the_bridge_like_in_the_echoes_process() {
    use crate::plugin_formats::vst3::host::Vst3Host;
    use crate::plugin_system::ffi_lifecycle_tests::discover_all;
    use crate::plugin_system::registry::PluginRegistry;

    let mut registry = PluginRegistry::new();
    for path in discover_all("ECHOES_TEST_VST3", "vst3") {
        Vst3Host::new().register_bundle(&path, &mut registry);
    }
    let Some(manifest) = registry
        .all()
        .find(|m| m.format == DeviceFormat::Vst3 && !m.is_instrument)
        .cloned()
    else {
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
