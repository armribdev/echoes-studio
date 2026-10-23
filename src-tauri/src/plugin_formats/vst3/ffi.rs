//! VST3 raw COM bindings — Windows implementation.
//!
//! Defines the minimal vtable subset needed to enumerate, load, and process
//! audio through VST3 plugins without pulling in the full VST3 SDK.
//!
//! ## Safety contract
//!
//! All COM method calls are unsafe.  They must only be made on the command
//! thread (prepare/lifecycle) or the audio thread (process).  The two are
//! never concurrent thanks to the atomic scheduler swap.
//!
//! The `Vst3Library` struct wraps the HMODULE and calls `FreeLibrary` on drop.
//! It must outlive every `VstWrapperNode` that was created from it — callers
//! must keep an `Arc<Vst3Library>` alive alongside the node.

use std::ffi::{c_char, c_void};
use std::path::Path;

/// VST3 result code (0 = OK, 1 = FALSE, negative = error).
pub type TResult = i32;
/// 16-byte interface / class identifier.
pub type Tuid = [u8; 16];

pub const K_RESULT_OK: TResult = 0;
pub const K_RESULT_FALSE: TResult = 1;

// `funknown.h`: on Windows the codes are the COM HRESULTs.
pub const K_NO_INTERFACE: TResult = if cfg!(windows) {
    0x8000_4002_u32 as TResult
} else {
    -1
};
pub const K_INVALID_ARGUMENT: TResult = if cfg!(windows) {
    0x8007_0057_u32 as TResult
} else {
    2
};
pub const K_OUT_OF_MEMORY: TResult = if cfg!(windows) {
    0x8007_000E_u32 as TResult
} else {
    6
};

// Derived from the VST3 SDK using FUID::toTUID() big-endian serialisation.

/// Windows VST3 is COM-compatible and matches interface IIDs in GUID byte order
/// (`Data1`/`Data2`/`Data3` little-endian, `Data4` unchanged).  The literals
/// below are the platform-neutral big-endian form; on Windows the first three
/// groups must be byte-swapped or `createInstance`/`queryInterface` return
/// E_NOINTERFACE for every plugin (see docs/active/audits/ffi_lifecycle_validation.md, F1).
const fn interface_iid(be: Tuid) -> Tuid {
    if cfg!(windows) {
        [
            be[3], be[2], be[1], be[0], be[5], be[4], be[7], be[6], be[8], be[9], be[10], be[11],
            be[12], be[13], be[14], be[15],
        ]
    } else {
        be
    }
}

/// Steinberg's FUnknown IID (non-COM-compatible, modern SDK convention).
pub const IID_FUNKNOWN: Tuid = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

pub const IID_ICOMPONENT: Tuid = interface_iid([
    0xE8, 0x31, 0xFF, 0x31, 0xF2, 0xD5, 0x43, 0x01, 0x92, 0x8E, 0xBB, 0xEE, 0x25, 0x69, 0x78, 0x02,
]);
pub const IID_IAUDIO_PROCESSOR: Tuid = interface_iid([
    0x42, 0x04, 0x3F, 0x99, 0xB7, 0xDA, 0x45, 0x3C, 0xA5, 0x69, 0xE7, 0x9D, 0x9A, 0xAE, 0xC3, 0x3D,
]);

pub const IID_IMIDI_MAPPING: Tuid = interface_iid([
    0xDF, 0x0F, 0xF9, 0xF7, 0x49, 0xB7, 0x46, 0x69, 0xB6, 0x3A, 0xB7, 0x32, 0x7A, 0xDB, 0xF5, 0xE5,
]);
pub const IID_IEVENT_LIST: Tuid = interface_iid([
    0x3A, 0x2C, 0x42, 0x14, 0x34, 0x63, 0x49, 0xFE, 0xB2, 0xC4, 0xF3, 0x97, 0xB9, 0x69, 0x5A, 0x44,
]);
pub const IID_IPLUGIN_FACTORY2: Tuid = interface_iid([
    0x00, 0x07, 0xB6, 0x50, 0xF2, 0x4B, 0x4C, 0x0B, 0xA4, 0x64, 0xED, 0xB9, 0xF0, 0x0B, 0x2A, 0xBB,
]);
pub const IID_IMESSAGE: Tuid = interface_iid([
    0x93, 0x6F, 0x03, 0x3B, 0xC6, 0xC0, 0x47, 0xDB, 0xBB, 0x08, 0x82, 0xF8, 0x13, 0xC1, 0xE6, 0x13,
]);
pub const IID_IATTRIBUTE_LIST: Tuid = interface_iid([
    0x1E, 0x5F, 0x0A, 0xEB, 0xCC, 0x7F, 0x45, 0x33, 0xA2, 0x54, 0x40, 0x11, 0x38, 0xAD, 0x5E, 0xE4,
]);
pub const IID_IINFO_LISTENER: Tuid = interface_iid([
    0x0F, 0x19, 0x47, 0x81, 0x8D, 0x98, 0x4A, 0xDA, 0xBB, 0xA0, 0xC1, 0xEF, 0xC0, 0x11, 0xD8, 0xD0,
]);
pub const IID_IUNIT_INFO: Tuid = interface_iid([
    0x3D, 0x4B, 0xD6, 0xB5, 0x91, 0x3A, 0x4F, 0xD2, 0xA8, 0x86, 0xE7, 0x68, 0xA5, 0xEB, 0x92, 0xC1,
]);

/// L | R  (bits 0 and 1)
pub const SPEAKER_STEREO: u64 = 0x0000_0003;

pub const CATEGORY_AUDIO_MODULE: &[u8] = b"Audio Module Class";

/// Minimal class info returned by IPluginFactory::getClassInfo.
#[repr(C)]
pub struct PClassInfo {
    pub cid: [u8; 16],
    pub cardinality: i32,
    pub category: [u8; 32],
    pub name: [u8; 64],
}

impl PClassInfo {
    pub fn name_str(&self) -> &str {
        nul_terminated_str(&self.name)
    }
    pub fn is_audio_module(&self) -> bool {
        self.category.starts_with(CATEGORY_AUDIO_MODULE)
    }
}

/// Base COM object — only the vtable pointer at offset 0.
#[repr(C)]
pub struct FUnknown {
    pub vtbl: *const FUnknownVtbl,
}

#[repr(C)]
pub struct FUnknownVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
}

#[repr(C)]
pub struct IPluginFactory {
    pub vtbl: *const IPluginFactoryVtbl,
}

#[repr(C)]
pub struct IPluginFactoryVtbl {
    // FUnknown
    pub query_interface:
        unsafe extern "C" fn(*mut IPluginFactory, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IPluginFactory) -> u32,
    pub release: unsafe extern "C" fn(*mut IPluginFactory) -> u32,
    // IPluginFactory
    pub get_factory_info: unsafe extern "C" fn(*mut IPluginFactory, *mut c_void) -> TResult,
    pub count_classes: unsafe extern "C" fn(*mut IPluginFactory) -> i32,
    pub get_class_info: unsafe extern "C" fn(*mut IPluginFactory, i32, *mut PClassInfo) -> TResult,
    pub create_instance: unsafe extern "C" fn(
        *mut IPluginFactory,
        *const u8,
        *const u8,
        *mut *mut c_void,
    ) -> TResult,
}

/// `PClassInfo2` (`ipluginbase.h`): `PClassInfo` plus the sub-categories.
#[repr(C)]
pub struct PClassInfo2 {
    pub cid: [u8; 16],
    pub cardinality: i32,
    pub category: [u8; 32],
    pub name: [u8; 64],
    pub class_flags: u32,
    pub sub_categories: [u8; 128],
    pub vendor: [u8; 64],
    pub version: [u8; 64],
    pub sdk_version: [u8; 64],
}

const _: () = assert!(
    std::mem::size_of::<PClassInfo2>() == 440,
    "PClassInfo2 layout drift"
);

impl PClassInfo2 {
    pub fn sub_categories_str(&self) -> &str {
        nul_terminated_str(&self.sub_categories)
    }
}

fn nul_terminated_str(bytes: &[u8]) -> &str {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).unwrap_or("")
}

#[repr(C)]
pub struct IPluginFactory2Vtbl {
    // FUnknown
    pub query_interface:
        unsafe extern "C" fn(*mut IPluginFactory, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IPluginFactory) -> u32,
    pub release: unsafe extern "C" fn(*mut IPluginFactory) -> u32,
    // IPluginFactory
    pub get_factory_info: unsafe extern "C" fn(*mut IPluginFactory, *mut c_void) -> TResult,
    pub count_classes: unsafe extern "C" fn(*mut IPluginFactory) -> i32,
    pub get_class_info: unsafe extern "C" fn(*mut IPluginFactory, i32, *mut PClassInfo) -> TResult,
    pub create_instance: unsafe extern "C" fn(
        *mut IPluginFactory,
        *const u8,
        *const u8,
        *mut *mut c_void,
    ) -> TResult,
    // IPluginFactory2
    pub get_class_info2:
        unsafe extern "C" fn(*mut IPluginFactory, i32, *mut PClassInfo2) -> TResult,
}

