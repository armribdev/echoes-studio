//! Sandboxed plugins end to end (ADR-015): `PluginHost` → the real
//! `plugin_host_bridge` → the real test CLAP plugin (`crates/test_clap_plugin`).
//! Both are built into their own target directory on first use.

use std::collections::{HashMap, HashSet};
use std::ffi::c_char;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::{MidiEvent, MidiEventKind, MidiEventList};
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::plugin_formats::clap::ffi::{
    take_plugin_params_rescan, ClapPluginParams, CLAP_EXT_PARAMS, CLAP_PARAM_RESCAN_INFO,
    CLAP_PARAM_RESCAN_TEXT,
};
use crate::plugin_formats::clap::params::flush_clap_param_values;
use crate::plugin_system::host::{PluginHost, PluginIsolation, UnavailableReason};
use crate::plugin_system::registry::PluginManifest;
use crate::plugin_system::wrapper::DeviceFormat;

const PLUGIN_ID: &str = "clap:echoes.test.gain";
const SLEEPY_PLUGIN_ID: &str = "clap:echoes.test.sleepy";
const KEY: &str = "fx0";
const INSTRUMENT_KEY: &str = "instrument:t1";
const GAIN: u32 = 0;
const CRASH: u32 = 2;
const LOOKAHEAD: u32 = 4;
const WRONG_THREAD: u32 = 5;
const STATUS: u32 = 6;
const TAIL: u32 = 7;
const PROCESS_CALLS: u32 = 8;
const RESCAN: u32 = 10;
const REQUEST_FLUSH: u32 = 11;
const TAIL_RANGE: f32 = 1_000_000.0;
const FRAMES: usize = 64;
const NOTE_ON: MidiEventKind = MidiEventKind::NoteOn {
    pitch: 60,
    velocity: 127,
};
const NOTE_OFF: MidiEventKind = MidiEventKind::NoteOff { pitch: 60 };

/// `(bridge exe, test plugin dll)`, built once per test run.
fn artifacts() -> &'static (PathBuf, PathBuf) {
    static ARTIFACTS: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    ARTIFACTS.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = workspace.join("target").join("sandbox-tests");
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "--quiet",
                "-p",
                "plugin_host_bridge",
                "-p",
                "test_clap_plugin",
                "--target-dir",
            ])
            .arg(&target)
            .current_dir(workspace)
            .status()
            .expect("run cargo");
        assert!(
            status.success(),
            "building the sandbox test artifacts failed"
        );
        let debug = target.join("debug");
        (
            debug.join("plugin_host_bridge.exe"),
            debug.join("test_clap_plugin.dll"),
        )
    })
}

fn host() -> PluginHost {
    let (bridge_exe, dll) = artifacts();
    let mut host = PluginHost::new();
    host.set_bridge_exe(bridge_exe.clone());
    for id in [PLUGIN_ID, SLEEPY_PLUGIN_ID] {
        host.registry_mut().add(PluginManifest {
            id: id.into(),
            name: "Echoes Test Gain".into(),
            vendor: "Echoes Studio".into(),
            format: DeviceFormat::Clap,
            path: Some(dll.clone()),
            categories: vec![],
            num_sidechain_inputs: 0,
            is_instrument: false,
        });
    }
    host
}

fn prepared(host: &mut PluginHost, sandboxed: bool, live: bool) -> Box<dyn AudioNode> {
    let mut node = host
        .instantiate(KEY, PLUGIN_ID, sandboxed)
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
    run_block_with_input(node, 0.5)
}

fn run_block_with_input(node: &mut Box<dyn AudioNode>, level: f32) -> Vec<f32> {
    let mut input = AudioBuffer::new(2, 4096);
    for channel in 0..2 {
        input.channel_mut(channel)[..FRAMES].fill(level);
    }
    let mut outputs = vec![AudioBuffer::new(2, 4096)];
    let transport = TransportInfo {
        block_size: FRAMES,
        ..TransportInfo::stopped(48_000.0)
    };
    node.process(&[&input], &mut outputs, &transport);
    outputs[0].channel(0)[..FRAMES].to_vec()
}

