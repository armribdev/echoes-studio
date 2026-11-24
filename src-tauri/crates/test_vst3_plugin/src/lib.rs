//! A minimal VST3 plugin for the host tests (ADR-033). Not shipped.
//!
//! One DLL, two classes built on the same single component (`IComponent`,
//! `IAudioProcessor` and `IEditController` are one object, as in many real plugins):
//! - `Echoes Test Gain` (`Fx`): output = input × gain;
//! - `Echoes Test Synth` (`Instrument|Synth`): no audio input and one event input;
//!   output = the velocity of the held note × gain, silence once it is released.
//!
//! Parameters (normalised value × range = plain value):
//! - 0 `Gain` (0..2, automatable, saved in the plugin state);
//! - 1 `Hang` — once set, every block sleeps for seconds (a hung plugin);
//! - 2 `Crash` — once set, the next block aborts the process;
//! - 3..6 — read-only diagnostics: `Init apartment` and `Process apartment` (what
//!   `CoGetApartmentType` said on the thread of `initialize` / of the first `process`:
//!   0 = COM not initialised on that thread, else 1 + the `APTTYPE`), `Process calls`,
//!   `Dll init calls` (how many times the host called `InitDll`);
//! - 7 `Self value` — a value the controller and the processor each keep their own
//!   copy of (outside the state), so a host that does not tell the controller what
//!   the processor got shows it; 12 `Processor self` reads the processor's copy;
//! - 8..11 — what the plugin does by itself, as a drag or a menu would, set on the
//!   processor: `Write output` puts a new `Self value` in `outputParameterChanges`;
//!   `Hold edit` keeps an edit of `Self value` open (`beginEdit`, `performEdit`, and
//!   `endEdit` when it is released) through the host's component handler;
//!   `Lookahead` switches the latency between 64 and 128 samples and asks for a
//!   restart, applied at the next `setActive`; `Rename` renames `Self value` and
//!   reports the new titles.
//!
//! - 13..19 — the editor view (`createView("editor")`), read-only diagnostics except
//!   the last two, which the host can set before it opens the view: `View calls` (the
//!   last six calls on the view as decimal digits, the oldest first: 1 `setFrame`
//!   with a frame, 2 `setFrame` without, 3 `attached`, 4 `onSize`, 5 `removed`,
//!   6 `resizeView` accepted, 7 `resizeView` refused, 8 `setContentScaleFactor`,
//!   9 last release), `View width` / `View height` (the size the view reports),
//!   `View scale` (the content scale × 100), `View ask` (`width * 1000 + height`: the
//!   view asks the host for that size from inside `attached`), `View crash` (the
//!   next `attached` aborts the process) and `View edit` (the next `attached` edits
//!   `Self value` from the window thread, as a knob dragged in the editor would).
//!
//! - 20..22 — the instrument's MIDI: its controller implements `IMidiMapping`, which
//!   maps the mod wheel (CC 1) to `Mod wheel` and pitch bend to `Bend`; the instrument
//!   plays `velocity × gain × (1 + mod wheel) × (1 + bend − 0.5)`, which is exactly
//!   `velocity × gain` at their defaults (0 and 0.5). 22 `Last pitch` is a read-only
//!   diagnostic: the pitch of the last note on the instrument received.
//!
//! The component reports 64 samples of latency without delaying its audio, like
//! `test_clap_plugin`. Parameter points and note events are applied at their sample
//! offset. The DLL also exports `InitDll` / `ExitDll`, and three functions a test can
//! read through `GetProcAddress` even after the module was unloaded by the host:
//! `test_vst3_init_calls`, `test_vst3_exit_calls`, `test_vst3_init_apartment`.
//!
//! Both classes have a second audio output, `Aux Out` (an auxiliary bus, closed unless the
//! host opens it): half of what the main output plays. Left alone it changes nothing.
//!
//! The effect also has a mono sidechain input (`Sidechain`, an auxiliary bus): when the host feeds
//! it, a quarter of its level is added to the output. A host that does not leaves it alone.
//!
//! - 23 `Program` — the selector (`kIsProgramChange`, three steps) of the one program list
//!   the controller's `IUnitInfo` offers on both classes: `Quiet`, `Unity` (the default) and
//!   `Loud`, which set the gain to 0.25, 1.0 and 1.75. Only the processor applies a program
//!   (it takes the selector as a parameter point), so a host that never forwards the choice
//!   to the processor hears nothing. The controller's `setState` (a `.vstpreset`'s `Cont`
//!   chunk) is one `f64`: the controller's `Self value`.
//!
//! - 24..26 — the track the host announced through `IInfoListener` (on the controller,
//!   both classes), read-only diagnostics: `Track calls` (how many times
//!   `setChannelContextInfos` was called), `Track name units` (UTF-16 units of the
//!   announced `channel name`, 0 without one) and `Track color` (the announced
//!   `channel color` without its alpha byte, 0 without one).
//!
//! Not there yet, added by the step that needs it: `IUnitHandler`.

use std::ffi::c_void;
use std::mem::{offset_of, zeroed};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, AtomicU64, Ordering};

use vst3_ffi::IID_IMIDI_MAPPING;
use vst3_ffi::{
    AudioBusBuffers, Event, FUnknown, HostParamValueQueue, HostParameterChanges, HostStream,
    IAttributeListVtbl, IAudioProcessor, IAudioProcessorVtbl, IBStreamVtbl, IComponent,
    IComponentHandlerVtbl, IComponentVtbl, IEditController, IEditControllerVtbl, IInfoListenerVtbl,
    IPlugFrameVtbl, IPlugView, IPlugViewContentScaleSupport, IPlugViewContentScaleSupportVtbl,
    IPlugViewVtbl, IPluginFactory, IPluginFactory2Vtbl, PClassInfo, PClassInfo2, ParameterInfo,
    ProcessData, ProcessSetup, TResult, Tuid, ViewRect, IID_FUNKNOWN, IID_IAUDIO_PROCESSOR,
    IID_ICOMPONENT, IID_IEDIT_CONTROLLER, IID_IINFO_LISTENER, IID_IPLUGIN_FACTORY,
    IID_IPLUGIN_FACTORY2, IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT, IID_IUNIT_INFO, K_AUDIO, K_EVENT,
    K_INPUT, K_INVALID_ARGUMENT, K_NOTE_OFF_EVENT, K_NOTE_ON_EVENT, K_NO_INTERFACE, K_OUTPUT,
    K_RESULT_FALSE, K_RESULT_OK, PARAM_CAN_AUTOMATE, PARAM_IS_PROGRAM_CHANGE, PARAM_IS_READ_ONLY,
    SPEAKER_STEREO,
};

pub const EFFECT_CLASS_ID: [u8; 16] = *b"EchoesTestGain_F";
pub const INSTRUMENT_CLASS_ID: [u8; 16] = *b"EchoesTestGain_I";
pub const LATENCY: u32 = 64;
pub const LOOKAHEAD_LATENCY: u32 = 128;

pub const GAIN: u32 = 0;
pub const HANG: u32 = 1;
pub const CRASH: u32 = 2;
pub const INIT_APARTMENT: u32 = 3;
pub const PROCESS_APARTMENT: u32 = 4;
pub const PROCESS_CALLS: u32 = 5;
pub const DLL_INIT_CALLS: u32 = 6;
pub const SELF_VALUE: u32 = 7;
pub const WRITE_OUTPUT: u32 = 8;
pub const HOLD_EDIT: u32 = 9;
pub const LOOKAHEAD: u32 = 10;
pub const RENAME: u32 = 11;
pub const PROCESSOR_SELF: u32 = 12;
pub const VIEW_CALLS: u32 = 13;
pub const VIEW_WIDTH: u32 = 14;
pub const VIEW_HEIGHT: u32 = 15;
pub const VIEW_SCALE: u32 = 16;
pub const VIEW_ASK: u32 = 17;
pub const VIEW_CRASH: u32 = 18;
pub const VIEW_EDIT: u32 = 19;
pub const MOD_WHEEL: u32 = 20;
pub const BEND: u32 = 21;
pub const LAST_PITCH: u32 = 22;
pub const PROGRAM: u32 = 23;
pub const TRACK_CALLS: u32 = 24;
pub const TRACK_NAME_UNITS: u32 = 25;
pub const TRACK_COLOR: u32 = 26;
/// The RGB of `Track color` fits in 24 bits.
pub const TRACK_COLOR_RANGE: f64 = 16_777_216.0;
/// The program list of `IUnitInfo`: its names, and the gain each one sets.
pub const PROGRAM_NAMES: [&str; 3] = ["Quiet", "Unity", "Loud"];
pub const PROGRAM_GAINS: [f64; 3] = [0.25, 1.0, 1.75];
pub const PROGRAM_LIST_NAME: &str = "Factory";
const PROGRAM_LIST_ID: i32 = 1;
const PROGRAM_UNIT_ID: i32 = 1;
const ROOT_UNIT_ID: i32 = 0;
const NO_PARENT_UNIT: i32 = -1;
const NO_PROGRAM_LIST: i32 = -1;
/// `ControllerNumbers` of `ivstmidicontrollers.h`: the mod wheel and pitch bend.
const CONTROLLER_MOD_WHEEL: i16 = 1;
const CONTROLLER_PITCH_BEND: i16 = 129;

/// Index of the audio output bus the host opens only when something reads it, and
/// the share of the main sound it carries.
const AUX_OUTPUT: i32 = 1;
const AUX_LEVEL: f32 = 0.5;

/// Index of the effect's second audio input, a mono sidechain, and the share of it
/// added to the effect's output when the host feeds it.
const SIDECHAIN_INPUT: i32 = 1;
const SIDECHAIN_LEVEL: f32 = 0.25;
const SPEAKER_MONO: u64 = 1 << 19;

/// The size of the view before the host tells it another one.
pub const VIEW_INITIAL_SIZE: (i32, i32) = (400, 300);

