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
    take_plugin_note_names_changed, take_plugin_params_rescan, ClapPluginParams, HostTrackInfo,
    CLAP_EXT_PARAMS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_PORTS_RESCAN_ALL,
    CLAP_NOTE_PORTS_RESCAN_NAMES, CLAP_PARAM_RESCAN_INFO, CLAP_PARAM_RESCAN_TEXT,
};
use crate::plugin_formats::clap::params::flush_clap_param_values;
use crate::plugin_system::host::{PluginHost, PluginIsolation, PluginNoteName, UnavailableReason};
use crate::plugin_system::instance::take_undelivered_track_info;
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::registry::PluginManifest;
use crate::plugin_system::track_info::test_track;
use crate::plugin_system::wrapper::DeviceFormat;
use crate::project::AppProject;

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
const TRACK_FLAGS: u32 = 12;
const TRACK_COLOR: u32 = 13;
const TRACK_NAME: u32 = 14;
const TRACK_NAME_LENGTH: u32 = 15;
const TRACK_INFO_READS: u32 = 16;
const TRACK_INFO_CHANGES: u32 = 17;
const TRACK_INFO_AUDIO_CALLS: u32 = 18;
const NOTE_NAMES_SET: u32 = 19;
const NOTE_PORTS_RESCAN: u32 = 20;
const HOST_NOTE_DIALECTS: u32 = 21;
const PARAM_COUNT: usize = 22;
const HAS_NAME: f64 = 1.0;
const HAS_NAME_AND_COLOR: f64 = 3.0;
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
    assert_eq!(host.plugin_params(KEY).unwrap().len(), PARAM_COUNT);

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

// ── clap.track-info ────────────────────────────────────────────────────────────

/// Bytes `Track name` packs: the first 6 of the name, big-endian.
fn packed_name(name: &str) -> f64 {
    name.bytes()
        .take(6)
        .fold(0u64, |packed, byte| (packed << 8) | u64::from(byte)) as f64
}

fn argb(red: u8, green: u8, blue: u8) -> f64 {
    f64::from(u32::from_be_bytes([255, red, green, blue]))
}

/// What the plugin placed at `key` read from the host, in process or sandboxed.
fn seen(host: &PluginHost, key: &str, native_id: u32) -> f64 {
    host.instance_by_key(key)
        .unwrap()
        .clap_param_value(native_id)
        .unwrap()
}

fn tell(host: &mut PluginHost, key: &str, info: HostTrackInfo) {
    host.apply_track_infos(HashMap::from([(key.to_string(), info)]));
}

fn lead() -> HostTrackInfo {
    HostTrackInfo::new("Lead", Some([0x12, 0x34, 0x56]))
}

fn a_plugin_reads_its_track_name_and_color(sandboxed: bool) {
    let mut host = host();
    tell(&mut host, KEY, lead());
    let _node = prepared(&mut host, sandboxed, false);
    assert_eq!(is_sandboxed(&host, KEY), sandboxed);

    assert_eq!(seen(&host, KEY, TRACK_FLAGS), HAS_NAME_AND_COLOR);
    assert_eq!(seen(&host, KEY, TRACK_COLOR), argb(0x12, 0x34, 0x56));
    assert_eq!(seen(&host, KEY, TRACK_NAME), packed_name("Lead"));
    assert_eq!(seen(&host, KEY, TRACK_NAME_LENGTH), 4.0);
    assert_eq!(seen(&host, KEY, TRACK_INFO_READS), 1.0, "read in init");
    assert_eq!(
        seen(&host, KEY, TRACK_INFO_CHANGES),
        0.0,
        "born knowing its track: nothing changed"
    );
}

#[test]
fn an_in_process_plugin_reads_its_track_name_and_color() {
    a_plugin_reads_its_track_name_and_color(false);
}

#[test]
fn a_sandboxed_plugin_reads_its_track_name_and_color() {
    a_plugin_reads_its_track_name_and_color(true);
}

fn a_track_without_color_has_no_color_flag(sandboxed: bool) {
    let mut host = host();
    tell(&mut host, KEY, HostTrackInfo::new("Bass", None));
    let _node = prepared(&mut host, sandboxed, false);

    assert_eq!(seen(&host, KEY, TRACK_FLAGS), HAS_NAME);
    assert_eq!(seen(&host, KEY, TRACK_COLOR), 0.0);
    assert_eq!(seen(&host, KEY, TRACK_NAME), packed_name("Bass"));
}