/// Whether a `PClassInfo2` sub-category string names an instrument: its first
/// `|`-separated token is `Instrument`.  `Fx|Instrument` is an effect that
/// processes the audio of an instrument.
pub fn is_instrument_subcategory(sub_categories: &str) -> bool {
    sub_categories.split('|').next() == Some("Instrument")
}

// Extends FUnknown (0–2) + IPluginBase (3–4) + IComponent own methods (5–13).

#[repr(C)]
pub struct IComponent {
    pub vtbl: *const IComponentVtbl,
}

#[repr(C)]
pub struct IComponentVtbl {
    // FUnknown
    pub query_interface:
        unsafe extern "C" fn(*mut IComponent, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IComponent) -> u32,
    pub release: unsafe extern "C" fn(*mut IComponent) -> u32,
    // IPluginBase
    pub initialize: unsafe extern "C" fn(*mut IComponent, *mut FUnknown) -> TResult,
    pub terminate: unsafe extern "C" fn(*mut IComponent) -> TResult,
    // IComponent
    pub get_controller_class_id: unsafe extern "C" fn(*mut IComponent, *mut u8) -> TResult,
    pub set_io_mode: unsafe extern "C" fn(*mut IComponent, i32) -> TResult,
    pub get_bus_count: unsafe extern "C" fn(*mut IComponent, i32, i32) -> i32,
    pub get_bus_info: unsafe extern "C" fn(*mut IComponent, i32, i32, i32, *mut c_void) -> TResult,
    pub get_routing_info:
        unsafe extern "C" fn(*mut IComponent, *mut c_void, *mut c_void) -> TResult,
    pub activate_bus: unsafe extern "C" fn(*mut IComponent, i32, i32, i32, u8) -> TResult,
    pub set_active: unsafe extern "C" fn(*mut IComponent, u8) -> TResult,
    pub set_state: unsafe extern "C" fn(*mut IComponent, *mut c_void) -> TResult,
    pub get_state: unsafe extern "C" fn(*mut IComponent, *mut c_void) -> TResult,
}

// Extends FUnknown (0–2) only.

#[repr(C)]
pub struct IAudioProcessor {
    pub vtbl: *const IAudioProcessorVtbl,
}

/// Audio bus channel pointer array + silence flags for one bus.
#[repr(C)]
pub struct AudioBusBuffers {
    pub num_channels: i32,
    pub silence_flags: u64,
    pub channel_buffers32: *mut *mut f32,
}

/// Setup passed to `setupProcessing`.
#[repr(C)]
pub struct ProcessSetup {
    pub process_mode: i32,         // 0 = realtime
    pub symbolic_sample_size: i32, // 0 = float32
    pub max_samples_per_block: i32,
    pub sample_rate: f64,
}

/// Per-block processing data (null pointers for unused fields are valid).
#[repr(C)]
pub struct ProcessData {
    pub process_mode: i32,         // 0 = realtime
    pub symbolic_sample_size: i32, // 0 = float32
    pub num_samples: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    pub inputs: *mut AudioBusBuffers,
    pub outputs: *mut AudioBusBuffers,
    pub input_param_changes: *mut c_void,
    pub output_param_changes: *mut c_void,
    pub input_events: *mut c_void,
    pub output_events: *mut c_void,
    pub process_context: *mut c_void,
}

/// `Vst::ProcessContext` (`ivstprocesscontext.h`): where the host's transport is.
/// Only the fields behind a `*_VALID` flag are meaningful.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessContext {
    pub state: u32,
    pub sample_rate: f64,
    pub project_time_samples: i64,
    pub system_time: i64,
    pub continuous_time_samples: i64,
    pub project_time_music: f64,
    pub bar_position_music: f64,
    pub cycle_start_music: f64,
    pub cycle_end_music: f64,
    pub tempo: f64,
    pub time_sig_numerator: i32,
    pub time_sig_denominator: i32,
    /// `Chord { keyNote: u8, rootNote: u8, chordMask: i16 }`, unused.
    pub chord: [u8; 4],
    pub smpte_offset_subframes: i32,
    /// `FrameRate { framesPerSecond: u32, flags: u32 }`, unused.
    pub frame_rate: [u32; 2],
    pub samples_to_next_clock: i32,
}

const _: () = assert!(
    std::mem::size_of::<ProcessContext>() == 112,
    "ProcessContext layout drift"
);

/// `ProcessContext::state` bits.
pub const CONTEXT_PLAYING: u32 = 1 << 1;
pub const CONTEXT_PROJECT_TIME_MUSIC_VALID: u32 = 1 << 9;
pub const CONTEXT_TEMPO_VALID: u32 = 1 << 10;
pub const CONTEXT_BAR_POSITION_VALID: u32 = 1 << 11;
pub const CONTEXT_TIME_SIG_VALID: u32 = 1 << 13;

/// `MediaTypes` and `BusDirections` (`ivstcomponent.h`), the first two arguments
/// of `getBusCount` and `activateBus`.
pub const K_AUDIO: i32 = 0;
pub const K_EVENT: i32 = 1;
pub const K_INPUT: i32 = 0;
pub const K_OUTPUT: i32 = 1;

/// `Event::EventTypes` (`ivstevents.h`).
pub const K_NOTE_ON_EVENT: u16 = 0;
pub const K_NOTE_OFF_EVENT: u16 = 1;
pub const K_POLY_PRESSURE_EVENT: u16 = 3;

/// `NoteOnEvent`, `NoteOffEvent` and `PolyPressureEvent` (`ivstevents.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NoteOnEvent {
    pub channel: i16,
    pub pitch: i16,
    pub tuning: f32,
    pub velocity: f32,
    pub length: i32,
    pub note_id: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NoteOffEvent {
    pub channel: i16,
    pub pitch: i16,
    pub velocity: f32,
    pub note_id: i32,
    pub tuning: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PolyPressureEvent {
    pub channel: i16,
    pub pitch: i16,
    pub pressure: f32,
    pub note_id: i32,
}

/// The anonymous union of `Vst::Event`.  `raw` stands for the members this host
/// never builds (data, chord, scale, note expression): the widest of them,
/// `NoteExpressionTextEvent`, is 24 bytes with 8-byte alignment.
#[repr(C)]
#[derive(Clone, Copy)]
pub union EventPayload {
    pub note_on: NoteOnEvent,
    pub note_off: NoteOffEvent,
    pub poly_pressure: PolyPressureEvent,
    raw: [u64; 3],
}

/// `Vst::Event` (`ivstevents.h`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Event {
    pub bus_index: i32,
    pub sample_offset: i32,
    pub ppq_position: f64,
    pub flags: u16,
    pub kind: u16,
    pub payload: EventPayload,
}

const _: () = {
    assert!(std::mem::size_of::<Event>() == 48, "Event layout drift");
    assert!(
        std::mem::offset_of!(Event, payload) == 24,
        "Event layout drift"
    );
};

impl Event {
    pub const EMPTY: Event = Event {
        bus_index: 0,
        sample_offset: 0,
        ppq_position: 0.0,
        flags: 0,
        kind: K_NOTE_ON_EVENT,
        payload: EventPayload { raw: [0; 3] },
    };

    fn on_main_bus(sample_offset: u32, kind: u16, payload: EventPayload) -> Self {
        Event {
            sample_offset: sample_offset as i32,
            kind,
            payload,
            ..Event::EMPTY
        }
    }

    /// `velocity` is normalised (0..1).  Channel 0, `note_id` -1: the plugin
    /// matches the note by pitch.
    pub fn note_on(sample_offset: u32, pitch: u8, velocity: f32, note_id: i32) -> Self {
        Self::on_main_bus(
            sample_offset,
            K_NOTE_ON_EVENT,
            EventPayload {
                note_on: NoteOnEvent {
                    channel: 0,
                    pitch: pitch as i16,
                    tuning: 0.0,
                    velocity,
                    length: 0,
                    note_id,
                },
            },
        )
    }

    pub fn note_off(sample_offset: u32, pitch: u8, note_id: i32) -> Self {
        Self::on_main_bus(
            sample_offset,
            K_NOTE_OFF_EVENT,
            EventPayload {
                note_off: NoteOffEvent {
                    channel: 0,
                    pitch: pitch as i16,
                    velocity: 0.0,
                    note_id,
                    tuning: 0.0,
                },
            },
        )
    }

    pub fn poly_pressure(sample_offset: u32, pitch: u8, pressure: f32, note_id: i32) -> Self {
        Self::on_main_bus(
            sample_offset,
            K_POLY_PRESSURE_EVENT,
            EventPayload {
                poly_pressure: PolyPressureEvent {
                    channel: 0,
                    pitch: pitch as i16,
                    pressure,
                    note_id,
                },
            },
        )
    }
}