pub const VIEW_SET_FRAME: u32 = 1;
pub const VIEW_CLEAR_FRAME: u32 = 2;
pub const VIEW_ATTACHED: u32 = 3;
pub const VIEW_ON_SIZE: u32 = 4;
pub const VIEW_REMOVED: u32 = 5;
pub const VIEW_RESIZE_ACCEPTED: u32 = 6;
pub const VIEW_RESIZE_REFUSED: u32 = 7;
pub const VIEW_SCALE_SET: u32 = 8;
pub const VIEW_RELEASED: u32 = 9;
const VIEW_CALL_DIGITS: u32 = 6;

pub const SELF_VALUE_TITLE: &str = "Self value";
pub const RENAMED_TITLE: &str = "Renamed value";
/// What `Write output` and `Hold edit` set `Self value` to.
pub const WRITTEN_VALUE: f64 = 0.9;
pub const HELD_VALUE: f64 = 0.25;

/// `Vst::RestartFlags`.
const RESTART_LATENCY_CHANGED: i32 = 1 << 3;
const RESTART_PARAM_TITLES_CHANGED: i32 = 1 << 4;

/// Plain value of a read-only diagnostic = its normalised value × this range.
pub const DIAGNOSTIC_RANGE: f64 = 1_000_000.0;
const GAIN_RANGE: f64 = 2.0;

/// `kNotImplemented` of `funknown.h` on Windows.
const K_NOT_IMPLEMENTED: TResult = if cfg!(windows) {
    0x8000_4001_u32 as TResult
} else {
    3
};
const MANY_INSTANCES: i32 = 0x7FFF_FFFF;
const BUS_FLAGS_DEFAULT_ACTIVE: u32 = 1;
const HANG_SECONDS: u64 = 5;

static INIT_CALLS: AtomicU32 = AtomicU32::new(0);
static EXIT_CALLS: AtomicU32 = AtomicU32::new(0);
static INIT_DLL_APARTMENT: AtomicI32 = AtomicI32::new(-1);

struct ParamSpec {
    id: u32,
    title: &'static str,
    range: f64,
    default_plain: f64,
    flags: i32,
}

const PARAMS: [ParamSpec; 27] = [
    ParamSpec {
        id: GAIN,
        title: "Gain",
        range: GAIN_RANGE,
        default_plain: 1.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: HANG,
        title: "Hang",
        range: 1.0,
        default_plain: 0.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: CRASH,
        title: "Crash",
        range: 1.0,
        default_plain: 0.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: INIT_APARTMENT,
        title: "Init apartment",
        range: DIAGNOSTIC_RANGE,
        default_plain: 0.0,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: PROCESS_APARTMENT,
        title: "Process apartment",
        range: DIAGNOSTIC_RANGE,
        default_plain: 0.0,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: PROCESS_CALLS,
        title: "Process calls",
        range: DIAGNOSTIC_RANGE,
        default_plain: 0.0,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: DLL_INIT_CALLS,
        title: "Dll init calls",
        range: DIAGNOSTIC_RANGE,
        default_plain: 0.0,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: SELF_VALUE,
        title: SELF_VALUE_TITLE,
        range: 1.0,
        default_plain: 0.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: WRITE_OUTPUT,
        title: "Write output",
        range: 1.0,
        default_plain: 0.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: HOLD_EDIT,
        title: "Hold edit",
        range: 1.0,
        default_plain: 0.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: LOOKAHEAD,
        title: "Lookahead",
        range: 1.0,
        default_plain: 0.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: RENAME,
        title: "Rename",
        range: 1.0,
        default_plain: 0.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: PROCESSOR_SELF,
        title: "Processor self",
        range: 1.0,
        default_plain: 0.0,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: VIEW_CALLS,
        title: "View calls",
        range: DIAGNOSTIC_RANGE,
        default_plain: 0.0,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: VIEW_WIDTH,
        title: "View width",
        range: DIAGNOSTIC_RANGE,
        default_plain: VIEW_INITIAL_SIZE.0 as f64,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: VIEW_HEIGHT,
        title: "View height",
        range: DIAGNOSTIC_RANGE,
        default_plain: VIEW_INITIAL_SIZE.1 as f64,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: VIEW_SCALE,
        title: "View scale",
        range: DIAGNOSTIC_RANGE,
        default_plain: 0.0,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: VIEW_ASK,
        title: "View ask",
        range: DIAGNOSTIC_RANGE,
        default_plain: 0.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: VIEW_CRASH,
        title: "View crash",
        range: 1.0,
        default_plain: 0.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: VIEW_EDIT,
        title: "View edit",
        range: 1.0,
        default_plain: 0.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: MOD_WHEEL,
        title: "Mod wheel",
        range: 1.0,
        default_plain: 0.0,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: BEND,
        title: "Bend",
        range: 1.0,
        default_plain: 0.5,
        flags: PARAM_CAN_AUTOMATE,
    },
    ParamSpec {
        id: LAST_PITCH,
        title: "Last pitch",
        range: DIAGNOSTIC_RANGE,
        default_plain: 0.0,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: PROGRAM,
        title: "Program",
        range: (PROGRAM_NAMES.len() - 1) as f64,
        default_plain: 1.0,
        flags: PARAM_IS_PROGRAM_CHANGE,
    },
    ParamSpec {
        id: TRACK_CALLS,
        title: "Track calls",
        range: DIAGNOSTIC_RANGE,
        default_plain: 0.0,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: TRACK_NAME_UNITS,
        title: "Track name units",
        range: DIAGNOSTIC_RANGE,
        default_plain: 0.0,
        flags: PARAM_IS_READ_ONLY,
    },
    ParamSpec {
        id: TRACK_COLOR,
        title: "Track color",
        range: TRACK_COLOR_RANGE,
        default_plain: 0.0,
        flags: PARAM_IS_READ_ONLY,
    },
];

fn spec(id: u32) -> Option<&'static ParamSpec> {
    PARAMS.iter().find(|param| param.id == id)
}

/// `0` when COM is not initialised on the calling thread, else 1 + the `APTTYPE`.
#[cfg(windows)]
fn apartment_code() -> i32 {
    #[link(name = "ole32")]
    extern "system" {
        fn CoGetApartmentType(apartment_type: *mut i32, qualifier: *mut i32) -> i32;
    }
    let (mut apartment_type, mut qualifier) = (0, 0);
    // SAFETY: both out-pointers are valid for the call.
    let result = unsafe { CoGetApartmentType(&mut apartment_type, &mut qualifier) };
    if result < 0 {
        0
    } else {
        apartment_type + 1
    }
}

#[cfg(not(windows))]
fn apartment_code() -> i32 {
    0
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Effect,
    Instrument,
}

/// The single component. The three vtable pointers come first so a pointer to any of
/// them is a valid COM interface pointer; `base_of` finds the object again.
#[repr(C)]
struct Plugin {
    component: *const IComponentVtbl,
    processor: *const IAudioProcessorVtbl,
    controller: *const IEditControllerVtbl,
    midi_mapping: *const IMidiMappingVtbl,
    unit_info: *const IUnitInfoVtbl,
    info_listener: *const IInfoListenerVtbl,
    references: AtomicU32,
    kind: Kind,
    /// What `setChannelContextInfos` was last given: the calls, the name's UTF-16
    /// units and the RGB color (0 for none).
    track_calls: AtomicU32,
    track_name_units: AtomicU32,
    track_color: AtomicU32,
    gain: AtomicU64,
    /// The selected program as an `f64` index.
    program: AtomicU64,
    held_velocity: AtomicU32,
    /// `Mod wheel` and `Bend` as `f64` bits, and the pitch of the last note on.
    mod_wheel: AtomicU64,
    bend: AtomicU64,
    last_pitch: AtomicU32,
    hang: AtomicBool,
    crash: AtomicBool,
    init_apartment: AtomicI32,
    process_apartment: AtomicI32,
    process_calls: AtomicU32,
    handler: AtomicPtr<FUnknown>,
    /// `Self value` as the controller and as the processor know it, as `f64` bits.
    controller_self: AtomicU64,
    processor_self: AtomicU64,
    write_output: AtomicBool,
    hold_edit: AtomicBool,
    edit_open: AtomicBool,
    lookahead: AtomicBool,
    latency: AtomicU32,
    pending_latency: AtomicU32,
    renamed: AtomicBool,
    view: ViewDiagnostics,
}

/// What the editor view saw, kept in the plugin so it outlives the view.
struct ViewDiagnostics {
    calls: AtomicU32,
    width: AtomicI32,
    height: AtomicI32,
    scale_percent: AtomicU32,
    ask: AtomicU32,
    crash: AtomicBool,
    edit: AtomicBool,
}

impl ViewDiagnostics {
    fn new() -> Self {
        Self {
            calls: AtomicU32::new(0),
            width: AtomicI32::new(VIEW_INITIAL_SIZE.0),
            height: AtomicI32::new(VIEW_INITIAL_SIZE.1),
            scale_percent: AtomicU32::new(0),
            ask: AtomicU32::new(0),
            crash: AtomicBool::new(false),
            edit: AtomicBool::new(false),
        }
    }

    /// Appends `call` to the digits, dropping the oldest once there are six.
    fn record(&self, call: u32) {
        let modulus = 10u32.pow(VIEW_CALL_DIGITS);
        let _ = self
            .calls
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |calls| {
                Some((calls * 10 + call) % modulus)
            });
    }
}

impl Plugin {
    fn gain(&self) -> f64 {
        f64::from_bits(self.gain.load(Ordering::Relaxed))
    }

    fn set_gain(&self, plain: f64) {
        self.gain.store(plain.to_bits(), Ordering::Relaxed);
    }