#[test]
fn an_in_process_track_without_color_has_no_color_flag() {
    a_track_without_color_has_no_color_flag(false);
}

#[test]
fn a_sandboxed_track_without_color_has_no_color_flag() {
    a_track_without_color_has_no_color_flag(true);
}

fn a_placement_without_a_track_reads_nothing(sandboxed: bool) {
    let mut host = host();
    let _node = prepared(&mut host, sandboxed, false);

    assert_eq!(seen(&host, KEY, TRACK_INFO_READS), 0.0, "get answers false");
    assert_eq!(seen(&host, KEY, TRACK_FLAGS), 0.0);
}

#[test]
fn an_in_process_placement_without_a_track_reads_nothing() {
    a_placement_without_a_track_reads_nothing(false);
}

#[test]
fn a_sandboxed_placement_without_a_track_reads_nothing() {
    a_placement_without_a_track_reads_nothing(true);
}

fn a_change_of_track_reaches_the_plugin_once(sandboxed: bool) {
    let mut host = host();
    tell(&mut host, KEY, lead());
    let _node = prepared(&mut host, sandboxed, false);

    tell(
        &mut host,
        KEY,
        HostTrackInfo::new("Pad", Some([0x12, 0x34, 0x56])),
    );
    assert_eq!(seen(&host, KEY, TRACK_INFO_CHANGES), 1.0, "renamed");
    assert_eq!(seen(&host, KEY, TRACK_INFO_READS), 2.0, "read again");
    assert_eq!(seen(&host, KEY, TRACK_NAME), packed_name("Pad"));
    assert_eq!(seen(&host, KEY, TRACK_NAME_LENGTH), 3.0);

    tell(
        &mut host,
        KEY,
        HostTrackInfo::new("Pad", Some([0x12, 0x34, 0x56])),
    );
    assert_eq!(
        seen(&host, KEY, TRACK_INFO_CHANGES),
        1.0,
        "the same track is not news"
    );

    tell(
        &mut host,
        KEY,
        HostTrackInfo::new("Pad", Some([0x65, 0x43, 0x21])),
    );
    assert_eq!(seen(&host, KEY, TRACK_INFO_CHANGES), 2.0, "recolored");
    assert_eq!(seen(&host, KEY, TRACK_COLOR), argb(0x65, 0x43, 0x21));

    tell(&mut host, KEY, HostTrackInfo::new("Pad", None));
    assert_eq!(seen(&host, KEY, TRACK_INFO_CHANGES), 3.0, "color removed");
    assert_eq!(seen(&host, KEY, TRACK_FLAGS), HAS_NAME);
    assert_eq!(seen(&host, KEY, TRACK_COLOR), 0.0);

    let _recompiled = prepared(&mut host, sandboxed, false);
    assert_eq!(
        seen(&host, KEY, TRACK_INFO_CHANGES),
        3.0,
        "a recompile that reuses the plugin tells it nothing"
    );
    assert_eq!(seen(&host, KEY, TRACK_INFO_AUDIO_CALLS), 0.0);
}

#[test]
fn a_change_of_track_reaches_an_in_process_plugin_once() {
    a_change_of_track_reaches_the_plugin_once(false);
}

#[test]
fn a_change_of_track_reaches_a_sandboxed_plugin_once() {
    a_change_of_track_reaches_the_plugin_once(true);
}

fn an_instrument_reads_its_track_and_hears_changes(sandboxed: bool) {
    let mut host = host();
    if sandboxed {
        host.set_isolation(PluginIsolation::All, HashSet::new());
    }
    tell(&mut host, INSTRUMENT_KEY, lead());
    let _instrument = prepared_instrument(&mut host, false);
    assert_eq!(is_sandboxed(&host, INSTRUMENT_KEY), sandboxed);
    assert_eq!(seen(&host, INSTRUMENT_KEY, TRACK_NAME), packed_name("Lead"));

    tell(&mut host, INSTRUMENT_KEY, HostTrackInfo::new("Keys", None));
    assert_eq!(seen(&host, INSTRUMENT_KEY, TRACK_INFO_CHANGES), 1.0);
    assert_eq!(seen(&host, INSTRUMENT_KEY, TRACK_NAME), packed_name("Keys"));
    assert_eq!(seen(&host, INSTRUMENT_KEY, TRACK_FLAGS), HAS_NAME);
}