fn prepared_instrument(host: &mut PluginHost, live: bool) -> Box<dyn InstrumentDevice> {
    prepared_instrument_of(host, PLUGIN_ID, live)
}

fn prepared_instrument_of(
    host: &mut PluginHost,
    plugin_id: &str,
    live: bool,
) -> Box<dyn InstrumentDevice> {
    let mut device = host
        .instantiate_instrument(INSTRUMENT_KEY, plugin_id)
        .expect("instrument starts");
    device.prepare(PrepareContext {
        sample_rate: 48_000.0,
        max_block_size: 4096,
        num_channels: 2,
        live,
    });
    device
}

fn play_block(device: &mut Box<dyn InstrumentDevice>, notes: &[MidiEventKind]) -> Vec<f32> {
    let mut events = MidiEventList::new();
    for &kind in notes {
        events.push(MidiEvent { offset: 0, kind });
    }
    let mut outputs = vec![AudioBuffer::new(2, 4096)];
    let transport = TransportInfo {
        block_size: FRAMES,
        ..TransportInfo::stopped(48_000.0)
    };
    device.process_midi(&events, &mut outputs, &transport);
    outputs[0].channel(0)[..FRAMES].to_vec()
}

fn is_sandboxed(host: &PluginHost, key: &str) -> bool {
    host.instance_by_key(key).unwrap().is_sandboxed()
}

fn saved_state(host: &PluginHost) -> Option<Vec<u8>> {
    host.collect_plugin_states()
        .into_iter()
        .find(|(key, _)| key == KEY)
        .map(|(_, bytes)| bytes)
}

fn gain_state(gain: f64) -> Vec<u8> {
    gain.to_bits().to_le_bytes().to_vec()
}

#[test]
fn sandboxed_effect_processes_parameters_and_reports_latency() {
    let mut host = host();
    let mut node = prepared(&mut host, true, false);
    assert!(host.instance_by_key(KEY).unwrap().is_sandboxed());
    assert_eq!(node.latency_samples(), 64);
    assert_eq!(host.plugin_params(KEY).unwrap().len(), 12);

    assert!(run_block(&mut node).iter().all(|&s| s == 0.5));
    // Normalised 0.25 of the 0..2 range = gain 0.5.
    assert!(host.push_plugin_param(KEY, GAIN, 0.25).unwrap());
    assert!(run_block(&mut node).iter().all(|&s| s == 0.25));
}

/// The plugin switches its latency and requests a restart from `process`; the
/// recompile that follows reactivates the same instance and reads the new value.
fn a_latency_change_is_applied_by_the_next_recompile(sandboxed: bool) {
    let mut host = host();
    let mut node = prepared(&mut host, sandboxed, true);
    let instance_id = host.instance_by_key(KEY).unwrap().instance_id();
    assert_eq!(node.latency_samples(), 64);

    assert!(host.push_plugin_param(KEY, LOOKAHEAD, 1.0).unwrap());
    run_block(&mut node);

    let mut recompiled = prepared(&mut host, sandboxed, true);
    assert_eq!(recompiled.latency_samples(), 128);
    assert_eq!(
        host.instance_by_key(KEY).unwrap().instance_id(),
        instance_id,
        "same instance, reactivated"
    );
    assert!(
        run_block(&mut recompiled).iter().all(|&s| s == 0.5),
        "still processing"
    );
}

#[test]
fn an_in_process_latency_change_is_applied_by_the_next_recompile() {
    a_latency_change_is_applied_by_the_next_recompile(false);
}

#[test]
fn a_sandboxed_latency_change_is_applied_by_the_next_recompile() {
    a_latency_change_is_applied_by_the_next_recompile(true);
}

/// Audio-thread calls the test plugin saw on a thread `clap.thread-check` did
/// not report as the audio thread.
fn wrong_thread_calls(host: &PluginHost) -> f64 {
    plugin_value(host, KEY, WRONG_THREAD)
}

/// How many times `process` ran on the in-process plugin placed at `key`.
fn process_calls(host: &PluginHost, key: &str) -> u32 {
    plugin_value(host, key, PROCESS_CALLS) as u32
}

