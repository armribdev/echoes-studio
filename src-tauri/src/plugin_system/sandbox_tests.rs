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
use crate::plugin_formats::clap::ffi::TimerSchedule;
use crate::plugin_formats::clap::ffi::{
    take_plugin_note_names_changed, take_plugin_params_rescan, ClapPluginParams, HostTrackInfo,
    CLAP_EXT_PARAMS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_PORTS_RESCAN_ALL,
    CLAP_NOTE_PORTS_RESCAN_NAMES, CLAP_PARAM_RESCAN_INFO, CLAP_PARAM_RESCAN_TEXT,
    CLAP_PRESET_IS_FACTORY_CONTENT, CLAP_PRESET_IS_USER_CONTENT, CLAP_PRESET_LOCATION_FILE,
    CLAP_PRESET_LOCATION_PLUGIN,
};
use crate::plugin_formats::clap::params::flush_clap_param_values;
use crate::plugin_system::host::{PluginHost, PluginIsolation, PluginNoteName, UnavailableReason};
use crate::plugin_system::instance::take_undelivered_track_info;
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::presets::{list_presets, PluginPreset};
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
const TIMER_REGISTER: u32 = 22;
const TIMER_UNREGISTER: u32 = 23;
const TIMER_REGISTER_RESULT: u32 = 24;
const TIMER_LAST_ID: u32 = 25;
const TIMER_UNREGISTER_RESULT: u32 = 26;
const TIMER_CALLS: u32 = 27;
const TIMER_LAST_FIRED: u32 = 28;
const TIMER_AUDIO_CALLS: u32 = 29;
const PRESET_TONE: u32 = 32;
const PARAM_COUNT: usize = 33;
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

// ── clap.timer-support ─────────────────────────────────────────────────────────

const ACCEPTED: f64 = 1.0;
const REFUSED: f64 = 2.0;
const INVALID_ID: f64 = u32::MAX as f64;

/// The id the plugin placed at `key` got from `register_timer`, if it was accepted.
fn register_timer(host: &PluginHost, key: &str, period_ms: f64) -> Option<f64> {
    flush_plugin_values(host, key, &[(TIMER_REGISTER, period_ms)]);
    (seen(host, key, TIMER_REGISTER_RESULT) == ACCEPTED).then(|| seen(host, key, TIMER_LAST_ID))
}

fn unregister_timer(host: &PluginHost, key: &str, id: f64) -> bool {
    flush_plugin_values(host, key, &[(TIMER_UNREGISTER, id)]);
    seen(host, key, TIMER_UNREGISTER_RESULT) == ACCEPTED
}

fn the_plugin_registers_timers_and_a_zero_period_is_refused(sandboxed: bool) {
    let mut host = host();
    let _node = prepared(&mut host, sandboxed, false);

    let first = register_timer(&host, KEY, 33.0).expect("30 Hz is allowed");
    let second = register_timer(&host, KEY, 50.0).expect("a second timer");
    assert_ne!(first, second, "ids are distinct");
    assert!(first > 0.0 && second > 0.0 && first != INVALID_ID);

    assert_eq!(register_timer(&host, KEY, 0.0), None);
    assert_eq!(seen(&host, KEY, TIMER_REGISTER_RESULT), REFUSED);
    assert_eq!(seen(&host, KEY, TIMER_LAST_ID), INVALID_ID);

    assert!(unregister_timer(&host, KEY, first));
    assert!(!unregister_timer(&host, KEY, first), "already gone");
    assert!(!unregister_timer(&host, KEY, 12_345.0), "never registered");
    assert!(unregister_timer(&host, KEY, second));
}

#[test]
fn an_in_process_plugin_registers_timers_and_a_zero_period_is_refused() {
    the_plugin_registers_timers_and_a_zero_period_is_refused(false);
}

#[test]
fn a_sandboxed_plugin_registers_timers_and_a_zero_period_is_refused() {
    the_plugin_registers_timers_and_a_zero_period_is_refused(true);
}

