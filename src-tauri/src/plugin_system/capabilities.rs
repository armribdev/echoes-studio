//! Plugin capabilities — introspected device metadata.
//!
//! Phase 1 of the migration plan (`docs/active/migration/migration_plan.md`) introduces the
//! type vocabulary that later phases will use to carry data introspected from
//! VST3 (`IComponent::getBusInfo`, `IEditController::getParameterInfo`) and
//! CLAP (`clap.audio-ports`, `clap.note-ports`, `clap.params`) plugins.
//!
//! ## Scope of Phase 1
//!
//! Only the **types** land here.  No call site reads `DeviceCapabilities`
//! yet — the graph compiler still forces stereo (2/2) in `num_inputs_for_kind`
//! / `num_outputs_for_kind`.  Capability-driven port allocation is Phase 5
//! work and depends on `PortKind` validation (ADR-008, target BP-2).
//!
//! The default impl on `Device::capabilities()` derives a safe shape from the
//! existing `DeviceInfo` so every wrapper compiles unchanged (target C-2).

use crate::plugin_system::wrapper::DeviceInfo;

// ── Audio bus ─────────────────────────────────────────────────────────────────

/// Speaker layout reported by the plugin.
///
/// Kept intentionally small: covers what VST3 `SpeakerArrangement` and CLAP
/// `clap_audio_port_info::port_type` encode in the common case.  Anything more
/// exotic (Atmos, ambisonic) falls back to `Custom { channel_count }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerLayout {
    Mono,
    Stereo,
    Quad,
    Surround5_1,
    Surround7_1,
    /// Anything not in the standard set; `channel_count` is authoritative.
    Custom {
        channel_count: u8,
    },
}

impl SpeakerLayout {
    pub fn channel_count(&self) -> u8 {
        match self {
            SpeakerLayout::Mono => 1,
            SpeakerLayout::Stereo => 2,
            SpeakerLayout::Quad => 4,
            SpeakerLayout::Surround5_1 => 6,
            SpeakerLayout::Surround7_1 => 8,
            SpeakerLayout::Custom { channel_count } => *channel_count,
        }
    }
}

/// Description of one audio bus reported by the plugin.
///
/// A bus is one named group of audio channels (main I/O, sidechain, aux).
/// VST3: `IComponent::getBusInfo`.  CLAP: `clap_audio_port_info`.
#[derive(Debug, Clone)]
pub struct AudioBusInfo {
    pub name: String,
    pub channel_count: u8,
    /// `true` for the primary bus (track signal path).  At most one main bus
    /// per direction is expected.
    pub is_main: bool,
    /// `true` for sidechain / aux buses.  Mutually exclusive with `is_main`
    /// (a bus is either the main signal or an auxiliary).
    pub is_aux: bool,
    pub speaker_layout: SpeakerLayout,
}

// ── MIDI / note port ──────────────────────────────────────────────────────────

/// Description of one MIDI / note port reported by the plugin.
///
/// VST3: bus of kind `kEvent`.  CLAP: `clap_note_port_info`.
#[derive(Debug, Clone)]
pub struct MidiPortInfo {
    pub name: String,
    pub is_main: bool,
}

// ── Parameter ─────────────────────────────────────────────────────────────────

/// Parameter automation / display flags.
///
/// Sourced from VST3 `ParameterInfo::flags` and CLAP `clap_param_info::flags`.
/// Stored as a bitfield so future flags can be added without breaking ABI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParameterFlags(pub u32);

impl ParameterFlags {
    pub const NONE: ParameterFlags = ParameterFlags(0);
    pub const AUTOMATABLE: ParameterFlags = ParameterFlags(1 << 0);
    pub const HIDDEN: ParameterFlags = ParameterFlags(1 << 1);
    pub const IS_BYPASS: ParameterFlags = ParameterFlags(1 << 2);
    pub const READ_ONLY: ParameterFlags = ParameterFlags(1 << 3);
    pub const STEPPED: ParameterFlags = ParameterFlags(1 << 4);

