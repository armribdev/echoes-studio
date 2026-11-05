//! A VST3 instrument in the sandbox (ADR-033, step 8): the VST3 test instrument
//! through the host, in the bridge process and in the Echoes process.  What the
//! offline render proves bit for bit (`render_harness::vst3_plugins`) is not repeated;
//! these cover what a render cannot: the isolation preference, a bridge that dies, a
//! bridge that cannot start, and a real third-party instrument.
//!
//! The test instrument plays `velocity × gain` from the sample of its note on to the
//! sample of its note off.

use std::collections::HashSet;
use std::ffi::c_void;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::{MidiEvent, MidiEventKind, MidiEventList};
use crate::audio_graph::node::{PrepareContext, TransportInfo};
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::plugin_formats::vst3::test_dll::{instrument_id, test_vst3_dll};
use crate::plugin_system::crash_marker;
use crate::plugin_system::host::{PluginHost, PluginIsolation, UnavailableReason};
use crate::plugin_system::registry::PluginManifest;
use crate::plugin_system::wrapper::DeviceFormat;

use super::sandbox_tests::artifacts;
use super::vst3_sandbox_tests::{current_thread_cpu, installed_vst3, process_cpu};

const KEY: &str = "instrument:midi";
const FRAMES: usize = 64;
const LATENCY: u32 = 64;

fn manifest() -> PluginManifest {
    PluginManifest {
        id: instrument_id(),
        name: "Echoes Test Synth".into(),
        vendor: "Echoes Studio".into(),
        format: DeviceFormat::Vst3,
        path: Some(test_vst3_dll().clone()),
        categories: vec![],
        num_sidechain_inputs: 0,
        is_instrument: true,
    }
}

fn host() -> PluginHost {
    let mut host = PluginHost::new();
    host.set_bridge_exe(artifacts().0.clone());
    host.registry_mut().add(manifest());
    host
}

fn prepared(host: &mut PluginHost, isolation: PluginIsolation) -> Box<dyn InstrumentDevice> {
    host.set_isolation(isolation, HashSet::new());
    let mut device = host
        .instantiate_instrument(KEY, &instrument_id())
        .expect("the instrument starts");
    device.prepare(PrepareContext {
        sample_rate: 48_000.0,
        max_block_size: 4096,
        num_channels: 2,
        live: false,
    });
    device
}

fn is_sandboxed(host: &PluginHost) -> bool {
    host.instance_by_key(KEY).unwrap().is_sandboxed()
}

fn play(device: &mut Box<dyn InstrumentDevice>, kinds: &[(u32, MidiEventKind)]) -> Vec<f32> {
    let mut events = MidiEventList::new();
    for &(offset, kind) in kinds {
        events.push(MidiEvent { offset, kind });
    }
    let mut outputs = vec![AudioBuffer::new(2, 4096)];
    let transport = TransportInfo {
        block_size: FRAMES,
        ..TransportInfo::stopped(48_000.0)
    };
    device.process_midi(&events, &mut outputs, &transport);
    outputs[0].channel(0)[..FRAMES].to_vec()
}

fn note_on(offset: u32) -> (u32, MidiEventKind) {
    (
        offset,
        MidiEventKind::NoteOn {
            pitch: 60,
            velocity: 127,
        },
    )
}

fn note_off(offset: u32) -> (u32, MidiEventKind) {
    (offset, MidiEventKind::NoteOff { pitch: 60 })
}

#[test]
fn a_sandboxed_vst3_instrument_plays_its_notes_and_reports_latency() {
    let mut host = host();
    let mut device = prepared(&mut host, PluginIsolation::All);
    assert!(is_sandboxed(&host));
    assert_eq!(device.latency_samples(), LATENCY);

    let first = play(&mut device, &[note_on(8)]);
    assert!(first[..8].iter().all(|&s| s == 0.0));
    assert!(first[8..].iter().all(|&s| s == 1.0), "the note holds");
    let second = play(&mut device, &[note_off(4)]);
    assert!(second[..4].iter().all(|&s| s == 1.0));
    assert!(second[4..].iter().all(|&s| s == 0.0), "the note ends");
    assert!(host.unavailable_plugins().is_empty());
}

