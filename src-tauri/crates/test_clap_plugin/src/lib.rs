//! A minimal CLAP effect for the plugin-sandbox tests (ADR-015).
//!
//! Output = input × (gain + gain modulation).  Without an audio input (hosted
//! as an instrument), every sample is the held note's velocity × gain, and
//! silence once it is released.  Parameters:
//! - 0 `Gain`  (0..2, modulatable) — saved in the plugin state;
//! - 1 `Hang`  — once set, every block sleeps for seconds (a hung plugin);
//! - 2 `Crash` — once set, the next block aborts the process;
//! - 3 `Touch` — once set, the plugin reports an edit, as a drag in its window would;
//! - 4 `Lookahead` — switches the latency between 64 and 128 samples: like a
//!   real plugin, it requests a restart and applies it at the next `activate`;
//! - 5 `Wrong thread` — read-only: how many `start_processing` / `process` /
//!   `stop_processing` calls the host's `clap.thread-check` did not report as
//!   the audio thread.
//! - 6 `Status` — the `clap_process_status` reported while the output is silent
//!   (default `CONTINUE`; 2 `CONTINUE_IF_NOT_QUIET`, 3 `TAIL`, 4 `SLEEP`);
//! - 7 `Tail` — samples reported by `clap.tail`; 0 (default) means the plugin
//!   does not provide the extension.  Changing it calls the host's `tail.changed`;
//! - 8 `Process calls` — read-only: how many times `process` ran;
//! - 9 `Liar` — once set, `Status` is reported even while the output sounds;
//! - 10 `Rescan` — the value is sent to the host's `clap.params.rescan` as flags;
//! - 11 `Request flush` — once set, calls the host's `clap.params.request_flush`.
//!
//! - 12..18 — read-only: what the plugin saw through the host's `clap.track-info`.
//!   It reads the track in `init` and again whenever the host calls its
//!   `clap_plugin_track_info.changed`: `Track flags`, `Track color` (ARGB),
//!   `Track name` (its first 6 bytes, big-endian), `Track name length`,
//!   `Track info reads` (successful `get` calls), `Track info changes`
//!   (`changed` calls) and `Track info audio calls` (`changed` calls the host's
//!   `clap.thread-check` reported as the audio thread — always 0 when it behaves).
//!
//! - 19 `Note names set` — 0 (default) or 1 picks which note names the plugin
//!   gives through `clap.note-name` (0: Kick on key 36 and Snare on 38, any
//!   port and channel; 1: Tom on key 41, port 0, channel 9); changing it calls
//!   the host's `clap.note-name.changed`;
//! - 20 `Note ports rescan` — the value is sent to the host's
//!   `clap.note-ports.rescan` as flags;
//! - 21 `Host note dialects` — read-only: what the host's
//!   `clap.note-ports.supported_dialects` answered in `init`.
//!
//! - 22 `Timer register` — setting it calls the host's `clap.timer-support`
//!   `register_timer` with the value as the period in ms (from whatever thread
//!   applies the event; 0 is an invalid period the host must refuse);
//! - 23 `Timer unregister` — setting it calls `unregister_timer` with the value as the id;
//! - 24 `Timer register result` — read-only: 0 never called, 1 accepted, 2 refused;
//! - 25 `Timer last id` — read-only: the id the last `register_timer` wrote
//!   (`u32::MAX` after a refusal);
//! - 26 `Timer unregister result` — read-only: 0 never called, 1 accepted, 2 refused;
//! - 27 `Timer calls` — read-only: how many times the host called `on_timer`;
//! - 28 `Timer last fired` — read-only: the id of the last `on_timer`;
//! - 29 `Timer audio calls` — read-only: `on_timer` calls `clap.thread-check`
//!   reported as the audio thread (always 0 when the host behaves).
//!
//! - 30 `Preset loaded` — setting it calls the host's `clap.preset-load.loaded`, as
//!   a preset picked in the plugin's window would;
//! - 31 `Preset error` — setting it calls the host's `clap.preset-load.on_error`.
//!
//! `Gain` renders its value as decibels through `clap.params.value_to_text`.
//!
//! A second plugin, `echoes.test.sleepy`, is the same code reporting `SLEEP`
//! by default, for projects that cannot set parameters.
//!
//! Reports 64 samples of latency by default.  Its `clap.gui` is a stub (a 320×200
//! area that draws nothing) so a test can open the host's real editor window.
//!
//! Declares `clap.audio-ports`: one main stereo input/output plus one
//! auxiliary stereo input (sidechain), so a test can prove real bus
//! introspection (ADR-014) end to end — the scanner reads the aux port, and
//! `process` sums it verbatim into the output whenever the host hands it a
//! second input port, so a test can detect it landed.  It also declares one
//! auxiliary **mono** output (multi-out), carrying half the main output.

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};

use clap_ffi::{
    ClapAudioPortInfo, ClapEventHeader, ClapEventNote, ClapEventParamGesture, ClapEventParamValue,
    ClapGuiExtension, ClapHost, ClapHostLatency, ClapHostNoteName, ClapHostNotePorts,
    ClapHostParams, ClapHostPresetLoad, ClapHostState, ClapHostTail, ClapHostThreadCheck,
    ClapHostTimerSupport, ClapHostTrackInfo, ClapIStream, ClapInputEvents, ClapNoteName,
    ClapOStream, ClapOutputEvents, ClapParamInfo, ClapPlugin, ClapPluginAudioPorts,
    ClapPluginDescriptor, ClapPluginEntry, ClapPluginFactory, ClapPluginLatency,
    ClapPluginNoteName, ClapPluginParams, ClapPluginState, ClapPluginTail, ClapPluginTimerSupport,
    ClapPluginTrackInfo, ClapProcess, ClapProcessStatus, ClapTrackInfo, ClapWindow,
    CLAP_AUDIO_PORT_IS_MAIN, CLAP_CORE_EVENT_SPACE, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON,
    CLAP_EVENT_PARAM_GESTURE_END, CLAP_EVENT_PARAM_MOD, CLAP_EVENT_PARAM_VALUE,
    CLAP_EXT_AUDIO_PORTS, CLAP_EXT_GUI, CLAP_EXT_LATENCY, CLAP_EXT_NOTE_NAME, CLAP_EXT_NOTE_PORTS,
    CLAP_EXT_PARAMS, CLAP_EXT_PRESET_LOAD, CLAP_EXT_STATE, CLAP_EXT_TAIL, CLAP_EXT_THREAD_CHECK,
    CLAP_EXT_TIMER_SUPPORT, CLAP_EXT_TRACK_INFO, CLAP_PARAM_IS_AUTOMATABLE,
    CLAP_PARAM_IS_MODULATABLE, CLAP_PLUGIN_FACTORY_ID, CLAP_PROCESS_CONTINUE, CLAP_PROCESS_SLEEP,
    CLAP_VERSION_1, CLAP_WINDOW_API_WIN32,
};

