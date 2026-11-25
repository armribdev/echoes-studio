//! The real bridge binary hosting the real test CLAP plugin (ADR-015).
//! Windows-only: the shared region is implemented on Windows only.
#![cfg(windows)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use clap_ffi::{
    HostTrackInfo, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_PORTS_RESCAN_ALL,
    CLAP_NOTE_PORTS_RESCAN_NAMES,
};
use plugin_bridge::control::Op;
use plugin_bridge::protocol::{
    WireNote, WireParam, WireTransport, BLOCK_NOTE_NAMES_CHANGED, BLOCK_PARAMS_RESCAN_MASK,
    BLOCK_PARAMS_RESCAN_SHIFT, BLOCK_PLUGIN_EDITED, BLOCK_RESTART_REQUESTED, MIDI, NOTE_OFF,
    NOTE_ON, PARAM_GESTURE_END, PARAM_MODULATION, PARAM_VALUE, TRANSPORT_PLAYING,
};
use plugin_bridge::records::{
    Handshake, NoteNameRecord, ParamRecord, ParamUnit, PortDirection, PortRole, PresetRecord,
    PresetSource, RemotePageRecord, SpeakerLayout, TrackRecord, PARAM_AUTOMATABLE, PARAM_MODULATABLE,
};
use plugin_bridge::session::{
    BlockOutcome, BlockReply, BlockRequest, BridgeLayout, BridgeProcess,
    DEAD_AFTER_CONSECUTIVE_TIMEOUTS,
};

const GAIN: u32 = 0;
const HANG: u32 = 1;
const CRASH: u32 = 2;
const TOUCH: u32 = 3;
const LOOKAHEAD: u32 = 4;
const WRONG_THREAD: u32 = 5;
const STATUS: u32 = 6;
const TAIL: u32 = 7;
const PROCESS_CALLS: u32 = 8;
const LIAR: u32 = 9;
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
const PRESET_LOADED: u32 = 30;
const PRESET_ERROR: u32 = 31;
const PARAM_COUNT: u32 = 33;
const FRAMES: usize = 64;
const CHANNELS: usize = 2;
const TIMEOUT: Duration = Duration::from_secs(5);

/// Builds the test plugin into its own target directory, once per test binary.
fn test_plugin_dll() -> &'static Path {
    static DLL: OnceLock<PathBuf> = OnceLock::new();
    DLL.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = workspace.join("target").join("test-plugin");
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "test_clap_plugin", "--target-dir"])
            .arg(&target)
            .current_dir(&workspace)
            .status()
            .expect("run cargo");
        assert!(status.success(), "building the test plugin failed");
        target.join("debug").join("test_clap_plugin.dll")
    })
}

fn spawn(plugin_id: &str) -> std::io::Result<BridgeProcess> {
    spawn_with_track(plugin_id, None)
}

fn spawn_with_track(
    plugin_id: &str,
    track_info: Option<&HostTrackInfo>,
) -> std::io::Result<BridgeProcess> {
    let mut args: Vec<OsString> = vec![
        "--plugin-dll".into(),
        test_plugin_dll().into(),
        "--plugin-id".into(),
        plugin_id.into(),
    ];
    if let Some(info) = track_info {
        args.extend(["--track-info".into(), info.to_hex().into()]);
    }
    BridgeProcess::spawn(
        Path::new(env!("CARGO_BIN_EXE_plugin_host_bridge")),
        BridgeLayout::new(4096, CHANNELS as u32, 64 * 1024),
        &args,
        TIMEOUT,
    )
}

fn activated() -> BridgeProcess {
    activated_plugin("echoes.test.gain")
}

fn activated_plugin(plugin_id: &str) -> BridgeProcess {
    activated_with_track(plugin_id, None)
}

fn activated_with_track(plugin_id: &str, track_info: Option<&HostTrackInfo>) -> BridgeProcess {
    let bridge = spawn_with_track(plugin_id, track_info).expect("bridge starts");
    let mut payload = 48_000.0f64.to_le_bytes().to_vec();
    payload.extend_from_slice(&4096u32.to_le_bytes());
    let latency = bridge
        .request(Op::Activate, &payload, TIMEOUT)
        .expect("activate");
    assert_eq!(
        latency,
        64u32.to_le_bytes(),
        "the plugin's latency is reported"
    );
    bridge
}

fn param_events(changes: &[(u32, u32, f64)]) -> Vec<WireParam> {
    changes
        .iter()
        .map(|&(kind, native_id, value)| WireParam::new(kind, 0, native_id, value))
        .collect()
}

fn track_record(info: &HostTrackInfo) -> Vec<u8> {
    TrackRecord {
        name: info.name.clone(),
        color: info.color,
    }
    .encode()
}

struct Sections<'a> {
    transport: Option<&'a WireTransport>,
    notes: &'a [WireNote],
    params: &'a [WireParam],
}

const NO_SECTIONS: Sections = Sections {
    transport: None,
    notes: &[],
    params: &[],
};