fn plugin_value(host: &PluginHost, key: &str, native_id: u32) -> f64 {
    let plugin = host
        .instance_by_key(key)
        .unwrap()
        .clap_plugin()
        .expect("in-process CLAP plugin");
    // SAFETY: live instance held by the host; no block is in flight.
    unsafe {
        let params = ((*plugin).get_extension)(plugin, CLAP_EXT_PARAMS.as_ptr() as *const c_char)
            as *const ClapPluginParams;
        let mut value = f64::NAN;
        assert!(((*params).get_value)(plugin, native_id, &mut value));
        value
    }
}

/// Sets plain values the way a plugin window would: outside `process`.
fn flush_plugin_values(host: &PluginHost, key: &str, values: &[(u32, f64)]) {
    let instance = host.instance_by_key(key).unwrap();
    match instance.bridge() {
        Some(bridge) => bridge.flush_values(values),
        // SAFETY: live instance held by the host; no block is in flight.
        None => unsafe { flush_clap_param_values(instance.clap_plugin().unwrap(), values) },
    }
}

fn the_plugin_renders_its_own_text(sandboxed: bool) {
    let mut host = host();
    let _node = prepared(&mut host, sandboxed, false);

    assert_eq!(
        host.plugin_param_text(KEY, GAIN, 0.5).as_deref(),
        Some("-6.0 dB")
    );
    assert_eq!(
        host.plugin_param_text(KEY, GAIN, 2.0).as_deref(),
        Some("6.0 dB")
    );
    assert_eq!(host.plugin_param_text(KEY, TAIL, 10.0), None);
    assert_eq!(host.plugin_param_text("unknown", GAIN, 0.5), None);
}

#[test]
fn an_in_process_plugin_renders_its_own_text() {
    the_plugin_renders_its_own_text(false);
}

#[test]
fn a_sandboxed_plugin_renders_its_own_text() {
    the_plugin_renders_its_own_text(true);
}

/// One test for both modes: the rescan flags are process-wide.
#[test]
fn a_parameter_rescan_reaches_the_app_in_process_and_sandboxed() {
    let mut host = host();
    take_plugin_params_rescan();

    let _in_process = prepared(&mut host, false, false);
    flush_plugin_values(&host, KEY, &[(RESCAN, f64::from(CLAP_PARAM_RESCAN_TEXT))]);
    assert_eq!(take_plugin_params_rescan(), CLAP_PARAM_RESCAN_TEXT);
    assert_eq!(take_plugin_params_rescan(), 0, "reported once");

    let mut sandboxed = prepared(&mut host, true, false);
    flush_plugin_values(&host, KEY, &[(RESCAN, f64::from(CLAP_PARAM_RESCAN_INFO))]);
    run_block(&mut sandboxed);
    assert_eq!(take_plugin_params_rescan(), CLAP_PARAM_RESCAN_INFO);
}

fn a_flush_request_wakes_a_sleeping_instrument(sandboxed: bool) {
    let mut host = host();
    if sandboxed {
        host.set_isolation(PluginIsolation::All, HashSet::new());
    }
    let mut instrument = prepared_instrument_of(&mut host, SLEEPY_PLUGIN_ID, false);
    assert_eq!(is_sandboxed(&host, INSTRUMENT_KEY), sandboxed);
    let calls = || {
        host.instance_by_key(INSTRUMENT_KEY)
            .unwrap()
            .clap_param_value(PROCESS_CALLS)
            .unwrap() as u32
    };

    play_block(&mut instrument, &[NOTE_ON]);
    play_block(&mut instrument, &[NOTE_OFF]);
    for _ in 0..5 {
        play_block(&mut instrument, &[]);
    }
    let calls_when_asleep = calls();

    flush_plugin_values(&host, INSTRUMENT_KEY, &[(REQUEST_FLUSH, 1.0)]);
    assert_eq!(
        calls(),
        calls_when_asleep,
        "the request alone does not process"
    );
    play_block(&mut instrument, &[]);
    assert_eq!(
        calls(),
        calls_when_asleep + 1,
        "the next block runs the plugin"
    );
    play_block(&mut instrument, &[]);
    assert_eq!(calls(), calls_when_asleep + 1, "and it falls asleep again");
}