    fn plain_value(&self, id: u32) -> f64 {
        match id {
            GAIN => self.gain(),
            HANG => f64::from(self.hang.load(Ordering::Relaxed)),
            CRASH => f64::from(self.crash.load(Ordering::Relaxed)),
            INIT_APARTMENT => f64::from(self.init_apartment.load(Ordering::Relaxed)),
            PROCESS_APARTMENT => f64::from(self.process_apartment.load(Ordering::Relaxed)),
            PROCESS_CALLS => f64::from(self.process_calls.load(Ordering::Relaxed)),
            DLL_INIT_CALLS => f64::from(INIT_CALLS.load(Ordering::Relaxed)),
            SELF_VALUE => f64::from_bits(self.controller_self.load(Ordering::Relaxed)),
            PROCESSOR_SELF => f64::from_bits(self.processor_self.load(Ordering::Relaxed)),
            HOLD_EDIT => f64::from(self.hold_edit.load(Ordering::Relaxed)),
            LOOKAHEAD => f64::from(self.lookahead.load(Ordering::Relaxed)),
            RENAME => f64::from(self.renamed.load(Ordering::Relaxed)),
            VIEW_CALLS => f64::from(self.view.calls.load(Ordering::Relaxed)),
            VIEW_WIDTH => f64::from(self.view.width.load(Ordering::Relaxed)),
            VIEW_HEIGHT => f64::from(self.view.height.load(Ordering::Relaxed)),
            VIEW_SCALE => f64::from(self.view.scale_percent.load(Ordering::Relaxed)),
            VIEW_ASK => f64::from(self.view.ask.load(Ordering::Relaxed)),
            VIEW_CRASH => f64::from(self.view.crash.load(Ordering::Relaxed)),
            VIEW_EDIT => f64::from(self.view.edit.load(Ordering::Relaxed)),
            MOD_WHEEL => f64::from_bits(self.mod_wheel.load(Ordering::Relaxed)),
            BEND => f64::from_bits(self.bend.load(Ordering::Relaxed)),
            LAST_PITCH => f64::from(self.last_pitch.load(Ordering::Relaxed)),
            PROGRAM => f64::from_bits(self.program.load(Ordering::Relaxed)),
            TRACK_CALLS => f64::from(self.track_calls.load(Ordering::Relaxed)),
            TRACK_NAME_UNITS => f64::from(self.track_name_units.load(Ordering::Relaxed)),
            TRACK_COLOR => f64::from(self.track_color.load(Ordering::Relaxed)),
            _ => 0.0,
        }
    }

    /// The processor takes the selector as a parameter point and sets the gain of the
    /// program it names; the controller only remembers the selector.
    fn apply_program(&self, normalised: f64) {
        self.set_selected_program(normalised);
        let index = self.plain_value(PROGRAM).round() as usize;
        if let Some(gain) = PROGRAM_GAINS.get(index) {
            self.set_gain(*gain);
        }
    }

    fn set_selected_program(&self, normalised: f64) {
        let plain = normalised * (PROGRAM_NAMES.len() - 1) as f64;
        self.program.store(plain.to_bits(), Ordering::Relaxed);
    }