#[test]
fn an_in_process_instrument_reads_its_track_and_hears_changes() {
    an_instrument_reads_its_track_and_hears_changes(false);
}

#[test]
fn a_sandboxed_instrument_reads_its_track_and_hears_changes() {
    an_instrument_reads_its_track_and_hears_changes(true);
}

fn project_with_track_named(name: &str, color: Option<&str>) -> AppProject {
    let mut project = AppProject::new();
    project.tracks = vec![test_track("t1", name, color, &[KEY])];
    project
}

fn a_rename_in_the_project_reaches_the_plugins_of_the_track(sandboxed: bool) {
    let mut host = host();
    host.sync_track_info(&project_with_track_named("Lead", Some("#123456")));
    let _node = prepared(&mut host, sandboxed, false);
    assert_eq!(seen(&host, KEY, TRACK_NAME), packed_name("Lead"));
    assert_eq!(seen(&host, KEY, TRACK_COLOR), argb(0x12, 0x34, 0x56));

    host.sync_track_info(&project_with_track_named("Pad", Some("#123456")));
    assert_eq!(seen(&host, KEY, TRACK_INFO_CHANGES), 1.0);
    assert_eq!(seen(&host, KEY, TRACK_NAME), packed_name("Pad"));

    host.sync_track_info(&project_with_track_named("Pad", Some("#123456")));
    assert_eq!(
        seen(&host, KEY, TRACK_INFO_CHANGES),
        1.0,
        "an edit of something else tells the plugin nothing"
    );

    host.sync_track_info(&project_with_track_named("Pad", None));
    assert_eq!(seen(&host, KEY, TRACK_INFO_CHANGES), 2.0);
    assert_eq!(seen(&host, KEY, TRACK_FLAGS), HAS_NAME);
}

#[test]
fn a_rename_in_the_project_reaches_an_in_process_plugin() {
    a_rename_in_the_project_reaches_the_plugins_of_the_track(false);
}

#[test]
fn a_rename_in_the_project_reaches_a_sandboxed_plugin() {
    a_rename_in_the_project_reaches_the_plugins_of_the_track(true);
}

/// The undelivered flag is process-wide: only this test touches it.
#[test]
fn a_notification_missed_while_the_plugin_is_busy_is_retried() {
    let mut host = host();
    tell(&mut host, KEY, lead());
    let _node = prepared(&mut host, false, false);
    let instance = host.instance_by_key(KEY).unwrap();
    take_undelivered_track_info();

    {
        let _block_in_flight = instance
            .offrt_try_acquire(std::time::Duration::from_millis(1))
            .unwrap();
        tell(&mut host, KEY, HostTrackInfo::new("Pad", None));
        assert_eq!(plugin_value(&host, KEY, TRACK_INFO_CHANGES), 0.0, "busy");
        assert_eq!(
            plugin_value(&host, KEY, TRACK_NAME),
            packed_name("Lead"),
            "not told yet"
        );
    }
    assert!(take_undelivered_track_info(), "the app is asked to retry");

    host.notify_pending_track_info();
    assert_eq!(plugin_value(&host, KEY, TRACK_INFO_CHANGES), 1.0);
    assert_eq!(plugin_value(&host, KEY, TRACK_NAME), packed_name("Pad"));
    host.notify_pending_track_info();
    assert_eq!(
        plugin_value(&host, KEY, TRACK_INFO_CHANGES),
        1.0,
        "delivered once"
    );
    assert!(!take_undelivered_track_info());
}

#[test]
fn an_in_process_plugin_that_crashed_is_not_told_about_its_track() {
    let mut host = host();
    tell(&mut host, KEY, lead());
    let _node = prepared(&mut host, false, false);
    host.instance_by_key(KEY)
        .unwrap()
        .set_lifecycle(LifecycleState::Error);

    tell(&mut host, KEY, HostTrackInfo::new("Pad", None));
    host.notify_pending_track_info();
    assert_eq!(plugin_value(&host, KEY, TRACK_INFO_CHANGES), 0.0);
}