fn after(start: std::time::Instant, millis: u64) -> std::time::Instant {
    start + std::time::Duration::from_millis(millis)
}

/// The clock is the test's: no wait, no window.
#[test]
fn an_in_process_timer_gets_on_timer_until_it_is_unregistered() {
    let mut host = host();
    let _node = prepared(&mut host, false, false);
    let instance = host.instance_by_key(KEY).unwrap();
    let first = register_timer(&host, KEY, 33.0).unwrap();
    let second = register_timer(&host, KEY, 50.0).unwrap();
    let start = std::time::Instant::now();
    let mut schedule = TimerSchedule::default();
    let calls = || seen(&host, KEY, TIMER_CALLS);

    assert_eq!(instance.run_due_timers(&mut schedule, start), 0);
    assert_eq!(instance.run_due_timers(&mut schedule, after(start, 32)), 0);
    assert_eq!(instance.run_due_timers(&mut schedule, after(start, 33)), 1);
    assert_eq!(seen(&host, KEY, TIMER_LAST_FIRED), first);
    assert_eq!(instance.run_due_timers(&mut schedule, after(start, 50)), 1);
    assert_eq!(seen(&host, KEY, TIMER_LAST_FIRED), second);
    assert_eq!(calls(), 2.0);

    assert!(unregister_timer(&host, KEY, first));
    assert_eq!(instance.run_due_timers(&mut schedule, after(start, 120)), 1);
    assert_eq!(seen(&host, KEY, TIMER_LAST_FIRED), second);
    assert_eq!(calls(), 3.0);

    assert!(unregister_timer(&host, KEY, second));
    assert_eq!(
        instance.run_due_timers(&mut schedule, after(start, 5_000)),
        0
    );
    assert_eq!(calls(), 3.0, "no timer, no call");
    assert_eq!(
        seen(&host, KEY, TIMER_AUDIO_CALLS),
        0.0,
        "never reported as the audio thread"
    );
}

#[test]
fn a_timer_registered_from_the_audio_thread_still_arrives_on_the_main_thread() {
    let mut host = host();
    let mut node = prepared(&mut host, false, true);
    let instance = host.instance_by_key(KEY).unwrap();

    // The plugin applies this event inside `process`: the audio thread calls
    // `register_timer`, which the spec reserves for the main thread.
    assert!(host
        .push_plugin_param(KEY, TIMER_REGISTER, 33.0 / 3_600_000.0)
        .unwrap());
    run_block(&mut node);
    assert_eq!(seen(&host, KEY, TIMER_REGISTER_RESULT), ACCEPTED);

    let start = std::time::Instant::now();
    let mut schedule = TimerSchedule::default();
    instance.run_due_timers(&mut schedule, start);
    assert_eq!(instance.run_due_timers(&mut schedule, after(start, 33)), 1);
    assert_eq!(seen(&host, KEY, TIMER_CALLS), 1.0);
    assert_eq!(seen(&host, KEY, TIMER_AUDIO_CALLS), 0.0);
}

#[test]
fn an_in_process_plugin_that_crashed_gets_no_more_timers() {
    let mut host = host();
    let _node = prepared(&mut host, false, false);
    let instance = host.instance_by_key(KEY).unwrap();
    register_timer(&host, KEY, 20.0).unwrap();
    let start = std::time::Instant::now();
    let mut schedule = TimerSchedule::default();
    instance.run_due_timers(&mut schedule, start);

    instance.set_lifecycle(LifecycleState::Error);
    assert_eq!(
        instance.run_due_timers(&mut schedule, after(start, 1_000)),
        0
    );
    assert_eq!(
        instance.clap_param_value(TIMER_CALLS),
        Some(0.0),
        "the plugin was not called"
    );
}