struct Block {
    output: Vec<f32>,
    edits: Vec<WireParam>,
    outcome: BlockOutcome,
}

fn process(
    bridge: &BridgeProcess,
    sequence: u64,
    input: &[f32],
    params: &[WireParam],
    budget: Duration,
) -> Block {
    process_sections(
        bridge,
        sequence,
        Some(input),
        Sections {
            params,
            ..NO_SECTIONS
        },
        budget,
    )
}

fn process_sections(
    bridge: &BridgeProcess,
    sequence: u64,
    input: Option<&[f32]>,
    sections: Sections,
    budget: Duration,
) -> Block {
    let mut output = vec![9.0f32; FRAMES * CHANNELS];
    let mut edits = vec![WireParam::new(0, 0, 0, 0.0); 64];
    let mut midi = vec![WireNote::new(0, 0, [0; 3]); 64];
    let request = BlockRequest {
        frames: FRAMES,
        input,
        input_channels: CHANNELS,
        output_channels: CHANNELS,
        transport: sections.transport,
        notes: sections.notes,
        params: sections.params,
        sequence,
        steady_time: sequence as i64 * FRAMES as i64,
        reset: false,
    };
    let mut reply = BlockReply {
        output: &mut output,
        edits: &mut edits,
        midi: &mut midi,
    };
    let outcome = bridge.audio().process_block(&request, &mut reply, budget);
    if let BlockOutcome::Processed { edit_count, .. } = outcome {
        edits.truncate(edit_count);
    }
    Block {
        output,
        edits,
        outcome,
    }
}

fn ramp() -> Vec<f32> {
    (0..FRAMES * CHANNELS).map(|i| i as f32 / 100.0).collect()
}

fn scaled(input: &[f32], gain: f32) -> Vec<f32> {
    input.iter().map(|s| s * gain).collect()
}

#[test]
fn processes_audio_with_parameter_changes_and_modulation() {
    let bridge = activated();
    let input = ramp();

    let block = process(&bridge, 0, &input, &[], TIMEOUT);
    assert!(matches!(block.outcome, BlockOutcome::Processed { .. }));
    assert_eq!(block.output, input, "unity gain by default");

    let events = param_events(&[(PARAM_VALUE, GAIN, 0.5)]);
    assert_eq!(
        process(&bridge, 1, &input, &events, TIMEOUT).output,
        scaled(&input, 0.5)
    );

    let events = param_events(&[(PARAM_MODULATION, GAIN, 0.25)]);
    assert_eq!(
        process(&bridge, 2, &input, &events, TIMEOUT).output,
        scaled(&input, 0.75)
    );
    assert_eq!(bridge.audio().timeout_count(), 0);
    let wrong_thread = bridge
        .request(Op::ParamValue, &WRONG_THREAD.to_le_bytes(), TIMEOUT)
        .unwrap();
    assert_eq!(
        f64::from_le_bytes(wrong_thread.try_into().unwrap()),
        0.0,
        "clap.thread-check reports the bridge's audio loop as the audio thread"
    );
}

#[test]
fn state_and_parameters_travel_over_the_control_channel() {
    let bridge = activated();

    let mut flush = GAIN.to_le_bytes().to_vec();
    flush.extend_from_slice(&0.3f64.to_le_bytes());
    bridge.request(Op::FlushValues, &flush, TIMEOUT).unwrap();
    let saved = bridge.request(Op::SaveState, &[], TIMEOUT).unwrap();
    assert_eq!(saved, 0.3f64.to_bits().to_le_bytes());

    bridge
        .request(Op::LoadState, &1.5f64.to_bits().to_le_bytes(), TIMEOUT)
        .unwrap();
    let value = bridge
        .request(Op::ParamValue, &GAIN.to_le_bytes(), TIMEOUT)
        .unwrap();
    assert_eq!(f64::from_le_bytes(value.try_into().unwrap()), 1.5);

    let params =
        ParamRecord::decode_list(&bridge.request(Op::Params, &[], TIMEOUT).unwrap()).unwrap();
    assert_eq!(params.len(), PARAM_COUNT as usize);
    assert_eq!(
        (params[0].native_id, params[0].name.as_str(), params[0].max),
        (GAIN, "Gain", 2.0)
    );
    assert_eq!(
        params[0].flags & (PARAM_AUTOMATABLE | PARAM_MODULATABLE),
        PARAM_AUTOMATABLE | PARAM_MODULATABLE
    );

    let error = bridge.request(Op::LoadState, &[1, 2], TIMEOUT).unwrap_err();
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::Other,
        "a plugin refusal keeps the channel open"
    );
    assert!(bridge.request(Op::SaveState, &[], TIMEOUT).is_ok());
}

#[test]
fn edits_made_in_the_plugin_come_back_with_the_block() {
    let bridge = activated();
    let events = param_events(&[(PARAM_VALUE, TOUCH, 1.0)]);
    let block = process(&bridge, 0, &ramp(), &events, TIMEOUT);

    let BlockOutcome::Processed { flags, .. } = block.outcome else {
        panic!("block not processed: {:?}", block.outcome);
    };
    assert_ne!(flags & BLOCK_PLUGIN_EDITED, 0, "mark_dirty is forwarded");
    let kinds: Vec<u32> = block.edits.iter().map(|edit| edit.kind).collect();
    assert_eq!(kinds, [PARAM_VALUE, PARAM_GESTURE_END]);
}