#[test]
fn a_sandboxed_plugin_that_crashed_is_skipped_without_waiting() {
    let mut host = host();
    tell(&mut host, KEY, lead());
    let mut node = prepared(&mut host, true, true);
    assert!(host.push_plugin_param(KEY, CRASH, 1.0).unwrap());
    for _ in 0..20 {
        run_block(&mut node);
    }
    let bridge_is_dead = || {
        host.instance_by_key(KEY)
            .unwrap()
            .bridge()
            .unwrap()
            .is_dead()
    };
    assert!(bridge_is_dead());

    let started = std::time::Instant::now();
    tell(&mut host, KEY, HostTrackInfo::new("Pad", None));
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "a dead bridge is not asked"
    );
}

// ── clap.note-ports and clap.note-name ─────────────────────────────────────────

fn kick_and_snare() -> Vec<PluginNoteName> {
    vec![
        PluginNoteName {
            name: "Kick".into(),
            port: -1,
            key: 36,
            channel: -1,
        },
        PluginNoteName {
            name: "Snare".into(),
            port: -1,
            key: 38,
            channel: -1,
        },
    ]
}

fn tom() -> Vec<PluginNoteName> {
    vec![PluginNoteName {
        name: "Tom".into(),
        port: 0,
        key: 41,
        channel: 9,
    }]
}

fn the_host_tells_the_plugin_which_note_dialects_it_sends(sandboxed: bool) {
    let mut host = host();
    let _node = prepared(&mut host, sandboxed, false);

    assert_eq!(
        seen(&host, KEY, HOST_NOTE_DIALECTS),
        f64::from(CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI)
    );
}

#[test]
fn the_host_tells_an_in_process_plugin_which_note_dialects_it_sends() {
    the_host_tells_the_plugin_which_note_dialects_it_sends(false);
}

#[test]
fn the_host_tells_a_sandboxed_plugin_which_note_dialects_it_sends() {
    the_host_tells_the_plugin_which_note_dialects_it_sends(true);
}

fn the_note_names_of_a_plugin_are_read(sandboxed: bool) {
    let mut host = host();
    let _node = prepared(&mut host, sandboxed, false);

    assert_eq!(host.plugin_note_names(KEY), Some(kick_and_snare()));
    assert_eq!(host.plugin_note_names("unknown"), None);

    flush_plugin_values(&host, KEY, &[(NOTE_NAMES_SET, 1.0)]);
    assert_eq!(host.plugin_note_names(KEY), Some(tom()));
}

#[test]
fn the_note_names_of_an_in_process_plugin_are_read() {
    the_note_names_of_a_plugin_are_read(false);
}

#[test]
fn the_note_names_of_a_sandboxed_plugin_are_read() {
    the_note_names_of_a_plugin_are_read(true);
}

/// One test for both modes: the flag is process-wide.
#[test]
fn a_change_of_note_names_reaches_the_app_in_process_and_sandboxed() {
    let mut host = host();
    take_plugin_note_names_changed();

    let _in_process = prepared(&mut host, false, false);
    assert!(!take_plugin_note_names_changed(), "nothing changed yet");
    flush_plugin_values(&host, KEY, &[(NOTE_NAMES_SET, 1.0)]);
    assert!(take_plugin_note_names_changed());
    assert!(!take_plugin_note_names_changed(), "reported once");
    flush_plugin_values(&host, KEY, &[(NOTE_NAMES_SET, 1.0)]);
    assert!(
        !take_plugin_note_names_changed(),
        "the plugin tells the host only when its names change"
    );

    let mut sandboxed = prepared(&mut host, true, false);
    flush_plugin_values(&host, KEY, &[(NOTE_NAMES_SET, 1.0)]);
    run_block(&mut sandboxed);
    assert!(take_plugin_note_names_changed());
    assert_eq!(host.plugin_note_names(KEY), Some(tom()));
}

#[test]
fn a_rescan_of_all_note_ports_makes_an_in_process_plugin_restart() {
    let mut host = host();
    let _node = prepared(&mut host, false, false);
    let instance = host.instance_by_key(KEY).unwrap();
    let context = instance.clap_host_context().unwrap();

    flush_plugin_values(
        &host,
        KEY,
        &[(NOTE_PORTS_RESCAN, f64::from(CLAP_NOTE_PORTS_RESCAN_NAMES))],
    );
    assert!(
        !context.take_restart_request(),
        "a renamed port is not news"
    );

    flush_plugin_values(
        &host,
        KEY,
        &[(NOTE_PORTS_RESCAN, f64::from(CLAP_NOTE_PORTS_RESCAN_ALL))],
    );
    assert!(context.take_restart_request());
}