    pub fn contains(self, other: ParameterFlags) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// Plugin-native parameter identifier.
///
/// VST3 uses a 32-bit `ParamID`; CLAP uses a 32-bit `clap_id`.  We carry both
/// here so the wrapper can translate `ParamKey` ↔ native ID without losing
/// the original value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NativeParamId(pub u32);

/// Description of one parameter introspected from the plugin.
///
/// Phase 1 only carries the type.  Phase 2 will populate it from VST3
/// `IEditController::getParameterInfo` and CLAP `clap.params.get_info`.
#[derive(Debug, Clone)]
pub struct ParameterInfo {
    pub native_id: NativeParamId,
    pub name: String,
    pub default_normalized: f32,
    /// Discrete step count (0 = continuous).  VST3 `stepCount`,
    /// CLAP `is_stepped` + `min/max`.
    pub step_count: u32,
    pub flags: ParameterFlags,
}

// ── DeviceCapabilities ────────────────────────────────────────────────────────

/// Aggregate of every static fact a plugin reports about itself.
///
/// Populated lazily by the wrapper at `prepare()` time (off-RT).  Once set,
/// every field is read-only for the lifetime of the wrapper.
///
/// Phase 1 leaves all collections empty by default; only `reported_latency_samples`
/// and `reported_tail_samples` are wired immediately (the rest fills in Phase 2+).
#[derive(Debug, Clone, Default)]
pub struct DeviceCapabilities {
    pub audio_input_buses: Vec<AudioBusInfo>,
    pub audio_output_buses: Vec<AudioBusInfo>,
    pub midi_input_ports: Vec<MidiPortInfo>,
    pub midi_output_ports: Vec<MidiPortInfo>,
    pub parameters: Vec<ParameterInfo>,

    pub supports_state: bool,
    pub supports_gui: bool,
    pub is_instrument: bool,

    /// Reported intrinsic processing latency, in samples.  Drives PDC.
    pub reported_latency_samples: u32,
    /// Reported reverb tail length, in samples.  Used for offline render
    /// flushing.  Phase 1 leaves this at 0.
    pub reported_tail_samples: u32,
}

impl DeviceCapabilities {
    /// Safe fallback derived from the wrapper's legacy `DeviceInfo` shape.
    ///
    /// Produces a single stereo (or mono / N-channel) main bus per direction,
    /// optional MIDI ports, no parameters.  Matches the pre-migration
    /// behaviour so any wrapper that has not yet been migrated to true
    /// introspection compiles unchanged.
    pub fn from_legacy_info(info: &DeviceInfo) -> Self {
        let layout_for = |n: usize| -> SpeakerLayout {
            match n {
                1 => SpeakerLayout::Mono,
                2 => SpeakerLayout::Stereo,
                4 => SpeakerLayout::Quad,
                6 => SpeakerLayout::Surround5_1,
                8 => SpeakerLayout::Surround7_1,
                _ => SpeakerLayout::Custom {
                    channel_count: n.min(u8::MAX as usize) as u8,
                },
            }
        };

        let mk_audio_bus = |n: usize, dir: &str| -> Vec<AudioBusInfo> {
            if n == 0 {
                Vec::new()
            } else {
                vec![AudioBusInfo {
                    name: format!("{dir} 1"),
                    channel_count: n.min(u8::MAX as usize) as u8,
                    is_main: true,
                    is_aux: false,
                    speaker_layout: layout_for(n),
                }]
            }
        };

        let mk_midi_port = |present: bool, dir: &str| -> Vec<MidiPortInfo> {
            if present {
                vec![MidiPortInfo {
                    name: format!("{dir} 1"),
                    is_main: true,
                }]
            } else {
                Vec::new()
            }
        };

        DeviceCapabilities {
            audio_input_buses: mk_audio_bus(info.num_inputs, "In"),
            audio_output_buses: mk_audio_bus(info.num_outputs, "Out"),
            midi_input_ports: mk_midi_port(info.has_midi_input, "MIDI In"),
            midi_output_ports: mk_midi_port(info.has_midi_output, "MIDI Out"),
            parameters: Vec::new(),
            supports_state: false,
            supports_gui: false,
            is_instrument: false,
            reported_latency_samples: 0,
            reported_tail_samples: 0,
        }
    }
}