#[repr(C)]
pub struct IAudioProcessorVtbl {
    // FUnknown
    pub query_interface:
        unsafe extern "C" fn(*mut IAudioProcessor, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
    pub release: unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
    // IAudioProcessor
    pub set_bus_arrangements:
        unsafe extern "C" fn(*mut IAudioProcessor, *mut u64, i32, *mut u64, i32) -> TResult,
    pub get_bus_arrangement:
        unsafe extern "C" fn(*mut IAudioProcessor, i32, i32, *mut u64) -> TResult,
    pub can_process_sample_size: unsafe extern "C" fn(*mut IAudioProcessor, i32) -> TResult,
    pub get_latency_samples: unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
    pub setup_processing:
        unsafe extern "C" fn(*mut IAudioProcessor, *const ProcessSetup) -> TResult,
    pub set_processing: unsafe extern "C" fn(*mut IAudioProcessor, u8) -> TResult,
    pub process: unsafe extern "C" fn(*mut IAudioProcessor, *mut ProcessData) -> TResult,
    pub get_tail_samples: unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
}

// ADR-007.  The host fills these structures each block from the SPSC parameter
// ring and hands them to the plugin via `ProcessData::input_param_changes`.
// The plugin only *reads* them (getParameterCount → getParameterData → per
// queue getParameterId / getPointCount / getPoint); the add* methods exist for
// spec completeness and are what the host uses internally to fill the data.

#[repr(C)]
pub struct IParamValueQueueVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut HostParamValueQueue, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut HostParamValueQueue) -> u32,
    pub release: unsafe extern "C" fn(*mut HostParamValueQueue) -> u32,
    pub get_parameter_id: unsafe extern "C" fn(*mut HostParamValueQueue) -> u32,
    pub get_point_count: unsafe extern "C" fn(*mut HostParamValueQueue) -> i32,
    pub get_point:
        unsafe extern "C" fn(*mut HostParamValueQueue, i32, *mut i32, *mut f64) -> TResult,
    pub add_point: unsafe extern "C" fn(*mut HostParamValueQueue, i32, f64, *mut i32) -> TResult,
}

#[repr(C)]
pub struct IParameterChangesVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut HostParameterChanges, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut HostParameterChanges) -> u32,
    pub release: unsafe extern "C" fn(*mut HostParameterChanges) -> u32,
    pub get_parameter_count: unsafe extern "C" fn(*mut HostParameterChanges) -> i32,
    pub get_parameter_data:
        unsafe extern "C" fn(*mut HostParameterChanges, i32) -> *mut HostParamValueQueue,
    pub add_parameter_data: unsafe extern "C" fn(
        *mut HostParameterChanges,
        *const u32,
        *mut i32,
    ) -> *mut HostParamValueQueue,
}

/// Max distinct parameters carried in one block, and max value points per
/// parameter per block.  `MAX_POINTS_PER_QUEUE` covers one sub-block modulation
/// point per `MODULATION_CONTROL_PERIOD` over a full `MAX_BLOCK_FRAMES` block
/// (ADR-019: ceil(4096 / 32) = 128) plus UI edits.
pub const MAX_PARAM_QUEUES: usize = 128;
pub const MAX_POINTS_PER_QUEUE: usize = 128;

/// One parameter's value points for the current block.  `#[repr(C)]` keeps the
/// `vtbl` at offset 0 so the plugin can dispatch through it; the remaining
/// fields are private host data the vtable methods read.
#[repr(C)]
pub struct HostParamValueQueue {
    pub vtbl: *const IParamValueQueueVtbl,
    pub param_id: u32,
    pub offsets: [i32; MAX_POINTS_PER_QUEUE],
    pub values: [f64; MAX_POINTS_PER_QUEUE],
    pub count: i32,
}

/// The per-block parameter-change list handed to the plugin.
#[repr(C)]
pub struct HostParameterChanges {
    pub vtbl: *const IParameterChangesVtbl,
    pub queues: [HostParamValueQueue; MAX_PARAM_QUEUES],
    pub count: i32,
}

unsafe extern "C" fn pvq_query_interface(
    _this: *mut HostParamValueQueue,
    _iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if !obj.is_null() {
        *obj = std::ptr::null_mut();
    }
    K_NO_INTERFACE
}
unsafe extern "C" fn pvq_add_ref(_this: *mut HostParamValueQueue) -> u32 {
    1
}
unsafe extern "C" fn pvq_release(_this: *mut HostParamValueQueue) -> u32 {
    1
}

unsafe extern "C" fn pvq_get_parameter_id(this: *mut HostParamValueQueue) -> u32 {
    (*this).param_id
}
unsafe extern "C" fn pvq_get_point_count(this: *mut HostParamValueQueue) -> i32 {
    (*this).count
}
unsafe extern "C" fn pvq_get_point(
    this: *mut HostParamValueQueue,
    index: i32,
    sample_offset: *mut i32,
    value: *mut f64,
) -> TResult {
    let q = &*this;
    if index < 0 || index >= q.count {
        return K_RESULT_FALSE;
    }
    let i = index as usize;
    if !sample_offset.is_null() {
        *sample_offset = q.offsets[i];
    }
    if !value.is_null() {
        *value = q.values[i];
    }
    K_RESULT_OK
}
unsafe extern "C" fn pvq_add_point(
    this: *mut HostParamValueQueue,
    sample_offset: i32,
    value: f64,
    index: *mut i32,
) -> TResult {
    let q = &mut *this;
    if (q.count as usize) >= MAX_POINTS_PER_QUEUE {
        return K_RESULT_FALSE;
    }
    let i = q.count as usize;
    q.offsets[i] = sample_offset;
    q.values[i] = value;
    q.count += 1;
    if !index.is_null() {
        *index = i as i32;
    }
    K_RESULT_OK
}

unsafe extern "C" fn pc_query_interface(
    _this: *mut HostParameterChanges,
    _iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if !obj.is_null() {
        *obj = std::ptr::null_mut();
    }
    K_NO_INTERFACE
}
unsafe extern "C" fn pc_add_ref(_this: *mut HostParameterChanges) -> u32 {
    1
}
unsafe extern "C" fn pc_release(_this: *mut HostParameterChanges) -> u32 {
    1
}

unsafe extern "C" fn pc_get_parameter_count(this: *mut HostParameterChanges) -> i32 {
    (*this).count
}
unsafe extern "C" fn pc_get_parameter_data(
    this: *mut HostParameterChanges,
    index: i32,
) -> *mut HostParamValueQueue {
    let pc = &mut *this;
    if index < 0 || index >= pc.count {
        return std::ptr::null_mut();
    }
    &mut pc.queues[index as usize] as *mut HostParamValueQueue
}
unsafe extern "C" fn pc_add_parameter_data(
    this: *mut HostParameterChanges,
    id: *const u32,
    index: *mut i32,
) -> *mut HostParamValueQueue {
    let pc = &mut *this;
    let param_id = if id.is_null() { 0 } else { *id };
    // Reuse an existing queue for this id if present.
    for i in 0..pc.count as usize {
        if pc.queues[i].param_id == param_id {
            if !index.is_null() {
                *index = i as i32;
            }
            return &mut pc.queues[i] as *mut HostParamValueQueue;
        }
    }
    if (pc.count as usize) >= MAX_PARAM_QUEUES {
        return std::ptr::null_mut();
    }
    let i = pc.count as usize;
    pc.queues[i].param_id = param_id;
    pc.queues[i].count = 0;
    pc.count += 1;
    if !index.is_null() {
        *index = i as i32;
    }
    &mut pc.queues[i] as *mut HostParamValueQueue
}

static PVQ_VTBL: IParamValueQueueVtbl = IParamValueQueueVtbl {
    query_interface: pvq_query_interface,
    add_ref: pvq_add_ref,
    release: pvq_release,
    get_parameter_id: pvq_get_parameter_id,
    get_point_count: pvq_get_point_count,
    get_point: pvq_get_point,
    add_point: pvq_add_point,
};

static PC_VTBL: IParameterChangesVtbl = IParameterChangesVtbl {
    query_interface: pc_query_interface,
    add_ref: pc_add_ref,
    release: pc_release,
    get_parameter_count: pc_get_parameter_count,
    get_parameter_data: pc_get_parameter_data,
    add_parameter_data: pc_add_parameter_data,
};

impl HostParameterChanges {
    /// Allocate a zeroed, vtable-wired parameter-change list on the heap
    /// (pointer-stable).  Call once at `prepare` — never on the audio thread.
    pub fn boxed() -> Box<Self> {
        // SAFETY: every field is plain data or a raw pointer; zeroing yields a
        // valid empty list.  We then patch in the vtable pointers.
        let mut b: Box<HostParameterChanges> = unsafe { Box::new(std::mem::zeroed()) };
        b.vtbl = &PC_VTBL as *const _;
        b.count = 0;
        for q in b.queues.iter_mut() {
            q.vtbl = &PVQ_VTBL as *const _;
            q.count = 0;
        }
        b
    }

    /// Reset to empty for a new block (no allocation).
    #[inline]
    pub fn reset(&mut self) {
        self.count = 0;
    }