#[test]
fn the_isolation_preference_decides_for_a_vst3_instrument() {
    let mut host = host();
    prepared(&mut host, PluginIsolation::Chosen);
    assert!(!is_sandboxed(&host), "Chosen: not isolated unless asked");

    prepared(&mut host, PluginIsolation::All);
    assert!(is_sandboxed(&host), "All: isolated");

    prepared(&mut host, PluginIsolation::Off);
    assert!(!is_sandboxed(&host), "Off: in the Echoes process");
}

#[test]
fn a_reset_releases_the_note_of_an_instrument_in_the_bridge_as_in_process() {
    for isolation in [PluginIsolation::Off, PluginIsolation::All] {
        let mut host = host();
        let mut device = prepared(&mut host, isolation);
        assert!(play(&mut device, &[note_on(0)]).iter().all(|&s| s == 1.0));

        device.reset();
        assert!(
            play(&mut device, &[]).iter().all(|&s| s == 0.0),
            "{isolation:?}: the next block releases the note"
        );
    }
}

#[test]
fn a_killed_bridge_of_an_instrument_is_contained_reported_and_recovered_on_recompile() {
    let mut host = host();
    let mut device = prepared(&mut host, PluginIsolation::All);
    assert!(play(&mut device, &[note_on(0)]).iter().all(|&s| s == 1.0));
    let crashes_before = crash_marker::crash_count();

    kill(bridge_process_id(&host));
    let mut dead = false;
    for _ in 0..40 {
        let output = play(&mut device, &[]);
        assert!(
            output.iter().all(|&s| s == 0.0 || s == 1.0),
            "silence or the plugin's sound, never garbage"
        );
        if host
            .instance_by_key(KEY)
            .unwrap()
            .bridge()
            .unwrap()
            .is_dead()
        {
            dead = true;
            break;
        }
    }
    assert!(dead, "the bridge was never condemned");
    assert!(crash_marker::crash_count() > crashes_before);
    assert_eq!(
        host.unavailable_plugins()[0].reason,
        UnavailableReason::Crashed
    );

    let mut restarted = prepared(&mut host, PluginIsolation::All);
    assert!(is_sandboxed(&host));
    assert!(play(&mut restarted, &[note_on(0)])
        .iter()
        .all(|&s| s == 1.0));
}

#[test]
fn a_vst3_instrument_that_cannot_start_in_the_sandbox_is_reported_unavailable() {
    let mut host = PluginHost::new();
    host.registry_mut().add(manifest());
    host.set_bridge_exe(std::path::PathBuf::from("does-not-exist.exe"));
    host.set_isolation(PluginIsolation::All, HashSet::new());

    assert!(host.instantiate_instrument(KEY, &instrument_id()).is_err());
    assert!(
        host.instance_by_key(KEY).is_none(),
        "never loaded in-process instead"
    );
    assert_eq!(host.unavailable_plugins().len(), 1);
}

/// ADR-033 step 8: a real VST3 instrument (Surge XT, when installed;
/// `ECHOES_TEST_VST3` names the file inside its bundle) plays in the bridge as it plays
/// in the Echoes process.  Surge XT is not deterministic (two runs in the same process
/// differ), so the proof is a sound of the same loudness, not the same samples; the
/// test instrument carries the bit-for-bit proof.
#[test]
fn a_real_vst3_instrument_plays_in_the_bridge_like_in_the_echoes_process() {
    let Some(manifest) = installed_vst3(|manifest| manifest.is_instrument) else {
        eprintln!("[sandbox] SKIP a real VST3 instrument in the bridge: none installed");
        return;
    };

    let render = |isolation: PluginIsolation| -> Vec<f32> {
        let mut host = PluginHost::new();
        host.set_bridge_exe(artifacts().0.clone());
        host.registry_mut().add(manifest.clone());
        host.set_isolation(isolation, HashSet::new());
        let mut device = host
            .instantiate_instrument(KEY, &manifest.id)
            .expect("the instrument starts");
        device.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 512,
            num_channels: 2,
            live: false,
        });
        assert_eq!(
            host.instance_by_key(KEY).unwrap().is_sandboxed(),
            isolation == PluginIsolation::All
        );
        let transport = TransportInfo {
            is_playing: true,
            block_size: 512,
            ..TransportInfo::stopped(48_000.0)
        };
        let mut rendered = Vec::new();
        for block in 0..96 {
            let mut events = MidiEventList::new();
            if block == 0 {
                events.push(MidiEvent {
                    offset: 0,
                    kind: MidiEventKind::NoteOn {
                        pitch: 60,
                        velocity: 100,
                    },
                });
            }
            if block == 48 {
                events.push(MidiEvent {
                    offset: 0,
                    kind: MidiEventKind::NoteOff { pitch: 60 },
                });
            }
            let mut outputs = vec![AudioBuffer::new(2, 512)];
            device.process_midi(&events, &mut outputs, &transport);
            rendered.extend_from_slice(outputs[0].channel(0));
        }
        assert!(host.unavailable_plugins().is_empty());
        rendered
    };
    let loudness = |samples: &[f32]| {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    };

    let in_process = loudness(&render(PluginIsolation::Off));
    let sandboxed = loudness(&render(PluginIsolation::All));
    eprintln!(
        "[sandbox] {}: loudness (RMS) in the Echoes process {in_process:.4}, in the bridge {sandboxed:.4}",
        manifest.name
    );
    assert!(in_process > 1e-3, "sound came out in process");
    assert!(sandboxed > 1e-3, "sound came out of the bridge");
    assert!(
        (0.5..2.0).contains(&(sandboxed / in_process)),
        "the bridge plays about as loud"
    );
}