/// A timer never holds the FFI gate, so it never makes the audio thread skip a
/// block; it only waits for an off-RT caller (state save, `changed()`) to finish.
#[test]
fn a_timer_waits_for_an_off_rt_caller_and_runs_beside_a_block() {
    let mut host = host();
    let _node = prepared(&mut host, false, false);
    let instance = host.instance_by_key(KEY).unwrap();
    register_timer(&host, KEY, 20.0).unwrap();
    let start = std::time::Instant::now();
    let mut schedule = TimerSchedule::default();
    instance.run_due_timers(&mut schedule, start);

    {
        let _saving_state = instance
            .offrt_try_acquire(std::time::Duration::from_millis(1))
            .unwrap();
        assert_eq!(
            instance.run_due_timers(&mut schedule, after(start, 20)),
            0,
            "skipped, without waiting"
        );
    }
    assert_eq!(
        instance.run_due_timers(&mut schedule, after(start, 21)),
        1,
        "still due at the next pass"
    );

    let block_in_flight = instance.rt_try_enter().expect("the gate is free");
    assert_eq!(
        instance.run_due_timers(&mut schedule, after(start, 60)),
        1,
        "CLAP lets on_timer run beside process"
    );
    drop(block_in_flight);
    assert!(
        instance.rt_try_enter().is_some(),
        "the timer left the gate alone"
    );
}

/// The Win32 wiring: the editor window's `WM_TIMER` drives the plugin's timers.
/// Opens a real (stub) editor window on this thread and runs its message loop.
#[test]
fn an_open_in_process_editor_runs_the_timers_through_its_window() {
    use crate::plugin_system::gui::{self, test_support::pump_thread_messages};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let mut host = host();
    let _node = prepared(&mut host, false, false);
    let instance = host.instance_by_key(KEY).unwrap();
    let context = instance.clap_host_context().unwrap();
    let first = register_timer(&host, KEY, 20.0).unwrap();

    let hwnd = unsafe {
        gui::open_clap_editor(
            Arc::clone(&instance),
            "Timer test",
            KEY,
            Arc::new(Mutex::new(HashMap::new())),
        )
    }
    .expect("the editor opens");
    assert!(context.editor_open());

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while seen(&host, KEY, TIMER_CALLS) < 3.0 {
        assert!(std::time::Instant::now() < deadline, "no timer fired");
        pump_thread_messages(Duration::from_millis(10));
    }
    assert_eq!(seen(&host, KEY, TIMER_LAST_FIRED), first);
    assert_eq!(seen(&host, KEY, TIMER_AUDIO_CALLS), 0.0);

    assert!(unregister_timer(&host, KEY, first));
    let calls = seen(&host, KEY, TIMER_CALLS);
    pump_thread_messages(Duration::from_millis(150));
    assert_eq!(seen(&host, KEY, TIMER_CALLS), calls, "unregistered");

    let second = register_timer(&host, KEY, 20.0).unwrap();
    gui::post_close(hwnd);
    pump_thread_messages(Duration::from_millis(50));
    assert!(!context.editor_open(), "the window is gone");
    let calls = seen(&host, KEY, TIMER_CALLS);
    pump_thread_messages(Duration::from_millis(150));
    assert_eq!(
        seen(&host, KEY, TIMER_CALLS),
        calls,
        "timer {second} waits for the next editor"
    );
}

#[test]
fn a_sandboxed_timer_does_not_run_while_its_editor_is_closed() {
    let mut host = host();
    let _node = prepared(&mut host, true, false);
    register_timer(&host, KEY, 20.0).expect("accepted");

    std::thread::sleep(std::time::Duration::from_millis(250));
    assert_eq!(seen(&host, KEY, TIMER_CALLS), 0.0);
}

// ── clap.preset-discovery-factory ──────────────────────────────────────────────

const PRESET_DIRECTORY_VARIABLE: &str = "ECHOES_TEST_PRESET_DIR";
const PRESET_CRASH_VARIABLE: &str = "ECHOES_TEST_PRESET_CRASH";