pub const PLUGIN_ID: &str = "echoes.test.gain";
pub const SLEEPY_PLUGIN_ID: &str = "echoes.test.sleepy";
pub const LATENCY: u32 = 64;
pub const LOOKAHEAD_LATENCY: u32 = 128;
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
const PARAM_COUNT: u32 = 32;
/// `Timer register result` / `Timer unregister result` once the host answered.
const TIMER_ACCEPTED: u32 = 1;
const TIMER_REFUSED: u32 = 2;
/// Bytes of the track name `Track name` packs.
const PACKED_NAME_BYTES: usize = 6;

struct Descriptor(ClapPluginDescriptor);
struct Features([*const c_char; 2]);
// SAFETY: every pointer targets 'static, immutable data.
unsafe impl Sync for Descriptor {}
unsafe impl Sync for Features {}

static FEATURES: Features = Features([c"audio-effect".as_ptr(), std::ptr::null()]);

static DESCRIPTOR: Descriptor = Descriptor(ClapPluginDescriptor {
    clap_version: CLAP_VERSION_1,
    id: c"echoes.test.gain".as_ptr(),
    name: c"Echoes Test Gain".as_ptr(),
    vendor: c"Echoes Studio".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"Sandbox test plugin".as_ptr(),
    features: FEATURES.0.as_ptr(),
});

static SLEEPY_DESCRIPTOR: Descriptor = Descriptor(ClapPluginDescriptor {
    clap_version: CLAP_VERSION_1,
    id: c"echoes.test.sleepy".as_ptr(),
    name: c"Echoes Test Sleepy".as_ptr(),
    vendor: c"Echoes Studio".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"Sandbox test plugin that reports SLEEP when silent".as_ptr(),
    features: FEATURES.0.as_ptr(),
});

#[no_mangle]
pub static clap_entry: ClapPluginEntry = ClapPluginEntry {
    clap_version: CLAP_VERSION_1,
    init: entry_init,
    deinit: entry_deinit,
    get_factory: entry_get_factory,
};

unsafe extern "C" fn entry_init(_: *const c_char) -> bool {
    true
}

unsafe extern "C" fn entry_deinit() {}

static FACTORY: ClapPluginFactory = ClapPluginFactory {
    get_plugin_count: factory_count,
    get_plugin_descriptor: factory_descriptor,
    create_plugin: factory_create,
};

unsafe extern "C" fn entry_get_factory(id: *const c_char) -> *const c_void {
    if CStr::from_ptr(id).to_bytes_with_nul() == CLAP_PLUGIN_FACTORY_ID {
        &FACTORY as *const ClapPluginFactory as *const c_void
    } else {
        std::ptr::null()
    }
}

unsafe extern "C" fn factory_count(_: *const ClapPluginFactory) -> u32 {
    2
}

unsafe extern "C" fn factory_descriptor(
    _: *const ClapPluginFactory,
    index: u32,
) -> *const ClapPluginDescriptor {
    match index {
        0 => &DESCRIPTOR.0,
        1 => &SLEEPY_DESCRIPTOR.0,
        _ => std::ptr::null(),
    }
}

struct State {
    host: *const ClapHost,
    gain: AtomicU64,
    gain_modulation: f64,
    note_velocity: f64,
    hang: AtomicBool,
    crash: bool,
    touch: bool,
    /// Latency reported since the last `activate`.
    latency: u32,
    /// Latency the next `activate` applies.
    pending_latency: AtomicU32,
    wrong_thread_calls: AtomicU32,
    status: AtomicI32,
    tail: AtomicU32,
    liar: AtomicBool,
    process_calls: AtomicU32,
    track_flags: AtomicU64,
    track_color: AtomicU32,
    track_name: AtomicU64,
    track_name_length: AtomicU32,
    track_info_reads: AtomicU32,
    track_info_changes: AtomicU32,
    track_info_audio_calls: AtomicU32,
    note_names_set: AtomicU32,
    host_note_dialects: AtomicU32,
    timer_register_result: AtomicU32,
    timer_last_id: AtomicU32,
    timer_unregister_result: AtomicU32,
    timer_calls: AtomicU32,
    timer_last_fired: AtomicU32,
    timer_audio_calls: AtomicU32,
}