#[test]
fn a_flush_request_wakes_a_sleeping_in_process_instrument() {
    a_flush_request_wakes_a_sleeping_instrument(false);
}

#[test]
fn a_flush_request_wakes_a_sleeping_sandboxed_instrument() {
    a_flush_request_wakes_a_sleeping_instrument(true);
}

#[test]
fn stop_processing_off_the_audio_callback_is_seen_as_the_audio_thread() {
    let mut host = host();
    let mut node = prepared(&mut host, false, true);
    assert!(host.push_plugin_param(KEY, LOOKAHEAD, 1.0).unwrap());
    // Blocks run on their own thread, like the audio callback.
    std::thread::scope(|scope| {
        scope.spawn(|| run_block(&mut node));
    });

    // The restart request makes this recompile stop and reactivate the plugin
    // from the command thread.
    let _recompiled = prepared(&mut host, false, true);
    assert_eq!(wrong_thread_calls(&host), 0.0);
}

#[test]
fn a_recompile_reuses_the_running_bridge() {
    let mut host = host();
    let _first = prepared(&mut host, true, true);
    let before = host.instance_by_key(KEY).unwrap().instance_id();
    let _second = prepared(&mut host, true, true);
    assert_eq!(host.instance_by_key(KEY).unwrap().instance_id(), before);
}

#[test]
fn state_is_saved_and_restored_through_the_sandbox() {
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
    assert!(!host.instance_by_key(KEY).unwrap().is_sandboxed());
    assert!(run_block(&mut in_process).iter().all(|&s| s == 0.25));

    let mut sandboxed_again = prepared(&mut host, true, false);
    assert!(host.instance_by_key(KEY).unwrap().is_sandboxed());
    assert!(run_block(&mut sandboxed_again).iter().all(|&s| s == 0.25));
}

#[test]
fn a_crash_is_contained_reported_and_recovered_on_recompile() {
    let mut host = host();
    host.set_pending_state_blobs(HashMap::from([(KEY.to_string(), gain_state(0.5))]));
    let mut node = prepared(&mut host, true, true);
    host.clear_pending_state_blobs();
    let crashes_before = crate::plugin_system::crash_marker::crash_count();

    assert!(host.push_plugin_param(KEY, CRASH, 1.0).unwrap());
    for _ in 0..20 {
        let output = run_block(&mut node);
        assert!(output.iter().all(|&s| s == 0.0 || s == 0.25));
    }
    assert!(host
        .instance_by_key(KEY)
        .unwrap()
        .bridge()
        .unwrap()
        .is_dead());
    assert!(crate::plugin_system::crash_marker::crash_count() > crashes_before);
    let unavailable = host.unavailable_plugins();
    assert_eq!(unavailable.len(), 1);
    assert_eq!(unavailable[0].reason, UnavailableReason::Crashed);

    let mut restarted = prepared(&mut host, true, false);
    assert!(!host
        .instance_by_key(KEY)
        .unwrap()
        .bridge()
        .unwrap()
        .is_dead());
    assert!(
        run_block(&mut restarted).iter().all(|&s| s == 0.25),
        "restarted with its last known state"
    );
    host.retain_devices(&HashSet::from([KEY.to_string()]));
    assert!(host.unavailable_plugins().is_empty());
}