    /// `(parameter id, last value)` of each queue that holds a point.
    pub fn latest_points(&self) -> impl Iterator<Item = (u32, f64)> + '_ {
        self.queues[..(self.count.max(0) as usize).min(MAX_PARAM_QUEUES)]
            .iter()
            .filter(|queue| queue.count > 0)
            .map(|queue| (queue.param_id, queue.values[queue.count as usize - 1]))
    }

    /// Append one normalised value point for `param_id` at `sample_offset`.
    /// Finds or creates the parameter's queue.  Silently drops if either the
    /// queue table or the per-queue point list is full.
    #[inline]
    pub fn push(&mut self, param_id: u32, sample_offset: i32, normalised: f64) {
        let this = self as *mut HostParameterChanges;
        unsafe {
            let q = pc_add_parameter_data(this, &param_id, std::ptr::null_mut());
            if !q.is_null() {
                pvq_add_point(q, sample_offset, normalised, std::ptr::null_mut());
            }
        }
    }
}

/// Thin kernel32 wrapper — avoids dependency on a specific windows-sys HMODULE type.
#[cfg(windows)]
mod win32 {
    #[link(name = "kernel32")]
    extern "system" {
        pub fn LoadLibraryW(lpLibFileName: *const u16) -> *mut std::ffi::c_void;
        pub fn FreeLibrary(hLibModule: *mut std::ffi::c_void) -> i32;
        pub fn GetProcAddress(
            hModule: *mut std::ffi::c_void,
            lpProcName: *const u8,
        ) -> Option<unsafe extern "system" fn() -> isize>;
    }
}

/// Owns a loaded .vst3 DLL.  `FreeLibrary` is called on drop.
#[cfg(windows)]
pub struct Vst3Library {
    handle: *mut c_void,
}

// SAFETY: HMODULE is process-global and safe to send across threads.
#[cfg(windows)]
unsafe impl Send for Vst3Library {}
#[cfg(windows)]
unsafe impl Sync for Vst3Library {}

#[cfg(windows)]
impl Vst3Library {
    pub fn load(dll_path: &Path) -> Result<(std::sync::Arc<Self>, *mut IPluginFactory), String> {
        let wide: Vec<u16> = dll_path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe { win32::LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() {
            return Err(format!("LoadLibraryW failed for {}", dll_path.display()));
        }

        let factory_fn =
            unsafe { win32::GetProcAddress(handle, c"GetPluginFactory".as_ptr().cast()) };
        let factory_fn = match factory_fn {
            Some(f) => f,
            None => {
                unsafe { win32::FreeLibrary(handle) };
                return Err(format!(
                    "GetPluginFactory not found in {}",
                    dll_path.display()
                ));
            }
        };

        type GetPluginFactoryFn = unsafe extern "C" fn() -> *mut IPluginFactory;
        let get_factory: GetPluginFactoryFn = unsafe { std::mem::transmute(factory_fn) };
        let factory = unsafe { get_factory() };

        if factory.is_null() {
            unsafe { win32::FreeLibrary(handle) };
            return Err("GetPluginFactory returned null".into());
        }

        let lib = std::sync::Arc::new(Vst3Library { handle });
        Ok((lib, factory))
    }

    /// Re-obtain the plugin factory from the already-mapped DLL.  The module is
    /// kept alive by this `Vst3Library`, so `GetPluginFactory` is safe to call
    /// again; the returned factory carries a ref the caller must release.
    ///
    /// # Safety
    /// Call off the audio thread.  The DLL must still be loaded (guaranteed
    /// while `self` is alive).
    pub unsafe fn factory(&self) -> Option<*mut IPluginFactory> {
        let factory_fn = win32::GetProcAddress(self.handle, c"GetPluginFactory".as_ptr().cast())?;
        type GetPluginFactoryFn = unsafe extern "C" fn() -> *mut IPluginFactory;
        let get_factory: GetPluginFactoryFn = std::mem::transmute(factory_fn);
        let factory = get_factory();
        if factory.is_null() {
            None
        } else {
            Some(factory)
        }
    }

    /// A library that maps nothing, for tests with a hand-made plugin.
    #[cfg(test)]
    pub(crate) fn placeholder() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            handle: std::ptr::null_mut(),
        })
    }
}

#[cfg(windows)]
impl Drop for Vst3Library {
    fn drop(&mut self) {
        unsafe { win32::FreeLibrary(self.handle) };
    }
}

// Stub for non-Windows builds so the rest of the crate still compiles.
#[cfg(not(windows))]
pub struct Vst3Library {
    pub path: std::path::PathBuf,
}

#[cfg(not(windows))]
impl Vst3Library {
    pub fn load(_dll_path: &Path) -> Result<(std::sync::Arc<Self>, *mut IPluginFactory), String> {
        Err("VST3 only supported on Windows (and macOS/Linux in a future release)".into())
    }
}

// Extends FUnknown (0–2) + IPluginBase (3–4) + IEditController own methods (5–17).

/// IEditController — UI/parameter controller interface, main-thread only.
/// Passed to the plugin's `queryInterface` / `createInstance`, so it must carry
/// COM byte order on Windows like `IID_ICOMPONENT` (F1/V4 — the GUI half of the
/// IID byte-order defect; without the swap QI/createInstance return E_NOINTERFACE).
pub const IID_IEDIT_CONTROLLER: Tuid = interface_iid([
    0xDC, 0xD7, 0xBB, 0xE3, 0x77, 0x42, 0x44, 0x8D, 0xA8, 0x74, 0xAA, 0xCC, 0x97, 0x9C, 0x75, 0x9E,
]);

#[repr(C)]
pub struct IEditController {
    pub vtbl: *const IEditControllerVtbl,
}

#[repr(C)]
pub struct IEditControllerVtbl {
    // FUnknown (0–2)
    pub query_interface:
        unsafe extern "C" fn(*mut IEditController, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IEditController) -> u32,
    pub release: unsafe extern "C" fn(*mut IEditController) -> u32,
    // IPluginBase (3–4)
    pub initialize: unsafe extern "C" fn(*mut IEditController, *mut FUnknown) -> TResult,
    pub terminate: unsafe extern "C" fn(*mut IEditController) -> TResult,
    // IEditController own (5–17); untyped slots are `*const c_void` placeholders.
    // (5) setComponentState — syncs a separate controller to the processor's
    // state (dual-component plugins, audit V4).
    pub set_component_state: unsafe extern "C" fn(*mut IEditController, *mut c_void) -> TResult,
    pub set_state: unsafe extern "C" fn(*mut IEditController, *mut c_void) -> TResult,
    pub _pad7: *const c_void, // getState
    pub get_parameter_count: unsafe extern "C" fn(*mut IEditController) -> i32,
    pub get_parameter_info:
        unsafe extern "C" fn(*mut IEditController, i32, *mut ParameterInfo) -> TResult,
    pub get_param_string_by_value:
        unsafe extern "C" fn(*mut IEditController, u32, f64, *mut u16) -> TResult,
    pub _pad11: *const c_void, // getParamValueByString
    pub normalized_param_to_plain: unsafe extern "C" fn(*mut IEditController, u32, f64) -> f64,
    pub plain_param_to_normalized: unsafe extern "C" fn(*mut IEditController, u32, f64) -> f64,
    pub get_param_normalized: unsafe extern "C" fn(*mut IEditController, u32) -> f64,
    pub set_param_normalized: unsafe extern "C" fn(*mut IEditController, u32, f64) -> TResult,
    // (16) setComponentHandler — typed so we can wire IComponentHandler
    pub set_component_handler: unsafe extern "C" fn(*mut IEditController, *mut FUnknown) -> TResult,
    // (17) createView — the one we call
    pub create_view: unsafe extern "C" fn(*mut IEditController, *const u8) -> *mut IPlugView,
}

/// Steinberg `String128`: UTF-16, NUL-terminated.
pub type String128 = [u16; 128];

/// `Vst::ParameterInfo`, as `IEditController::getParameterInfo` fills it.
#[repr(C)]
pub struct ParameterInfo {
    pub id: u32,
    pub title: String128,
    pub short_title: String128,
    pub units: String128,
    pub step_count: i32,
    pub default_normalized_value: f64,
    pub unit_id: i32,
    pub flags: i32,
}

const _: () = assert!(
    std::mem::size_of::<ParameterInfo>() == 792,
    "ParameterInfo layout drift"
);

/// `ParameterInfo::flags` bits.
pub const PARAM_CAN_AUTOMATE: i32 = 1 << 0;
pub const PARAM_IS_READ_ONLY: i32 = 1 << 1;
pub const PARAM_IS_HIDDEN: i32 = 1 << 4;
pub const PARAM_IS_PROGRAM_CHANGE: i32 = 1 << 15;

/// Decodes a `String128` up to its NUL.
pub fn string128_to_string(text: &[u16]) -> String {
    let end = text
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(text.len());
    String::from_utf16_lossy(&text[..end])
}

/// Plugin-preferred view rectangle (in screen pixels).
#[repr(C)]
pub struct ViewRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
pub struct IPlugView {
    pub vtbl: *const IPlugViewVtbl,
}