unsafe extern "C" fn factory_create(
    _: *const ClapPluginFactory,
    host: *const ClapHost,
    id: *const c_char,
) -> *const ClapPlugin {
    let id = CStr::from_ptr(id).to_bytes();
    let (descriptor, status) = if id == PLUGIN_ID.as_bytes() {
        (&DESCRIPTOR, CLAP_PROCESS_CONTINUE)
    } else if id == SLEEPY_PLUGIN_ID.as_bytes() {
        (&SLEEPY_DESCRIPTOR, CLAP_PROCESS_SLEEP)
    } else {
        return std::ptr::null();
    };
    let state = Box::new(State {
        host,
        gain: AtomicU64::new(1.0f64.to_bits()),
        gain_modulation: 0.0,
        note_velocity: 0.0,
        hang: AtomicBool::new(false),
        crash: false,
        touch: false,
        latency: LATENCY,
        pending_latency: AtomicU32::new(LATENCY),
        wrong_thread_calls: AtomicU32::new(0),
        status: AtomicI32::new(status),
        tail: AtomicU32::new(0),
        liar: AtomicBool::new(false),
        process_calls: AtomicU32::new(0),
        track_flags: AtomicU64::new(0),
        track_color: AtomicU32::new(0),
        track_name: AtomicU64::new(0),
        track_name_length: AtomicU32::new(0),
        track_info_reads: AtomicU32::new(0),
        track_info_changes: AtomicU32::new(0),
        track_info_audio_calls: AtomicU32::new(0),
        note_names_set: AtomicU32::new(0),
        host_note_dialects: AtomicU32::new(0),
        timer_register_result: AtomicU32::new(0),
        timer_last_id: AtomicU32::new(0),
        timer_unregister_result: AtomicU32::new(0),
        timer_calls: AtomicU32::new(0),
        timer_last_fired: AtomicU32::new(0),
        timer_audio_calls: AtomicU32::new(0),
    });
    Box::into_raw(Box::new(ClapPlugin {
        desc: &descriptor.0,
        plugin_data: Box::into_raw(state) as *mut c_void,
        init: plugin_init,
        destroy: plugin_destroy,
        activate: plugin_activate,
        deactivate: plugin_noop,
        start_processing: plugin_start,
        stop_processing: plugin_stop,
        reset: plugin_noop,
        process: plugin_process,
        get_extension: plugin_get_extension,
        on_main_thread: plugin_noop,
    }))
}

unsafe fn state<'a>(plugin: *const ClapPlugin) -> &'a mut State {
    &mut *((*plugin).plugin_data as *mut State)
}

unsafe extern "C" fn plugin_init(plugin: *const ClapPlugin) -> bool {
    let state = state(plugin);
    read_track_info(state);
    read_host_note_dialects(state);
    true
}

fn read_host_note_dialects(state: &State) {
    // SAFETY: the host outlives the plugin; its extension tables are static.
    let dialects = unsafe {
        let extension = ((*state.host).get_extension)(
            state.host,
            CLAP_EXT_NOTE_PORTS.as_ptr() as *const c_char,
        );
        match (extension as *const ClapHostNotePorts).as_ref() {
            Some(ports) => (ports.supported_dialects)(state.host),
            None => 0,
        }
    };
    state.host_note_dialects.store(dialects, Ordering::Relaxed);
}

fn rescan_note_ports(state: &State, flags: u32) {
    // SAFETY: the host outlives the plugin; `rescan` only records the request.
    unsafe {
        let extension = ((*state.host).get_extension)(
            state.host,
            CLAP_EXT_NOTE_PORTS.as_ptr() as *const c_char,
        );
        if let Some(ports) = (extension as *const ClapHostNotePorts).as_ref() {
            (ports.rescan)(state.host, flags);
        }
    }
}

fn change_note_names(state: &State, set: u32) {
    if state.note_names_set.swap(set, Ordering::AcqRel) == set {
        return;
    }
    // SAFETY: the host outlives the plugin; `changed` is thread-safe.
    unsafe {
        let extension =
            ((*state.host).get_extension)(state.host, CLAP_EXT_NOTE_NAME.as_ptr() as *const c_char);
        if let Some(names) = (extension as *const ClapHostNoteName).as_ref() {
            (names.changed)(state.host);
        }
    }
}