#[test]
fn the_isolation_preference_overrides_the_effect_choice() {
    let mut host = host();
    let is_sandboxed = |host: &PluginHost| host.instance_by_key(KEY).unwrap().is_sandboxed();

    prepared(&mut host, false, false);
    assert!(!is_sandboxed(&host), "Chosen: follows the effect");

    let always = HashSet::from([PLUGIN_ID.to_string()]);
    assert!(host.set_isolation(PluginIsolation::Chosen, always.clone()));
    prepared(&mut host, false, false);
    assert!(
        is_sandboxed(&host),
        "Chosen: a plugin marked always isolated is isolated everywhere"
    );

    assert!(host.set_isolation(PluginIsolation::All, HashSet::new()));
    prepared(&mut host, false, false);
    assert!(is_sandboxed(&host), "All: isolated without being chosen");

    assert!(host.set_isolation(PluginIsolation::Off, always.clone()));
    prepared(&mut host, true, false);
    assert!(!is_sandboxed(&host), "Off: in-process even when chosen");
    assert!(
        !host.set_isolation(PluginIsolation::Off, always),
        "unchanged settings need no recompile"
    );
}

#[test]
fn a_sandboxed_instrument_plays_notes_and_parameters() {
    let mut host = host();
    host.set_isolation(PluginIsolation::All, HashSet::new());
    let mut instrument = prepared_instrument(&mut host, false);
    assert!(is_sandboxed(&host, INSTRUMENT_KEY));

    assert!(
        play_block(&mut instrument, &[]).iter().all(|&s| s == 0.0),
        "silent until a note"
    );
    assert!(play_block(&mut instrument, &[NOTE_ON])
        .iter()
        .all(|&s| s == 1.0));
    // Normalised 0.25 of the 0..2 range = gain 0.5.
    assert!(host.push_plugin_param(INSTRUMENT_KEY, GAIN, 0.25).unwrap());
    assert!(
        play_block(&mut instrument, &[]).iter().all(|&s| s == 0.5),
        "the note is still held"
    );
    assert!(play_block(&mut instrument, &[NOTE_OFF])
        .iter()
        .all(|&s| s == 0.0));
}

#[test]
fn an_instrument_follows_the_isolation_preference_and_keeps_its_state() {
    let mut host = host();
    host.set_pending_state_blobs(HashMap::from([(
        INSTRUMENT_KEY.to_string(),
        gain_state(0.5),
    )]));
    let _in_process = prepared_instrument(&mut host, false);
    host.clear_pending_state_blobs();
    assert!(
        !is_sandboxed(&host, INSTRUMENT_KEY),
        "Chosen: an instrument has no choice of its own"
    );

    let always = HashSet::from([PLUGIN_ID.to_string()]);
    host.set_isolation(PluginIsolation::Chosen, always.clone());
    let mut sandboxed = prepared_instrument(&mut host, false);
    assert!(
        is_sandboxed(&host, INSTRUMENT_KEY),
        "Chosen: a plugin always isolated is isolated as an instrument"
    );
    assert!(
        play_block(&mut sandboxed, &[NOTE_ON])
            .iter()
            .all(|&s| s == 0.5),
        "state carried into the sandbox"
    );

    host.set_isolation(PluginIsolation::Off, always);
    let mut in_process = prepared_instrument(&mut host, false);
    assert!(!is_sandboxed(&host, INSTRUMENT_KEY), "Off: in-process");
    assert!(
        play_block(&mut in_process, &[NOTE_ON])
            .iter()
            .all(|&s| s == 0.5),
        "state carried back"
    );
}

#[test]
fn a_crashed_instrument_is_contained_reported_and_restarted() {
    let mut host = host();
    host.set_isolation(PluginIsolation::All, HashSet::new());
    host.set_pending_state_blobs(HashMap::from([(
        INSTRUMENT_KEY.to_string(),
        gain_state(0.5),
    )]));
    let mut instrument = prepared_instrument(&mut host, true);
    host.clear_pending_state_blobs();

    assert!(host.push_plugin_param(INSTRUMENT_KEY, CRASH, 1.0).unwrap());
    for _ in 0..20 {
        let output = play_block(&mut instrument, &[NOTE_ON]);
        assert!(output.iter().all(|&s| s == 0.0 || s == 0.5));
    }
    assert!(host
        .instance_by_key(INSTRUMENT_KEY)
        .unwrap()
        .bridge()
        .unwrap()
        .is_dead());
    let unavailable = host.unavailable_plugins();
    assert_eq!(unavailable.len(), 1);
    assert_eq!(unavailable[0].reason, UnavailableReason::Crashed);

    let mut restarted = prepared_instrument(&mut host, false);
    assert!(host.unavailable_plugins().is_empty());
    assert!(
        play_block(&mut restarted, &[NOTE_ON])
            .iter()
            .all(|&s| s == 0.5),
        "restarted with its last known state"
    );
}