#[test]
fn a_restart_request_comes_back_with_the_block_and_the_next_activate_applies_it() {
    let bridge = activated();
    let events = param_events(&[(PARAM_VALUE, LOOKAHEAD, 1.0)]);
    let block = process(&bridge, 0, &ramp(), &events, TIMEOUT);

    let BlockOutcome::Processed { flags, .. } = block.outcome else {
        panic!("block not processed: {:?}", block.outcome);
    };
    assert_ne!(
        flags & BLOCK_RESTART_REQUESTED,
        0,
        "request_restart is forwarded"
    );
    let later = process(&bridge, 1, &ramp(), &[], TIMEOUT);
    let BlockOutcome::Processed { flags, .. } = later.outcome else {
        panic!("block not processed: {:?}", later.outcome);
    };
    assert_eq!(flags & BLOCK_RESTART_REQUESTED, 0, "reported once");

    let mut payload = 48_000.0f64.to_le_bytes().to_vec();
    payload.extend_from_slice(&4096u32.to_le_bytes());
    let latency = bridge.request(Op::Activate, &payload, TIMEOUT).unwrap();
    assert_eq!(
        latency,
        128u32.to_le_bytes(),
        "same config, reactivated anyway"
    );
    let input = ramp();
    assert_eq!(
        process(&bridge, 2, &input, &[], TIMEOUT).output,
        input,
        "processing resumes"
    );
}

fn param_text(bridge: &BridgeProcess, native_id: u32, value: f64) -> Vec<u8> {
    let mut payload = native_id.to_le_bytes().to_vec();
    payload.extend_from_slice(&value.to_le_bytes());
    bridge.request(Op::ParamText, &payload, TIMEOUT).unwrap()
}

#[test]
fn the_plugin_writes_the_text_of_its_values() {
    let bridge = activated();
    assert_eq!(param_text(&bridge, GAIN, 0.5), b"-6.0 dB");
    assert_eq!(param_text(&bridge, GAIN, 2.0), b"6.0 dB");
    assert!(
        param_text(&bridge, TAIL, 10.0).is_empty(),
        "no text, no error"
    );
}

#[test]
fn a_parameter_rescan_comes_back_with_the_block_once() {
    let bridge = activated();
    let events = param_events(&[(PARAM_VALUE, RESCAN, 6.0)]);
    let block = process(&bridge, 0, &ramp(), &events, TIMEOUT);

    let BlockOutcome::Processed { flags, .. } = block.outcome else {
        panic!("block not processed: {:?}", block.outcome);
    };
    assert_eq!(
        (flags & BLOCK_PARAMS_RESCAN_MASK) >> BLOCK_PARAMS_RESCAN_SHIFT,
        6,
        "the plugin's rescan flags are forwarded"
    );
    let later = process(&bridge, 1, &ramp(), &[], TIMEOUT);
    let BlockOutcome::Processed { flags, .. } = later.outcome else {
        panic!("block not processed: {:?}", later.outcome);
    };
    assert_eq!(flags & BLOCK_PARAMS_RESCAN_MASK, 0, "reported once");
}

#[test]
fn a_flush_request_from_outside_the_audio_thread_wakes_a_sleeping_plugin() {
    let bridge = activated_plugin("echoes.test.sleepy");
    process(&bridge, 0, &silence(), &[], TIMEOUT);
    for sequence in 1..=3 {
        process(&bridge, sequence, &silence(), &[], TIMEOUT);
    }
    assert_eq!(process_calls(&bridge), 1, "asleep");

    let mut flush = REQUEST_FLUSH.to_le_bytes().to_vec();
    flush.extend_from_slice(&1.0f64.to_le_bytes());
    bridge.request(Op::FlushValues, &flush, TIMEOUT).unwrap();
    assert_eq!(
        process_calls(&bridge),
        1,
        "the request alone does not process"
    );

    process(&bridge, 4, &silence(), &[], TIMEOUT);
    assert_eq!(process_calls(&bridge), 2, "the next block runs the plugin");
    process(&bridge, 5, &silence(), &[], TIMEOUT);
    assert_eq!(process_calls(&bridge), 2, "and it falls asleep again");
}

#[test]
fn a_crashing_plugin_only_kills_its_bridge() {
    let bridge = activated();
    let input = ramp();
    let events = param_events(&[(PARAM_VALUE, CRASH, 1.0)]);
    let block = process(&bridge, 0, &input, &events, Duration::from_millis(500));
    assert_eq!(block.outcome, BlockOutcome::TimedOut);
    assert!(
        block.output.iter().all(|&s| s == 0.0),
        "silence instead of the crashed plugin"
    );

    for sequence in 1..DEAD_AFTER_CONSECUTIVE_TIMEOUTS as u64 {
        process(&bridge, sequence, &input, &[], Duration::from_millis(20));
    }
    assert!(bridge.audio().is_dead());
    assert!(bridge.request(Op::SaveState, &[], TIMEOUT).is_err());
}

