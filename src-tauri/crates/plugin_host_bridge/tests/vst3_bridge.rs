//! The real bridge binary hosting the VST3 test effect (ADR-033, step 5).
//! Windows-only: the shared region is implemented on Windows only.
#![cfg(windows)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use plugin_bridge::control::Op;
use plugin_bridge::protocol::{WireParam, PARAM_VALUE};
use plugin_bridge::records::{Handshake, ParamUnit, PortDirection, PortRole, SpeakerLayout};
use plugin_bridge::session::{
    BlockOutcome, BlockReply, BlockRequest, BridgeLayout, BridgeProcess,
    DEAD_AFTER_CONSECUTIVE_TIMEOUTS,
};

const GAIN: u32 = 0;
const HANG: u32 = 1;
const CRASH: u32 = 2;
const FRAMES: usize = 64;
const CHANNELS: usize = 2;
const LATENCY: u32 = 64;
const TIMEOUT: Duration = Duration::from_secs(5);

fn test_plugin_dll() -> &'static Path {
    static DLL: OnceLock<PathBuf> = OnceLock::new();
    DLL.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = workspace.join("target").join("vst3-bridge-test-plugin");
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "test_vst3_plugin", "--target-dir"])
            .arg(&target)
            .current_dir(&workspace)
            .status()
            .expect("run cargo");
        assert!(status.success(), "building the VST3 test plugin failed");
        target.join("debug").join("test_vst3_plugin.dll")
    })
}

