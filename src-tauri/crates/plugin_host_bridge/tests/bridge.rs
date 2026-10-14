//! The real bridge binary hosting the real test CLAP plugin (ADR-015).
//! Windows-only: the shared region is implemented on Windows only.
#![cfg(windows)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use clap_ffi::{
    read_pod, split_events, ClapEventBuffer, ClapEventHeader, ClapParamInfo, HostTrackInfo,
    CLAP_EVENT_PARAM_GESTURE_END, CLAP_EVENT_PARAM_VALUE,
};
use plugin_bridge::control::{Op, PayloadReader};
use plugin_bridge::protocol::{
    BLOCK_PARAMS_RESCAN_MASK, BLOCK_PARAMS_RESCAN_SHIFT, BLOCK_PLUGIN_EDITED,
    BLOCK_RESTART_REQUESTED,
};
use plugin_bridge::session::{
    BlockOutcome, BlockRequest, BridgeLayout, BridgeProcess, DEAD_AFTER_CONSECUTIVE_TIMEOUTS,
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
const PARAM_COUNT: u32 = 19;
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

fn param_events(changes: &[(u16, u32, f64)]) -> Vec<u8> {
    let mut buffer = ClapEventBuffer::new();
    for &(kind, native_id, value) in changes {
        if kind == CLAP_EVENT_PARAM_VALUE {
            buffer.push_param_value(0, native_id, value);
        } else {
            buffer.push_param_mod(0, native_id, value);
        }
    }
    buffer.sort();
    let mut bytes = vec![0u8; 4096];
    let end = buffer.write_bytes(&mut bytes, 0).unwrap();
    bytes.truncate(end);
    bytes
}

struct Block {
    output: Vec<f32>,
    out_events: Vec<u8>,
    outcome: BlockOutcome,
}

fn process(
    bridge: &BridgeProcess,
    sequence: u64,
    input: &[f32],
    events: &[u8],
    budget: Duration,
) -> Block {
    let mut output = vec![9.0f32; FRAMES * CHANNELS];
    let mut out_events = vec![0u8; 4096];
    let request = BlockRequest {
        frames: FRAMES,
        input: Some(input),
        events,
        sequence,
        steady_time: sequence as i64 * FRAMES as i64,
        reset: false,
    };
    let outcome = bridge
        .audio()
        .process_block(&request, &mut output, &mut out_events, budget);
    if let BlockOutcome::Processed { event_bytes, .. } = outcome {
        out_events.truncate(event_bytes);
    }
    Block {
        output,
        out_events,
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

    let events = param_events(&[(CLAP_EVENT_PARAM_VALUE, GAIN, 0.5)]);
    assert_eq!(
        process(&bridge, 1, &input, &events, TIMEOUT).output,
        scaled(&input, 0.5)
    );

    let events = param_events(&[(clap_ffi::CLAP_EVENT_PARAM_MOD, GAIN, 0.25)]);
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

    let params = bridge.request(Op::Params, &[], TIMEOUT).unwrap();
    let mut reader = PayloadReader::new(&params);
    assert_eq!(reader.u32().unwrap(), PARAM_COUNT);
    let first: ClapParamInfo = read_pod(&params[4..]).unwrap();
    assert_eq!(
        (first.id, &first.name[..4], first.max_value),
        (GAIN, &b"Gain"[..], 2.0)
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
    let events = param_events(&[(CLAP_EVENT_PARAM_VALUE, TOUCH, 1.0)]);
    let block = process(&bridge, 0, &ramp(), &events, TIMEOUT);

    let BlockOutcome::Processed { flags, .. } = block.outcome else {
        panic!("block not processed: {:?}", block.outcome);
    };
    assert_ne!(flags & BLOCK_PLUGIN_EDITED, 0, "mark_dirty is forwarded");
    let kinds: Vec<u16> = split_events(&block.out_events)
        .map(|event| read_pod::<ClapEventHeader>(event).unwrap().r#type)
        .collect();
    assert_eq!(
        kinds,
        [CLAP_EVENT_PARAM_VALUE, CLAP_EVENT_PARAM_GESTURE_END]
    );
}

#[test]
fn a_restart_request_comes_back_with_the_block_and_the_next_activate_applies_it() {
    let bridge = activated();
    let events = param_events(&[(CLAP_EVENT_PARAM_VALUE, LOOKAHEAD, 1.0)]);
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
    let events = param_events(&[(CLAP_EVENT_PARAM_VALUE, RESCAN, 6.0)]);
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
    let events = param_events(&[(CLAP_EVENT_PARAM_VALUE, CRASH, 1.0)]);
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
    let events = param_events(&[(CLAP_EVENT_PARAM_VALUE, HANG, 1.0)]);
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
        .request(Op::TrackInfo, &renamed.encode(), TIMEOUT)
        .unwrap();
    assert_eq!(
        track_info_values(&bridge),
        expected_track(3, [255, 0x12, 0x34, 0x56], "Pad", 2, 1)
    );

    bridge
        .request(Op::TrackInfo, &renamed.encode(), TIMEOUT)
        .unwrap();
    assert_eq!(
        track_info_values(&bridge)[5],
        1.0,
        "the same track is not news"
    );

    let recolored = HostTrackInfo::new("Pad", None);
    bridge
        .request(Op::TrackInfo, &recolored.encode(), TIMEOUT)
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
                &HostTrackInfo::new(name, None).encode(),
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

    let events = param_events(&[(CLAP_EVENT_PARAM_VALUE, GAIN, 1.0)]);
    process(&bridge, 9, &silence(), &events, TIMEOUT);
    assert_eq!(process_calls(&bridge), 4, "an event wakes it");
    assert_eq!(bridge.audio().timeout_count(), 0);
}

#[test]
fn a_tail_is_not_cut_before_its_length_of_quiet_input() {
    let bridge = activated();
    let tail = 200.0;
    let events = param_events(&[
        (CLAP_EVENT_PARAM_VALUE, STATUS, 3.0),
        (CLAP_EVENT_PARAM_VALUE, TAIL, tail),
    ]);
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
    let events = param_events(&[
        (CLAP_EVENT_PARAM_VALUE, STATUS, 4.0),
        (CLAP_EVENT_PARAM_VALUE, LIAR, 1.0),
    ]);
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