/// CPU of several VST3 instruments at the pace of the live callback (ADR-033, "Real-time
/// risks": more cores in the live wait), as `measure_eight_vst3_effects` does for effects:
/// the audio thread's CPU and the bridges' CPU, as a share of one core.
/// `cargo test --bin echoes-studio measure_eight_vst3_instruments -- --ignored --nocapture`.
#[test]
#[ignore = "timing measure, prints its figures"]
fn measure_eight_vst3_instruments() {
    const FRAMES_PER_BLOCK: usize = 512;
    const BLOCK_COUNT: usize = 500;
    const INSTRUMENTS: usize = 8;

    for isolation in [PluginIsolation::Off, PluginIsolation::All] {
        let mut host = host();
        host.set_isolation(isolation, HashSet::new());
        let mut devices: Vec<Box<dyn InstrumentDevice>> = (0..INSTRUMENTS)
            .map(|index| {
                let mut device = host
                    .instantiate_instrument(&format!("instrument{index}"), &instrument_id())
                    .expect("the instrument starts");
                device.prepare(PrepareContext {
                    sample_rate: 48_000.0,
                    max_block_size: 4096,
                    num_channels: 2,
                    live: true,
                });
                device
            })
            .collect();
        let process_ids: Vec<u32> = (0..INSTRUMENTS)
            .filter(|_| isolation == PluginIsolation::All)
            .map(|index| {
                host.instance_by_key(&format!("instrument{index}"))
                    .unwrap()
                    .bridge()
                    .unwrap()
                    .process_id()
            })
            .collect();

        let mut outputs = vec![AudioBuffer::new(2, 4096)];
        let transport = TransportInfo {
            is_playing: true,
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
            let mut events = MidiEventList::new();
            if block == 0 {
                events.push(MidiEvent {
                    offset: 0,
                    kind: note_on(0).1,
                });
            }
            for device in &mut devices {
                device.process_midi(&events, &mut outputs, &transport);
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
            "{INSTRUMENTS} VST3 instruments {:<10}: audio thread {thread:5.1} % of a core, bridges {bridges:5.1} % of a core, {late} late block(s) of {BLOCK_COUNT}",
            if isolation == PluginIsolation::All { "sandboxed" } else { "in-process" }
        );
        assert!(host.unavailable_plugins().is_empty());
    }
}

// ── Win32 helpers (a bridge killed from outside, as from the Task Manager) ──────

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut c_void;
    fn TerminateProcess(process: *mut c_void, exit_code: u32) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

const PROCESS_TERMINATE: u32 = 0x0001;

fn bridge_process_id(host: &PluginHost) -> u32 {
    host.instance_by_key(KEY)
        .unwrap()
        .bridge()
        .unwrap()
        .process_id()
}

fn kill(process_id: u32) {
    // SAFETY: plain Win32 calls on a process this test started.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, process_id);
        assert!(!handle.is_null(), "open the bridge process");
        assert_ne!(TerminateProcess(handle, 1), 0);
        CloseHandle(handle);
    }
}