#[repr(C)]
pub struct IPlugViewVtbl {
    // FUnknown (0–2)
    pub query_interface:
        unsafe extern "C" fn(*mut IPlugView, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IPlugView) -> u32,
    pub release: unsafe extern "C" fn(*mut IPlugView) -> u32,
    // IPlugView (3–14)
    pub is_platform_type_supported: unsafe extern "C" fn(*mut IPlugView, *const u8) -> TResult,
    pub attached: unsafe extern "C" fn(*mut IPlugView, *mut c_void, *const u8) -> TResult,
    pub removed: unsafe extern "C" fn(*mut IPlugView) -> TResult,
    pub _pad6: *const c_void, // onWheel
    pub _pad7: *const c_void, // onKeyDown
    pub _pad8: *const c_void, // onKeyUp
    pub get_size: unsafe extern "C" fn(*mut IPlugView, *mut ViewRect) -> TResult,
    pub on_size: unsafe extern "C" fn(*mut IPlugView, *mut ViewRect) -> TResult,
    pub _pad11: *const c_void, // onFocus
    pub set_frame: unsafe extern "C" fn(*mut IPlugView, *mut FUnknown) -> TResult,
    pub can_resize: unsafe extern "C" fn(*mut IPlugView) -> TResult,
    pub check_size_constraint: unsafe extern "C" fn(*mut IPlugView, *mut ViewRect) -> TResult,
}

// Fail-fast: vtable layouts depend on each entry being pointer-sized.  These
// asserts catch accidental width drift on exotic targets.
const _: () = assert!(
    std::mem::size_of::<IPlugViewVtbl>() == 15 * std::mem::size_of::<usize>(),
    "IPlugViewVtbl layout drift",
);
const _: () = assert!(
    std::mem::size_of::<IEditControllerVtbl>() == 18 * std::mem::size_of::<usize>(),
    "IEditControllerVtbl layout drift",
);

/// Optional `IPlugView` companion (Windows only): the host passes the monitor
/// scale, since Win32 view sizes are in physical pixels.
pub const IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT: Tuid = interface_iid([
    0x65, 0xED, 0x96, 0x90, 0x8A, 0xC4, 0x45, 0x25, 0x8A, 0xAD, 0xEF, 0x7A, 0x72, 0xEA, 0x70, 0x3F,
]);

#[repr(C)]
pub struct IPlugViewContentScaleSupport {
    pub vtbl: *const IPlugViewContentScaleSupportVtbl,
}

#[repr(C)]
pub struct IPlugViewContentScaleSupportVtbl {
    pub query_interface: unsafe extern "C" fn(
        *mut IPlugViewContentScaleSupport,
        *const Tuid,
        *mut *mut c_void,
    ) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IPlugViewContentScaleSupport) -> u32,
    pub release: unsafe extern "C" fn(*mut IPlugViewContentScaleSupport) -> u32,
    pub set_content_scale_factor:
        unsafe extern "C" fn(*mut IPlugViewContentScaleSupport, f32) -> TResult,
}

// Dual-component plugins (audit V4) expose IComponent and IEditController as two
// distinct COM objects.  They are wired together through IConnectionPoint so the
// processor and controller can exchange IMessage notifications.

pub const IID_ICONNECTION_POINT: Tuid = interface_iid([
    0x70, 0xA4, 0x15, 0x6F, 0x6E, 0x6E, 0x40, 0x26, 0x98, 0x91, 0x48, 0xBF, 0xAA, 0x60, 0xD8, 0xD1,
]);

#[repr(C)]
pub struct IConnectionPoint {
    pub vtbl: *const IConnectionPointVtbl,
}

#[repr(C)]
pub struct IConnectionPointVtbl {
    // FUnknown (0–2)
    pub query_interface:
        unsafe extern "C" fn(*mut IConnectionPoint, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IConnectionPoint) -> u32,
    pub release: unsafe extern "C" fn(*mut IConnectionPoint) -> u32,
    // IConnectionPoint (3–5)
    pub connect: unsafe extern "C" fn(*mut IConnectionPoint, *mut IConnectionPoint) -> TResult,
    pub disconnect: unsafe extern "C" fn(*mut IConnectionPoint, *mut IConnectionPoint) -> TResult,
    pub notify: unsafe extern "C" fn(*mut IConnectionPoint, *mut c_void) -> TResult,
}

/// A separately-created `IEditController` for a dual-component VST3, plus the
/// two `IConnectionPoint`s wiring it to the processor's `IComponent` (null when
/// the plugin exposes none).  Returned by [`create_separate_controller`] and
/// torn down by [`teardown_separate_controller`].
pub struct SeparateController {
    pub controller: *mut IEditController,
    comp_connection: *mut IConnectionPoint,
    ctrl_connection: *mut IConnectionPoint,
}

/// Create and wire the `IEditController` for a dual-component plugin: read the
/// controller class id from the component, instantiate it through the factory,
/// initialise it with the host context, connect the two halves, and sync the
/// controller to the processor's current state (`getState` → `setComponentState`).
///
/// # Safety
/// `comp` must be a valid, initialised `IComponent` from `library`.  Call on the
/// command/main thread, never the audio thread.
pub unsafe fn create_separate_controller(
    library: &Vst3Library,
    comp: *mut IComponent,
    host_context: *mut FUnknown,
) -> Result<SeparateController, String> {
    let mut ctrl_cid: Tuid = [0u8; 16];
    let r = ((*(*comp).vtbl).get_controller_class_id)(comp, ctrl_cid.as_mut_ptr());
    if r != K_RESULT_OK {
        return Err(format!("getControllerClassId failed: {r}"));
    }

    let factory = library
        .factory()
        .ok_or_else(|| "GetPluginFactory returned null".to_string())?;

    let mut ctrl_raw: *mut c_void = std::ptr::null_mut();
    let r = ((*(*factory).vtbl).create_instance)(
        factory,
        ctrl_cid.as_ptr(),
        IID_IEDIT_CONTROLLER.as_ptr(),
        &mut ctrl_raw,
    );
    ((*(*factory).vtbl).release)(factory);
    if r != K_RESULT_OK || ctrl_raw.is_null() {
        return Err(format!("createInstance(IEditController) failed: {r}"));
    }
    let ctrl = ctrl_raw as *mut IEditController;

    let r = ((*(*ctrl).vtbl).initialize)(ctrl, host_context);
    if r != K_RESULT_OK && r != K_RESULT_FALSE {
        ((*(*ctrl).vtbl).release)(ctrl as *mut _);
        return Err(format!("IEditController::initialize failed: {r}"));
    }

    sync_controller_state(comp, ctrl);

    let (comp_connection, ctrl_connection) = connect_component_controller(comp, ctrl);

    Ok(SeparateController {
        controller: ctrl,
        comp_connection,
        ctrl_connection,
    })
}

/// Reverse [`create_separate_controller`]: disconnect the two halves, terminate
/// the controller, and release every ref this host added.
///
/// # Safety
/// `sc` must come from [`create_separate_controller`] and not have been torn
/// down already.  Call on the command/main thread.
pub unsafe fn teardown_separate_controller(sc: SeparateController) {
    if !sc.comp_connection.is_null() && !sc.ctrl_connection.is_null() {
        ((*(*sc.comp_connection).vtbl).disconnect)(sc.comp_connection, sc.ctrl_connection);
        ((*(*sc.ctrl_connection).vtbl).disconnect)(sc.ctrl_connection, sc.comp_connection);
    }
    if !sc.comp_connection.is_null() {
        ((*(*sc.comp_connection).vtbl).release)(sc.comp_connection);
    }
    if !sc.ctrl_connection.is_null() {
        ((*(*sc.ctrl_connection).vtbl).release)(sc.ctrl_connection);
    }
    ((*(*sc.controller).vtbl).terminate)(sc.controller);
    ((*(*sc.controller).vtbl).release)(sc.controller as *mut _);
}

/// The `IEditController` of a live instance: the component itself for a
/// single-component plugin, or a separately created one (audit V4).
pub enum Vst3Controller {
    Shared(std::ptr::NonNull<IEditController>),
    Separate(SeparateController),
}

impl Vst3Controller {
    pub fn as_ptr(&self) -> *mut IEditController {
        match self {
            Self::Shared(controller) => controller.as_ptr(),
            Self::Separate(separate) => separate.controller,
        }
    }
}

/// Find (single-component) or create (dual-component) the controller of `comp`
/// and give it the host's component handler.
///
/// # Safety
/// As [`create_separate_controller`].
pub unsafe fn create_controller(
    library: &Vst3Library,
    comp: *mut IComponent,
    host_context: *mut FUnknown,
    component_handler: *mut FUnknown,
) -> Result<Vst3Controller, String> {
    let mut shared: *mut c_void = std::ptr::null_mut();
    let r = ((*(*comp).vtbl).query_interface)(
        comp,
        IID_IEDIT_CONTROLLER.as_ptr() as *const Tuid,
        &mut shared,
    );
    let controller = match std::ptr::NonNull::new(shared as *mut IEditController) {
        Some(shared) if r == K_RESULT_OK => Vst3Controller::Shared(shared),
        _ => Vst3Controller::Separate(create_separate_controller(library, comp, host_context)?),
    };
    let ctrl = controller.as_ptr();
    ((*(*ctrl).vtbl).set_component_handler)(ctrl, component_handler);
    Ok(controller)
}