#[test]
fn a_hung_plugin_times_out_then_is_condemned() {
    let bridge = activated();
    let input = ramp();
    let events = param_events(&[(PARAM_VALUE, HANG, 1.0)]);
    for sequence in 0..DEAD_AFTER_CONSECUTIVE_TIMEOUTS as u64 {
        let events = if sequence == 0 {
            events.as_slice()
        } else {
            &[]
        };
        let block = process(&bridge, sequence, &input, events, Duration::from_millis(20));
        assert_eq!(block.outcome, BlockOutcome::TimedOut);
    }
    assert!(bridge.audio().is_dead());
    assert_eq!(
        process(&bridge, 99, &input, &[], TIMEOUT).outcome,
        BlockOutcome::Dead
    );
}

#[test]
fn an_unknown_plugin_fails_to_start() {
    assert!(spawn("echoes.test.missing").is_err());
}

fn param_value(bridge: &BridgeProcess, native_id: u32) -> f64 {
    let value = bridge
        .request(Op::ParamValue, &native_id.to_le_bytes(), TIMEOUT)
        .unwrap();
    f64::from_le_bytes(value.try_into().unwrap())
}

fn process_calls(bridge: &BridgeProcess) -> u32 {
    param_value(bridge, PROCESS_CALLS) as u32
}

fn track_info_values(bridge: &BridgeProcess) -> [f64; 6] {
    [
        TRACK_FLAGS,
        TRACK_COLOR,
        TRACK_NAME,
        TRACK_NAME_LENGTH,
        TRACK_INFO_READS,
        TRACK_INFO_CHANGES,
    ]
    .map(|native_id| param_value(bridge, native_id))
}

/// `[flags, color, name, name length, reads, changes]` the plugin reports.
fn expected_track(flags: u32, color: [u8; 4], name: &str, reads: u32, changes: u32) -> [f64; 6] {
    let packed_name = name
        .bytes()
        .take(6)
        .fold(0u64, |packed, byte| (packed << 8) | u64::from(byte));
    [
        f64::from(flags),
        f64::from(u32::from_be_bytes(color)),
        packed_name as f64,
        name.len() as f64,
        f64::from(reads),
        f64::from(changes),
    ]
}

#[test]
fn a_bridge_started_with_a_track_lets_the_plugin_read_it_in_init() {
    let info = HostTrackInfo::new("Lead", Some([0x12, 0x34, 0x56]));
    let bridge = activated_with_track("echoes.test.gain", Some(&info));

    assert_eq!(
        track_info_values(&bridge),
        expected_track(3, [255, 0x12, 0x34, 0x56], "Lead", 1, 0)
    );
}

#[test]
fn a_bridge_started_without_a_track_gives_the_plugin_nothing_to_read() {
    let bridge = activated();

    assert_eq!(
        track_info_values(&bridge),
        expected_track(0, [0; 4], "", 0, 0)
    );
}

#[test]
fn a_track_without_color_is_read_without_the_color_flag() {
    let bridge = activated_with_track("echoes.test.gain", Some(&HostTrackInfo::new("Bass", None)));

    assert_eq!(
        track_info_values(&bridge),
        expected_track(1, [0; 4], "Bass", 1, 0)
    );
}

#[test]
fn a_new_track_over_the_control_channel_calls_changed_on_the_main_thread() {
    let info = HostTrackInfo::new("Lead", Some([0x12, 0x34, 0x56]));
    let bridge = activated_with_track("echoes.test.gain", Some(&info));

    let renamed = HostTrackInfo::new("Pad", Some([0x12, 0x34, 0x56]));
    bridge
        .request(Op::TrackInfo, &track_record(&renamed), TIMEOUT)
        .unwrap();
    assert_eq!(
        track_info_values(&bridge),
        expected_track(3, [255, 0x12, 0x34, 0x56], "Pad", 2, 1)
    );

    bridge
        .request(Op::TrackInfo, &track_record(&renamed), TIMEOUT)
        .unwrap();
    assert_eq!(
        track_info_values(&bridge)[5],
        1.0,
        "the same track is not news"
    );

    let recolored = HostTrackInfo::new("Pad", None);
    bridge
        .request(Op::TrackInfo, &track_record(&recolored), TIMEOUT)
        .unwrap();
    assert_eq!(
        track_info_values(&bridge),
        expected_track(1, [0; 4], "Pad", 3, 2)
    );
    assert_eq!(
        param_value(&bridge, TRACK_INFO_AUDIO_CALLS),
        0.0,
        "never on the audio thread"
    );
}

#[test]
fn a_garbled_track_is_refused_and_the_channel_stays_open() {
    let bridge = activated();

    let error = bridge.request(Op::TrackInfo, &[1, 2], TIMEOUT).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    assert_eq!(track_info_values(&bridge)[5], 0.0);
    assert!(bridge.request(Op::SaveState, &[], TIMEOUT).is_ok());
}

