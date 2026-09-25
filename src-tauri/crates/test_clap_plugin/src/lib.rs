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
//!
//! Reports 64 samples of latency by default.  No GUI.

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use clap_ffi::{
    ClapEventHeader, ClapEventNote, ClapEventParamGesture, ClapEventParamValue, ClapHost,
    ClapHostLatency, ClapHostState, ClapHostThreadCheck, ClapIStream, ClapInputEvents, ClapOStream,
    ClapOutputEvents, ClapParamInfo,
    ClapPlugin, ClapPluginDescriptor, ClapPluginEntry, ClapPluginFactory, ClapPluginLatency,
    ClapPluginParams, ClapPluginState, ClapProcess, ClapProcessStatus, CLAP_CORE_EVENT_SPACE,
    CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_GESTURE_END, CLAP_EVENT_PARAM_MOD,
    CLAP_EVENT_PARAM_VALUE, CLAP_EXT_LATENCY, CLAP_EXT_PARAMS, CLAP_EXT_STATE, CLAP_EXT_THREAD_CHECK,
    CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_MODULATABLE, CLAP_PLUGIN_FACTORY_ID,
    CLAP_PROCESS_CONTINUE, CLAP_VERSION_1,
};

pub const PLUGIN_ID: &str = "echoes.test.gain";
pub const LATENCY: u32 = 64;
pub const LOOKAHEAD_LATENCY: u32 = 128;
const GAIN: u32 = 0;
const HANG: u32 = 1;
const CRASH: u32 = 2;
const TOUCH: u32 = 3;
const LOOKAHEAD: u32 = 4;
const WRONG_THREAD: u32 = 5;

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
    1
}

unsafe extern "C" fn factory_descriptor(
    _: *const ClapPluginFactory,
    index: u32,
) -> *const ClapPluginDescriptor {
    if index == 0 {
        &DESCRIPTOR.0
    } else {
        std::ptr::null()
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
}

unsafe extern "C" fn factory_create(
    _: *const ClapPluginFactory,
    host: *const ClapHost,
    id: *const c_char,
) -> *const ClapPlugin {
    if CStr::from_ptr(id).to_bytes() != PLUGIN_ID.as_bytes() {
        return std::ptr::null();
    }
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
    });
    Box::into_raw(Box::new(ClapPlugin {
        desc: &DESCRIPTOR.0,
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

unsafe extern "C" fn plugin_init(_: *const ClapPlugin) -> bool {
    true
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
    for channel in 0..output.channel_count as usize {
        let out = std::slice::from_raw_parts_mut(*output.data32.add(channel), frames);
        if process.audio_inputs_count == 0 {
            out.fill(state.note_velocity as f32 * gain);
            continue;
        }
        let input = &*process.audio_inputs;
        let source = std::slice::from_raw_parts(*input.data32.add(channel), frames);
        for (sample, dry) in out.iter_mut().zip(source) {
            *sample = dry * gain;
        }
    }
    CLAP_PROCESS_CONTINUE
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

unsafe extern "C" fn plugin_get_extension(
    _: *const ClapPlugin,
    id: *const c_char,
) -> *const c_void {
    let id = CStr::from_ptr(id).to_bytes_with_nul();
    if id == CLAP_EXT_PARAMS {
        &PARAMS as *const ClapPluginParams as *const c_void
    } else if id == CLAP_EXT_STATE {
        &STATE as *const ClapPluginState as *const c_void
    } else if id == CLAP_EXT_LATENCY {
        &LATENCY_EXT as *const ClapPluginLatency as *const c_void
    } else {
        std::ptr::null()
    }
}

unsafe extern "C" fn params_count(_: *const ClapPlugin) -> u32 {
    6
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
        _ => return false,
    };
    true
}

unsafe extern "C" fn params_value_to_text(
    _: *const ClapPlugin,
    _: u32,
    _: f64,
    _: *mut c_char,
    _: u32,
) -> bool {
    false
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