/// Reverse [`create_controller`].  Must run before the component's `terminate`.
///
/// # Safety
/// `controller` must come from [`create_controller`].  Call off the audio thread.
pub unsafe fn release_controller(controller: Vst3Controller) {
    let ctrl = controller.as_ptr();
    ((*(*ctrl).vtbl).set_component_handler)(ctrl, std::ptr::null_mut());
    match controller {
        Vst3Controller::Shared(_) => {
            ((*(*ctrl).vtbl).release)(ctrl);
        }
        Vst3Controller::Separate(separate) => teardown_separate_controller(separate),
    }
}

/// Copy the processor's current state into the controller so the editor opens
/// with the right values (`IComponent::getState` → `IEditController::setComponentState`).
/// Best-effort: a plugin that reports no state simply leaves the controller at
/// its defaults.
unsafe fn sync_controller_state(comp: *mut IComponent, ctrl: *mut IEditController) {
    let mut stream = HostStream::empty();
    let ib = stream.as_mut() as *mut HostStream as *mut c_void;
    if ((*(*comp).vtbl).get_state)(comp, ib) != K_RESULT_OK {
        return;
    }
    stream.pos = 0;
    ((*(*ctrl).vtbl).set_component_state)(ctrl, ib);
}

/// Wire the processor and controller halves together via `IConnectionPoint`
/// (peer-to-peer).  Returns the two connection points so the caller can
/// disconnect/release them on teardown; either is null when the plugin exposes
/// no connection point on that half.
unsafe fn connect_component_controller(
    comp: *mut IComponent,
    ctrl: *mut IEditController,
) -> (*mut IConnectionPoint, *mut IConnectionPoint) {
    let comp_cp = query_connection_point((*comp).vtbl as *const _, comp as *mut c_void);
    let ctrl_cp = query_connection_point((*ctrl).vtbl as *const _, ctrl as *mut c_void);
    if !comp_cp.is_null() && !ctrl_cp.is_null() {
        ((*(*comp_cp).vtbl).connect)(comp_cp, ctrl_cp);
        ((*(*ctrl_cp).vtbl).connect)(ctrl_cp, comp_cp);
    }
    (comp_cp, ctrl_cp)
}

/// `queryInterface(IConnectionPoint)` on a COM object reached through its
/// FUnknown-compatible vtable head.  Returns null when unsupported.
unsafe fn query_connection_point(
    vtbl: *const FUnknownVtbl,
    object: *mut c_void,
) -> *mut IConnectionPoint {
    let mut out: *mut c_void = std::ptr::null_mut();
    let r = ((*vtbl).query_interface)(
        object as *mut FUnknown,
        IID_ICONNECTION_POINT.as_ptr() as *const Tuid,
        &mut out,
    );
    if r == K_RESULT_OK && !out.is_null() {
        out as *mut IConnectionPoint
    } else {
        std::ptr::null_mut()
    }
}

// Minimal host context passed to IComponent::initialize.  Plugins QI for this
// to fetch the host name and to ask the host to create auxiliary objects
// (IMessage, IAttributeList).  Returning kResultFalse for unsupported requests
// is the documented fall-back; well-behaved plugins must accept it.

pub const IID_IHOST_APPLICATION: Tuid = interface_iid([
    0x58, 0xE5, 0x95, 0xCC, 0xDB, 0x2D, 0x49, 0x69, 0x8B, 0x6A, 0xAF, 0x8C, 0x36, 0xA6, 0x64, 0xE5,
]);

#[repr(C)]
pub struct IHostApplicationVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
    /// Fill `name` with at most 128 UTF-16 code units (Steinberg `String128`).
    pub get_name: unsafe extern "C" fn(*mut FUnknown, *mut u16) -> TResult,
    pub create_instance:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *const Tuid, *mut *mut c_void) -> TResult,
}

/// `IAttributeList`: `AttrID` is a `const char*`, `TChar` is UTF-16, and the
/// `get*` references of the SDK are pointers.
#[repr(C)]
pub struct IAttributeListVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub set_int: unsafe extern "C" fn(*mut FUnknown, *const c_char, i64) -> TResult,
    pub get_int: unsafe extern "C" fn(*mut FUnknown, *const c_char, *mut i64) -> TResult,
    pub set_float: unsafe extern "C" fn(*mut FUnknown, *const c_char, f64) -> TResult,
    pub get_float: unsafe extern "C" fn(*mut FUnknown, *const c_char, *mut f64) -> TResult,
    pub set_string: unsafe extern "C" fn(*mut FUnknown, *const c_char, *const u16) -> TResult,
    /// The size is in bytes, not in characters.
    pub get_string: unsafe extern "C" fn(*mut FUnknown, *const c_char, *mut u16, u32) -> TResult,
    pub set_binary:
        unsafe extern "C" fn(*mut FUnknown, *const c_char, *const c_void, u32) -> TResult,
    pub get_binary:
        unsafe extern "C" fn(*mut FUnknown, *const c_char, *mut *const c_void, *mut u32) -> TResult,
}

/// `IMessage`: an identifier and an `IAttributeList` the message owns.
#[repr(C)]
pub struct IMessageVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub get_message_id: unsafe extern "C" fn(*mut FUnknown) -> *const c_char,
    pub set_message_id: unsafe extern "C" fn(*mut FUnknown, *const c_char),
    pub get_attributes: unsafe extern "C" fn(*mut FUnknown) -> *mut FUnknown,
}

/// `IInfoListener` (`ivstchannelcontextinfo.h`): implemented by the edit controller,
/// which the host tells about its channel with an `IAttributeList`.
#[repr(C)]
pub struct IInfoListenerVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub set_channel_context_infos: unsafe extern "C" fn(*mut FUnknown, *mut FUnknown) -> TResult,
}

// Plugins call this to notify the host of parameter edits (begin/perform/end)
// and to request a component restart.  Many plugins assume the handler is
// non-null and crash on the first knob touch without one.

pub const IID_ICOMPONENT_HANDLER: Tuid = interface_iid([
    0x93, 0xA0, 0xBE, 0xA3, 0x0B, 0xD0, 0x45, 0xDB, 0x8E, 0x89, 0x0B, 0x0C, 0xC1, 0xE4, 0x6A, 0xC6,
]);

#[repr(C)]
pub struct IComponentHandlerVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub begin_edit: unsafe extern "C" fn(*mut FUnknown, u32) -> TResult,
    pub perform_edit: unsafe extern "C" fn(*mut FUnknown, u32, f64) -> TResult,
    pub end_edit: unsafe extern "C" fn(*mut FUnknown, u32) -> TResult,
    pub restart_component: unsafe extern "C" fn(*mut FUnknown, i32) -> TResult,
}

pub const IID_ICOMPONENT_HANDLER2: Tuid = interface_iid([
    0xF0, 0x40, 0xB4, 0xB3, 0xA3, 0x60, 0x45, 0xEC, 0xAB, 0xCD, 0xC0, 0x45, 0xB4, 0xD5, 0xA2, 0xCC,
]);

/// `TBool` is a `uint8` and `FIDString` a `const char*` (`ftypes.h`).
#[repr(C)]
pub struct IComponentHandler2Vtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub set_dirty: unsafe extern "C" fn(*mut FUnknown, u8) -> TResult,
    pub request_open_editor: unsafe extern "C" fn(*mut FUnknown, *const c_char) -> TResult,
    pub start_group_edit: unsafe extern "C" fn(*mut FUnknown) -> TResult,
    pub finish_group_edit: unsafe extern "C" fn(*mut FUnknown) -> TResult,
}

// Set on the IPlugView so the plugin can request a resize of its container.

pub const IID_IPLUG_FRAME: Tuid = interface_iid([
    0x36, 0x7F, 0xAF, 0x01, 0xAF, 0xA9, 0x46, 0x93, 0x8D, 0x4D, 0xA2, 0xA0, 0xED, 0x08, 0x82, 0xA3,
]);

#[repr(C)]
pub struct IPlugFrameVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub resize_view: unsafe extern "C" fn(*mut FUnknown, *mut IPlugView, *mut ViewRect) -> TResult,
}

/// An audio-module class of a factory.  `cid` is passed back to
/// `IPluginFactory::createInstance` to create the audio component.
pub struct Vst3Class {
    pub cid: [u8; 16],
    pub name: String,
    /// `None` when the factory does not implement `IPluginFactory2`.
    pub sub_categories: Option<String>,
}

/// Enumerate all audio-module classes in a loaded factory, with their
/// sub-categories when the factory offers `IPluginFactory2`.
///
/// # Safety
/// `factory` must be a live `IPluginFactory`.
pub unsafe fn enumerate_classes(factory: *mut IPluginFactory) -> Vec<Vst3Class> {
    let count = ((*(*factory).vtbl).count_classes)(factory);
    let mut out = Vec::with_capacity(count.max(0) as usize);

    let mut factory2: *mut c_void = std::ptr::null_mut();
    let has_factory2 =
        ((*(*factory).vtbl).query_interface)(factory, &IID_IPLUGIN_FACTORY2, &mut factory2)
            == K_RESULT_OK
            && !factory2.is_null();

    for i in 0..count {
        let mut info: PClassInfo = std::mem::zeroed();
        let r = ((*(*factory).vtbl).get_class_info)(factory, i, &mut info);
        if r != K_RESULT_OK {
            continue;
        }
        if !info.is_audio_module() {
            continue;
        }
        let sub_categories = if has_factory2 {
            let factory2 = factory2 as *mut IPluginFactory;
            let vtbl = (*factory2).vtbl as *const IPluginFactory2Vtbl;
            let mut info2: PClassInfo2 = std::mem::zeroed();
            let read = ((*vtbl).get_class_info2)(factory2, i, &mut info2);
            (read == K_RESULT_OK).then(|| info2.sub_categories_str().to_string())
        } else {
            None
        };
        out.push(Vst3Class {
            cid: info.cid,
            name: info.name_str().to_string(),
            sub_categories,
        });
    }

    if has_factory2 {
        let factory2 = factory2 as *mut IPluginFactory;
        ((*(*factory2).vtbl).release)(factory2);
    }
    out
}

