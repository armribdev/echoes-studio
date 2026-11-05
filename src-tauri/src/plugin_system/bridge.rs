//! Engine-side client of a sandboxed plugin's bridge process (ADR-015).
//!
//! [`BridgedPlugin`] is the native handle of a sandboxed `PluginInstance`: it
//! owns the `plugin_host_bridge` child and turns the instance's off-RT calls
//! (activation, state, parameters, editor) into control requests.  Audio goes
//! through [`BridgedPlugin::audio`] from the graph node.
//!
//! Every request runs on the command or UI thread and is bounded by a timeout,
//! so a hung plugin cannot freeze the app; it then fails like a refused call.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use plugin_bridge::control::Op;
use plugin_bridge::session::{AudioPort, BridgeLayout, BridgeProcess};

use plugin_bridge::records::{
    Handshake, NoteNameRecord, ParamRecord, ParamUnit, PortDirection, PresetRecord, PresetSource,
    PresetStateRecord, ProgramListRecord, TrackRecord, MAX_PORTS, PARAM_AUTOMATABLE, PARAM_HIDDEN,
    PARAM_MODULATABLE, PORT_CHANNELS,
};

use crate::plugin_formats::clap::ffi::{
    mark_plugin_restart_requested, HostTrackInfo, NoteName, PresetRequest,
    CLAP_PRESET_LOCATION_FILE,
};
use crate::plugin_formats::vst3::preset_file::PresetChunks;
use crate::plugin_formats::vst3::units::ProgramList;
use crate::plugin_system::parameter::PluginParamInfo;
use crate::plugin_system::wrapper::DeviceFormat;

/// Largest block a sandboxed plugin processes (the engine's `MAX_BLOCK_FRAMES`).
pub const MAX_FRAMES: u32 = 4096;
/// Room for a block's sections in each direction: the transport, 256 notes and
/// 1024 parameter events fit with room to spare.
pub const EVENT_CAPACITY: u32 = 64 * 1024;

/// Loading a large plugin (wavetables, samples) can take seconds.
const START_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// A condemned bridge may be gone; do not keep the caller waiting.
const DEAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

/// `plugin_host_bridge` next to the running engine binary.
pub fn locate_bridge_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.with_file_name(format!(
        "plugin_host_bridge{}",
        std::env::consts::EXE_SUFFIX
    ));
    exe.exists().then_some(exe)
}

/// `(sample_rate bits, max_frames, latency, auxiliary outputs wanted)` of the
/// current activation.
type Activation = (u32, u32, u32, u32);

/// What the engine uses the hosted plugin for (`--plugin-role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginRole {
    Effect,
    Instrument,
}

pub struct BridgedPlugin {
    process: BridgeProcess,
    activation: Mutex<Option<Activation>>,
    /// Set from the audio thread when the plugin requests a restart.
    restart_requested: AtomicBool,
    /// The track the bridge was last given, to send only changes.
    track_info: Mutex<Option<HostTrackInfo>>,
    /// Channels of the input and of the output samples of a block.
    input_channels: usize,
    output_channels: usize,
}

