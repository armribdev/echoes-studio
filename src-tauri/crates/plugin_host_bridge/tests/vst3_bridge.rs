//! The real bridge binary hosting the VST3 test effect (ADR-033, step 5).
//! Windows-only: the shared region is implemented on Windows only.
#![cfg(windows)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use plugin_bridge::control::Op;
use plugin_bridge::protocol::{
    WireParam, BLOCK_PARAMS_RESCAN_MASK, BLOCK_PARAMS_RESCAN_SHIFT, BLOCK_PLUGIN_EDITED,
    BLOCK_RESTART_REQUESTED, PARAMS_INFO, PARAMS_TEXT, PARAM_GESTURE_BEGIN, PARAM_GESTURE_END,
    PARAM_MODULATION, PARAM_VALUE,
};
use plugin_bridge::records::{
    Handshake, ParamRecord, ParamUnit, PortDirection, PortRole, SpeakerLayout, PARAM_AUTOMATABLE,
    PARAM_HIDDEN,
};
use plugin_bridge::session::{
    BlockOutcome, BlockReply, BlockRequest, BridgeLayout, BridgeProcess,
    DEAD_AFTER_CONSECUTIVE_TIMEOUTS,
};

const GAIN: u32 = 0;
const HANG: u32 = 1;
const CRASH: u32 = 2;
const SELF_VALUE: u32 = 7;
const WRITE_OUTPUT: u32 = 8;
const HOLD_EDIT: u32 = 9;
const LOOKAHEAD: u32 = 10;
const RENAME: u32 = 11;
const PROCESSOR_SELF: u32 = 12;
const PARAMETER_COUNT: usize = 23;
const LOOKAHEAD_LATENCY: u32 = 128;
const WRITTEN_VALUE: f64 = 0.9;
const HELD_VALUE: f64 = 0.25;
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
    flags: u32,
    /// What the plugin did to its own parameters, and its gestures.
    edits: Vec<WireParam>,
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
    let (flags, edit_count) = match outcome {
        BlockOutcome::Processed {
            flags, edit_count, ..
        } => (flags, edit_count),
        _ => (0, 0),
    };
    Block {
        output,
        outcome,
        flags,
        edits: edits[..edit_count].to_vec(),
    }
}

fn value(native_id: u32, normalised: f64) -> WireParam {
    WireParam::new(PARAM_VALUE, 0, native_id, normalised)
}

fn value_at(offset: u32, native_id: u32, normalised: f64) -> WireParam {
    WireParam::new(PARAM_VALUE, offset, native_id, normalised)
}

fn param_value(bridge: &BridgeProcess, native_id: u32) -> f64 {
    let reply = bridge
        .request(Op::ParamValue, &native_id.to_le_bytes(), TIMEOUT)
        .expect("param value");
    f64::from_le_bytes(reply.try_into().expect("a value is eight bytes"))
}

fn param_text(bridge: &BridgeProcess, native_id: u32, normalised: f64) -> String {
    let mut payload = native_id.to_le_bytes().to_vec();
    payload.extend_from_slice(&normalised.to_le_bytes());
    let reply = bridge
        .request(Op::ParamText, &payload, TIMEOUT)
        .expect("param text");
    String::from_utf8(reply).expect("text")
}

fn params(bridge: &BridgeProcess) -> Vec<ParamRecord> {
    let reply = bridge.request(Op::Params, &[], TIMEOUT).expect("params");
    ParamRecord::decode_list(&reply).expect("a parameter list")
}