#[test]
fn a_sleeping_instrument_is_not_processed_and_plays_again_on_the_next_note() {
    let mut host = host();
    let mut instrument = prepared_instrument_of(&mut host, SLEEPY_PLUGIN_ID, false);

    assert!(play_block(&mut instrument, &[NOTE_ON])
        .iter()
        .all(|&s| s == 1.0));
    assert!(play_block(&mut instrument, &[NOTE_OFF])
        .iter()
        .all(|&s| s == 0.0));
    let calls_when_asleep = process_calls(&host, INSTRUMENT_KEY);
    assert_eq!(calls_when_asleep, 2);

    for _ in 0..10 {
        assert!(play_block(&mut instrument, &[]).iter().all(|&s| s == 0.0));
    }
    assert_eq!(
        process_calls(&host, INSTRUMENT_KEY),
        calls_when_asleep,
        "no process call while asleep"
    );

    assert!(play_block(&mut instrument, &[NOTE_ON])
        .iter()
        .all(|&s| s == 1.0));
    assert_eq!(process_calls(&host, INSTRUMENT_KEY), calls_when_asleep + 1);
}

#[test]
fn an_effect_tail_is_processed_in_full_before_the_plugin_sleeps() {
    const TAIL_SAMPLES: usize = 200;
    let mut host = host();
    let mut node = prepared(&mut host, false, false);
    assert!(host.push_plugin_param(KEY, STATUS, 3.0 / 4.0).unwrap());
    assert!(host
        .push_plugin_param(KEY, TAIL, TAIL_SAMPLES as f32 / TAIL_RANGE)
        .unwrap());

    run_block_with_input(&mut node, 0.0);
    let mut calls = Vec::new();
    for _ in 0..20 {
        run_block_with_input(&mut node, 0.0);
        calls.push(process_calls(&host, KEY));
    }

    let quiet_blocks_needed = TAIL_SAMPLES.div_ceil(FRAMES);
    assert!(
        calls[quiet_blocks_needed - 1] as usize > quiet_blocks_needed,
        "still processed through the tail: {calls:?}"
    );
    assert_eq!(
        calls[10], calls[19],
        "asleep once the tail has ended: {calls:?}"
    );

    let asleep_calls = calls[19];
    assert!(
        run_block_with_input(&mut node, 0.25)
            .iter()
            .all(|&s| s == 0.25),
        "a signal wakes the effect"
    );
    assert_eq!(process_calls(&host, KEY), asleep_calls + 1);
}

#[test]
fn an_effect_without_clap_tail_never_sleeps_on_a_tail_status() {
    let mut host = host();
    let mut node = prepared(&mut host, false, false);
    assert!(host.push_plugin_param(KEY, STATUS, 3.0 / 4.0).unwrap());

    for _ in 0..30 {
        run_block_with_input(&mut node, 0.0);
    }
    assert_eq!(process_calls(&host, KEY), 30);
}

#[test]
fn a_sandboxed_sleeping_instrument_still_plays_the_next_note() {
    let mut host = host();
    host.set_isolation(PluginIsolation::All, HashSet::new());
    let mut instrument = prepared_instrument_of(&mut host, SLEEPY_PLUGIN_ID, false);
    assert!(is_sandboxed(&host, INSTRUMENT_KEY));

    assert!(play_block(&mut instrument, &[NOTE_ON])
        .iter()
        .all(|&s| s == 1.0));
    assert!(play_block(&mut instrument, &[NOTE_OFF])
        .iter()
        .all(|&s| s == 0.0));
    for _ in 0..10 {
        assert!(play_block(&mut instrument, &[]).iter().all(|&s| s == 0.0));
    }
    assert!(play_block(&mut instrument, &[NOTE_ON])
        .iter()
        .all(|&s| s == 1.0));
}