/// The test plugin's provider reads the environment, which every test shares.
static PRESET_ENVIRONMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Holds the environment lock with no preset directory and no fault set.
fn quiet_preset_environment() -> std::sync::MutexGuard<'static, ()> {
    let guard = PRESET_ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var(PRESET_DIRECTORY_VARIABLE);
    std::env::remove_var(PRESET_CRASH_VARIABLE);
    guard
}

fn preset_names(presets: &[PluginPreset]) -> Vec<&str> {
    presets.iter().map(|preset| preset.name.as_str()).collect()
}

fn presets_of(host: &PluginHost) -> Vec<PluginPreset> {
    list_presets(&host.preset_source(KEY).expect("a CLAP placement has a source"))
}

fn scratch_directory(files: &[&str]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("echoes_presets_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    for file in files {
        std::fs::write(directory.join(file), b"x").unwrap();
    }
    directory
}

fn the_presets_of_a_plugin_are_listed(sandboxed: bool) {
    let _environment = quiet_preset_environment();
    let mut host = host();
    let _node = prepared(&mut host, sandboxed, false);

    let source = host.preset_source(KEY).unwrap();
    assert_eq!(source.library, artifacts().1);
    assert_eq!(source.clap_plugin_id, "echoes.test.gain");
    assert_eq!(host.preset_source("unknown"), None);

    let presets = list_presets(&source);
    assert_eq!(preset_names(&presets), ["Loud", "Quiet"]);
    let quiet = &presets[1];
    assert_eq!(quiet.load_key.as_deref(), Some("quiet"));
    assert_eq!(quiet.location_kind, CLAP_PRESET_LOCATION_PLUGIN);
    assert_eq!(quiet.location, None);
    assert_eq!(quiet.provider_id, "echoes.test.presets");
    assert_eq!(quiet.flags, CLAP_PRESET_IS_FACTORY_CONTENT);
    assert_eq!(quiet.creator.as_deref(), Some("Echoes"));
    assert_eq!(quiet.features, ["quiet"]);
}

#[test]
fn the_presets_of_an_in_process_plugin_are_listed() {
    the_presets_of_a_plugin_are_listed(false);
}

#[test]
fn the_presets_of_a_sandboxed_plugin_are_listed() {
    the_presets_of_a_plugin_are_listed(true);
}