#[test]
fn track_changes_do_not_stop_the_audio() {
    let bridge = activated();
    let input = ramp();
    for (sequence, name) in ["A", "B", "C", "D"].into_iter().enumerate() {
        bridge
            .request(
                Op::TrackInfo,
                &track_record(&HostTrackInfo::new(name, None)),
                TIMEOUT,
            )
            .unwrap();
        assert_eq!(
            process(&bridge, sequence as u64, &input, &[], TIMEOUT).output,
            input
        );
    }
}

fn silence() -> Vec<f32> {
    vec![0.0; FRAMES * CHANNELS]
}

#[test]
fn a_sleeping_plugin_is_not_processed_until_a_signal_reaches_it() {
    let bridge = activated_plugin("echoes.test.sleepy");

    let first = process(&bridge, 0, &silence(), &[], TIMEOUT);
    assert_eq!(first.output, silence());
    assert_eq!(
        process_calls(&bridge),
        1,
        "the first block shows it is quiet"
    );

    for sequence in 1..=5 {
        let block = process(&bridge, sequence, &silence(), &[], TIMEOUT);
        assert_eq!(block.output, silence(), "an asleep plugin outputs silence");
    }
    assert_eq!(process_calls(&bridge), 1, "asleep: process is not called");

    let input = ramp();
    assert_eq!(process(&bridge, 6, &input, &[], TIMEOUT).output, input);
    assert_eq!(process_calls(&bridge), 2, "a signal wakes it");

    process(&bridge, 7, &silence(), &[], TIMEOUT);
    assert_eq!(
        process_calls(&bridge),
        3,
        "awake while it was still sounding"
    );
    process(&bridge, 8, &silence(), &[], TIMEOUT);
    assert_eq!(process_calls(&bridge), 3, "asleep again once quiet");

    let events = param_events(&[(PARAM_VALUE, GAIN, 1.0)]);
    process(&bridge, 9, &silence(), &events, TIMEOUT);
    assert_eq!(process_calls(&bridge), 4, "an event wakes it");
    assert_eq!(bridge.audio().timeout_count(), 0);
}

#[test]
fn a_tail_is_not_cut_before_its_length_of_quiet_input() {
    let bridge = activated();
    let tail = 200.0;
    let events = param_events(&[(PARAM_VALUE, STATUS, 3.0), (PARAM_VALUE, TAIL, tail)]);
    process(&bridge, 0, &silence(), &events, TIMEOUT);

    let mut calls = Vec::new();
    for sequence in 1..=20 {
        process(&bridge, sequence, &silence(), &[], TIMEOUT);
        calls.push(process_calls(&bridge));
    }
    let quiet_blocks_needed = (tail as usize).div_ceil(FRAMES) as u32;
    assert!(
        calls[(quiet_blocks_needed - 1) as usize] > quiet_blocks_needed,
        "the plugin ran through the whole tail: {calls:?}"
    );
    assert_eq!(calls[10], calls[19], "then it sleeps: {calls:?}");
}

#[test]
fn a_plugin_that_claims_to_sleep_while_sounding_stays_awake() {
    let bridge = activated();
    let events = param_events(&[(PARAM_VALUE, STATUS, 4.0), (PARAM_VALUE, LIAR, 1.0)]);
    let input = ramp();
    process(&bridge, 0, &input, &events, TIMEOUT);
    for sequence in 1..=5 {
        assert_eq!(
            process(&bridge, sequence, &input, &[], TIMEOUT).output,
            input
        );
    }
    assert_eq!(
        process_calls(&bridge),
        6,
        "never put to sleep while audible"
    );
}

fn note_names(bridge: &BridgeProcess) -> Vec<NoteNameRecord> {
    let reply = bridge.request(Op::NoteNames, &[], TIMEOUT).unwrap();
    NoteNameRecord::decode_list(&reply).expect("a well-formed list")
}

fn remote_pages(bridge: &BridgeProcess) -> Vec<RemotePageRecord> {
    let reply = bridge.request(Op::RemoteControls, &[], TIMEOUT).unwrap();
    RemotePageRecord::decode_list(&reply).expect("a well-formed list")
}

fn note_name(name: &str, port: i16, key: i16, channel: i16) -> NoteNameRecord {
    NoteNameRecord {
        name: name.into(),
        port,
        key,
        channel,
    }
}

fn block_flags(bridge: &BridgeProcess, sequence: u64, events: &[WireParam]) -> u32 {
    let block = process(bridge, sequence, &ramp(), events, TIMEOUT);
    let BlockOutcome::Processed { flags, .. } = block.outcome else {
        panic!("block not processed: {:?}", block.outcome);
    };
    flags
}