fn rescan_bits(flags: u32) -> u32 {
    (flags & BLOCK_PARAMS_RESCAN_MASK) >> BLOCK_PARAMS_RESCAN_SHIFT
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

/// Presets are step 9 of ADR-033.
#[test]
fn what_a_vst3_plugin_cannot_do_yet_is_refused_and_the_channel_stays_open() {
    let bridge = activated();
    assert!(bridge
        .request(Op::PresetLoad, &0u32.to_le_bytes(), TIMEOUT)
        .is_err());
    assert!(bridge.request(Op::SaveState, &[], TIMEOUT).is_ok());
}

#[test]
fn the_parameter_list_comes_from_the_controller_in_normalised_units() {
    let bridge = activated();
    let list = params(&bridge);
    assert_eq!(list.len(), PARAMETER_COUNT);
    let gain = list.iter().find(|p| p.native_id == GAIN).unwrap();
    assert_eq!(gain.name, "Gain");
    assert_eq!((gain.min, gain.max, gain.default), (0.0, 1.0, 0.5));
    assert_ne!(gain.flags & PARAM_AUTOMATABLE, 0);
    assert_eq!(gain.flags & PARAM_HIDDEN, 0);
    let diagnostic = list.iter().find(|p| p.native_id == PROCESSOR_SELF).unwrap();
    assert_eq!(diagnostic.flags & PARAM_AUTOMATABLE, 0, "read-only");
}

#[test]
fn the_value_and_the_text_of_a_parameter_come_from_the_plugin() {
    let bridge = activated();
    assert_eq!(param_value(&bridge, GAIN), 0.5, "a gain of 1 in 0..2");
    bridge
        .request(Op::LoadState, &1.5f64.to_le_bytes(), TIMEOUT)
        .unwrap();
    assert_eq!(param_value(&bridge, GAIN), 0.75);
    assert_eq!(param_text(&bridge, GAIN, 0.25), "0.50", "the plugin's text");
    assert_eq!(
        param_text(&bridge, 999, 0.25),
        "",
        "no text for an unknown id"
    );
}

#[test]
fn a_parameter_value_is_refused_cleanly_when_the_id_is_unknown_and_the_channel_stays_open() {
    let bridge = activated();
    assert_eq!(param_value(&bridge, 999), 0.0);
    assert!(bridge.request(Op::ParamValue, &[1], TIMEOUT).is_err());
    assert!(bridge.request(Op::SaveState, &[], TIMEOUT).is_ok());
}

#[test]
fn several_points_of_one_parameter_apply_in_order_at_their_offsets() {
    let bridge = activated();
    let block = process(
        &bridge,
        0,
        &[value_at(0, GAIN, 0.25), value_at(32, GAIN, 1.0)],
        TIMEOUT,
    );
    let input = ramp();
    let half = FRAMES / 2;
    // Planar: the first half of each channel, then the second.
    for channel in 0..CHANNELS {
        for frame in 0..FRAMES {
            let gain = if frame < half { 0.5 } else { 2.0 };
            let index = channel * FRAMES + frame;
            assert_eq!(block.output[index], input[index] * gain, "frame {frame}");
        }
    }
}

#[test]
fn flushing_values_puts_a_parameter_on_its_base_before_the_state_is_saved() {
    let bridge = activated();
    bridge
        .request(Op::LoadState, &1.0f64.to_le_bytes(), TIMEOUT)
        .unwrap();
    // Modulation moved the gain off the user's setting (a normalised 0.75 is 1.5).
    process(&bridge, 0, &[value(GAIN, 0.1)], TIMEOUT);
    assert_eq!(param_value(&bridge, GAIN), 0.1);

    let flush: Vec<u8> = [
        GAIN.to_le_bytes().as_slice(),
        0.75f64.to_le_bytes().as_slice(),
    ]
    .concat();
    bridge.request(Op::FlushValues, &flush, TIMEOUT).unwrap();
    assert_eq!(
        bridge.request(Op::SaveState, &[], TIMEOUT).unwrap(),
        1.5f64.to_le_bytes(),
        "the state holds the user's setting, not a point of the modulation"
    );
}

#[test]
fn the_plugins_own_edit_is_forwarded_to_its_processor_and_comes_back_with_its_gesture() {
    let bridge = activated();
    let held = process(&bridge, 0, &[value(HOLD_EDIT, 1.0)], TIMEOUT);
    assert_ne!(
        held.flags & BLOCK_PLUGIN_EDITED,
        0,
        "an edit in the plugin window marks the project modified"
    );
    assert!(held.edits.is_empty(), "the edit happened during this block");

    let next = process(&bridge, 1, &[], TIMEOUT);
    assert_eq!(
        next.edits,
        [
            WireParam::new(PARAM_GESTURE_BEGIN, 0, SELF_VALUE, 0.0),
            WireParam::new(PARAM_VALUE, 0, SELF_VALUE, HELD_VALUE),
        ]
    );
    assert_eq!(
        param_value(&bridge, PROCESSOR_SELF),
        HELD_VALUE,
        "the controller's edit reached the processor"
    );

    let released = process(&bridge, 2, &[value(HOLD_EDIT, 0.0)], TIMEOUT);
    assert!(released.edits.is_empty(), "the gesture is still open");
    let ended = process(&bridge, 3, &[], TIMEOUT);
    assert_eq!(
        ended.edits,
        [WireParam::new(PARAM_GESTURE_END, 0, SELF_VALUE, 0.0)]
    );
    assert_eq!(process(&bridge, 4, &[], TIMEOUT).edits, []);
}

#[test]
fn what_the_processor_writes_to_its_output_stays_for_the_controller() {
    let bridge = activated();
    let block = process(&bridge, 0, &[value(WRITE_OUTPUT, 1.0)], TIMEOUT);
    assert_eq!(block.flags & BLOCK_PLUGIN_EDITED, 0, "not an edit");
    assert!(block.edits.is_empty());
    assert_eq!(param_value(&bridge, PROCESSOR_SELF), WRITTEN_VALUE);
    assert_eq!(
        param_value(&bridge, SELF_VALUE),
        0.0,
        "no window is open, so the controller is not told"
    );
}

#[test]
fn a_restart_request_comes_back_with_the_block_and_the_next_activation_reads_the_new_latency() {
    let bridge = activated();
    let block = process(&bridge, 0, &[value(LOOKAHEAD, 1.0)], TIMEOUT);
    assert_ne!(block.flags & BLOCK_RESTART_REQUESTED, 0);
    assert_eq!(
        process(&bridge, 1, &[], TIMEOUT).flags & BLOCK_RESTART_REQUESTED,
        0,
        "reported once"
    );
    assert_eq!(
        activate(&bridge),
        LOOKAHEAD_LATENCY,
        "the same activation request now reactivates the plugin"
    );
    assert_eq!(activate(&bridge), LOOKAHEAD_LATENCY, "and only once");
    assert_eq!(
        process(&bridge, 2, &[], TIMEOUT).output,
        ramp(),
        "the plugin still processes after its reactivation"
    );
}

#[test]
fn new_parameter_titles_come_back_as_a_rescan_and_the_list_follows() {
    let bridge = activated();
    let name = |bridge: &BridgeProcess| {
        params(bridge)
            .into_iter()
            .find(|p| p.native_id == SELF_VALUE)
            .unwrap()
            .name
    };
    assert_eq!(name(&bridge), "Self value");
    let block = process(&bridge, 0, &[value(RENAME, 1.0)], TIMEOUT);
    assert_eq!(
        rescan_bits(block.flags),
        PARAMS_TEXT | PARAMS_INFO,
        "texts and titles changed"
    );
    assert_eq!(rescan_bits(process(&bridge, 1, &[], TIMEOUT).flags), 0);
    assert_eq!(name(&bridge), "Renamed value");
}

#[test]
fn an_idle_plugin_reports_nothing_in_its_replies() {
    let bridge = activated();
    for sequence in 0..4 {
        let block = process(&bridge, sequence, &[], TIMEOUT);
        assert!(block.edits.is_empty());
        assert_eq!(block.flags, 0);
    }
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

/// ADR-033 open question 2: the plugin loads on the bridge main thread, which
/// `OleInitialize` made the process main single-threaded apartment; the audio thread
/// has no COM, as the engine audio thread has none.
#[test]
fn the_plugin_loads_in_the_main_apartment_and_processes_without_com() {
    const INIT_APARTMENT: u32 = 3;
    const PROCESS_APARTMENT: u32 = 4;
    const DIAGNOSTIC_RANGE: f64 = 1_000_000.0;
    const MAIN_SINGLE_THREADED: f64 = 4.0;
    const NO_COM: f64 = 0.0;

    let bridge = activated();
    process(&bridge, 0, &[], TIMEOUT);
    assert_eq!(
        param_value(&bridge, INIT_APARTMENT) * DIAGNOSTIC_RANGE,
        MAIN_SINGLE_THREADED
    );
    assert_eq!(
        param_value(&bridge, PROCESS_APARTMENT) * DIAGNOSTIC_RANGE,
        NO_COM
    );
}

/// Step 6, decision b: gestures from the engine (`IEditControllerHostEditing`, appendix
/// B of the ADR) stay ignored (the step 7 editor did not need them), and so is a
/// modulation offset: VST3 is modulated by value around a base (ADR-021).
#[test]
fn gestures_and_modulation_offsets_from_the_engine_are_ignored() {
    let bridge = activated();
    let block = process(
        &bridge,
        0,
        &[
            WireParam::new(PARAM_GESTURE_BEGIN, 0, GAIN, 0.0),
            WireParam::new(PARAM_MODULATION, 0, GAIN, 1.0),
            WireParam::new(PARAM_GESTURE_END, 0, GAIN, 0.0),
        ],
        TIMEOUT,
    );
    assert!(matches!(block.outcome, BlockOutcome::Processed { .. }));
    assert_eq!(block.output, ramp(), "unity gain: nothing was applied");
    assert!(block.edits.is_empty());
    assert_eq!(param_value(&bridge, GAIN), 0.5);
}

// ── The editor (ADR-033, step 7) ──────────────────────────────────────────────
//
// The test plugin's view records what the host does to it (`View calls`: the last six
// calls as decimal digits) and reads like any parameter. The window belongs to the
// bridge process; the test looks at it from outside, as the user would.

const VIEW_CALLS: u32 = 13;
const VIEW_WIDTH: u32 = 14;
const VIEW_HEIGHT: u32 = 15;
const VIEW_SCALE: u32 = 16;
const VIEW_ASK: u32 = 17;
const VIEW_CRASH: u32 = 18;
const VIEW_EDIT: u32 = 19;
const DIAGNOSTIC_RANGE: f64 = 1_000_000.0;
const VIEW_INITIAL_SIZE: (i32, i32) = (400, 300);

// setFrame, attached, setContentScaleFactor, setFrame(null), removed, last release.
const OPEN_AND_CLOSE_CALLS: u32 = 138_259;
// The same, with a view that asks for a size from inside `attached`: its `onSize`
// and the answer of `resizeView` come first (the oldest two calls drop off).
const OPEN_ASKING_AND_CLOSE_CALLS: u32 = 468_259;

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[link(name = "user32")]
extern "system" {
    fn IsWindow(hwnd: *mut std::ffi::c_void) -> i32;
    fn GetWindow(hwnd: *mut std::ffi::c_void, command: u32) -> *mut std::ffi::c_void;
    fn GetClientRect(hwnd: *mut std::ffi::c_void, rect: *mut Rect) -> i32;
    fn GetDpiForWindow(hwnd: *mut std::ffi::c_void) -> u32;
    fn GetWindowThreadProcessId(hwnd: *mut std::ffi::c_void, process: *mut u32) -> u32;
    fn PostMessageW(hwnd: *mut std::ffi::c_void, message: u32, wparam: usize, lparam: isize)
        -> i32;
}

const GW_CHILD: u32 = 5;
const WM_CLOSE: u32 = 0x0010;

fn diagnostic(bridge: &BridgeProcess, native_id: u32) -> u32 {
    (param_value(bridge, native_id) * DIAGNOSTIC_RANGE).round() as u32
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Asks the view to request `width` × `height` from inside `attached`.
fn make_the_view_ask(bridge: &BridgeProcess, width: u32, height: u32) {
    let ask = f64::from(width * 1000 + height) / DIAGNOSTIC_RANGE;
    process(bridge, 0, &[value(VIEW_ASK, ask)], TIMEOUT);
}

fn open_editor(bridge: &BridgeProcess) -> *mut std::ffi::c_void {
    let reply = bridge
        .request(Op::OpenEditor, b"Test effect", TIMEOUT)
        .expect("the editor opens");
    u64::from_le_bytes(reply.try_into().expect("a window handle is eight bytes")) as usize
        as *mut std::ffi::c_void
}

fn plugin_area_size(window: *mut std::ffi::c_void) -> (i32, i32) {
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `window` came from the bridge; both calls only read window state.
    unsafe { GetClientRect(GetWindow(window, GW_CHILD), &mut rect) };
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// The user closes the window with its × button: `WM_CLOSE`, handled by the bridge.
fn close_window(bridge: &BridgeProcess, window: *mut std::ffi::c_void) {
    // SAFETY: posting a message to a window of another process is allowed.
    unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
    wait_until("the window to close", || unsafe { IsWindow(window) } == 0);
    wait_until("the view to be released", || {
        diagnostic(bridge, VIEW_CALLS) % 10 == 9
    });
}

#[test]
fn the_editor_of_a_sandboxed_effect_opens_in_a_bridge_window_and_closes_in_order() {
    let bridge = activated();

    let window = open_editor(&bridge);
    let mut owner = 0u32;
    // SAFETY: reads which process owns the window.
    unsafe {
        assert_ne!(IsWindow(window), 0, "the reply is a live window");
        GetWindowThreadProcessId(window, &mut owner);
    }
    assert_eq!(
        owner,
        bridge.process_id(),
        "the window belongs to the bridge"
    );
    assert_eq!(plugin_area_size(window), VIEW_INITIAL_SIZE);

    close_window(&bridge, window);
    assert_eq!(diagnostic(&bridge, VIEW_CALLS), OPEN_AND_CLOSE_CALLS);
    assert!(
        bridge.request(Op::SaveState, &[], TIMEOUT).is_ok(),
        "the bridge is still serving"
    );
}

#[test]
fn a_size_the_view_asks_for_while_attaching_resizes_the_window_and_reaches_on_size() {
    let bridge = activated();
    make_the_view_ask(&bridge, 700, 500);

    let window = open_editor(&bridge);
    assert_eq!(plugin_area_size(window), (700, 500));
    assert_eq!(
        (
            diagnostic(&bridge, VIEW_WIDTH),
            diagnostic(&bridge, VIEW_HEIGHT)
        ),
        (700, 500),
        "the view was told its new size"
    );

    close_window(&bridge, window);
    assert_eq!(
        diagnostic(&bridge, VIEW_CALLS),
        OPEN_ASKING_AND_CLOSE_CALLS,
        "onSize and the accepted answer come inside the one resizeView call"
    );
}

#[test]
fn the_view_is_given_the_scale_of_its_window() {
    let bridge = activated();
    let window = open_editor(&bridge);

    // SAFETY: reads a property of a window of another process.
    let dpi = unsafe { GetDpiForWindow(window) };
    assert_ne!(dpi, 0);
    let percent = (f64::from(dpi) / 96.0 * 100.0).round() as u32;
    assert_eq!(diagnostic(&bridge, VIEW_SCALE), percent);

    close_window(&bridge, window);
}

#[test]
fn an_instance_has_one_window_and_opening_again_brings_it_back_and_a_new_one_follows_a_close() {
    let bridge = activated();
    let first = open_editor(&bridge);
    assert_eq!(open_editor(&bridge), first, "the same window");
    assert_eq!(
        diagnostic(&bridge, VIEW_CALLS),
        138,
        "no second view: setFrame, attached, setContentScaleFactor"
    );

    close_window(&bridge, first);
    let second = open_editor(&bridge);
    // SAFETY: only asks whether the handle is a live window.
    assert_ne!(unsafe { IsWindow(second) }, 0);
    close_window(&bridge, second);
    assert_eq!(
        diagnostic(&bridge, VIEW_CALLS),
        OPEN_AND_CLOSE_CALLS,
        "the second round is the same sequence"
    );
}

#[test]
fn the_controller_follows_the_processor_while_the_window_is_open_and_not_after() {
    let bridge = activated();
    process(&bridge, 0, &[value(SELF_VALUE, 0.7)], TIMEOUT);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        param_value(&bridge, SELF_VALUE),
        0.0,
        "no window, no follow"
    );

    let window = open_editor(&bridge);
    process(&bridge, 1, &[value(SELF_VALUE, 0.6)], TIMEOUT);
    wait_until("the knob to follow the automation", || {
        param_value(&bridge, SELF_VALUE) == 0.6
    });

    close_window(&bridge, window);
    process(&bridge, 2, &[value(SELF_VALUE, 0.3)], TIMEOUT);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(param_value(&bridge, SELF_VALUE), 0.6, "closed again");
}

#[test]
fn an_edit_made_in_the_window_marks_the_project_and_reaches_the_engine_with_the_next_block() {
    let bridge = activated();
    process(&bridge, 0, &[value(VIEW_EDIT, 1.0)], TIMEOUT);

    let window = open_editor(&bridge);
    let block = process(&bridge, 1, &[], TIMEOUT);
    assert_ne!(
        block.flags & BLOCK_PLUGIN_EDITED,
        0,
        "the project reads as modified"
    );
    assert!(
        block.edits.iter().any(|edit| edit.kind == PARAM_VALUE
            && edit.native_id == SELF_VALUE
            && edit.value == HELD_VALUE),
        "{:?}",
        block.edits
    );
    close_window(&bridge, window);
}

#[test]
fn a_crash_in_the_editor_only_kills_its_bridge() {
    let bridge = activated();
    process(&bridge, 0, &[value(VIEW_CRASH, 1.0)], TIMEOUT);

    assert!(bridge.request(Op::OpenEditor, b"Crash", TIMEOUT).is_err());
    for sequence in 1..=DEAD_AFTER_CONSECUTIVE_TIMEOUTS as u64 {
        process(&bridge, sequence, &[], Duration::from_millis(20));
    }
    assert!(bridge.audio().is_dead());
    assert!(bridge.request(Op::SaveState, &[], TIMEOUT).is_err());
}