/// Create `IComponent` + `IAudioProcessor` for a given class ID.
///
/// Returns both pointers (which may point to the same COM object) on success.
pub unsafe fn instantiate_processor(
    factory: *mut IPluginFactory,
    cid: &[u8; 16],
    host_context: *mut FUnknown,
) -> Result<(*mut IComponent, *mut IAudioProcessor), String> {
    // Create IComponent
    let mut component: *mut c_void = std::ptr::null_mut();
    let r = ((*(*factory).vtbl).create_instance)(
        factory,
        cid.as_ptr(),
        IID_ICOMPONENT.as_ptr(),
        &mut component,
    );
    if r != K_RESULT_OK || component.is_null() {
        return Err(format!("createInstance(IComponent) failed: {r}"));
    }
    let comp = component as *mut IComponent;

    let r = ((*(*comp).vtbl).initialize)(comp, host_context);
    if r != K_RESULT_OK && r != K_RESULT_FALSE {
        ((*(*comp).vtbl).release)(comp);
        return Err(format!("IComponent::initialize failed: {r}"));
    }

    // Query IAudioProcessor — queryInterface is the first vtable entry,
    // shared layout with FUnknown; cast comp through *mut c_void to avoid
    // the C++ vtable cast that would normally happen in a C++ class hierarchy.
    let mut processor: *mut c_void = std::ptr::null_mut();
    let r = ((*(*comp).vtbl).query_interface)(
        comp,
        IID_IAUDIO_PROCESSOR.as_ptr() as *const Tuid,
        &mut processor,
    );
    if r != K_RESULT_OK || processor.is_null() {
        ((*(*comp).vtbl).terminate)(comp);
        ((*(*comp).vtbl).release)(comp);
        return Err("queryInterface(IAudioProcessor) failed".into());
    }

    Ok((comp, processor as *mut IAudioProcessor))
}

// Phase 4b (ADR-010).  `IComponent::getState` asks the host for an `IBStream`
// to *write* the plugin's processor state into; `setState` asks for one to
// *read* it back from.  The VST3 SDK ships a concrete `MemoryStream`, but we do
// not link the SDK — so the host provides its own `Vec<u8>`-backed stream
// exposing the four IBStream methods (read / write / seek / tell) on top of
// FUnknown.
//
// Lifetime: a `HostStream` is built on the command thread for the duration of a
// single get/set_state call and dropped immediately after.  The plugin only
// touches it synchronously inside that call.  Refcount methods return a dummy
// count (the host owns the box outright), matching the pattern used by
// `HostParameterChanges` and the GUI-side COM stubs.

/// Steinberg `IBStream` IID.  The plugin queries our host stream against it, so
/// it carries COM byte order on Windows like the other interface IIDs.
pub const IID_IBSTREAM: Tuid = interface_iid([
    0xC3, 0xBF, 0x6E, 0xA2, 0x30, 0x99, 0x47, 0x52, 0x9B, 0x6B, 0xF9, 0x90, 0x1E, 0xE3, 0x3E, 0x9B,
]);

/// `IBStream::IStreamSeekMode`
const K_IB_SEEK_SET: i32 = 0;
const K_IB_SEEK_CUR: i32 = 1;
const K_IB_SEEK_END: i32 = 2;

#[repr(C)]
pub struct IBStreamVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut HostStream, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut HostStream) -> u32,
    pub release: unsafe extern "C" fn(*mut HostStream) -> u32,
    pub read: unsafe extern "C" fn(*mut HostStream, *mut c_void, i32, *mut i32) -> TResult,
    pub write: unsafe extern "C" fn(*mut HostStream, *mut c_void, i32, *mut i32) -> TResult,
    pub seek: unsafe extern "C" fn(*mut HostStream, i64, i32, *mut i64) -> TResult,
    pub tell: unsafe extern "C" fn(*mut HostStream, *mut i64) -> TResult,
}

/// Host-side `IBStream` backed by an in-memory byte buffer.  `vtbl` MUST stay
/// at offset 0 so the plugin can dispatch through it.
#[repr(C)]
pub struct HostStream {
    vtbl: *const IBStreamVtbl,
    data: Vec<u8>,
    pos: usize,
}