#[test]
fn the_presets_in_a_user_directory_follow_the_bundled_ones() {
    let _environment = quiet_preset_environment();
    let directory = scratch_directory(&["b warm.testpreset", "a cold.TESTPRESET", "readme.txt"]);
    std::env::set_var(PRESET_DIRECTORY_VARIABLE, &directory);
    let mut host = host();
    let _node = prepared(&mut host, false, false);

    let presets = presets_of(&host);

    assert_eq!(preset_names(&presets), ["Loud", "Quiet", "a cold", "b warm"]);
    let warm = &presets[3];
    assert_eq!(warm.location_kind, CLAP_PRESET_LOCATION_FILE);
    assert!(warm.location.as_deref().unwrap().ends_with("b warm.testpreset"));
    assert_eq!(warm.load_key, None);
    assert_eq!(warm.flags, CLAP_PRESET_IS_USER_CONTENT);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_plugin_is_offered_only_the_presets_that_name_it() {
    let _environment = quiet_preset_environment();
    let mut host = host();
    let _node = host
        .instantiate(KEY, SLEEPY_PLUGIN_ID, false)
        .expect("plugin starts");

    assert_eq!(preset_names(&presets_of(&host)), ["Sleepy only"]);
}

#[test]
fn a_provider_that_faults_lists_nothing_and_the_next_listing_works() {
    let _environment = quiet_preset_environment();
    let mut host = host();
    let _node = prepared(&mut host, false, false);

    std::env::set_var(PRESET_CRASH_VARIABLE, "1");
    assert!(presets_of(&host).is_empty());

    std::env::remove_var(PRESET_CRASH_VARIABLE);
    assert_eq!(preset_names(&presets_of(&host)), ["Loud", "Quiet"]);
}

// ── clap.preset-load, from the host (ADR-031 step 2) ───────────────────────────

use crate::plugin_formats::clap::ffi::{take_plugin_state_edited, PresetRequest};

/// Lists the placement's presets and remembers them, as the app's command does.
fn offered_presets(host: &mut PluginHost) -> Vec<PluginPreset> {
    let source = host
        .preset_source(KEY)
        .expect("a CLAP placement has a source");
    let presets = list_presets(&source);
    host.remember_presets(&source, presets.clone());
    presets
}

fn request_named(presets: &[PluginPreset], name: &str) -> PresetRequest {
    presets
        .iter()
        .find(|preset| preset.name == name)
        .unwrap_or_else(|| panic!("no preset named {name}"))
        .request()
}

fn load_offered(host: &PluginHost, request: &PresetRequest) -> Result<(), String> {
    host.instance_for_preset(KEY, request)?.load_preset(request)
}

/// A user directory holding one preset file per `(name, text)`.
fn user_presets(files: &[(&str, &str)]) -> PathBuf {
    let directory = scratch_directory(&[]);
    for (name, text) in files {
        std::fs::write(directory.join(format!("{name}.testpreset")), text).unwrap();
    }
    std::env::set_var(PRESET_DIRECTORY_VARIABLE, &directory);
    directory
}

fn a_listed_preset_changes_the_sound_and_is_saved_with_the_state(sandboxed: bool) {
    let _environment = quiet_preset_environment();
    let mut host = host();
    let mut node = prepared(&mut host, sandboxed, false);
    let presets = offered_presets(&mut host);
    assert!(run_block(&mut node).iter().all(|&s| s == 0.5));

    load_offered(&host, &request_named(&presets, "Loud")).unwrap();
    assert_eq!(seen(&host, KEY, GAIN), 2.0);
    assert_eq!(seen(&host, KEY, PRESET_TONE), 0.9);
    assert!(run_block(&mut node).iter().all(|&s| s == 1.0));

    load_offered(&host, &request_named(&presets, "Quiet")).unwrap();
    assert!(run_block(&mut node).iter().all(|&s| s == 0.25));
    let saved = host.instance_by_key(KEY).unwrap().save_state_blob();
    assert_eq!(saved, Some(gain_state(0.5)));
}

#[test]
fn a_listed_preset_changes_the_sound_of_an_in_process_plugin() {
    a_listed_preset_changes_the_sound_and_is_saved_with_the_state(false);
}

#[test]
fn a_listed_preset_changes_the_sound_of_a_sandboxed_plugin() {
    a_listed_preset_changes_the_sound_and_is_saved_with_the_state(true);
}

#[test]
fn only_a_preset_the_plugin_offered_can_be_loaded() {
    let _environment = quiet_preset_environment();
    let mut host = host();
    let _node = prepared(&mut host, false, false);
    let guessed = PresetRequest {
        location_kind: CLAP_PRESET_LOCATION_PLUGIN,
        location: None,
        load_key: Some("loud".into()),
    };

    let refused = host.instance_for_preset(KEY, &guessed).err();
    assert_eq!(
        refused.as_deref(),
        Some("this preset is not offered by the plugin"),
        "nothing was listed yet"
    );

    let presets = offered_presets(&mut host);
    assert!(host.instance_for_preset(KEY, &guessed).is_ok());
    let other_key = PresetRequest {
        load_key: Some("louder".into()),
        ..guessed.clone()
    };
    let other_path = PresetRequest {
        location_kind: CLAP_PRESET_LOCATION_FILE,
        location: Some("C:\\anywhere\\else.testpreset".into()),
        load_key: None,
    };
    for request in [other_key, other_path] {
        assert!(host.instance_for_preset(KEY, &request).is_err());
    }
    assert_eq!(
        host.instance_for_preset("unknown", &request_named(&presets, "Loud"))
            .err()
            .as_deref(),
        Some("this plugin has no presets")
    );
}

fn a_plugin_that_refuses_a_preset_says_why(sandboxed: bool) {
    let _environment = quiet_preset_environment();
    let directory = user_presets(&[("broken", "not a preset")]);
    let mut host = host();
    let _node = prepared(&mut host, sandboxed, false);
    let request = request_named(&offered_presets(&mut host), "broken");

    let message = load_offered(&host, &request).unwrap_err();

    assert!(message.contains("unreadable preset"), "{message}");
    assert_eq!(
        seen(&host, KEY, GAIN),
        1.0,
        "a refused preset changes nothing"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_in_process_plugin_that_refuses_a_preset_says_why() {
    a_plugin_that_refuses_a_preset_says_why(false);
}

#[test]
fn a_sandboxed_plugin_that_refuses_a_preset_says_why() {
    a_plugin_that_refuses_a_preset_says_why(true);
}

fn a_loaded_preset_marks_the_project_unsaved_whatever_the_plugin_says(sandboxed: bool) {
    let _environment = quiet_preset_environment();
    let directory = user_presets(&[("warm", "0.75 0.2")]);
    let mut host = host();
    let _node = prepared(&mut host, sandboxed, false);
    let request = request_named(&offered_presets(&mut host), "warm");

    // The test plugin does not tell the host about a file preset. The unsaved flag
    // is process-wide and one other unit test reads it, so one load may lose the
    // race for it; fifty cannot all lose.
    let marked = (0..50).any(|_| {
        load_offered(&host, &request).unwrap();
        take_plugin_state_edited()
    });

    assert!(marked);
    assert_eq!(seen(&host, KEY, GAIN), 0.75);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_preset_marks_the_project_unsaved_in_process() {
    a_loaded_preset_marks_the_project_unsaved_whatever_the_plugin_says(false);
}

#[test]
fn a_preset_marks_the_project_unsaved_when_sandboxed() {
    a_loaded_preset_marks_the_project_unsaved_whatever_the_plugin_says(true);
}

#[test]
fn a_preset_load_holds_the_ffi_gate_until_it_is_done() {
    let _environment = quiet_preset_environment();
    let directory = user_presets(&[("slow", "1.5 0.3 400")]);
    let mut host = host();
    let _node = prepared(&mut host, false, false);
    let request = request_named(&offered_presets(&mut host), "slow");
    let instance = host.instance_by_key(KEY).unwrap();

    let mut audio_thread_was_kept_out = false;
    std::thread::scope(|scope| {
        let load = scope.spawn(|| instance.load_preset(&request));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !load.is_finished() && std::time::Instant::now() < deadline {
            audio_thread_was_kept_out |= instance.rt_try_enter().is_none();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        load.join().unwrap().unwrap();
    });

    assert!(
        audio_thread_was_kept_out,
        "the audio thread skips the plugin meanwhile"
    );
    assert!(
        instance.rt_try_enter().is_some(),
        "and gets it back afterwards"
    );
    assert_eq!(seen(&host, KEY, GAIN), 1.5);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_plugin_busy_in_a_block_refuses_a_load_after_a_short_wait() {
    let _environment = quiet_preset_environment();
    let mut host = host();
    let _node = prepared(&mut host, false, false);
    let request = request_named(&offered_presets(&mut host), "Loud");
    let instance = host.instance_by_key(KEY).unwrap();

    let started = std::time::Instant::now();
    let busy = {
        let _block_in_flight = instance.rt_try_enter().expect("the gate is free");
        instance.load_preset(&request)
    };

    assert_eq!(busy.unwrap_err(), "the plugin is busy, try again");
    assert!(started.elapsed() >= std::time::Duration::from_millis(40));
    assert_eq!(seen(&host, KEY, GAIN), 1.0);
    assert!(instance.load_preset(&request).is_ok());
}

fn the_base_of_a_value_modulated_parameter_follows_a_preset(sandboxed: bool) {
    use crate::audio_graph::plugin_params::{PluginParamRange, ValueModulatedParam};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    let _environment = quiet_preset_environment();
    let mut host = host();
    let _node = prepared(&mut host, sandboxed, false);
    let presets = offered_presets(&mut host);
    let base = Arc::new(AtomicU32::new(0.5f32.to_bits()));
    let range = PluginParamRange {
        min: 0.0,
        max: 1.0,
        modulatable: false,
        automatable: true,
    };
    host.instance_by_key(KEY)
        .unwrap()
        .replace_value_modulated_params(vec![ValueModulatedParam::new(
            PRESET_TONE,
            range,
            Arc::clone(&base),
        )]);
    let base_now = || f32::from_bits(base.load(Ordering::Relaxed));

    load_offered(&host, &request_named(&presets, "Loud")).unwrap();
    assert!((base_now() - 0.9).abs() < 1e-6, "{}", base_now());

    load_offered(&host, &request_named(&presets, "Quiet")).unwrap();
    assert!((base_now() - 0.1).abs() < 1e-6, "{}", base_now());
}

#[test]
fn the_base_of_a_value_modulated_parameter_follows_a_preset_in_process() {
    the_base_of_a_value_modulated_parameter_follows_a_preset(false);
}

#[test]
fn the_base_of_a_value_modulated_parameter_follows_a_preset_when_sandboxed() {
    the_base_of_a_value_modulated_parameter_follows_a_preset(true);
}

/// C-2: a project saved after a preset load renders like the preset itself.
fn the_state_saved_after_a_preset_renders_like_the_preset(sandboxed: bool) {
    let _environment = quiet_preset_environment();
    let mut host = host();
    let mut node = prepared(&mut host, sandboxed, false);
    let request = request_named(&offered_presets(&mut host), "Quiet");
    load_offered(&host, &request).unwrap();
    let saved = host
        .instance_by_key(KEY)
        .unwrap()
        .save_state_blob()
        .unwrap();
    let from_the_preset = run_block(&mut node);

    let mut reopened = self::host();
    let mut restored = prepared(&mut reopened, sandboxed, false);
    assert!(reopened
        .instance_by_key(KEY)
        .unwrap()
        .load_state_blob(&saved));

    assert_eq!(run_block(&mut restored), from_the_preset);
}

#[test]
fn the_state_saved_after_a_preset_renders_like_the_preset_in_process() {
    the_state_saved_after_a_preset_renders_like_the_preset(false);
}

#[test]
fn the_state_saved_after_a_preset_renders_like_the_preset_when_sandboxed() {
    the_state_saved_after_a_preset_renders_like_the_preset(true);
}

fn a_sleeping_plugin_wakes_for_a_preset(sandboxed: bool) {
    // Read through the parameter, which also reaches a sandboxed plugin.
    let calls = |host: &PluginHost| seen(host, KEY, PROCESS_CALLS) as u32;
    let _environment = quiet_preset_environment();
    let mut host = host();
    let mut node = prepared(&mut host, sandboxed, false);
    let request = request_named(&offered_presets(&mut host), "Loud");
    assert!(host.push_plugin_param(KEY, STATUS, 1.0).unwrap());
    for _ in 0..10 {
        run_block_with_input(&mut node, 0.0);
    }
    let asleep = calls(&host);
    run_block_with_input(&mut node, 0.0);
    assert_eq!(calls(&host), asleep, "asleep on silence");

    load_offered(&host, &request).unwrap();
    run_block_with_input(&mut node, 0.0);

    assert_eq!(calls(&host), asleep + 1, "woken by the preset");
}

#[test]
fn a_sleeping_in_process_plugin_wakes_for_a_preset() {
    a_sleeping_plugin_wakes_for_a_preset(false);
}

#[test]
fn a_sleeping_sandboxed_plugin_wakes_for_a_preset() {
    a_sleeping_plugin_wakes_for_a_preset(true);
}