    fn title(&self, spec: &'static ParamSpec) -> &'static str {
        if spec.id == SELF_VALUE && self.renamed.load(Ordering::Relaxed) {
            RENAMED_TITLE
        } else {
            spec.title
        }
    }

    /// What the host's component handler is told, from whichever thread the plugin
    /// is on; nothing without a handler.
    fn with_handler(&self, call: impl FnOnce(*mut FUnknown, &IComponentHandlerVtbl)) {
        let handler = self.handler.load(Ordering::Relaxed);
        if !handler.is_null() {
            // SAFETY: the handler object starts with its `IComponentHandler` table
            // and stays alive while we hold the reference `setComponentHandler` took.
            call(handler, unsafe {
                &*((*handler).vtbl as *const IComponentHandlerVtbl)
            });
        }
    }

    /// A value the host sent the processor as a parameter point.
    fn apply_point(&self, id: u32, value: f64) {
        let on = value > 0.5;
        match id {
            SELF_VALUE => self
                .processor_self
                .store(value.to_bits(), Ordering::Relaxed),
            PROGRAM => self.apply_program(value),
            WRITE_OUTPUT => self.write_output.store(on, Ordering::Relaxed),
            HOLD_EDIT => self.hold_edit.store(on, Ordering::Relaxed),
            LOOKAHEAD => {
                if self.lookahead.swap(on, Ordering::Relaxed) != on {
                    let latency = if on { LOOKAHEAD_LATENCY } else { LATENCY };
                    self.pending_latency.store(latency, Ordering::Relaxed);
                    self.with_handler(|handler, table| {
                        // SAFETY: see `with_handler`.
                        unsafe { (table.restart_component)(handler, RESTART_LATENCY_CHANGED) };
                    });
                }
            }
            RENAME => {
                if on && !self.renamed.swap(true, Ordering::Relaxed) {
                    self.with_handler(|handler, table| {
                        // SAFETY: see `with_handler`.
                        unsafe { (table.restart_component)(handler, RESTART_PARAM_TITLES_CHANGED) };
                    });
                }
            }
            _ => self.set_normalised(id, value),
        }
    }

    /// Once per block, after the points were read: what the plugin does by itself.
    /// `output` is the host's `outputParameterChanges`, or null.
    unsafe fn act_by_itself(&self, output: *mut HostParameterChanges) {
        if self.write_output.swap(false, Ordering::Relaxed) {
            self.processor_self
                .store(WRITTEN_VALUE.to_bits(), Ordering::Relaxed);
            if !output.is_null() {
                let id = SELF_VALUE;
                let queue =
                    ((*(*output).vtbl).add_parameter_data)(output, &id, std::ptr::null_mut());
                if !queue.is_null() {
                    ((*(*queue).vtbl).add_point)(queue, 0, WRITTEN_VALUE, std::ptr::null_mut());
                }
            }
        }
        let held = self.hold_edit.load(Ordering::Relaxed);
        if held != self.edit_open.swap(held, Ordering::Relaxed) {
            if held {
                self.controller_self
                    .store(HELD_VALUE.to_bits(), Ordering::Relaxed);
            }
            self.with_handler(|handler, table| {
                // SAFETY: see `with_handler`.
                unsafe {
                    if held {
                        (table.begin_edit)(handler, SELF_VALUE);
                        (table.perform_edit)(handler, SELF_VALUE, HELD_VALUE);
                    } else {
                        (table.end_edit)(handler, SELF_VALUE);
                    }
                }
            });
        }
    }

    fn set_normalised(&self, id: u32, normalised: f64) {
        match id {
            GAIN => self.set_gain(normalised * GAIN_RANGE),
            HANG => self.hang.store(normalised > 0.5, Ordering::Relaxed),
            CRASH => self.crash.store(normalised > 0.5, Ordering::Relaxed),
            SELF_VALUE => self
                .controller_self
                .store(normalised.to_bits(), Ordering::Relaxed),
            VIEW_ASK => self.view.ask.store(
                (normalised * DIAGNOSTIC_RANGE).round() as u32,
                Ordering::Relaxed,
            ),
            VIEW_CRASH => self.view.crash.store(normalised > 0.5, Ordering::Relaxed),
            VIEW_EDIT => self.view.edit.store(normalised > 0.5, Ordering::Relaxed),
            MOD_WHEEL => self
                .mod_wheel
                .store(normalised.to_bits(), Ordering::Relaxed),
            BEND => self.bend.store(normalised.to_bits(), Ordering::Relaxed),
            PROGRAM => self.set_selected_program(normalised),
            _ => {}
        }
    }
}

unsafe fn base_of<T>(this: *mut T, offset: usize) -> *mut Plugin {
    (this as *mut u8).sub(offset) as *mut Plugin
}

unsafe fn from_component(this: *mut IComponent) -> *mut Plugin {
    base_of(this, offset_of!(Plugin, component))
}

unsafe fn from_processor(this: *mut IAudioProcessor) -> *mut Plugin {
    base_of(this, offset_of!(Plugin, processor))
}

unsafe fn from_controller(this: *mut IEditController) -> *mut Plugin {
    base_of(this, offset_of!(Plugin, controller))
}

unsafe fn from_midi_mapping(this: *mut IMidiMapping) -> *mut Plugin {
    base_of(this, offset_of!(Plugin, midi_mapping))
}

unsafe fn query(plugin: *mut Plugin, iid: *const Tuid, object: *mut *mut c_void) -> TResult {
    if iid.is_null() || object.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let iid = *iid;
    let interface: *mut c_void = if iid == IID_FUNKNOWN || iid == IID_ICOMPONENT {
        &raw mut (*plugin).component as *mut c_void
    } else if iid == IID_IAUDIO_PROCESSOR {
        &raw mut (*plugin).processor as *mut c_void
    } else if iid == IID_IEDIT_CONTROLLER {
        &raw mut (*plugin).controller as *mut c_void
    } else if iid == IID_IMIDI_MAPPING && (*plugin).kind == Kind::Instrument {
        &raw mut (*plugin).midi_mapping as *mut c_void
    } else if iid == IID_IUNIT_INFO {
        &raw mut (*plugin).unit_info as *mut c_void
    } else if iid == IID_IINFO_LISTENER {
        &raw mut (*plugin).info_listener as *mut c_void
    } else {
        *object = std::ptr::null_mut();
        return K_NO_INTERFACE;
    };
    (*plugin).references.fetch_add(1, Ordering::Relaxed);
    *object = interface;
    K_RESULT_OK
}

unsafe fn add_reference(plugin: *mut Plugin) -> u32 {
    (*plugin).references.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe fn release_reference(plugin: *mut Plugin) -> u32 {
    let remaining = (*plugin).references.fetch_sub(1, Ordering::AcqRel) - 1;
    if remaining == 0 {
        let handler = (*plugin)
            .handler
            .swap(std::ptr::null_mut(), Ordering::Relaxed);
        if !handler.is_null() {
            ((*(*handler).vtbl).release)(handler);
        }
        drop(Box::from_raw(plugin));
    }
    remaining
}

// ---- IComponent ------------------------------------------------------------

unsafe extern "C" fn component_query(
    this: *mut IComponent,
    iid: *const Tuid,
    object: *mut *mut c_void,
) -> TResult {
    query(from_component(this), iid, object)
}

unsafe extern "C" fn component_add_ref(this: *mut IComponent) -> u32 {
    add_reference(from_component(this))
}

unsafe extern "C" fn component_release(this: *mut IComponent) -> u32 {
    release_reference(from_component(this))
}

unsafe extern "C" fn component_initialize(
    this: *mut IComponent,
    _context: *mut FUnknown,
) -> TResult {
    (*from_component(this))
        .init_apartment
        .store(apartment_code(), Ordering::Relaxed);
    K_RESULT_OK
}

unsafe extern "C" fn component_terminate(_this: *mut IComponent) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn component_controller_class_id(
    _this: *mut IComponent,
    _id: *mut u8,
) -> TResult {
    K_NOT_IMPLEMENTED
}

unsafe extern "C" fn component_set_io_mode(_this: *mut IComponent, _mode: i32) -> TResult {
    K_RESULT_OK
}

fn bus_count(kind: Kind, media: i32, direction: i32) -> i32 {
    match (media, direction, kind) {
        (K_AUDIO, K_INPUT, Kind::Effect) => 2,
        (K_AUDIO, K_OUTPUT, _) => 2,
        (K_EVENT, K_INPUT, Kind::Instrument) => 1,
        _ => 0,
    }
}

unsafe extern "C" fn component_bus_count(this: *mut IComponent, media: i32, direction: i32) -> i32 {
    bus_count((*from_component(this)).kind, media, direction)
}

/// `Vst::BusInfo` (`ivstcomponent.h`).
#[repr(C)]
struct BusInfo {
    media_type: i32,
    direction: i32,
    channel_count: i32,
    name: [u16; 128],
    bus_type: i32,
    flags: u32,
}

const _: () = assert!(
    std::mem::size_of::<BusInfo>() == 276,
    "BusInfo layout drift"
);

fn write_utf16(destination: &mut [u16], text: &str) {
    for (slot, unit) in destination.iter_mut().zip(text.encode_utf16().chain([0])) {
        *slot = unit;
    }
}

unsafe extern "C" fn component_bus_info(
    this: *mut IComponent,
    media: i32,
    direction: i32,
    index: i32,
    info: *mut c_void,
) -> TResult {
    if index < 0
        || info.is_null()
        || index >= bus_count((*from_component(this)).kind, media, direction)
    {
        return K_INVALID_ARGUMENT;
    }
    let info = &mut *(info as *mut BusInfo);
    info.media_type = media;
    info.direction = direction;
    let is_aux_output = media == K_AUDIO && direction == K_OUTPUT && index == AUX_OUTPUT;
    let is_sidechain = media == K_AUDIO && direction == K_INPUT && index == SIDECHAIN_INPUT;
    info.channel_count = if is_sidechain || media != K_AUDIO {
        1
    } else {
        2
    };
    let name = match (media, direction) {
        _ if is_aux_output => "Aux Out",
        _ if is_sidechain => "Sidechain",
        (K_AUDIO, K_INPUT) => "Stereo In",
        (K_AUDIO, _) => "Stereo Out",
        _ => "Events",
    };
    write_utf16(&mut info.name, name);
    info.bus_type = i32::from(is_aux_output || is_sidechain);
    info.flags = if is_aux_output || is_sidechain {
        0
    } else {
        BUS_FLAGS_DEFAULT_ACTIVE
    };
    K_RESULT_OK
}

unsafe extern "C" fn component_routing_info(
    _this: *mut IComponent,
    _in: *mut c_void,
    _out: *mut c_void,
) -> TResult {
    K_NOT_IMPLEMENTED
}

unsafe extern "C" fn component_activate_bus(
    _this: *mut IComponent,
    _media: i32,
    _direction: i32,
    _index: i32,
    _state: u8,
) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn component_set_active(this: *mut IComponent, state: u8) -> TResult {
    let plugin = &*from_component(this);
    plugin.held_velocity.store(0, Ordering::Relaxed);
    if state != 0 {
        plugin.latency.store(
            plugin.pending_latency.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }
    K_RESULT_OK
}

/// The `IBStream` the host hands to `getState` / `setState`.
#[repr(C)]
struct Stream {
    vtbl: *const IBStreamVtbl,
}

unsafe fn read_gain(stream: *mut c_void) -> Option<f64> {
    let stream = stream as *mut Stream;
    let mut bytes = [0u8; 8];
    let mut read = 0;
    let result = ((*(*stream).vtbl).read)(
        stream as *mut HostStream,
        bytes.as_mut_ptr() as *mut c_void,
        8,
        &mut read,
    );
    (result == K_RESULT_OK && read == 8).then(|| f64::from_le_bytes(bytes))
}

unsafe extern "C" fn component_set_state(this: *mut IComponent, stream: *mut c_void) -> TResult {
    match read_gain(stream) {
        Some(gain) => {
            (*from_component(this)).set_gain(gain);
            K_RESULT_OK
        }
        None => K_RESULT_FALSE,
    }
}

unsafe extern "C" fn component_get_state(this: *mut IComponent, stream: *mut c_void) -> TResult {
    let stream = stream as *mut Stream;
    let mut bytes = (*from_component(this)).gain().to_le_bytes();
    let mut written = 0;
    let result = ((*(*stream).vtbl).write)(
        stream as *mut HostStream,
        bytes.as_mut_ptr() as *mut c_void,
        8,
        &mut written,
    );
    if result == K_RESULT_OK && written == 8 {
        K_RESULT_OK
    } else {
        K_RESULT_FALSE
    }
}

static COMPONENT_VTBL: IComponentVtbl = IComponentVtbl {
    query_interface: component_query,
    add_ref: component_add_ref,
    release: component_release,
    initialize: component_initialize,
    terminate: component_terminate,
    get_controller_class_id: component_controller_class_id,
    set_io_mode: component_set_io_mode,
    get_bus_count: component_bus_count,
    get_bus_info: component_bus_info,
    get_routing_info: component_routing_info,
    activate_bus: component_activate_bus,
    set_active: component_set_active,
    set_state: component_set_state,
    get_state: component_get_state,
};

// ---- IAudioProcessor -------------------------------------------------------

unsafe extern "C" fn processor_query(
    this: *mut IAudioProcessor,
    iid: *const Tuid,
    object: *mut *mut c_void,
) -> TResult {
    query(from_processor(this), iid, object)
}

unsafe extern "C" fn processor_add_ref(this: *mut IAudioProcessor) -> u32 {
    add_reference(from_processor(this))
}

unsafe extern "C" fn processor_release(this: *mut IAudioProcessor) -> u32 {
    release_reference(from_processor(this))
}

unsafe extern "C" fn processor_set_bus_arrangements(
    this: *mut IAudioProcessor,
    inputs: *mut u64,
    input_count: i32,
    outputs: *mut u64,
    output_count: i32,
) -> TResult {
    let kind = (*from_processor(this)).kind;
    let wanted_inputs = bus_count(kind, K_AUDIO, K_INPUT);
    let arranged = |arrangements: *mut u64, count: i32, mono_at: i32| {
        (0..count).all(|i| {
            let expected = if i == mono_at {
                SPEAKER_MONO
            } else {
                SPEAKER_STEREO
            };
            *arrangements.add(i as usize) == expected
        })
    };
    if input_count == wanted_inputs
        && output_count == bus_count(kind, K_AUDIO, K_OUTPUT)
        && arranged(inputs, input_count, SIDECHAIN_INPUT)
        && arranged(outputs, output_count, -1)
    {
        K_RESULT_OK
    } else {
        K_RESULT_FALSE
    }
}

unsafe extern "C" fn processor_bus_arrangement(
    this: *mut IAudioProcessor,
    direction: i32,
    index: i32,
    arrangement: *mut u64,
) -> TResult {
    if index < 0 || index >= bus_count((*from_processor(this)).kind, K_AUDIO, direction) {
        return K_INVALID_ARGUMENT;
    }
    *arrangement = if direction == K_INPUT && index == SIDECHAIN_INPUT {
        SPEAKER_MONO
    } else {
        SPEAKER_STEREO
    };
    K_RESULT_OK
}

unsafe extern "C" fn processor_can_process_sample_size(
    _this: *mut IAudioProcessor,
    symbolic_sample_size: i32,
) -> TResult {
    if symbolic_sample_size == 0 {
        K_RESULT_OK
    } else {
        K_RESULT_FALSE
    }
}

unsafe extern "C" fn processor_latency(this: *mut IAudioProcessor) -> u32 {
    (*from_processor(this)).latency.load(Ordering::Relaxed)
}

unsafe extern "C" fn processor_setup(
    _this: *mut IAudioProcessor,
    _setup: *const ProcessSetup,
) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn processor_set_processing(_this: *mut IAudioProcessor, _state: u8) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn processor_tail(_this: *mut IAudioProcessor) -> u32 {
    0
}

/// `IEventList` as the host implements it (`ivstevents.h`).
#[repr(C)]
struct EventListVtbl {
    query_interface: usize,
    add_ref: usize,
    release: usize,
    get_event_count: unsafe extern "C" fn(*mut c_void) -> i32,
    get_event: unsafe extern "C" fn(*mut c_void, i32, *mut Event) -> TResult,
    add_event: usize,
}

#[repr(C)]
struct EventList {
    vtbl: *const EventListVtbl,
}

/// What a block changes, at a sample offset.
const MAX_CHANGES: usize = 128;

struct Changes<T: Copy> {
    items: [(i32, T); MAX_CHANGES],
    count: usize,
    next: usize,
}

impl<T: Copy + Default> Changes<T> {
    fn new() -> Self {
        Self {
            items: [(0, T::default()); MAX_CHANGES],
            count: 0,
            next: 0,
        }
    }

    fn push(&mut self, offset: i32, value: T) {
        if self.count < MAX_CHANGES {
            self.items[self.count] = (offset, value);
            self.count += 1;
        }
    }

    fn due(&mut self, frame: usize) -> Option<T> {
        let (offset, value) = *self.items[..self.count].get(self.next)?;
        if offset.max(0) as usize <= frame {
            self.next += 1;
            Some(value)
        } else {
            None
        }
    }
}

unsafe fn read_param_changes(
    plugin: &Plugin,
    changes: *mut HostParameterChanges,
    gain: &mut Changes<f64>,
) {
    if changes.is_null() {
        return;
    }
    let table = &*(*changes).vtbl;
    for index in 0..(table.get_parameter_count)(changes) {
        let queue: *mut HostParamValueQueue = (table.get_parameter_data)(changes, index);
        if queue.is_null() {
            continue;
        }
        let queue_table = &*(*queue).vtbl;
        let id = (queue_table.get_parameter_id)(queue);
        for point in 0..(queue_table.get_point_count)(queue) {
            let (mut offset, mut value) = (0, 0.0);
            if (queue_table.get_point)(queue, point, &mut offset, &mut value) != K_RESULT_OK {
                continue;
            }
            if id == GAIN {
                gain.push(offset, value * GAIN_RANGE);
            } else {
                plugin.apply_point(id, value);
            }
        }
    }
}

unsafe fn read_notes(plugin: &Plugin, events: *mut c_void, notes: &mut Changes<f32>) {
    if events.is_null() {
        return;
    }
    let list = events as *mut EventList;
    let table = &*(*list).vtbl;
    for index in 0..(table.get_event_count)(events) {
        let mut event: Event = Event::EMPTY;
        if (table.get_event)(events, index, &mut event) != K_RESULT_OK {
            continue;
        }
        match event.kind {
            K_NOTE_ON_EVENT => {
                plugin
                    .last_pitch
                    .store(event.payload.note_on.pitch as u32, Ordering::Relaxed);
                notes.push(event.sample_offset, event.payload.note_on.velocity);
            }
            K_NOTE_OFF_EVENT => notes.push(event.sample_offset, 0.0),
            _ => {}
        }
    }
}

unsafe fn channel(buses: *mut AudioBusBuffers, index: usize) -> Option<*mut f32> {
    if buses.is_null() || index >= (*buses).num_channels as usize {
        return None;
    }
    Some(*(*buses).channel_buffers32.add(index))
}

unsafe extern "C" fn processor_process(
    this: *mut IAudioProcessor,
    data: *mut ProcessData,
) -> TResult {
    let plugin = &*from_processor(this);
    let data = &*data;
    if plugin.process_calls.fetch_add(1, Ordering::Relaxed) == 0 {
        plugin
            .process_apartment
            .store(apartment_code(), Ordering::Relaxed);
    }

    let mut gain_changes = Changes::<f64>::new();
    let mut note_changes = Changes::<f32>::new();
    read_param_changes(
        plugin,
        data.input_param_changes as *mut HostParameterChanges,
        &mut gain_changes,
    );
    read_notes(plugin, data.input_events, &mut note_changes);
    plugin.act_by_itself(data.output_param_changes as *mut HostParameterChanges);

    if plugin.crash.load(Ordering::Relaxed) {
        std::process::abort();
    }
    if plugin.hang.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_secs(HANG_SECONDS));
    }

    let mut gain = plugin.gain();
    let mut held = f32::from_bits(plugin.held_velocity.load(Ordering::Relaxed));
    let wheel = f64::from_bits(plugin.mod_wheel.load(Ordering::Relaxed));
    let bend = f64::from_bits(plugin.bend.load(Ordering::Relaxed));
    let shape = ((1.0 + wheel) * (1.0 + (bend - 0.5))) as f32;
    let sidechain = match plugin.kind {
        Kind::Effect if data.num_inputs > SIDECHAIN_INPUT => {
            channel(data.inputs.add(SIDECHAIN_INPUT as usize), 0)
        }
        _ => None,
    };
    for frame in 0..data.num_samples.max(0) as usize {
        while let Some(plain) = gain_changes.due(frame) {
            gain = plain;
        }
        while let Some(velocity) = note_changes.due(frame) {
            held = velocity;
        }
        for index in 0..2 {
            let Some(output) = channel(data.outputs, index) else {
                break;
            };
            let sound = match plugin.kind {
                Kind::Effect => {
                    let heard = channel(data.inputs, index)
                        .map_or(0.0, |input| *input.add(frame) * gain as f32);
                    match sidechain {
                        Some(side) => heard + *side.add(frame) * SIDECHAIN_LEVEL,
                        None => heard,
                    }
                }
                Kind::Instrument => held * gain as f32 * shape,
            };
            *output.add(frame) = sound;
            if data.num_outputs > AUX_OUTPUT {
                if let Some(aux) = channel(data.outputs.add(AUX_OUTPUT as usize), index) {
                    *aux.add(frame) = sound * AUX_LEVEL;
                }
            }
        }
    }
    if data.num_samples <= 0 {
        // A parameter flush: no audio, the points are the whole message.
        while let Some(plain) = gain_changes.due(0) {
            gain = plain;
        }
    }
    plugin.set_gain(gain);
    plugin
        .held_velocity
        .store(held.to_bits(), Ordering::Relaxed);
    K_RESULT_OK
}

static PROCESSOR_VTBL: IAudioProcessorVtbl = IAudioProcessorVtbl {
    query_interface: processor_query,
    add_ref: processor_add_ref,
    release: processor_release,
    set_bus_arrangements: processor_set_bus_arrangements,
    get_bus_arrangement: processor_bus_arrangement,
    can_process_sample_size: processor_can_process_sample_size,
    get_latency_samples: processor_latency,
    setup_processing: processor_setup,
    set_processing: processor_set_processing,
    process: processor_process,
    get_tail_samples: processor_tail,
};

// ---- IEditController -------------------------------------------------------

unsafe extern "C" fn controller_query(
    this: *mut IEditController,
    iid: *const Tuid,
    object: *mut *mut c_void,
) -> TResult {
    query(from_controller(this), iid, object)
}

unsafe extern "C" fn controller_add_ref(this: *mut IEditController) -> u32 {
    add_reference(from_controller(this))
}

unsafe extern "C" fn controller_release(this: *mut IEditController) -> u32 {
    release_reference(from_controller(this))
}

unsafe extern "C" fn controller_initialize(
    _this: *mut IEditController,
    _context: *mut FUnknown,
) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn controller_terminate(_this: *mut IEditController) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn controller_set_component_state(
    this: *mut IEditController,
    stream: *mut c_void,
) -> TResult {
    match read_gain(stream) {
        Some(gain) => {
            (*from_controller(this)).set_gain(gain);
            K_RESULT_OK
        }
        None => K_RESULT_FALSE,
    }
}

/// The controller's own state is its `Self value`, one `f64`.
unsafe extern "C" fn controller_set_state(
    this: *mut IEditController,
    stream: *mut c_void,
) -> TResult {
    match read_gain(stream) {
        Some(value) => {
            (*from_controller(this)).set_normalised(SELF_VALUE, value);
            K_RESULT_OK
        }
        None => K_RESULT_FALSE,
    }
}

unsafe extern "C" fn controller_get_state(
    _this: *mut IEditController,
    _stream: *mut c_void,
) -> TResult {
    K_RESULT_FALSE
}

unsafe extern "C" fn controller_parameter_count(_this: *mut IEditController) -> i32 {
    PARAMS.len() as i32
}

unsafe extern "C" fn controller_parameter_info(
    this: *mut IEditController,
    index: i32,
    info: *mut ParameterInfo,
) -> TResult {
    let Some(param) = PARAMS.get(index as usize) else {
        return K_INVALID_ARGUMENT;
    };
    let info = &mut *info;
    *info = zeroed();
    info.id = param.id;
    let title = (*from_controller(this)).title(param);
    write_utf16(&mut info.title, title);
    write_utf16(&mut info.short_title, title);
    info.default_normalized_value = param.default_plain / param.range;
    info.flags = param.flags;
    if param.id == PROGRAM {
        info.step_count = (PROGRAM_NAMES.len() - 1) as i32;
        info.unit_id = PROGRAM_UNIT_ID;
    }
    K_RESULT_OK
}

unsafe extern "C" fn controller_string_by_value(
    _this: *mut IEditController,
    id: u32,
    normalised: f64,
    text: *mut u16,
) -> TResult {
    let Some(param) = spec(id) else {
        return K_INVALID_ARGUMENT;
    };
    let rendered = format!("{:.2}", normalised * param.range);
    write_utf16(std::slice::from_raw_parts_mut(text, 128), &rendered);
    K_RESULT_OK
}

unsafe extern "C" fn controller_value_by_string(
    _this: *mut IEditController,
    _id: u32,
    _text: *mut u16,
    _value: *mut f64,
) -> TResult {
    K_RESULT_FALSE
}

unsafe extern "C" fn controller_normalised_to_plain(
    _this: *mut IEditController,
    id: u32,
    normalised: f64,
) -> f64 {
    spec(id).map_or(0.0, |param| normalised * param.range)
}

unsafe extern "C" fn controller_plain_to_normalised(
    _this: *mut IEditController,
    id: u32,
    plain: f64,
) -> f64 {
    spec(id).map_or(0.0, |param| plain / param.range)
}

unsafe extern "C" fn controller_get_normalised(this: *mut IEditController, id: u32) -> f64 {
    spec(id).map_or(0.0, |param| {
        (*from_controller(this)).plain_value(id) / param.range
    })
}

unsafe extern "C" fn controller_set_normalised(
    this: *mut IEditController,
    id: u32,
    normalised: f64,
) -> TResult {
    if spec(id).is_none() {
        return K_INVALID_ARGUMENT;
    }
    (*from_controller(this)).set_normalised(id, normalised);
    K_RESULT_OK
}

unsafe extern "C" fn controller_set_handler(
    this: *mut IEditController,
    handler: *mut FUnknown,
) -> TResult {
    if !handler.is_null() {
        ((*(*handler).vtbl).add_ref)(handler);
    }
    let previous = (*from_controller(this))
        .handler
        .swap(handler, Ordering::Relaxed);
    if !previous.is_null() {
        ((*(*previous).vtbl).release)(previous);
    }
    K_RESULT_OK
}

// ---- IMidiMapping ----------------------------------------------------------

#[repr(C)]
struct IMidiMapping {
    vtbl: *const IMidiMappingVtbl,
}

#[repr(C)]
struct IMidiMappingVtbl {
    query_interface:
        unsafe extern "C" fn(*mut IMidiMapping, *const Tuid, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut IMidiMapping) -> u32,
    release: unsafe extern "C" fn(*mut IMidiMapping) -> u32,
    get_midi_controller_assignment:
        unsafe extern "C" fn(*mut IMidiMapping, i32, i16, i16, *mut u32) -> TResult,
}

unsafe extern "C" fn midi_mapping_query(
    this: *mut IMidiMapping,
    iid: *const Tuid,
    object: *mut *mut c_void,
) -> TResult {
    query(from_midi_mapping(this), iid, object)
}

unsafe extern "C" fn midi_mapping_add_ref(this: *mut IMidiMapping) -> u32 {
    add_reference(from_midi_mapping(this))
}

unsafe extern "C" fn midi_mapping_release(this: *mut IMidiMapping) -> u32 {
    release_reference(from_midi_mapping(this))
}

/// The main event bus, channel 0 only, as a host asks for it.
unsafe extern "C" fn midi_mapping_assignment(
    _this: *mut IMidiMapping,
    bus: i32,
    channel: i16,
    controller: i16,
    id: *mut u32,
) -> TResult {
    if bus != 0 || channel != 0 || id.is_null() {
        return K_RESULT_FALSE;
    }
    match controller {
        CONTROLLER_MOD_WHEEL => *id = MOD_WHEEL,
        CONTROLLER_PITCH_BEND => *id = BEND,
        _ => return K_RESULT_FALSE,
    }
    K_RESULT_OK
}

static MIDI_MAPPING_VTBL: IMidiMappingVtbl = IMidiMappingVtbl {
    query_interface: midi_mapping_query,
    add_ref: midi_mapping_add_ref,
    release: midi_mapping_release,
    get_midi_controller_assignment: midi_mapping_assignment,
};

// ---- IInfoListener ---------------------------------------------------------

unsafe fn from_info_listener(this: *mut FUnknown) -> *mut Plugin {
    base_of(this, offset_of!(Plugin, info_listener))
}

unsafe extern "C" fn info_listener_query(
    this: *mut FUnknown,
    iid: *const Tuid,
    object: *mut *mut c_void,
) -> TResult {
    query(from_info_listener(this), iid, object)
}

unsafe extern "C" fn info_listener_add_ref(this: *mut FUnknown) -> u32 {
    add_reference(from_info_listener(this))
}

unsafe extern "C" fn info_listener_release(this: *mut FUnknown) -> u32 {
    release_reference(from_info_listener(this))
}

/// Keeps what the host announced: the name's length in UTF-16 units and the color's
/// RGB, each 0 when the list does not carry it.
unsafe extern "C" fn info_listener_set_channel_context_infos(
    this: *mut FUnknown,
    list: *mut FUnknown,
) -> TResult {
    if list.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let plugin = &*from_info_listener(this);
    let list_vtbl = (*list).vtbl as *const IAttributeListVtbl;

    let mut name = [0u16; 128];
    let name_bytes = std::mem::size_of_val(&name) as u32;
    let name_units = if ((*list_vtbl).get_string)(
        list,
        c"channel name".as_ptr(),
        name.as_mut_ptr(),
        name_bytes,
    ) == K_RESULT_OK
    {
        name.iter().take_while(|&&unit| unit != 0).count() as u32
    } else {
        0
    };

    let mut color = 0i64;
    let rgb = if ((*list_vtbl).get_int)(list, c"channel color".as_ptr(), &mut color) == K_RESULT_OK
    {
        (color as u32) & 0x00FF_FFFF
    } else {
        0
    };

    plugin.track_name_units.store(name_units, Ordering::Relaxed);
    plugin.track_color.store(rgb, Ordering::Relaxed);
    plugin.track_calls.fetch_add(1, Ordering::Relaxed);
    K_RESULT_OK
}

static INFO_LISTENER_VTBL: IInfoListenerVtbl = IInfoListenerVtbl {
    query_interface: info_listener_query,
    add_ref: info_listener_add_ref,
    release: info_listener_release,
    set_channel_context_infos: info_listener_set_channel_context_infos,
};

// ---- IUnitInfo -------------------------------------------------------------

#[repr(C)]
struct IUnitInfo {
    vtbl: *const IUnitInfoVtbl,
}

/// `Vst::UnitInfo` and `Vst::ProgramListInfo` (`ivstunits.h`).
#[repr(C)]
struct UnitInfo {
    id: i32,
    parent_unit_id: i32,
    name: [u16; 128],
    program_list_id: i32,
}

#[repr(C)]
struct ProgramListInfo {
    id: i32,
    name: [u16; 128],
    program_count: i32,
}

#[repr(C)]
struct IUnitInfoVtbl {
    query_interface: unsafe extern "C" fn(*mut IUnitInfo, *const Tuid, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut IUnitInfo) -> u32,
    release: unsafe extern "C" fn(*mut IUnitInfo) -> u32,
    get_unit_count: unsafe extern "C" fn(*mut IUnitInfo) -> i32,
    get_unit_info: unsafe extern "C" fn(*mut IUnitInfo, i32, *mut UnitInfo) -> TResult,
    get_program_list_count: unsafe extern "C" fn(*mut IUnitInfo) -> i32,
    get_program_list_info:
        unsafe extern "C" fn(*mut IUnitInfo, i32, *mut ProgramListInfo) -> TResult,
    get_program_name: unsafe extern "C" fn(*mut IUnitInfo, i32, i32, *mut u16) -> TResult,
    get_program_info:
        unsafe extern "C" fn(*mut IUnitInfo, i32, i32, *const u8, *mut u16) -> TResult,
    has_program_pitch_names: unsafe extern "C" fn(*mut IUnitInfo, i32, i32) -> TResult,
    get_program_pitch_name:
        unsafe extern "C" fn(*mut IUnitInfo, i32, i32, i16, *mut u16) -> TResult,
    get_selected_unit: unsafe extern "C" fn(*mut IUnitInfo) -> i32,
    select_unit: unsafe extern "C" fn(*mut IUnitInfo, i32) -> TResult,
    get_unit_by_bus: unsafe extern "C" fn(*mut IUnitInfo, i32, i32, i32, i32, *mut i32) -> TResult,
    set_unit_program_data: unsafe extern "C" fn(*mut IUnitInfo, i32, i32, *mut c_void) -> TResult,
}

unsafe fn from_unit_info(this: *mut IUnitInfo) -> *mut Plugin {
    base_of(this, offset_of!(Plugin, unit_info))
}

unsafe extern "C" fn unit_info_query(
    this: *mut IUnitInfo,
    iid: *const Tuid,
    object: *mut *mut c_void,
) -> TResult {
    query(from_unit_info(this), iid, object)
}

unsafe extern "C" fn unit_info_add_ref(this: *mut IUnitInfo) -> u32 {
    add_reference(from_unit_info(this))
}

unsafe extern "C" fn unit_info_release(this: *mut IUnitInfo) -> u32 {
    release_reference(from_unit_info(this))
}

/// The root unit, and the one the `Program` selector belongs to, which uses the list.
unsafe extern "C" fn unit_info_unit_count(_this: *mut IUnitInfo) -> i32 {
    2
}

unsafe extern "C" fn unit_info_unit_info(
    _this: *mut IUnitInfo,
    index: i32,
    info: *mut UnitInfo,
) -> TResult {
    let (id, parent_unit_id, name, program_list_id) = match index {
        0 => (ROOT_UNIT_ID, NO_PARENT_UNIT, "Root", NO_PROGRAM_LIST),
        1 => (PROGRAM_UNIT_ID, ROOT_UNIT_ID, "Programs", PROGRAM_LIST_ID),
        _ => return K_INVALID_ARGUMENT,
    };
    let info = &mut *info;
    *info = zeroed();
    info.id = id;
    info.parent_unit_id = parent_unit_id;
    info.program_list_id = program_list_id;
    write_utf16(&mut info.name, name);
    K_RESULT_OK
}

unsafe extern "C" fn unit_info_program_list_count(_this: *mut IUnitInfo) -> i32 {
    1
}

unsafe extern "C" fn unit_info_program_list_info(
    _this: *mut IUnitInfo,
    index: i32,
    info: *mut ProgramListInfo,
) -> TResult {
    if index != 0 {
        return K_INVALID_ARGUMENT;
    }
    let info = &mut *info;
    *info = zeroed();
    info.id = PROGRAM_LIST_ID;
    info.program_count = PROGRAM_NAMES.len() as i32;
    write_utf16(&mut info.name, PROGRAM_LIST_NAME);
    K_RESULT_OK
}

unsafe extern "C" fn unit_info_program_name(
    _this: *mut IUnitInfo,
    list_id: i32,
    program: i32,
    name: *mut u16,
) -> TResult {
    let program = usize::try_from(program)
        .ok()
        .and_then(|i| PROGRAM_NAMES.get(i));
    match (list_id, program) {
        (PROGRAM_LIST_ID, Some(program)) => {
            write_utf16(std::slice::from_raw_parts_mut(name, 128), program);
            K_RESULT_OK
        }
        _ => K_INVALID_ARGUMENT,
    }
}

unsafe extern "C" fn unit_info_program_info(
    _this: *mut IUnitInfo,
    _list_id: i32,
    _program: i32,
    _attribute: *const u8,
    _value: *mut u16,
) -> TResult {
    K_RESULT_FALSE
}

unsafe extern "C" fn unit_info_has_pitch_names(
    _this: *mut IUnitInfo,
    _list_id: i32,
    _program: i32,
) -> TResult {
    K_RESULT_FALSE
}

unsafe extern "C" fn unit_info_pitch_name(
    _this: *mut IUnitInfo,
    _list_id: i32,
    _program: i32,
    _pitch: i16,
    _name: *mut u16,
) -> TResult {
    K_RESULT_FALSE
}

unsafe extern "C" fn unit_info_selected_unit(_this: *mut IUnitInfo) -> i32 {
    ROOT_UNIT_ID
}

unsafe extern "C" fn unit_info_select_unit(_this: *mut IUnitInfo, _unit: i32) -> TResult {
    K_RESULT_FALSE
}

unsafe extern "C" fn unit_info_unit_by_bus(
    _this: *mut IUnitInfo,
    _media: i32,
    _direction: i32,
    _bus: i32,
    _channel: i32,
    _unit: *mut i32,
) -> TResult {
    K_RESULT_FALSE
}

unsafe extern "C" fn unit_info_set_program_data(
    _this: *mut IUnitInfo,
    _list_id: i32,
    _program: i32,
    _stream: *mut c_void,
) -> TResult {
    K_RESULT_FALSE
}

static UNIT_INFO_VTBL: IUnitInfoVtbl = IUnitInfoVtbl {
    query_interface: unit_info_query,
    add_ref: unit_info_add_ref,
    release: unit_info_release,
    get_unit_count: unit_info_unit_count,
    get_unit_info: unit_info_unit_info,
    get_program_list_count: unit_info_program_list_count,
    get_program_list_info: unit_info_program_list_info,
    get_program_name: unit_info_program_name,
    get_program_info: unit_info_program_info,
    has_program_pitch_names: unit_info_has_pitch_names,
    get_program_pitch_name: unit_info_pitch_name,
    get_selected_unit: unit_info_selected_unit,
    select_unit: unit_info_select_unit,
    get_unit_by_bus: unit_info_unit_by_bus,
    set_unit_program_data: unit_info_set_program_data,
};

// ---- IPlugView -------------------------------------------------------------

/// The editor view: a window-less one that only keeps its size and records what the
/// host does to it in the plugin's `ViewDiagnostics`. It holds a reference on the
/// plugin, so a host that releases the controller first does not free it under the
/// view. `IPlugViewContentScaleSupport` is the second table of the same object.
#[repr(C)]
struct View {
    view: *const IPlugViewVtbl,
    scale_support: *const IPlugViewContentScaleSupportVtbl,
    references: AtomicU32,
    plugin: *mut Plugin,
    frame: AtomicPtr<FUnknown>,
}

unsafe fn diagnostics<'a>(view: *mut View) -> &'a ViewDiagnostics {
    &(*(*view).plugin).view
}

unsafe fn view_of(this: *mut IPlugView) -> *mut View {
    this as *mut View
}

unsafe fn view_of_scale_support(this: *mut IPlugViewContentScaleSupport) -> *mut View {
    (this as *mut u8).sub(offset_of!(View, scale_support)) as *mut View
}

unsafe fn view_add_reference(view: *mut View) -> u32 {
    (*view).references.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe fn view_release_reference(view: *mut View) -> u32 {
    let remaining = (*view).references.fetch_sub(1, Ordering::AcqRel) - 1;
    if remaining == 0 {
        let plugin = (*view).plugin;
        diagnostics(view).record(VIEW_RELEASED);
        drop(Box::from_raw(view));
        release_reference(plugin);
    }
    remaining
}

unsafe extern "C" fn view_query_interface(
    this: *mut IPlugView,
    iid: *const Tuid,
    object: *mut *mut c_void,
) -> TResult {
    if iid.is_null() || object.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let view = view_of(this);
    let interface: *mut c_void = if *iid == IID_FUNKNOWN {
        view as *mut c_void
    } else if *iid == IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT {
        &raw mut (*view).scale_support as *mut c_void
    } else {
        *object = std::ptr::null_mut();
        return K_NO_INTERFACE;
    };
    view_add_reference(view);
    *object = interface;
    K_RESULT_OK
}

unsafe extern "C" fn view_add_ref(this: *mut IPlugView) -> u32 {
    view_add_reference(view_of(this))
}

unsafe extern "C" fn view_release(this: *mut IPlugView) -> u32 {
    view_release_reference(view_of(this))
}

unsafe extern "C" fn view_is_platform_type_supported(
    _: *mut IPlugView,
    kind: *const u8,
) -> TResult {
    if !kind.is_null() && std::ffi::CStr::from_ptr(kind.cast()).to_bytes() == b"HWND" {
        K_RESULT_OK
    } else {
        K_RESULT_FALSE
    }
}

unsafe fn ask_host_to_resize(view: *mut View, width: i32, height: i32) {
    let frame = (*view).frame.load(Ordering::Relaxed);
    if frame.is_null() {
        return;
    }
    let mut rect = ViewRect {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    let table = &**(frame as *const *const IPlugFrameVtbl);
    let answer = (table.resize_view)(frame, view as *mut IPlugView, &mut rect);
    diagnostics(view).record(if answer == K_RESULT_OK {
        VIEW_RESIZE_ACCEPTED
    } else {
        VIEW_RESIZE_REFUSED
    });
}

unsafe extern "C" fn view_attached(
    this: *mut IPlugView,
    parent: *mut c_void,
    _kind: *const u8,
) -> TResult {
    if parent.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let view = view_of(this);
    let state = diagnostics(view);
    state.record(VIEW_ATTACHED);
    if state.crash.load(Ordering::Relaxed) {
        std::process::abort();
    }
    match state.ask.load(Ordering::Relaxed) {
        0 => {}
        ask => ask_host_to_resize(view, (ask / 1000) as i32, (ask % 1000) as i32),
    }
    if state.edit.load(Ordering::Relaxed) {
        let plugin = &*(*view).plugin;
        plugin
            .controller_self
            .store(HELD_VALUE.to_bits(), Ordering::Relaxed);
        plugin.with_handler(|handler, table| {
            // SAFETY: see `with_handler`.
            unsafe {
                (table.begin_edit)(handler, SELF_VALUE);
                (table.perform_edit)(handler, SELF_VALUE, HELD_VALUE);
                (table.end_edit)(handler, SELF_VALUE);
            }
        });
    }
    K_RESULT_OK
}

unsafe extern "C" fn view_removed(this: *mut IPlugView) -> TResult {
    diagnostics(view_of(this)).record(VIEW_REMOVED);
    K_RESULT_OK
}

unsafe extern "C" fn view_get_size(this: *mut IPlugView, rect: *mut ViewRect) -> TResult {
    let state = diagnostics(view_of(this));
    *rect = ViewRect {
        left: 0,
        top: 0,
        right: state.width.load(Ordering::Relaxed),
        bottom: state.height.load(Ordering::Relaxed),
    };
    K_RESULT_OK
}

unsafe extern "C" fn view_on_size(this: *mut IPlugView, rect: *mut ViewRect) -> TResult {
    let state = diagnostics(view_of(this));
    state.record(VIEW_ON_SIZE);
    state
        .width
        .store((*rect).right - (*rect).left, Ordering::Relaxed);
    state
        .height
        .store((*rect).bottom - (*rect).top, Ordering::Relaxed);
    K_RESULT_OK
}

unsafe extern "C" fn view_set_frame(this: *mut IPlugView, frame: *mut FUnknown) -> TResult {
    let view = view_of(this);
    (*view).frame.store(frame, Ordering::Relaxed);
    diagnostics(view).record(if frame.is_null() {
        VIEW_CLEAR_FRAME
    } else {
        VIEW_SET_FRAME
    });
    K_RESULT_OK
}

unsafe extern "C" fn view_can_resize(_: *mut IPlugView) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn view_check_size_constraint(_: *mut IPlugView, _: *mut ViewRect) -> TResult {
    K_RESULT_OK
}

static VIEW_VTBL: Shared<IPlugViewVtbl> = Shared(IPlugViewVtbl {
    query_interface: view_query_interface,
    add_ref: view_add_ref,
    release: view_release,
    is_platform_type_supported: view_is_platform_type_supported,
    attached: view_attached,
    removed: view_removed,
    _pad6: std::ptr::null(),
    _pad7: std::ptr::null(),
    _pad8: std::ptr::null(),
    get_size: view_get_size,
    on_size: view_on_size,
    _pad11: std::ptr::null(),
    set_frame: view_set_frame,
    can_resize: view_can_resize,
    check_size_constraint: view_check_size_constraint,
});

unsafe extern "C" fn scale_query_interface(
    this: *mut IPlugViewContentScaleSupport,
    iid: *const Tuid,
    object: *mut *mut c_void,
) -> TResult {
    let view = view_of_scale_support(this);
    view_query_interface(view as *mut IPlugView, iid, object)
}

unsafe extern "C" fn scale_add_ref(this: *mut IPlugViewContentScaleSupport) -> u32 {
    view_add_reference(view_of_scale_support(this))
}

unsafe extern "C" fn scale_release(this: *mut IPlugViewContentScaleSupport) -> u32 {
    view_release_reference(view_of_scale_support(this))
}

unsafe extern "C" fn scale_set_factor(
    this: *mut IPlugViewContentScaleSupport,
    factor: f32,
) -> TResult {
    let state = diagnostics(view_of_scale_support(this));
    state.record(VIEW_SCALE_SET);
    state
        .scale_percent
        .store((factor * 100.0).round() as u32, Ordering::Relaxed);
    K_RESULT_OK
}

static SCALE_SUPPORT_VTBL: Shared<IPlugViewContentScaleSupportVtbl> =
    Shared(IPlugViewContentScaleSupportVtbl {
        query_interface: scale_query_interface,
        add_ref: scale_add_ref,
        release: scale_release,
        set_content_scale_factor: scale_set_factor,
    });

unsafe extern "C" fn controller_create_view(
    this: *mut IEditController,
    name: *const u8,
) -> *mut IPlugView {
    if name.is_null() || std::ffi::CStr::from_ptr(name.cast()).to_bytes() != b"editor" {
        return std::ptr::null_mut();
    }
    let plugin = from_controller(this);
    add_reference(plugin);
    Box::into_raw(Box::new(View {
        view: &VIEW_VTBL.0,
        scale_support: &SCALE_SUPPORT_VTBL.0,
        references: AtomicU32::new(1),
        plugin,
        frame: AtomicPtr::new(std::ptr::null_mut()),
    })) as *mut IPlugView
}

/// Raw function pointers in a `static` need a wrapper to be `Sync`.
struct Shared<T>(T);
// SAFETY: the tables are immutable and hold only function pointers.
unsafe impl<T> Sync for Shared<T> {}

static CONTROLLER_VTBL: Shared<IEditControllerVtbl> = Shared(IEditControllerVtbl {
    query_interface: controller_query,
    add_ref: controller_add_ref,
    release: controller_release,
    initialize: controller_initialize,
    terminate: controller_terminate,
    set_component_state: controller_set_component_state,
    set_state: controller_set_state,
    _pad7: controller_get_state as *const c_void,
    get_parameter_count: controller_parameter_count,
    get_parameter_info: controller_parameter_info,
    get_param_string_by_value: controller_string_by_value,
    _pad11: controller_value_by_string as *const c_void,
    normalized_param_to_plain: controller_normalised_to_plain,
    plain_param_to_normalized: controller_plain_to_normalised,
    get_param_normalized: controller_get_normalised,
    set_param_normalized: controller_set_normalised,
    set_component_handler: controller_set_handler,
    create_view: controller_create_view,
});

// ---- Factory ---------------------------------------------------------------

struct ClassSpec {
    id: [u8; 16],
    name: &'static str,
    sub_categories: &'static str,
    kind: Kind,
}

const CLASSES: [ClassSpec; 2] = [
    ClassSpec {
        id: EFFECT_CLASS_ID,
        name: "Echoes Test Gain",
        sub_categories: "Fx",
        kind: Kind::Effect,
    },
    ClassSpec {
        id: INSTRUMENT_CLASS_ID,
        name: "Echoes Test Synth",
        sub_categories: "Instrument|Synth",
        kind: Kind::Instrument,
    },
];

fn copy_text(destination: &mut [u8], text: &str) {
    destination.fill(0);
    let length = text.len().min(destination.len() - 1);
    destination[..length].copy_from_slice(&text.as_bytes()[..length]);
}

/// `PFactoryInfo` (`ipluginbase.h`).
#[repr(C)]
struct FactoryInfo {
    vendor: [u8; 64],
    url: [u8; 256],
    email: [u8; 128],
    flags: i32,
}

#[repr(C)]
struct Factory {
    vtbl: *const IPluginFactory2Vtbl,
}

unsafe extern "C" fn factory_query(
    this: *mut IPluginFactory,
    iid: *const Tuid,
    object: *mut *mut c_void,
) -> TResult {
    let iid = *iid;
    if iid == IID_FUNKNOWN || iid == IID_IPLUGIN_FACTORY || iid == IID_IPLUGIN_FACTORY2 {
        *object = this as *mut c_void;
        K_RESULT_OK
    } else {
        *object = std::ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "C" fn factory_add_ref(_this: *mut IPluginFactory) -> u32 {
    2
}

unsafe extern "C" fn factory_release(_this: *mut IPluginFactory) -> u32 {
    1
}

unsafe extern "C" fn factory_info(_this: *mut IPluginFactory, info: *mut c_void) -> TResult {
    let info = &mut *(info as *mut FactoryInfo);
    copy_text(&mut info.vendor, "Echoes Studio");
    copy_text(&mut info.url, "");
    copy_text(&mut info.email, "");
    info.flags = 1 << 4;
    K_RESULT_OK
}

unsafe extern "C" fn factory_class_count(_this: *mut IPluginFactory) -> i32 {
    CLASSES.len() as i32
}

unsafe extern "C" fn factory_class_info(
    _this: *mut IPluginFactory,
    index: i32,
    info: *mut PClassInfo,
) -> TResult {
    let Some(class) = CLASSES.get(index as usize) else {
        return K_INVALID_ARGUMENT;
    };
    let info = &mut *info;
    info.cid = class.id;
    info.cardinality = MANY_INSTANCES;
    copy_text(&mut info.category, "Audio Module Class");
    copy_text(&mut info.name, class.name);
    K_RESULT_OK
}

unsafe extern "C" fn factory_class_info2(
    _this: *mut IPluginFactory,
    index: i32,
    info: *mut PClassInfo2,
) -> TResult {
    let Some(class) = CLASSES.get(index as usize) else {
        return K_INVALID_ARGUMENT;
    };
    let info = &mut *info;
    info.cid = class.id;
    info.cardinality = MANY_INSTANCES;
    copy_text(&mut info.category, "Audio Module Class");
    copy_text(&mut info.name, class.name);
    info.class_flags = 0;
    copy_text(&mut info.sub_categories, class.sub_categories);
    copy_text(&mut info.vendor, "Echoes Studio");
    copy_text(&mut info.version, "1.0.0");
    copy_text(&mut info.sdk_version, "VST 3.8");
    K_RESULT_OK
}

unsafe extern "C" fn factory_create_instance(
    _this: *mut IPluginFactory,
    class_id: *const u8,
    iid: *const u8,
    object: *mut *mut c_void,
) -> TResult {
    let class_id = *(class_id as *const [u8; 16]);
    let Some(class) = CLASSES.iter().find(|class| class.id == class_id) else {
        return K_INVALID_ARGUMENT;
    };
    let plugin = Box::into_raw(Box::new(Plugin {
        component: &COMPONENT_VTBL,
        processor: &PROCESSOR_VTBL,
        controller: &CONTROLLER_VTBL.0,
        midi_mapping: &MIDI_MAPPING_VTBL,
        unit_info: &UNIT_INFO_VTBL,
        info_listener: &INFO_LISTENER_VTBL,
        references: AtomicU32::new(0),
        kind: class.kind,
        track_calls: AtomicU32::new(0),
        track_name_units: AtomicU32::new(0),
        track_color: AtomicU32::new(0),
        gain: AtomicU64::new(1.0_f64.to_bits()),
        program: AtomicU64::new(1.0_f64.to_bits()),
        held_velocity: AtomicU32::new(0),
        mod_wheel: AtomicU64::new(0.0_f64.to_bits()),
        bend: AtomicU64::new(0.5_f64.to_bits()),
        last_pitch: AtomicU32::new(0),
        hang: AtomicBool::new(false),
        crash: AtomicBool::new(false),
        init_apartment: AtomicI32::new(0),
        process_apartment: AtomicI32::new(0),
        process_calls: AtomicU32::new(0),
        handler: AtomicPtr::new(std::ptr::null_mut()),
        controller_self: AtomicU64::new(0.0_f64.to_bits()),
        processor_self: AtomicU64::new(0.0_f64.to_bits()),
        write_output: AtomicBool::new(false),
        hold_edit: AtomicBool::new(false),
        edit_open: AtomicBool::new(false),
        lookahead: AtomicBool::new(false),
        latency: AtomicU32::new(LATENCY),
        pending_latency: AtomicU32::new(LATENCY),
        renamed: AtomicBool::new(false),
        view: ViewDiagnostics::new(),
    }));
    let result = query(plugin, iid as *const Tuid, object);
    if result != K_RESULT_OK {
        drop(Box::from_raw(plugin));
    }
    result
}

static FACTORY_VTBL: IPluginFactory2Vtbl = IPluginFactory2Vtbl {
    query_interface: factory_query,
    add_ref: factory_add_ref,
    release: factory_release,
    get_factory_info: factory_info,
    count_classes: factory_class_count,
    get_class_info: factory_class_info,
    create_instance: factory_create_instance,
    get_class_info2: factory_class_info2,
};

static FACTORY: Shared<Factory> = Shared(Factory {
    vtbl: &FACTORY_VTBL,
});

// ---- Module entry points ---------------------------------------------------

#[no_mangle]
pub extern "C" fn GetPluginFactory() -> *mut IPluginFactory {
    &FACTORY.0 as *const Factory as *mut IPluginFactory
}

#[no_mangle]
#[allow(non_snake_case)]
pub extern "C" fn InitDll() -> bool {
    INIT_DLL_APARTMENT.store(apartment_code(), Ordering::Relaxed);
    INIT_CALLS.fetch_add(1, Ordering::Relaxed);
    true
}

#[no_mangle]
#[allow(non_snake_case)]
pub extern "C" fn ExitDll() -> bool {
    EXIT_CALLS.fetch_add(1, Ordering::Relaxed);
    true
}

#[no_mangle]
pub extern "C" fn test_vst3_init_calls() -> u32 {
    INIT_CALLS.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn test_vst3_exit_calls() -> u32 {
    EXIT_CALLS.load(Ordering::Relaxed)
}

/// Same code as the `Init apartment` parameter, for the thread that ran `InitDll`;
/// -1 before `InitDll` ran.
#[no_mangle]
pub extern "C" fn test_vst3_init_apartment() -> i32 {
    INIT_DLL_APARTMENT.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests;
