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
use crate::plugin_formats::clap::ffi::{ClapPluginParams, CLAP_EXT_PARAMS};
use crate::plugin_system::host::{PluginHost, PluginIsolation, UnavailableReason};
use crate::plugin_system::registry::PluginManifest;
use crate::plugin_system::wrapper::DeviceFormat;

const PLUGIN_ID: &str = "clap:echoes.test.gain";
const KEY: &str = "fx0";
const INSTRUMENT_KEY: &str = "instrument:t1";
const GAIN: u32 = 0;
const CRASH: u32 = 2;
const LOOKAHEAD: u32 = 4;
const WRONG_THREAD: u32 = 5;
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
    host.registry_mut().add(PluginManifest {
        id: PLUGIN_ID.into(),
        name: "Echoes Test Gain".into(),
        vendor: "Echoes Studio".into(),
        format: DeviceFormat::Clap,
        path: Some(dll.clone()),
        categories: vec![],
        num_sidechain_inputs: 0,
        is_instrument: false,
    });
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

fn prepared_instrument(host: &mut PluginHost, live: bool) -> Box<dyn InstrumentDevice> {
    let mut device = host
        .instantiate_instrument(INSTRUMENT_KEY, PLUGIN_ID)
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
    assert_eq!(host.plugin_params(KEY).unwrap().len(), 6);

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
    let plugin = host
        .instance_by_key(KEY)
        .unwrap()
        .clap_plugin()
        .expect("in-process CLAP plugin");
    // SAFETY: live instance held by the host; no block is in flight.
    unsafe {
        let params = ((*plugin).get_extension)(plugin, CLAP_EXT_PARAMS.as_ptr() as *const c_char)
            as *const ClapPluginParams;
        let mut value = f64::NAN;
        assert!(((*params).get_value)(plugin, WRONG_THREAD, &mut value));
        value
    }
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