impl BridgedPlugin {
    /// Spawn a bridge hosting the plugin `native_id` of `dll` (a CLAP id, or the
    /// hexadecimal VST3 class id).  Returns once the plugin is loaded and
    /// initialised in the child, which already knows `track_info`.
    pub fn spawn(
        bridge_exe: &Path,
        dll: &Path,
        format: DeviceFormat,
        native_id: &str,
        role: PluginRole,
        track_info: Option<&HostTrackInfo>,
    ) -> io::Result<Self> {
        let (format_name, unit) = match format {
            DeviceFormat::Clap => ("clap", ParamUnit::Plain),
            DeviceFormat::Vst3 => ("vst3", ParamUnit::Normalised),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("{format:?} plugins cannot run in the sandbox"),
                ))
            }
        };
        let mut args: Vec<OsString> = vec![
            "--plugin-dll".into(),
            dll.into(),
            "--plugin-format".into(),
            format_name.into(),
            "--plugin-id".into(),
            native_id.into(),
        ];
        if role == PluginRole::Instrument {
            args.extend(["--plugin-role".into(), "instrument".into()]);
        }
        if let Some(info) = track_info {
            args.extend(["--track-info".into(), info.to_hex().into()]);
        }
        let region_channels = match format {
            DeviceFormat::Vst3 => PORT_CHANNELS * MAX_PORTS,
            _ => PORT_CHANNELS,
        };
        let layout = BridgeLayout::new(MAX_FRAMES, region_channels as u32, EVENT_CAPACITY);
        let process = BridgeProcess::spawn(bridge_exe, layout, &args, START_TIMEOUT)?;
        let handshake = read_handshake(
            &process.request(Op::Handshake, &[], START_TIMEOUT)?,
            unit,
            region_channels,
        )?;
        // A CLAP bridge always presents one stereo pair each way; a VST3 one the
        // active ports of its table.
        let (input_channels, output_channels) = match format {
            DeviceFormat::Vst3 => (
                handshake.block_channels(PortDirection::Input),
                handshake.block_channels(PortDirection::Output),
            ),
            _ => (PORT_CHANNELS, PORT_CHANNELS),
        };
        Ok(BridgedPlugin {
            process,
            activation: Mutex::new(None),
            restart_requested: AtomicBool::new(false),
            track_info: Mutex::new(track_info.cloned()),
            input_channels,
            output_channels,
        })
    }

    /// Channels of the input samples of a block: every active input port.
    pub fn input_channels(&self) -> usize {
        self.input_channels
    }

    /// Output ports a block carries, the main one first.
    pub fn output_ports(&self) -> usize {
        self.output_channels / PORT_CHANNELS
    }

    /// Lock-free: called from the audio thread.  The next [`Self::activate`]
    /// asks the bridge to reactivate the plugin.
    pub fn request_restart(&self) {
        self.restart_requested.store(true, Ordering::Release);
        mark_plugin_restart_requested();
    }

    #[cfg(test)]
    pub fn process_id(&self) -> u32 {
        self.process.process_id()
    }

    pub fn audio(&self) -> &AudioPort {
        self.process.audio()
    }

    /// The watchdog condemned the bridge: its plugin crashed or hung.
    pub fn is_dead(&self) -> bool {
        self.audio().is_dead()
    }

    fn request(&self, op: Op, payload: &[u8]) -> io::Result<Vec<u8>> {
        let timeout = if self.is_dead() {
            DEAD_REQUEST_TIMEOUT
        } else {
            REQUEST_TIMEOUT
        };
        let reply = self.process.request(op, payload, timeout);
        if let Err(error) = &reply {
            eprintln!("[bridge] {op:?} failed: {error}");
        }
        reply
    }

    /// Activate at `(sample_rate, max_frames)` with the auxiliary output buses
    /// `wanted_outputs` names (bit `n` = output port `n`), unless already active so
    /// and no restart is pending.  Returns the plugin's latency, or `None` when
    /// activation failed.
    pub fn activate(&self, sample_rate: f32, max_frames: u32, wanted_outputs: u32) -> Option<u32> {
        let mut activation = self.activation.lock().unwrap_or_else(|e| e.into_inner());
        let restart = self.restart_requested.swap(false, Ordering::AcqRel);
        if let Some((rate, frames, latency, outputs)) = *activation {
            if !restart
                && rate == sample_rate.to_bits()
                && frames == max_frames
                && outputs == wanted_outputs
            {
                return Some(latency);
            }
        }
        *activation = None;
        let mut payload = (sample_rate as f64).to_le_bytes().to_vec();
        payload.extend_from_slice(&max_frames.to_le_bytes());
        payload.extend_from_slice(&wanted_outputs.to_le_bytes());
        let reply = self.request(Op::Activate, &payload).ok()?;
        let latency = u32::from_le_bytes(reply.get(..4)?.try_into().ok()?);
        *activation = Some((sample_rate.to_bits(), max_frames, latency, wanted_outputs));
        Some(latency)
    }

    pub fn save_state(&self) -> Option<Vec<u8>> {
        self.request(Op::SaveState, &[]).ok()
    }

    pub fn load_state(&self, bytes: &[u8]) -> bool {
        self.request(Op::LoadState, bytes).is_ok()
    }

    /// Asks the plugin to load a preset; the error is the plugin's own message.
    pub fn load_preset(&self, request: &PresetRequest) -> Result<(), String> {
        let source = if request.location_kind == CLAP_PRESET_LOCATION_FILE {
            PresetSource::File
        } else {
            PresetSource::Plugin
        };
        let record = PresetRecord {
            source,
            location: request.location.clone(),
            load_key: request.load_key.clone(),
        };
        let payload = record.encode().ok_or("the preset location is too long")?;
        self.request(Op::PresetLoad, &payload)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// The program lists a VST3 plugin offers; empty when the bridge cannot answer.
    pub fn program_lists(&self) -> Vec<ProgramList> {
        let Ok(reply) = self.request(Op::PresetList, &[]) else {
            return Vec::new();
        };
        ProgramListRecord::decode_list(&reply)
            .unwrap_or_default()
            .into_iter()
            .map(|record| ProgramList {
                id: record.id,
                name: record.name,
                program_change_param: record.selector,
                programs: record.programs,
                current_program: record.current.map(|index| index as usize),
            })
            .collect()
    }

    /// Gives a VST3 plugin the states of a preset file the engine has read.
    pub fn load_preset_state(&self, chunks: PresetChunks) -> Result<(), String> {
        let record = PresetStateRecord {
            component: chunks.component,
            controller: chunks.controller,
        };
        let payload = record.encode().ok_or("the preset is too large")?;
        self.request(Op::PresetLoadState, &payload)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn params(&self) -> Vec<PluginParamInfo> {
        let Ok(reply) = self.request(Op::Params, &[]) else {
            return Vec::new();
        };
        ParamRecord::decode_list(&reply)
            .unwrap_or_default()
            .into_iter()
            .map(|record| PluginParamInfo {
                native_id: record.native_id,
                name: record.name,
                module: record.module,
                min: record.min,
                max: record.max,
                default: record.default,
                automatable: record.flags & PARAM_AUTOMATABLE != 0,
                modulatable: record.flags & PARAM_MODULATABLE != 0,
                hidden: record.flags & PARAM_HIDDEN != 0,
            })
            .collect()
    }

    pub fn param_value(&self, native_id: u32) -> Option<f64> {
        let reply = self
            .request(Op::ParamValue, &native_id.to_le_bytes())
            .ok()?;
        Some(f64::from_le_bytes(reply.get(..8)?.try_into().ok()?))
    }

    pub fn param_text(&self, native_id: u32, value: f64) -> Option<String> {
        let mut payload = native_id.to_le_bytes().to_vec();
        payload.extend_from_slice(&value.to_le_bytes());
        let reply = self.request(Op::ParamText, &payload).ok()?;
        String::from_utf8(reply)
            .ok()
            .filter(|text| !text.is_empty())
    }

    /// The plugin's note names (`clap.note-name`); `None` when the bridge cannot answer.
    pub fn note_names(&self) -> Option<Vec<NoteName>> {
        let records = NoteNameRecord::decode_list(&self.request(Op::NoteNames, &[]).ok()?).ok()?;
        Some(
            records
                .into_iter()
                .map(|record| NoteName {
                    name: record.name,
                    port: record.port,
                    key: record.key,
                    channel: record.channel,
                })
                .collect(),
        )
    }

    /// Gives the plugin its track when it differs from the last one sent; the
    /// bridge then calls the plugin's `changed` on its main thread.  A dead
    /// bridge is skipped, and a failed request is retried by the next call.
    pub fn set_track_info(&self, info: &HostTrackInfo) {
        let mut sent = self.track_info.lock().unwrap_or_else(|e| e.into_inner());
        if sent.as_ref() == Some(info) || self.is_dead() {
            return;
        }
        let record = TrackRecord {
            name: info.name.clone(),
            color: info.color,
        };
        if self.request(Op::TrackInfo, &record.encode()).is_ok() {
            *sent = Some(info.clone());
        }
    }

    /// Set plain parameter values outside `process`.
    pub fn flush_values(&self, values: &[(u32, f64)]) {
        let payload: Vec<u8> = values
            .iter()
            .flat_map(|(native_id, value)| {
                native_id
                    .to_le_bytes()
                    .into_iter()
                    .chain(value.to_le_bytes())
            })
            .collect();
        let _ = self.request(Op::FlushValues, &payload);
    }

    /// Open the plugin's editor in the bridge process, or bring it to the front, and
    /// return the handle of its window.
    pub fn open_editor(&self, title: &str) -> Result<u64, String> {
        allow_foreground_window(self.process.process_id());
        let reply = self
            .request(Op::OpenEditor, title.as_bytes())
            .map_err(|e| e.to_string())?;
        let handle: [u8; 8] = reply
            .try_into()
            .map_err(|_| "the bridge answered with an invalid window handle".to_string())?;
        Ok(u64::from_le_bytes(handle))
    }
}

/// Reads the bridge's handshake and refuses a bridge whose parameter values are not
/// in the unit the engine sends for the plugin's format (plain for CLAP, normalised
/// for VST3, ADR-021) or whose active ports carry more channels than the shared
/// region holds.  A refused plugin is reported unavailable, never loaded in-process
/// instead.
fn read_handshake(
    reply: &[u8],
    expected_unit: ParamUnit,
    region_channels: usize,
) -> io::Result<Handshake> {
    let handshake = Handshake::decode(reply)?;
    if handshake.param_unit != expected_unit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the bridge reports {:?} parameter values, the engine sends {expected_unit:?}",
                handshake.param_unit
            ),
        ));
    }
    for direction in [PortDirection::Input, PortDirection::Output] {
        if handshake.block_channels(direction) > region_channels {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("more {direction:?} audio channels than the shared region holds"),
            ));
        }
    }
    Ok(handshake)
}

/// Lets the bridge bring its editor window to the front: Windows only grants
/// that to a process the foreground process allows.
#[cfg(windows)]
fn allow_foreground_window(process_id: u32) {
    #[link(name = "user32")]
    extern "system" {
        fn AllowSetForegroundWindow(process_id: u32) -> i32;
    }
    unsafe { AllowSetForegroundWindow(process_id) };
}

#[cfg(not(windows))]
fn allow_foreground_window(_: u32) {}