/// What a plugin does with `clap.track-info`: asks the host for its track.
fn read_track_info(state: &State) {
    // SAFETY: the host outlives the plugin; its extension tables are static.
    let info = unsafe {
        let extension = ((*state.host).get_extension)(
            state.host,
            CLAP_EXT_TRACK_INFO.as_ptr() as *const c_char,
        );
        let Some(extension) = (extension as *const ClapHostTrackInfo).as_ref() else {
            return;
        };
        let mut info: ClapTrackInfo = std::mem::zeroed();
        if !(extension.get)(state.host, &mut info) {
            return;
        }
        info
    };
    let name_length = info.name.iter().position(|&byte| byte == 0).unwrap_or(0);
    let packed_name = info.name[..name_length.min(PACKED_NAME_BYTES)]
        .iter()
        .fold(0u64, |packed, &byte| (packed << 8) | u64::from(byte));
    let color = u32::from_be_bytes([
        info.color.alpha,
        info.color.red,
        info.color.green,
        info.color.blue,
    ]);
    state.track_flags.store(info.flags, Ordering::Relaxed);
    state.track_color.store(color, Ordering::Relaxed);
    state.track_name.store(packed_name, Ordering::Relaxed);
    state
        .track_name_length
        .store(name_length as u32, Ordering::Relaxed);
    state.track_info_reads.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn track_info_changed(plugin: *const ClapPlugin) {
    let state = state(plugin);
    state.track_info_changes.fetch_add(1, Ordering::Relaxed);
    if on_audio_thread(state) {
        state.track_info_audio_calls.fetch_add(1, Ordering::Relaxed);
    }
    read_track_info(state);
}

fn host_timers(state: &State) -> Option<&ClapHostTimerSupport> {
    // SAFETY: the host outlives the plugin; its extension tables are static.
    unsafe {
        let extension = ((*state.host).get_extension)(
            state.host,
            CLAP_EXT_TIMER_SUPPORT.as_ptr() as *const c_char,
        );
        (extension as *const ClapHostTimerSupport).as_ref()
    }
}

fn answer(accepted: bool) -> u32 {
    if accepted {
        TIMER_ACCEPTED
    } else {
        TIMER_REFUSED
    }
}

fn register_timer(state: &State, period_ms: u32) {
    let mut id = 0;
    let accepted = host_timers(state).is_some_and(|timers|
        // SAFETY: `register_timer` writes the id through the pointer it is given.
        unsafe { (timers.register_timer)(state.host, period_ms, &mut id) });
    state.timer_last_id.store(id, Ordering::Relaxed);
    state
        .timer_register_result
        .store(answer(accepted), Ordering::Relaxed);
}

fn unregister_timer(state: &State, id: u32) {
    let accepted = host_timers(state)
        // SAFETY: `unregister_timer` only reads the id.
        .is_some_and(|timers| unsafe { (timers.unregister_timer)(state.host, id) });
    state
        .timer_unregister_result
        .store(answer(accepted), Ordering::Relaxed);
}

unsafe extern "C" fn timer_on_timer(plugin: *const ClapPlugin, timer_id: u32) {
    let state = state(plugin);
    state.timer_last_fired.store(timer_id, Ordering::Relaxed);
    if on_audio_thread(state) {
        state.timer_audio_calls.fetch_add(1, Ordering::Relaxed);
    }
    state.timer_calls.fetch_add(1, Ordering::Relaxed);
}

fn host_preset_load(state: &State) -> Option<&ClapHostPresetLoad> {
    // SAFETY: the host outlives the plugin; its extension tables are static.
    unsafe {
        let extension = ((*state.host).get_extension)(
            state.host,
            CLAP_EXT_PRESET_LOAD.as_ptr() as *const c_char,
        );
        (extension as *const ClapHostPresetLoad).as_ref()
    }
}

fn report_preset_loaded(state: &State) {
    if let Some(presets) = host_preset_load(state) {
        // SAFETY: the location is a NUL-terminated literal; the load key may be null.
        unsafe {
            (presets.loaded)(
                state.host,
                0,
                c"/presets/warm.clap-preset".as_ptr(),
                std::ptr::null(),
            )
        };
    }
}

fn report_preset_error(state: &State) {
    if let Some(presets) = host_preset_load(state) {
        // SAFETY: the strings are NUL-terminated literals; the load key may be null.
        unsafe {
            (presets.on_error)(
                state.host,
                0,
                c"/presets/missing.clap-preset".as_ptr(),
                std::ptr::null(),
                2,
                c"file not found".as_ptr(),
            )
        };
    }
}

fn on_audio_thread(state: &State) -> bool {
    // SAFETY: the host outlives the plugin; its extension tables are static.
    unsafe {
        let check = ((*state.host).get_extension)(
            state.host,
            CLAP_EXT_THREAD_CHECK.as_ptr() as *const c_char,
        );
        (check as *const ClapHostThreadCheck)
            .as_ref()
            .is_some_and(|check| (check.is_audio_thread)(state.host))
    }
}

unsafe extern "C" fn plugin_destroy(plugin: *const ClapPlugin) {
    let plugin = Box::from_raw(plugin as *mut ClapPlugin);
    drop(Box::from_raw(plugin.plugin_data as *mut State));
}

unsafe extern "C" fn plugin_activate(plugin: *const ClapPlugin, _: f64, _: u32, _: u32) -> bool {
    let state = state(plugin);
    let latency = state.pending_latency.load(Ordering::Acquire);
    if latency != state.latency {
        state.latency = latency;
        let host_latency =
            ((*state.host).get_extension)(state.host, CLAP_EXT_LATENCY.as_ptr() as *const c_char);
        if !host_latency.is_null() {
            ((*(host_latency as *const ClapHostLatency)).changed)(state.host);
        }
    }
    true
}

fn set_tail(state: &State, tail: u32) {
    if state.tail.swap(tail, Ordering::AcqRel) != tail {
        // SAFETY: the host outlives the plugin; `changed` is thread-safe.
        unsafe {
            let host_tail =
                ((*state.host).get_extension)(state.host, CLAP_EXT_TAIL.as_ptr() as *const c_char);
            if !host_tail.is_null() {
                ((*(host_tail as *const ClapHostTail)).changed)(state.host);
            }
        }
    }
}

fn host_params(state: &State) -> Option<&ClapHostParams> {
    // SAFETY: the host outlives the plugin; its extension tables are static.
    unsafe {
        let extension =
            ((*state.host).get_extension)(state.host, CLAP_EXT_PARAMS.as_ptr() as *const c_char);
        (extension as *const ClapHostParams).as_ref()
    }
}

fn rescan_params(state: &State, flags: u32) {
    if let Some(params) = host_params(state) {
        // SAFETY: `rescan` only records the flags.
        unsafe { (params.rescan)(state.host, flags) };
    }
}

fn request_flush(state: &State) {
    if let Some(params) = host_params(state) {
        // SAFETY: `request_flush` is thread-safe.
        unsafe { (params.request_flush)(state.host) };
    }
}

/// CLAP: an active plugin may only change its latency inside `activate`.
fn request_latency(state: &State, latency: u32) {
    if state.pending_latency.swap(latency, Ordering::AcqRel) != latency {
        // SAFETY: the host outlives the plugin; `request_restart` is thread-safe.
        unsafe { ((*state.host).request_restart)(state.host) };
    }
}

unsafe extern "C" fn plugin_start(plugin: *const ClapPlugin) -> bool {
    check_audio_thread(state(plugin));
    true
}

unsafe extern "C" fn plugin_stop(plugin: *const ClapPlugin) {
    check_audio_thread(state(plugin));
}

unsafe extern "C" fn plugin_noop(_: *const ClapPlugin) {}

fn apply_event(state: &mut State, header: &ClapEventHeader) {
    if header.space_id != CLAP_CORE_EVENT_SPACE {
        return;
    }
    match header.r#type {
        CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF => apply_note(state, header),
        CLAP_EVENT_PARAM_VALUE | CLAP_EVENT_PARAM_MOD => apply_param(state, header),
        _ => {}
    }
}