#[test]
fn a_preset_loaded_by_the_plugin_comes_back_as_an_edit_and_an_error_does_not() {
    let bridge = activated();

    let failed = param_events(&[(PARAM_VALUE, PRESET_ERROR, 1.0)]);
    assert_eq!(
        block_flags(&bridge, 0, &failed) & BLOCK_PLUGIN_EDITED,
        0,
        "a failed load changed nothing"
    );

    let loaded = param_events(&[(PARAM_VALUE, PRESET_LOADED, 1.0)]);
    assert_ne!(
        block_flags(&bridge, 1, &loaded) & BLOCK_PLUGIN_EDITED,
        0,
        "clap.preset-load.loaded is forwarded"
    );
    assert_eq!(
        block_flags(&bridge, 2, &[]) & BLOCK_PLUGIN_EDITED,
        0,
        "reported once"
    );
}

fn bundled_preset(key: &str) -> Vec<u8> {
    PresetRecord {
        source: PresetSource::Plugin,
        location: None,
        load_key: Some(key.into()),
    }
    .encode()
    .unwrap()
}

#[test]
fn a_preset_load_travels_over_the_control_channel_and_a_refusal_keeps_it_open() {
    let bridge = activated();
    assert!(
        bridge
            .request(Op::LoadedPreset, &[], TIMEOUT)
            .unwrap()
            .is_empty(),
        "no preset reported yet"
    );

    bridge
        .request(Op::PresetLoad, &bundled_preset("loud"), TIMEOUT)
        .expect("the plugin loads a bundled preset");
    assert_eq!(param_value(&bridge, GAIN), 2.0);
    assert_eq!(
        bridge.request(Op::LoadedPreset, &[], TIMEOUT).unwrap(),
        bundled_preset("loud"),
        "the preset the plugin reported comes back"
    );
    assert_ne!(
        block_flags(&bridge, 0, &[]) & BLOCK_PLUGIN_EDITED,
        0,
        "clap.preset-load.loaded is forwarded"
    );

    let refused = bridge
        .request(Op::PresetLoad, &bundled_preset("nope"), TIMEOUT)
        .unwrap_err();
    assert!(
        refused.to_string().contains("unreadable preset"),
        "{refused}"
    );
    let malformed = bridge
        .request(Op::PresetLoad, &[1, 2], TIMEOUT)
        .unwrap_err();
    assert!(
        malformed.to_string().contains("invalid preset request"),
        "{malformed}"
    );
    assert_eq!(param_value(&bridge, GAIN), 2.0, "a refusal changes nothing");
    assert!(bridge.request(Op::SaveState, &[], TIMEOUT).is_ok());
}

#[test]
fn the_plugin_sees_the_note_dialects_the_host_sends() {
    let bridge = activated();

    assert_eq!(
        param_value(&bridge, HOST_NOTE_DIALECTS),
        f64::from(CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI)
    );
}

#[test]
fn the_remote_control_pages_of_the_plugin_travel_over_the_control_channel() {
    let bridge = activated();
    assert_eq!(
        remote_pages(&bridge),
        [RemotePageRecord {
            section: "Main".into(),
            name: "Level".into(),
            param_ids: vec![GAIN, TAIL],
        }],
        "the page without a parameter is not offered"
    );
}

#[test]
fn the_note_names_of_the_plugin_travel_over_the_control_channel() {
    let bridge = activated();
    assert_eq!(
        note_names(&bridge),
        [
            note_name("Kick", -1, 36, -1),
            note_name("Snare", -1, 38, -1)
        ]
    );

    let events = param_events(&[(PARAM_VALUE, NOTE_NAMES_SET, 1.0)]);
    block_flags(&bridge, 0, &events);
    assert_eq!(note_names(&bridge), [note_name("Tom", 0, 41, 9)]);
}

#[test]
fn a_change_of_note_names_comes_back_with_the_block_once() {
    let bridge = activated();
    assert_eq!(
        block_flags(&bridge, 0, &[]) & BLOCK_NOTE_NAMES_CHANGED,
        0,
        "nothing changed yet"
    );

    let events = param_events(&[(PARAM_VALUE, NOTE_NAMES_SET, 1.0)]);
    assert_ne!(
        block_flags(&bridge, 1, &events) & BLOCK_NOTE_NAMES_CHANGED,
        0,
        "clap.note-name.changed is forwarded"
    );
    assert_eq!(
        block_flags(&bridge, 2, &[]) & BLOCK_NOTE_NAMES_CHANGED,
        0,
        "reported once"
    );
    assert_eq!(
        block_flags(&bridge, 3, &events) & BLOCK_NOTE_NAMES_CHANGED,
        0,
        "the plugin tells the host only when its names change"
    );
}

#[test]
fn a_rescan_of_all_note_ports_comes_back_as_a_restart_request() {
    let bridge = activated();

    let renamed = param_events(&[(
        PARAM_VALUE,
        NOTE_PORTS_RESCAN,
        f64::from(CLAP_NOTE_PORTS_RESCAN_NAMES),
    )]);
    assert_eq!(
        block_flags(&bridge, 0, &renamed) & BLOCK_RESTART_REQUESTED,
        0,
        "a renamed port is not news"
    );

    let all = param_events(&[(
        PARAM_VALUE,
        NOTE_PORTS_RESCAN,
        f64::from(CLAP_NOTE_PORTS_RESCAN_ALL),
    )]);
    assert_ne!(block_flags(&bridge, 1, &all) & BLOCK_RESTART_REQUESTED, 0);
}