fn class_id(name: &[u8; 16]) -> String {
    name.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn effect_class_id() -> String {
    class_id(b"EchoesTestGain_F")
}

fn spawn(class_id: &str) -> std::io::Result<BridgeProcess> {
    let args: Vec<OsString> = vec![
        "--plugin-dll".into(),
        test_plugin_dll().into(),
        "--plugin-format".into(),
        "vst3".into(),
        "--plugin-id".into(),
        class_id.into(),
    ];
    BridgeProcess::spawn(
        Path::new(env!("CARGO_BIN_EXE_plugin_host_bridge")),
        BridgeLayout::new(4096, CHANNELS as u32, 64 * 1024),
        &args,
        TIMEOUT,
    )
}

fn activate(bridge: &BridgeProcess) -> u32 {
    let mut payload = 48_000.0f64.to_le_bytes().to_vec();
    payload.extend_from_slice(&4096u32.to_le_bytes());
    let reply = bridge
        .request(Op::Activate, &payload, TIMEOUT)
        .expect("activate");
    u32::from_le_bytes(reply.try_into().expect("latency is four bytes"))
}

fn activated() -> BridgeProcess {
    let bridge = spawn(&effect_class_id()).expect("bridge starts");
    assert_eq!(activate(&bridge), LATENCY, "the plugin's latency");
    bridge
}

fn ramp() -> Vec<f32> {
    (0..FRAMES * CHANNELS).map(|i| i as f32 / 100.0).collect()
}

fn scaled(input: &[f32], gain: f32) -> Vec<f32> {
    input.iter().map(|s| s * gain).collect()
}

struct Block {
    output: Vec<f32>,
    outcome: BlockOutcome,
}

fn process(bridge: &BridgeProcess, sequence: u64, params: &[WireParam], budget: Duration) -> Block {
    let input = ramp();
    let mut output = vec![9.0f32; FRAMES * CHANNELS];
    let mut edits = vec![WireParam::new(0, 0, 0, 0.0); 16];
    let mut midi = Vec::new();
    let request = BlockRequest {
        frames: FRAMES,
        input: Some(&input),
        transport: None,
        notes: &[],
        params,
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
    Block { output, outcome }
}

fn value(native_id: u32, normalised: f64) -> WireParam {
    WireParam::new(PARAM_VALUE, 0, native_id, normalised)
}

#[test]
fn the_handshake_declares_normalised_values_and_one_active_port_each_way() {
    let bridge = spawn(&effect_class_id()).expect("bridge starts");
    let reply = bridge.request(Op::Handshake, &[], TIMEOUT).unwrap();
    let handshake = Handshake::decode(&reply).unwrap();
    assert_eq!(handshake.param_unit, ParamUnit::Normalised);
    for direction in [PortDirection::Input, PortDirection::Output] {
        let port = handshake
            .active_port(direction)
            .unwrap()
            .expect("an active port");
        assert_eq!(port.role, PortRole::Main);
        assert_eq!(port.channels, 2);
        assert_eq!(port.layout, SpeakerLayout::Stereo);
    }
}

#[test]
fn processes_audio_and_applies_normalised_parameter_changes() {
    let bridge = activated();
    let input = ramp();

    let block = process(&bridge, 0, &[], TIMEOUT);
    assert!(matches!(block.outcome, BlockOutcome::Processed { .. }));
    assert_eq!(block.output, input, "unity gain by default");

    // Normalised 0.25 of the 0..2 range is a gain of 0.5; the bridge converts nothing.
    let block = process(&bridge, 1, &[value(GAIN, 0.25)], TIMEOUT);
    assert_eq!(block.output, scaled(&input, 0.5));
    assert_eq!(bridge.audio().timeout_count(), 0);
}

#[test]
fn the_state_travels_over_the_control_channel() {
    let bridge = activated();
    let gain = 1.5f64.to_le_bytes().to_vec();
    bridge.request(Op::LoadState, &gain, TIMEOUT).unwrap();
    assert_eq!(
        bridge.request(Op::SaveState, &[], TIMEOUT).unwrap(),
        gain,
        "the state the plugin saved is the one it was given"
    );
    assert_eq!(
        process(&bridge, 0, &[], TIMEOUT).output,
        scaled(&ramp(), 1.5)
    );
    assert!(bridge.request(Op::LoadState, &[1, 2], TIMEOUT).is_err());
    assert!(
        bridge.request(Op::SaveState, &[], TIMEOUT).is_ok(),
        "a refused state keeps the channel open"
    );
}

#[test]
fn activating_again_at_a_new_size_keeps_the_plugin_and_its_state() {
    let bridge = activated();
    bridge
        .request(Op::LoadState, &0.5f64.to_le_bytes(), TIMEOUT)
        .unwrap();
    let mut payload = 44_100.0f64.to_le_bytes().to_vec();
    payload.extend_from_slice(&1024u32.to_le_bytes());
    let latency = bridge.request(Op::Activate, &payload, TIMEOUT).unwrap();
    assert_eq!(latency, LATENCY.to_le_bytes());
    assert_eq!(
        process(&bridge, 0, &[], TIMEOUT).output,
        scaled(&ramp(), 0.5)
    );
}

#[test]
fn what_a_vst3_plugin_cannot_do_yet_is_refused_and_the_channel_stays_open() {
    let bridge = activated();
    for op in [Op::ParamValue, Op::PresetLoad, Op::OpenEditor] {
        assert!(
            bridge.request(op, &0u32.to_le_bytes(), TIMEOUT).is_err(),
            "{op:?}"
        );
    }
    assert!(bridge.request(Op::SaveState, &[], TIMEOUT).is_ok());
}

#[test]
fn a_crashing_plugin_only_kills_its_bridge() {
    let bridge = activated();
    let block = process(&bridge, 0, &[value(CRASH, 1.0)], Duration::from_millis(500));
    assert_eq!(block.outcome, BlockOutcome::TimedOut);
    assert!(
        block.output.iter().all(|&s| s == 0.0),
        "silence instead of the crashed plugin"
    );
    for sequence in 1..DEAD_AFTER_CONSECUTIVE_TIMEOUTS as u64 {
        process(&bridge, sequence, &[], Duration::from_millis(20));
    }
    assert!(bridge.audio().is_dead());
    assert!(bridge.request(Op::SaveState, &[], TIMEOUT).is_err());
}

#[test]
fn a_hung_plugin_times_out_then_is_condemned() {
    let bridge = activated();
    for sequence in 0..DEAD_AFTER_CONSECUTIVE_TIMEOUTS as u64 {
        let events = if sequence == 0 {
            vec![value(HANG, 1.0)]
        } else {
            Vec::new()
        };
        let block = process(&bridge, sequence, &events, Duration::from_millis(20));
        assert_eq!(block.outcome, BlockOutcome::TimedOut);
    }
    assert!(bridge.audio().is_dead());
    assert_eq!(
        process(&bridge, 99, &[], TIMEOUT).outcome,
        BlockOutcome::Dead
    );
}

#[test]
fn an_unknown_class_or_a_malformed_id_fails_to_start() {
    assert!(spawn(&class_id(b"NoSuchClass_____")).is_err());
    assert!(spawn("not-hexadecimal").is_err());
}