fn apply_note(state: &mut State, header: &ClapEventHeader) {
    // SAFETY: both types share the `clap_event_note` layout.
    let note = unsafe { &*(header as *const ClapEventHeader as *const ClapEventNote) };
    state.note_velocity = if header.r#type == CLAP_EVENT_NOTE_ON {
        note.velocity
    } else {
        0.0
    };
}

fn apply_param(state: &mut State, header: &ClapEventHeader) {
    // SAFETY: both types share the `clap_event_param_value` layout.
    let event = unsafe { &*(header as *const ClapEventHeader as *const ClapEventParamValue) };
    let on = event.value >= 0.5;
    match (header.r#type, event.param_id) {
        (CLAP_EVENT_PARAM_VALUE, GAIN) => {
            state.gain.store(event.value.to_bits(), Ordering::Relaxed)
        }
        (CLAP_EVENT_PARAM_MOD, GAIN) => state.gain_modulation = event.value,
        (CLAP_EVENT_PARAM_VALUE, HANG) => state.hang.store(on, Ordering::Relaxed),
        (CLAP_EVENT_PARAM_VALUE, CRASH) => state.crash = on,
        (CLAP_EVENT_PARAM_VALUE, TOUCH) => state.touch = on,
        (CLAP_EVENT_PARAM_VALUE, LOOKAHEAD) => {
            request_latency(state, if on { LOOKAHEAD_LATENCY } else { LATENCY })
        }
        (CLAP_EVENT_PARAM_VALUE, STATUS) => state
            .status
            .store(event.value.round() as i32, Ordering::Relaxed),
        (CLAP_EVENT_PARAM_VALUE, TAIL) => set_tail(state, event.value.round() as u32),
        (CLAP_EVENT_PARAM_VALUE, LIAR) => state.liar.store(on, Ordering::Relaxed),
        (CLAP_EVENT_PARAM_VALUE, RESCAN) => rescan_params(state, event.value.round() as u32),
        (CLAP_EVENT_PARAM_VALUE, REQUEST_FLUSH) if on => request_flush(state),
        (CLAP_EVENT_PARAM_VALUE, NOTE_NAMES_SET) => change_note_names(state, u32::from(on)),
        (CLAP_EVENT_PARAM_VALUE, NOTE_PORTS_RESCAN) => {
            rescan_note_ports(state, event.value.round() as u32)
        }
        (CLAP_EVENT_PARAM_VALUE, TIMER_REGISTER) => {
            register_timer(state, event.value.round() as u32)
        }
        (CLAP_EVENT_PARAM_VALUE, TIMER_UNREGISTER) => {
            unregister_timer(state, event.value.round() as u32)
        }
        (CLAP_EVENT_PARAM_VALUE, PRESET_LOADED) if on => report_preset_loaded(state),
        (CLAP_EVENT_PARAM_VALUE, PRESET_ERROR) if on => report_preset_error(state),
        _ => {}
    }
}

unsafe fn apply_events(state: &mut State, events: *const ClapInputEvents) {
    if events.is_null() {
        return;
    }
    for index in 0..((*events).size)(events) {
        let header = ((*events).get)(events, index);
        if !header.is_null() {
            apply_event(state, &*header);
        }
    }
}

unsafe fn report_edit(state: &mut State, out: *const ClapOutputEvents) {
    state.touch = false;
    let value = ClapEventParamValue {
        header: ClapEventHeader {
            size: std::mem::size_of::<ClapEventParamValue>() as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE,
            r#type: CLAP_EVENT_PARAM_VALUE,
            flags: 0,
        },
        param_id: GAIN,
        cookie: std::ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        _pad: 0,
        value: f64::from_bits(state.gain.load(Ordering::Relaxed)),
    };
    let gesture_end = ClapEventParamGesture {
        header: ClapEventHeader {
            size: std::mem::size_of::<ClapEventParamGesture>() as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE,
            r#type: CLAP_EVENT_PARAM_GESTURE_END,
            flags: 0,
        },
        param_id: GAIN,
    };
    ((*out).try_push)(out, &value.header);
    ((*out).try_push)(out, &gesture_end.header);
    let host_state =
        ((*state.host).get_extension)(state.host, CLAP_EXT_STATE.as_ptr() as *const c_char);
    if !host_state.is_null() {
        ((*(host_state as *const ClapHostState)).mark_dirty)(state.host);
    }
}