unsafe extern "C" fn stream_query_interface(
    this: *mut HostStream,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() {
        return K_INVALID_ARGUMENT;
    }
    if !iid.is_null() && (*iid == IID_IBSTREAM || *iid == IID_FUNKNOWN) {
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = std::ptr::null_mut();
    K_NO_INTERFACE
}
unsafe extern "C" fn stream_add_ref(_this: *mut HostStream) -> u32 {
    1
}
unsafe extern "C" fn stream_release(_this: *mut HostStream) -> u32 {
    1
}

unsafe extern "C" fn stream_read(
    this: *mut HostStream,
    buffer: *mut c_void,
    num_bytes: i32,
    num_read: *mut i32,
) -> TResult {
    let s = &mut *this;
    let want = num_bytes.max(0) as usize;
    let avail = s.data.len().saturating_sub(s.pos);
    let n = want.min(avail);
    if n > 0 && !buffer.is_null() {
        std::ptr::copy_nonoverlapping(s.data.as_ptr().add(s.pos), buffer as *mut u8, n);
        s.pos += n;
    }
    if !num_read.is_null() {
        *num_read = n as i32;
    }
    K_RESULT_OK
}

unsafe extern "C" fn stream_write(
    this: *mut HostStream,
    buffer: *mut c_void,
    num_bytes: i32,
    num_written: *mut i32,
) -> TResult {
    let s = &mut *this;
    let n = num_bytes.max(0) as usize;
    if n > 0 && !buffer.is_null() {
        // Overwrite/extend at the current position (MemoryStream semantics).
        if s.pos + n > s.data.len() {
            s.data.resize(s.pos + n, 0);
        }
        std::ptr::copy_nonoverlapping(buffer as *const u8, s.data.as_mut_ptr().add(s.pos), n);
        s.pos += n;
    }
    if !num_written.is_null() {
        *num_written = n as i32;
    }
    K_RESULT_OK
}

unsafe extern "C" fn stream_seek(
    this: *mut HostStream,
    pos: i64,
    mode: i32,
    result: *mut i64,
) -> TResult {
    let s = &mut *this;
    let base = match mode {
        K_IB_SEEK_SET => 0i64,
        K_IB_SEEK_CUR => s.pos as i64,
        K_IB_SEEK_END => s.data.len() as i64,
        _ => return K_RESULT_FALSE,
    };
    // Allow seeking past end (a following write() will zero-fill the gap).
    s.pos = (base + pos).max(0) as usize;
    if !result.is_null() {
        *result = s.pos as i64;
    }
    K_RESULT_OK
}

unsafe extern "C" fn stream_tell(this: *mut HostStream, pos: *mut i64) -> TResult {
    if pos.is_null() {
        return K_INVALID_ARGUMENT;
    }
    *pos = (*this).pos as i64;
    K_RESULT_OK
}

static IBSTREAM_VTBL: IBStreamVtbl = IBStreamVtbl {
    query_interface: stream_query_interface,
    add_ref: stream_add_ref,
    release: stream_release,
    read: stream_read,
    write: stream_write,
    seek: stream_seek,
    tell: stream_tell,
};

impl HostStream {
    fn empty() -> Box<Self> {
        Box::new(HostStream {
            vtbl: &IBSTREAM_VTBL,
            data: Vec::new(),
            pos: 0,
        })
    }
    fn with_data(data: Vec<u8>) -> Box<Self> {
        Box::new(HostStream {
            vtbl: &IBSTREAM_VTBL,
            data,
            pos: 0,
        })
    }
}

/// Read the processor state of a VST3 component into a byte buffer via a
/// host-provided `IBStream` (`IComponent::getState`).  Returns `None` if the
/// plugin reports failure or writes no bytes.
///
/// # Safety
/// `comp` must be a valid, initialised `IComponent`.  Call off the audio thread
/// (see the threading caveat in `plugin_system::instance`).
pub unsafe fn read_component_state(comp: *mut IComponent) -> Option<Vec<u8>> {
    let mut stream = HostStream::empty();
    let ib = stream.as_mut() as *mut HostStream as *mut c_void;
    let r = ((*(*comp).vtbl).get_state)(comp, ib);
    if r != K_RESULT_OK {
        return None;
    }
    let data = std::mem::take(&mut stream.data);
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

/// Restore a previously-saved blob into a VST3 component
/// (`IComponent::setState`).  Returns `true` on `kResultOk`.
///
/// # Safety
/// As [`read_component_state`].
pub unsafe fn write_component_state(comp: *mut IComponent, bytes: &[u8]) -> bool {
    let mut stream = HostStream::with_data(bytes.to_vec());
    let ib = stream.as_mut() as *mut HostStream as *mut c_void;
    ((*(*comp).vtbl).set_state)(comp, ib) == K_RESULT_OK
}

/// Hand a restored processor state to a separate controller
/// (`IEditController::setComponentState`), so its parameters match the sound.
/// A single-component controller already shares the component's state.
///
/// # Safety
/// As [`read_component_state`].
pub unsafe fn sync_controller_to_state(controller: &Vst3Controller, bytes: &[u8]) {
    let Vst3Controller::Separate(separate) = controller else {
        return;
    };
    set_controller_component_state(separate.controller, bytes);
}

/// `IEditController::setState` with `bytes`: the controller's own state, which a
/// preset file may hold besides the processor's.  Returns `true` on `kResultOk`.
///
/// # Safety
/// As [`read_component_state`], with `controller` a live `IEditController`.
pub unsafe fn set_controller_state(controller: *mut IEditController, bytes: &[u8]) -> bool {
    let mut stream = HostStream::with_data(bytes.to_vec());
    let ib = stream.as_mut() as *mut HostStream as *mut c_void;
    ((*(*controller).vtbl).set_state)(controller, ib) == K_RESULT_OK
}

/// `IEditController::setComponentState` with `bytes`, for a controller of either
/// kind: how a host tells a controller what its processor now holds.
///
/// # Safety
/// As [`read_component_state`], with `controller` a live `IEditController`.
pub unsafe fn set_controller_component_state(controller: *mut IEditController, bytes: &[u8]) {
    let mut stream = HostStream::with_data(bytes.to_vec());
    let ib = stream.as_mut() as *mut HostStream as *mut c_void;
    ((*(*controller).vtbl).set_component_state)(controller, ib);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An interface IID in the SDK's `DECLARE_CLASS_IID (I, l1, l2, l3, l4)` form.
    fn sdk_iid(l1: u32, l2: u32, l3: u32, l4: u32) -> Tuid {
        let mut bytes = [0u8; 16];
        for (chunk, long) in bytes.chunks_mut(4).zip([l1, l2, l3, l4]) {
            chunk.copy_from_slice(&long.to_be_bytes());
        }
        interface_iid(bytes)
    }

    #[test]
    fn interface_iids_match_the_sdk() {
        let expected = [
            (
                IID_ICOMPONENT,
                sdk_iid(0xE831FF31, 0xF2D54301, 0x928EBBEE, 0x25697802),
            ),
            (
                IID_IAUDIO_PROCESSOR,
                sdk_iid(0x42043F99, 0xB7DA453C, 0xA569E79D, 0x9AAEC33D),
            ),
            (
                IID_IEDIT_CONTROLLER,
                sdk_iid(0xDCD7BBE3, 0x7742448D, 0xA874AACC, 0x979C759E),
            ),
            (
                IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT,
                sdk_iid(0x65ED9690, 0x8AC44525, 0x8AADEF7A, 0x72EA703F),
            ),
            (
                IID_ICONNECTION_POINT,
                sdk_iid(0x70A4156F, 0x6E6E4026, 0x989148BF, 0xAA60D8D1),
            ),
            (
                IID_IHOST_APPLICATION,
                sdk_iid(0x58E595CC, 0xDB2D4969, 0x8B6AAF8C, 0x36A664E5),
            ),
            (
                IID_ICOMPONENT_HANDLER,
                sdk_iid(0x93A0BEA3, 0x0BD045DB, 0x8E890B0C, 0xC1E46AC6),
            ),
            (
                IID_ICOMPONENT_HANDLER2,
                sdk_iid(0xF040B4B3, 0xA36045EC, 0xABCDC045, 0xB4D5A2CC),
            ),
            (
                IID_IPLUG_FRAME,
                sdk_iid(0x367FAF01, 0xAFA94693, 0x8D4DA2A0, 0xED0882A3),
            ),
            (
                IID_IBSTREAM,
                sdk_iid(0xC3BF6EA2, 0x30994752, 0x9B6BF990, 0x1EE33E9B),
            ),
            (
                IID_IPLUGIN_FACTORY2,
                sdk_iid(0x0007B650, 0xF24B4C0B, 0xA464EDB9, 0xF00B2ABB),
            ),
            (
                IID_IMIDI_MAPPING,
                sdk_iid(0xDF0FF9F7, 0x49B74669, 0xB63AB732, 0x7ADBF5E5),
            ),
            (
                IID_IEVENT_LIST,
                sdk_iid(0x3A2C4214, 0x346349FE, 0xB2C4F397, 0xB9695A44),
            ),
            (
                IID_IMESSAGE,
                sdk_iid(0x936F033B, 0xC6C047DB, 0xBB0882F8, 0x13C1E613),
            ),
            (
                IID_IATTRIBUTE_LIST,
                sdk_iid(0x1E5F0AEB, 0xCC7F4533, 0xA2544011, 0x38AD5EE4),
            ),
            (
                IID_IINFO_LISTENER,
                sdk_iid(0x0F194781, 0x8D984ADA, 0xBBA0C1EF, 0xC011D8D0),
            ),
            (
                IID_IUNIT_INFO,
                sdk_iid(0x3D4BD6B5, 0x913A4FD2, 0xA886E768, 0xA5EB92C1),
            ),
        ];
        for (index, (actual, sdk)) in expected.iter().enumerate() {
            assert_eq!(actual, sdk, "IID #{index} differs from the SDK");
        }
    }

    #[test]
    fn result_codes_are_the_windows_values_of_funknown_h() {
        if cfg!(windows) {
            assert_eq!(K_NO_INTERFACE as u32, 0x8000_4002);
            assert_eq!(K_INVALID_ARGUMENT as u32, 0x8007_0057);
            assert_eq!(K_OUT_OF_MEMORY as u32, 0x8007_000E);
        }
        assert_eq!((K_RESULT_OK, K_RESULT_FALSE), (0, 1));
    }

    #[test]
    fn the_host_stream_answers_the_sdk_codes() {
        let mut stream = HostStream::empty();
        let this: *mut HostStream = &mut *stream;
        let table = unsafe { &*(*this).vtbl };
        let mut found: *mut c_void = std::ptr::null_mut();
        let unknown = [7u8; 16];

        unsafe {
            assert_eq!(
                (table.query_interface)(this, &IID_IBSTREAM, &mut found),
                K_RESULT_OK
            );
            assert_eq!(
                (table.query_interface)(this, &unknown, &mut found),
                K_NO_INTERFACE
            );
            assert!(found.is_null());
            assert_eq!(
                (table.query_interface)(this, &IID_IBSTREAM, std::ptr::null_mut()),
                K_INVALID_ARGUMENT
            );
            assert_eq!((table.tell)(this, std::ptr::null_mut()), K_INVALID_ARGUMENT);
        }
    }

    #[test]
    fn the_parameter_lists_refuse_every_interface_with_the_sdk_code() {
        let mut changes = HostParameterChanges::boxed();
        let this: *mut HostParameterChanges = &mut *changes;
        let queue: *mut HostParamValueQueue = unsafe { &mut (*this).queues[0] };
        let mut found: *mut c_void = 1 as *mut c_void;

        unsafe {
            let table = &*(*this).vtbl;
            assert_eq!(
                (table.query_interface)(this, &IID_FUNKNOWN, &mut found),
                K_NO_INTERFACE
            );
            assert!(found.is_null());
            let queue_table = &*(*queue).vtbl;
            found = 1 as *mut c_void;
            assert_eq!(
                (queue_table.query_interface)(queue, &IID_FUNKNOWN, &mut found),
                K_NO_INTERFACE
            );
            assert!(found.is_null());
        }
    }

    #[test]
    fn instrument_subcategories_are_told_from_effects() {
        assert!(is_instrument_subcategory("Instrument"));
        assert!(is_instrument_subcategory("Instrument|Synth"));
        assert!(is_instrument_subcategory("Instrument|Synth|Sampler"));
        assert!(!is_instrument_subcategory("Fx|Instrument"));
        assert!(!is_instrument_subcategory("Fx|Instrument|External"));
        assert!(!is_instrument_subcategory("Fx"));
        assert!(!is_instrument_subcategory(""));
    }

    #[test]
    fn class_info2_sub_categories_stop_at_nul() {
        let mut info: PClassInfo2 = unsafe { std::mem::zeroed() };
        for (slot, byte) in info.sub_categories.iter_mut().zip(b"Instrument|Synth") {
            *slot = *byte;
        }
        assert_eq!(info.sub_categories_str(), "Instrument|Synth");
    }

    #[test]
    fn string128_stops_at_nul() {
        let mut text = [0u16; 128];
        for (slot, unit) in text.iter_mut().zip("Gain".encode_utf16()) {
            *slot = unit;
        }
        assert_eq!(string128_to_string(&text), "Gain");
    }
}