// ── clap.timer-support ─────────────────────────────────────────────────────────

const ACCEPTED: f64 = 1.0;
const REFUSED: f64 = 2.0;
const INVALID_ID: f64 = u32::MAX as f64;

/// Sets plain values on the bridge's main thread (`clap.params.flush`).
fn flush_values(bridge: &BridgeProcess, values: &[(u32, f64)]) {
    let mut payload = Vec::new();
    for &(native_id, value) in values {
        payload.extend_from_slice(&native_id.to_le_bytes());
        payload.extend_from_slice(&value.to_le_bytes());
    }
    bridge.request(Op::FlushValues, &payload, TIMEOUT).unwrap();
}

/// Registers a timer from the bridge's main thread; the id the plugin got, if accepted.
fn register_timer(bridge: &BridgeProcess, period_ms: f64) -> Option<f64> {
    flush_values(bridge, &[(TIMER_REGISTER, period_ms)]);
    (param_value(bridge, TIMER_REGISTER_RESULT) == ACCEPTED)
        .then(|| param_value(bridge, TIMER_LAST_ID))
}

fn unregister_timer(bridge: &BridgeProcess, id: f64) -> bool {
    flush_values(bridge, &[(TIMER_UNREGISTER, id)]);
    param_value(bridge, TIMER_UNREGISTER_RESULT) == ACCEPTED
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_plugin_registers_timers_from_any_thread_and_a_zero_period_is_refused() {
    let bridge = activated();

    // From the audio thread: a plugin may call the host from the wrong thread.
    let events = param_events(&[(PARAM_VALUE, TIMER_REGISTER, 33.0)]);
    block_flags(&bridge, 0, &events);
    assert_eq!(param_value(&bridge, TIMER_REGISTER_RESULT), ACCEPTED);
    let from_audio = param_value(&bridge, TIMER_LAST_ID);

    let from_main = register_timer(&bridge, 50.0).expect("a second timer");
    assert_ne!(from_audio, from_main, "ids are distinct");
    assert!(from_audio > 0.0 && from_main > 0.0 && from_audio != INVALID_ID);

    assert_eq!(register_timer(&bridge, 0.0), None, "a zero period");
    assert_eq!(param_value(&bridge, TIMER_REGISTER_RESULT), REFUSED);
    assert_eq!(param_value(&bridge, TIMER_LAST_ID), INVALID_ID);

    assert!(unregister_timer(&bridge, from_audio));
    assert!(!unregister_timer(&bridge, from_audio), "already gone");
    assert!(!unregister_timer(&bridge, 12_345.0), "never registered");
}

#[test]
fn a_timer_does_not_run_while_the_editor_is_closed() {
    let bridge = activated();
    register_timer(&bridge, 20.0).expect("accepted");

    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(param_value(&bridge, TIMER_CALLS), 0.0);
}

/// Opens the bridge's real editor window (the test plugin's `clap.gui` is a stub),
/// so the timers are driven by the same message loop as with a real plugin.
#[test]
fn an_open_editor_runs_the_timers_until_they_are_unregistered() {
    let bridge = activated();
    let first = register_timer(&bridge, 20.0).expect("first timer");
    let second = register_timer(&bridge, 25.0).expect("second timer");
    assert_ne!(first, second);
    bridge
        .request(Op::OpenEditor, b"Timer test", TIMEOUT)
        .expect("the editor opens");

    let mut fired = std::collections::BTreeSet::new();
    wait_until("both timers to fire", || {
        fired.insert(param_value(&bridge, TIMER_LAST_FIRED) as u32);
        fired.contains(&(first as u32)) && fired.contains(&(second as u32))
    });
    assert_eq!(
        param_value(&bridge, TIMER_AUDIO_CALLS),
        0.0,
        "never from the audio thread"
    );

    assert!(unregister_timer(&bridge, first));
    let calls = param_value(&bridge, TIMER_CALLS);
    wait_until("the other timer to keep firing", || {
        param_value(&bridge, TIMER_CALLS) >= calls + 3.0
    });
    assert_eq!(
        param_value(&bridge, TIMER_LAST_FIRED),
        second,
        "the unregistered timer is silent"
    );

    assert!(unregister_timer(&bridge, second));
    let calls = param_value(&bridge, TIMER_CALLS);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(param_value(&bridge, TIMER_CALLS), calls, "no timer left");
}

// ── Protocol v3: handshake and sections ───────────────────────────────────────

#[test]
fn the_handshake_declares_plain_values_and_the_port_table() {
    let bridge = activated();
    let reply = bridge.request(Op::Handshake, &[], TIMEOUT).unwrap();
    let handshake = Handshake::decode(&reply).unwrap();

    assert_eq!(handshake.param_unit, ParamUnit::Plain);
    assert_eq!(handshake.ports.len(), 4, "two inputs and two outputs");
    assert_eq!(handshake.active_port_count(PortDirection::Input), 1);
    assert_eq!(handshake.active_port_count(PortDirection::Output), 1);
    let main_ports = handshake
        .ports
        .iter()
        .filter(|port| port.active)
        .collect::<Vec<_>>();
    for port in main_ports {
        assert_eq!(
            (port.role, port.layout, port.channels),
            (PortRole::Main, SpeakerLayout::Stereo, 2)
        );
    }
    let aux_output = handshake
        .ports
        .iter()
        .find(|port| port.direction == PortDirection::Output && port.role == PortRole::Auxiliary)
        .expect("the declared auxiliary output");
    assert_eq!(
        (aux_output.channels, aux_output.layout, aux_output.active),
        (1, SpeakerLayout::Mono, false)
    );
}

fn note(offset: u32, kind: u8, data: [u8; 3]) -> WireNote {
    WireNote::new(offset, kind, data)
}

#[test]
fn notes_and_parameters_of_a_block_reach_a_generator() {
    let bridge = activated();
    let play = |sequence, notes: &[WireNote], params: &[WireParam]| {
        process_sections(
            &bridge,
            sequence,
            None,
            Sections {
                transport: None,
                notes,
                params,
            },
            TIMEOUT,
        )
        .output[0]
    };

    assert_eq!(play(0, &[], &[]), 0.0, "silent until a note");
    assert_eq!(play(1, &[note(0, NOTE_ON, [60, 127, 0])], &[]), 1.0);
    let half = param_events(&[(PARAM_VALUE, GAIN, 0.5)]);
    assert_eq!(play(2, &[], &half), 0.5, "the note is still held");
    assert_eq!(play(3, &[note(0, NOTE_OFF, [60, 0, 0])], &[]), 0.0);
}

#[test]
fn a_note_and_a_parameter_of_one_block_are_both_delivered() {
    let bridge = activated();
    let notes = [note(10, NOTE_ON, [60, 127, 0])];
    let params = [WireParam::new(PARAM_VALUE, 20, GAIN, 0.5)];
    let block = process_sections(
        &bridge,
        0,
        None,
        Sections {
            transport: None,
            notes: &notes,
            params: &params,
        },
        TIMEOUT,
    );
    assert!(
        block.output[..FRAMES].iter().all(|&sample| sample == 0.5),
        "the test plugin applies a block's events before it renders: note 1.0 x gain 0.5"
    );
}

#[test]
fn raw_midi_and_gestures_from_the_engine_do_not_disturb_the_plugin() {
    let bridge = activated();
    let notes = [note(0, MIDI, [0xE0, 0, 64])];
    let params = [
        WireParam::new(plugin_bridge::protocol::PARAM_GESTURE_BEGIN, 0, GAIN, 0.0),
        WireParam::new(PARAM_GESTURE_END, 0, GAIN, 0.0),
    ];
    let input = ramp();
    let block = process_sections(
        &bridge,
        0,
        Some(&input),
        Sections {
            transport: None,
            notes: &notes,
            params: &params,
        },
        TIMEOUT,
    );
    assert!(matches!(block.outcome, BlockOutcome::Processed { .. }));
    assert_eq!(block.output, input);
}

#[test]
fn the_plugin_reads_the_transport_of_the_block() {
    let bridge = activated_plugin("echoes.test.sleepy");
    let transport = |playing: bool| WireTransport {
        flags: if playing { TRANSPORT_PLAYING } else { 0 },
        bar_number: 0,
        sample_rate: 48_000.0,
        position_samples: 0,
        position_seconds: 0.0,
        position_beats: 0.0,
        bar_start_beats: 0.0,
        tempo: 120.0,
        time_signature_numerator: 4,
        time_signature_denominator: 4,
        reserved: 0,
    };
    let quiet = |sequence, playing: &WireTransport| {
        process_sections(
            &bridge,
            sequence,
            Some(&silence()),
            Sections {
                transport: Some(playing),
                ..NO_SECTIONS
            },
            TIMEOUT,
        )
    };

    let stopped = transport(false);
    for sequence in 0..4 {
        quiet(sequence, &stopped);
    }
    assert_eq!(
        process_calls(&bridge),
        1,
        "a stopped transport lets it sleep"
    );

    let playing = transport(true);
    quiet(4, &playing);
    assert_eq!(process_calls(&bridge), 2, "starting the transport wakes it");
}

#[test]
fn a_handshake_request_after_a_crash_fails_like_any_other() {
    let bridge = activated();
    let params = param_events(&[(PARAM_VALUE, CRASH, 1.0)]);
    process(&bridge, 0, &ramp(), &params, Duration::from_millis(500));
    for sequence in 1..DEAD_AFTER_CONSECUTIVE_TIMEOUTS as u64 {
        process(&bridge, sequence, &ramp(), &[], Duration::from_millis(20));
    }
    assert!(bridge.request(Op::Handshake, &[], TIMEOUT).is_err());
}