/// What a thread-checking plugin (clap-helpers, JUCE) verifies on every
/// audio-thread call.
unsafe fn check_audio_thread(state: &State) {
    let check =
        ((*state.host).get_extension)(state.host, CLAP_EXT_THREAD_CHECK.as_ptr() as *const c_char);
    if !check.is_null() && !((*(check as *const ClapHostThreadCheck)).is_audio_thread)(state.host) {
        state.wrong_thread_calls.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe extern "C" fn plugin_process(
    plugin: *const ClapPlugin,
    process: *const ClapProcess,
) -> ClapProcessStatus {
    let state = state(plugin);
    let process = &*process;
    check_audio_thread(state);
    state.process_calls.fetch_add(1, Ordering::Relaxed);
    apply_events(state, process.in_events);
    if state.crash {
        std::process::abort();
    }
    if state.hang.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    if state.touch {
        report_edit(state, process.out_events);
    }

    let gain = (f64::from_bits(state.gain.load(Ordering::Relaxed)) + state.gain_modulation) as f32;
    let frames = process.frames_count as usize;
    let output = &*process.audio_outputs;
    // Port 1, when the host provides it, is the aux (sidechain) bus declared by
    // `clap.audio-ports` below — summed in verbatim so a test can prove it was
    // actually delivered, distinct from the gained main signal.
    let aux = (process.audio_inputs_count >= 2).then(|| &*process.audio_inputs.add(1));
    for channel in 0..output.channel_count as usize {
        let out = std::slice::from_raw_parts_mut(*output.data32.add(channel), frames);
        if process.audio_inputs_count == 0 {
            out.fill(state.note_velocity as f32 * gain);
            continue;
        }
        let input = &*process.audio_inputs;
        let source = std::slice::from_raw_parts(*input.data32.add(channel), frames);
        match aux {
            Some(aux) => {
                let aux_source = std::slice::from_raw_parts(*aux.data32.add(channel), frames);
                for ((sample, dry), sc) in out.iter_mut().zip(source).zip(aux_source) {
                    *sample = dry * gain + sc;
                }
            }
            None => {
                for (sample, dry) in out.iter_mut().zip(source) {
                    *sample = dry * gain;
                }
            }
        }
    }
    if process.audio_outputs_count >= 2 && output.channel_count > 0 {
        let aux_out = &*process.audio_outputs.add(1);
        let main_left = std::slice::from_raw_parts(*output.data32, frames);
        let aux_left = std::slice::from_raw_parts_mut(*aux_out.data32, frames);
        for (sample, main) in aux_left.iter_mut().zip(main_left) {
            *sample = main * 0.5;
        }
    }
    reported_status(state, output, frames)
}

/// A plugin reports its sleepy status only once it has gone quiet, unless it lies.
unsafe fn reported_status(
    state: &State,
    output: &clap_ffi::ClapAudioBuffer,
    frames: usize,
) -> ClapProcessStatus {
    let silent = (0..output.channel_count as usize).all(|channel| {
        clap_ffi::is_silent(std::slice::from_raw_parts(
            *output.data32.add(channel),
            frames,
        ))
    });
    if silent || state.liar.load(Ordering::Relaxed) {
        state.status.load(Ordering::Relaxed)
    } else {
        CLAP_PROCESS_CONTINUE
    }
}

static PARAMS: ClapPluginParams = ClapPluginParams {
    count: params_count,
    get_info: params_info,
    get_value: params_value,
    value_to_text: params_value_to_text,
    text_to_value: params_text_to_value,
    flush: params_flush,
};

static STATE: ClapPluginState = ClapPluginState {
    save: state_save,
    load: state_load,
};

static LATENCY_EXT: ClapPluginLatency = ClapPluginLatency { get: latency_get };

static TAIL_EXT: ClapPluginTail = ClapPluginTail { get: tail_get };

static AUDIO_PORTS: ClapPluginAudioPorts = ClapPluginAudioPorts {
    count: audio_ports_count,
    get: audio_ports_get,
};

static NOTE_NAME: ClapPluginNoteName = ClapPluginNoteName {
    count: note_name_count,
    get: note_name_get,
};

static TRACK_INFO: ClapPluginTrackInfo = ClapPluginTrackInfo {
    changed: track_info_changed,
};

static TIMER_SUPPORT: ClapPluginTimerSupport = ClapPluginTimerSupport {
    on_timer: timer_on_timer,
};

static GUI: ClapGuiExtension = ClapGuiExtension {
    is_api_supported: gui_is_api_supported,
    get_preferred_api: gui_get_preferred_api,
    create: gui_create,
    destroy: plugin_noop,
    set_scale: gui_accept_scale,
    get_size: gui_get_size,
    can_resize: gui_can_resize,
    get_resize_hints: gui_refuse_hints,
    adjust_size: gui_accept_adjusted_size,
    set_size: gui_set_size,
    set_parent: gui_set_parent,
    set_transient: gui_set_transient,
    suggest_title: gui_suggest_title,
    show: gui_accept,
    hide: gui_accept,
};

const GUI_SIZE: (u32, u32) = (320, 200);

unsafe extern "C" fn gui_is_api_supported(
    _: *const ClapPlugin,
    api: *const c_char,
    floating: bool,
) -> bool {
    !floating && CStr::from_ptr(api).to_bytes_with_nul() == CLAP_WINDOW_API_WIN32
}

unsafe extern "C" fn gui_get_preferred_api(
    _: *const ClapPlugin,
    _: *mut *const c_char,
    _: *mut bool,
) -> bool {
    false
}

unsafe extern "C" fn gui_create(_: *const ClapPlugin, _: *const c_char, _: bool) -> bool {
    true
}

unsafe extern "C" fn gui_accept_scale(_: *const ClapPlugin, _: f64) -> bool {
    true
}

unsafe extern "C" fn gui_get_size(_: *const ClapPlugin, width: *mut u32, height: *mut u32) -> bool {
    *width = GUI_SIZE.0;
    *height = GUI_SIZE.1;
    true
}

unsafe extern "C" fn gui_can_resize(_: *const ClapPlugin) -> bool {
    false
}

unsafe extern "C" fn gui_refuse_hints(_: *const ClapPlugin, _: *mut c_void) -> bool {
    false
}

unsafe extern "C" fn gui_accept_adjusted_size(
    _: *const ClapPlugin,
    _: *mut u32,
    _: *mut u32,
) -> bool {
    true
}

unsafe extern "C" fn gui_set_size(_: *const ClapPlugin, _: u32, _: u32) -> bool {
    true
}

unsafe extern "C" fn gui_set_parent(_: *const ClapPlugin, _: *const ClapWindow) -> bool {
    true
}

unsafe extern "C" fn gui_set_transient(_: *const ClapPlugin, _: *const ClapWindow) -> bool {
    false
}

unsafe extern "C" fn gui_suggest_title(_: *const ClapPlugin, _: *const c_char) {}

unsafe extern "C" fn gui_accept(_: *const ClapPlugin) -> bool {
    true
}

unsafe extern "C" fn plugin_get_extension(
    plugin: *const ClapPlugin,
    id: *const c_char,
) -> *const c_void {
    let id = CStr::from_ptr(id).to_bytes_with_nul();
    if id == CLAP_EXT_TAIL {
        return if state(plugin).tail.load(Ordering::Acquire) == 0 {
            std::ptr::null()
        } else {
            &TAIL_EXT as *const ClapPluginTail as *const c_void
        };
    }
    if id == CLAP_EXT_PARAMS {
        &PARAMS as *const ClapPluginParams as *const c_void
    } else if id == CLAP_EXT_STATE {
        &STATE as *const ClapPluginState as *const c_void
    } else if id == CLAP_EXT_LATENCY {
        &LATENCY_EXT as *const ClapPluginLatency as *const c_void
    } else if id == CLAP_EXT_AUDIO_PORTS {
        &AUDIO_PORTS as *const ClapPluginAudioPorts as *const c_void
    } else if id == CLAP_EXT_NOTE_NAME {
        &NOTE_NAME as *const ClapPluginNoteName as *const c_void
    } else if id == CLAP_EXT_TRACK_INFO {
        &TRACK_INFO as *const ClapPluginTrackInfo as *const c_void
    } else if id == CLAP_EXT_TIMER_SUPPORT {
        &TIMER_SUPPORT as *const ClapPluginTimerSupport as *const c_void
    } else if id == CLAP_EXT_GUI {
        &GUI as *const ClapGuiExtension as *const c_void
    } else {
        std::ptr::null()
    }
}

/// One main stereo bus per direction, plus one auxiliary stereo input
/// (sidechain) and one auxiliary mono output (multi-out) — exercises ADR-014
/// end to end.
unsafe extern "C" fn audio_ports_count(_: *const ClapPlugin, _is_input: bool) -> u32 {
    2
}

unsafe extern "C" fn audio_ports_get(
    _: *const ClapPlugin,
    index: u32,
    is_input: bool,
    info: *mut ClapAudioPortInfo,
) -> bool {
    let (name, flags, channel_count): (&[u8], u32, u32) = match (is_input, index) {
        (true, 0) => (b"Main In", CLAP_AUDIO_PORT_IS_MAIN, 2),
        (true, 1) => (b"Sidechain", 0, 2),
        (false, 0) => (b"Main Out", CLAP_AUDIO_PORT_IS_MAIN, 2),
        (false, 1) => (b"Aux Out", 0, 1),
        _ => return false,
    };
    let info = &mut *info;
    info.id = index;
    info.name = [0; clap_ffi::CLAP_NAME_SIZE];
    info.name[..name.len()].copy_from_slice(name);
    info.flags = flags;
    info.channel_count = channel_count;
    info.port_type = std::ptr::null();
    info.in_place_pair = u32::MAX;
    true
}

/// The two sets of note names `Note names set` switches between:
/// `(name, port, key, channel)`, `-1` meaning every one.
type TestNoteName = (&'static [u8], i16, i16, i16);

const NOTE_NAME_SETS: [&[TestNoteName]; 2] = [
    &[(b"Kick", -1, 36, -1), (b"Snare", -1, 38, -1)],
    &[(b"Tom", 0, 41, 9)],
];

unsafe extern "C" fn note_name_count(plugin: *const ClapPlugin) -> u32 {
    NOTE_NAME_SETS[state(plugin).note_names_set.load(Ordering::Acquire) as usize].len() as u32
}

unsafe extern "C" fn note_name_get(
    plugin: *const ClapPlugin,
    index: u32,
    note_name: *mut ClapNoteName,
) -> bool {
    let set = NOTE_NAME_SETS[state(plugin).note_names_set.load(Ordering::Acquire) as usize];
    let Some(&(name, port, key, channel)) = set.get(index as usize) else {
        return false;
    };
    let note_name = &mut *note_name;
    note_name.name = [0; clap_ffi::CLAP_NAME_SIZE];
    note_name.name[..name.len()].copy_from_slice(name);
    note_name.port = port;
    note_name.key = key;
    note_name.channel = channel;
    true
}

unsafe extern "C" fn params_count(_: *const ClapPlugin) -> u32 {
    PARAM_COUNT
}

unsafe extern "C" fn params_info(
    _: *const ClapPlugin,
    index: u32,
    info: *mut ClapParamInfo,
) -> bool {
    let (name, max, default, flags): (&[u8], f64, f64, u32) = match index {
        GAIN => (
            b"Gain",
            2.0,
            1.0,
            CLAP_PARAM_IS_AUTOMATABLE | CLAP_PARAM_IS_MODULATABLE,
        ),
        HANG => (b"Hang", 1.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        CRASH => (b"Crash", 1.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        TOUCH => (b"Touch", 1.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        LOOKAHEAD => (b"Lookahead", 1.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        WRONG_THREAD => (b"Wrong thread", f64::from(u32::MAX), 0.0, 0),
        STATUS => (b"Status", 4.0, 1.0, CLAP_PARAM_IS_AUTOMATABLE),
        TAIL => (b"Tail", 1_000_000.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        PROCESS_CALLS => (b"Process calls", f64::from(u32::MAX), 0.0, 0),
        LIAR => (b"Liar", 1.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        RESCAN => (b"Rescan", 15.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        REQUEST_FLUSH => (b"Request flush", 1.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        TRACK_FLAGS => (b"Track flags", f64::from(u32::MAX), 0.0, 0),
        TRACK_COLOR => (b"Track color", f64::from(u32::MAX), 0.0, 0),
        TRACK_NAME => (b"Track name", 281_474_976_710_655.0, 0.0, 0),
        TRACK_NAME_LENGTH => (b"Track name length", 255.0, 0.0, 0),
        TRACK_INFO_READS => (b"Track info reads", f64::from(u32::MAX), 0.0, 0),
        TRACK_INFO_CHANGES => (b"Track info changes", f64::from(u32::MAX), 0.0, 0),
        TRACK_INFO_AUDIO_CALLS => (b"Track info audio calls", f64::from(u32::MAX), 0.0, 0),
        NOTE_NAMES_SET => (b"Note names set", 1.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        NOTE_PORTS_RESCAN => (b"Note ports rescan", 3.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        HOST_NOTE_DIALECTS => (b"Host note dialects", 15.0, 0.0, 0),
        TIMER_REGISTER => (
            b"Timer register",
            3_600_000.0,
            0.0,
            CLAP_PARAM_IS_AUTOMATABLE,
        ),
        TIMER_UNREGISTER => (
            b"Timer unregister",
            f64::from(u32::MAX),
            0.0,
            CLAP_PARAM_IS_AUTOMATABLE,
        ),
        TIMER_REGISTER_RESULT => (b"Timer register result", 2.0, 0.0, 0),
        TIMER_LAST_ID => (b"Timer last id", f64::from(u32::MAX), 0.0, 0),
        TIMER_UNREGISTER_RESULT => (b"Timer unregister result", 2.0, 0.0, 0),
        TIMER_CALLS => (b"Timer calls", f64::from(u32::MAX), 0.0, 0),
        TIMER_LAST_FIRED => (b"Timer last fired", f64::from(u32::MAX), 0.0, 0),
        TIMER_AUDIO_CALLS => (b"Timer audio calls", f64::from(u32::MAX), 0.0, 0),
        PRESET_LOADED => (b"Preset loaded", 1.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        PRESET_ERROR => (b"Preset error", 1.0, 0.0, CLAP_PARAM_IS_AUTOMATABLE),
        _ => return false,
    };
    let info = &mut *info;
    info.id = index;
    info.flags = flags;
    info.cookie = std::ptr::null_mut();
    info.name = [0; clap_ffi::CLAP_NAME_SIZE];
    info.name[..name.len()].copy_from_slice(name);
    info.module = [0; clap_ffi::CLAP_PATH_SIZE];
    info.min_value = 0.0;
    info.max_value = max;
    info.default_value = default;
    true
}

unsafe extern "C" fn params_value(plugin: *const ClapPlugin, id: u32, value: *mut f64) -> bool {
    let state = state(plugin);
    *value = match id {
        GAIN => f64::from_bits(state.gain.load(Ordering::Relaxed)),
        WRONG_THREAD => f64::from(state.wrong_thread_calls.load(Ordering::Relaxed)),
        PROCESS_CALLS => f64::from(state.process_calls.load(Ordering::Relaxed)),
        TRACK_FLAGS => state.track_flags.load(Ordering::Relaxed) as f64,
        TRACK_COLOR => f64::from(state.track_color.load(Ordering::Relaxed)),
        TRACK_NAME => state.track_name.load(Ordering::Relaxed) as f64,
        TRACK_NAME_LENGTH => f64::from(state.track_name_length.load(Ordering::Relaxed)),
        TRACK_INFO_READS => f64::from(state.track_info_reads.load(Ordering::Relaxed)),
        TRACK_INFO_CHANGES => f64::from(state.track_info_changes.load(Ordering::Relaxed)),
        TRACK_INFO_AUDIO_CALLS => f64::from(state.track_info_audio_calls.load(Ordering::Relaxed)),
        NOTE_NAMES_SET => f64::from(state.note_names_set.load(Ordering::Relaxed)),
        HOST_NOTE_DIALECTS => f64::from(state.host_note_dialects.load(Ordering::Relaxed)),
        TIMER_REGISTER_RESULT => f64::from(state.timer_register_result.load(Ordering::Relaxed)),
        TIMER_LAST_ID => f64::from(state.timer_last_id.load(Ordering::Relaxed)),
        TIMER_UNREGISTER_RESULT => f64::from(state.timer_unregister_result.load(Ordering::Relaxed)),
        TIMER_CALLS => f64::from(state.timer_calls.load(Ordering::Relaxed)),
        TIMER_LAST_FIRED => f64::from(state.timer_last_fired.load(Ordering::Relaxed)),
        TIMER_AUDIO_CALLS => f64::from(state.timer_audio_calls.load(Ordering::Relaxed)),
        _ => return false,
    };
    true
}

unsafe extern "C" fn params_value_to_text(
    _: *const ClapPlugin,
    id: u32,
    value: f64,
    text: *mut c_char,
    capacity: u32,
) -> bool {
    if id != GAIN || capacity == 0 {
        return false;
    }
    let decibels = if value > 0.0 {
        format!("{:.1} dB", 20.0 * value.log10())
    } else {
        "-inf dB".to_string()
    };
    let bytes = decibels.as_bytes();
    let length = bytes.len().min(capacity as usize - 1);
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), text as *mut u8, length);
    *text.add(length) = 0;
    true
}

unsafe extern "C" fn params_text_to_value(
    _: *const ClapPlugin,
    _: u32,
    _: *const c_char,
    _: *mut f64,
) -> bool {
    false
}

unsafe extern "C" fn params_flush(
    plugin: *const ClapPlugin,
    events: *const ClapInputEvents,
    _: *const ClapOutputEvents,
) {
    apply_events(state(plugin), events);
}

unsafe extern "C" fn state_save(plugin: *const ClapPlugin, stream: *const ClapOStream) -> bool {
    let bytes = state(plugin).gain.load(Ordering::Relaxed).to_le_bytes();
    ((*stream).write)(stream, bytes.as_ptr() as *const c_void, bytes.len() as u64)
        == bytes.len() as i64
}

unsafe extern "C" fn state_load(plugin: *const ClapPlugin, stream: *const ClapIStream) -> bool {
    let mut bytes = [0u8; 8];
    if ((*stream).read)(stream, bytes.as_mut_ptr() as *mut c_void, 8) != 8 {
        return false;
    }
    state(plugin)
        .gain
        .store(u64::from_le_bytes(bytes), Ordering::Relaxed);
    true
}

unsafe extern "C" fn latency_get(plugin: *const ClapPlugin) -> u32 {
    state(plugin).latency
}

unsafe extern "C" fn tail_get(plugin: *const ClapPlugin) -> u32 {
    state(plugin).tail.load(Ordering::Acquire)
}
