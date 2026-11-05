//! Format-neutral records of the control channel (ADR-033 step 4).
//!
//! The control operations carry these instead of the CLAP wire forms they used in
//! protocol v2.  Each record has an `encode` / `decode` pair over
//! [`PayloadWriter`] / [`PayloadReader`]; a decode never trusts a count from the
//! other side to size an allocation.

use std::io;

use crate::control::{PayloadReader, PayloadWriter};

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Capacity reserved for a list read from the wire: never more than the bytes left
/// could hold.
fn bounded_capacity(count: u32, reader: &PayloadReader, minimum_record: usize) -> usize {
    (count as usize).min(reader.remaining() / minimum_record)
}

// ── Handshake ───────────────────────────────────────────────────────────────

/// The unit of a parameter value on the wire.  The bridge never converts a value:
/// the engine sends the unit the format uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamUnit {
    /// The plugin's own range (CLAP).
    Plain = 0,
    /// 0..1 over the plugin's range (VST3).
    Normalised = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDirection {
    Input = 0,
    Output = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortRole {
    Main = 0,
    Auxiliary = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerLayout {
    Mono = 0,
    Stereo = 1,
    /// Any other arrangement; `channels` says how many.
    Other = 2,
}

/// Channels each active port takes in a block, whatever its own arrangement: the
/// engine's buffers are stereo, a mono bus is spread over both and a wider bus is cut
/// to its first two.
pub const PORT_CHANNELS: usize = 2;

/// Audio ports a sandboxed plugin exposes in a direction: its main one and eight
/// auxiliary ones.  The shared region is sized for them; a plugin with more buses
/// keeps the extra ones closed.
pub const MAX_PORTS: usize = 9;

/// One audio port of the plugin.  Only the `active` ports carry samples in a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPortRecord {
    pub direction: PortDirection,
    pub role: PortRole,
    pub channels: u32,
    pub layout: SpeakerLayout,
    pub active: bool,
}

impl AudioPortRecord {
    const BYTES: usize = 8;
}

/// What the bridge tells the engine once, after its plugin is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub param_unit: ParamUnit,
    pub ports: Vec<AudioPortRecord>,
}

impl Handshake {
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PayloadWriter::new();
        writer
            .u8(self.param_unit as u8)
            .u32(self.ports.len() as u32);
        for port in &self.ports {
            writer
                .u8(port.direction as u8)
                .u8(port.role as u8)
                .u32(port.channels)
                .u8(port.layout as u8)
                .u8(u8::from(port.active));
        }
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let mut reader = PayloadReader::new(bytes);
        let param_unit = match reader.u8()? {
            0 => ParamUnit::Plain,
            1 => ParamUnit::Normalised,
            _ => return Err(invalid("unknown parameter unit")),
        };
        let count = reader.u32()?;
        let mut ports =
            Vec::with_capacity(bounded_capacity(count, &reader, AudioPortRecord::BYTES));
        for _ in 0..count {
            ports.push(AudioPortRecord {
                direction: match reader.u8()? {
                    0 => PortDirection::Input,
                    1 => PortDirection::Output,
                    _ => return Err(invalid("unknown port direction")),
                },
                role: match reader.u8()? {
                    0 => PortRole::Main,
                    1 => PortRole::Auxiliary,
                    _ => return Err(invalid("unknown port role")),
                },
                channels: reader.u32()?,
                layout: match reader.u8()? {
                    0 => SpeakerLayout::Mono,
                    1 => SpeakerLayout::Stereo,
                    2 => SpeakerLayout::Other,
                    _ => return Err(invalid("unknown speaker layout")),
                },
                active: reader.u8()? != 0,
            });
        }
        Ok(Handshake { param_unit, ports })
    }

    /// Ports of `direction` that carry samples in a block.
    pub fn active_port_count(&self, direction: PortDirection) -> usize {
        self.ports
            .iter()
            .filter(|port| port.direction == direction && port.active)
            .count()
    }

    /// Channels a block carries in `direction`: every active port takes
    /// [`PORT_CHANNELS`], whatever its own arrangement.
    pub fn block_channels(&self, direction: PortDirection) -> usize {
        self.active_port_count(direction) * PORT_CHANNELS
    }
}

// ── Parameters ──────────────────────────────────────────────────────────────

pub const PARAM_AUTOMATABLE: u8 = 1 << 0;
pub const PARAM_MODULATABLE: u8 = 1 << 1;
pub const PARAM_HIDDEN: u8 = 1 << 2;

/// One parameter of the plugin, in the unit the handshake declared.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamRecord {
    pub native_id: u32,
    pub name: String,
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// `PARAM_*`.
    pub flags: u8,
}

impl ParamRecord {
    /// `native_id`, three `f64`, the flags and two empty texts.
    const MINIMUM_BYTES: usize = 4 + 24 + 1 + 4;

    pub fn encode_list(records: &[ParamRecord]) -> Vec<u8> {
        let mut writer = PayloadWriter::new();
        writer.u32(records.len() as u32);
        for record in records {
            writer
                .u32(record.native_id)
                .f64(record.min)
                .f64(record.max)
                .f64(record.default)
                .u8(record.flags)
                .text(&record.name)
                .text(&record.module);
        }
        writer.finish()
    }

    pub fn decode_list(bytes: &[u8]) -> io::Result<Vec<ParamRecord>> {
        let mut reader = PayloadReader::new(bytes);
        let count = reader.u32()?;
        let mut records = Vec::with_capacity(bounded_capacity(count, &reader, Self::MINIMUM_BYTES));
        for _ in 0..count {
            records.push(ParamRecord {
                native_id: reader.u32()?,
                min: reader.f64()?,
                max: reader.f64()?,
                default: reader.f64()?,
                flags: reader.u8()?,
                name: reader.text()?,
                module: reader.text()?,
            });
        }
        Ok(records)
    }
}

// ── Track ───────────────────────────────────────────────────────────────────

/// What the host tells a plugin about its track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackRecord {
    pub name: Option<String>,
    pub color: Option<[u8; 3]>,
}

impl TrackRecord {
    const HAS_NAME: u8 = 1 << 0;
    const HAS_COLOR: u8 = 1 << 1;

    pub fn encode(&self) -> Vec<u8> {
        let mut present = 0;
        if self.name.is_some() {
            present |= Self::HAS_NAME;
        }
        if self.color.is_some() {
            present |= Self::HAS_COLOR;
        }
        let [red, green, blue] = self.color.unwrap_or_default();
        let mut writer = PayloadWriter::new();
        writer.u8(present).u8(red).u8(green).u8(blue);
        writer.text(self.name.as_deref().unwrap_or_default());
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let mut reader = PayloadReader::new(bytes);
        let present = reader.u8()?;
        let color = [reader.u8()?, reader.u8()?, reader.u8()?];
        let name = reader.text()?;
        Ok(TrackRecord {
            name: (present & Self::HAS_NAME != 0).then_some(name),
            color: (present & Self::HAS_COLOR != 0).then_some(color),
        })
    }
}

// ── Note names ──────────────────────────────────────────────────────────────

/// A name the plugin gives to notes: `-1` in `port`, `key` or `channel` means every one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteNameRecord {
    pub name: String,
    pub port: i16,
    pub key: i16,
    pub channel: i16,
}

impl NoteNameRecord {
    const MINIMUM_BYTES: usize = 8;

    pub fn encode_list(records: &[NoteNameRecord]) -> Vec<u8> {
        let mut writer = PayloadWriter::new();
        writer.u32(records.len() as u32);
        for record in records {
            writer
                .i16(record.port)
                .i16(record.key)
                .i16(record.channel)
                .text(&record.name);
        }
        writer.finish()
    }

    pub fn decode_list(bytes: &[u8]) -> io::Result<Vec<NoteNameRecord>> {
        let mut reader = PayloadReader::new(bytes);
        let count = reader.u32()?;
        let mut records = Vec::with_capacity(bounded_capacity(count, &reader, Self::MINIMUM_BYTES));
        for _ in 0..count {
            records.push(NoteNameRecord {
                port: reader.i16()?,
                key: reader.i16()?,
                channel: reader.i16()?,
                name: reader.text()?,
            });
        }
        Ok(records)
    }
}

// ── Presets ─────────────────────────────────────────────────────────────────

/// Where a preset lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetSource {
    /// A file on disk (`location` is its path).
    File = 0,
    /// Inside the plugin (`location` is empty; `load_key` names the preset).
    Plugin = 1,
}

/// One preset to load: where it lives and the plugin's own key inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetRecord {
    pub source: PresetSource,
    pub location: Option<String>,
    pub load_key: Option<String>,
}

impl PresetRecord {
    const HAS_LOCATION: u8 = 1 << 0;
    const HAS_LOAD_KEY: u8 = 1 << 1;

    /// `None` when a text is too long for the wire.
    pub fn encode(&self) -> Option<Vec<u8>> {
        let mut present = 0;
        if self.location.is_some() {
            present |= Self::HAS_LOCATION;
        }
        if self.load_key.is_some() {
            present |= Self::HAS_LOAD_KEY;
        }
        let mut writer = PayloadWriter::new();
        writer.u8(self.source as u8).u8(present);
        for text in [&self.location, &self.load_key].into_iter().flatten() {
            if !writer.text_exact(text) {
                return None;
            }
        }
        Some(writer.finish())
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let mut reader = PayloadReader::new(bytes);
        let source = match reader.u8()? {
            0 => PresetSource::File,
            1 => PresetSource::Plugin,
            _ => return Err(invalid("unknown preset source")),
        };
        let present = reader.u8()?;
        let location = (present & Self::HAS_LOCATION != 0)
            .then(|| reader.text())
            .transpose()?;
        let load_key = (present & Self::HAS_LOAD_KEY != 0)
            .then(|| reader.text())
            .transpose()?;
        if !reader.is_empty() {
            return Err(invalid("trailing bytes after a preset"));
        }
        Ok(PresetRecord {
            source,
            location,
            load_key,
        })
    }
}

/// A list of programs the plugin offers, chosen by one of its parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramListRecord {
    pub id: i32,
    pub name: String,
    /// The parameter that selects a program of this list; without one the host
    /// cannot choose.
    pub selector: Option<u32>,
    /// The program the selector says is in use.
    pub current: Option<u32>,
    pub programs: Vec<String>,
}

impl ProgramListRecord {
    const HAS_SELECTOR: u8 = 1 << 0;
    const HAS_CURRENT: u8 = 1 << 1;
    const MINIMUM_BYTES: usize = 17;

    pub fn encode_list(records: &[ProgramListRecord]) -> Vec<u8> {
        let mut writer = PayloadWriter::new();
        writer.u32(records.len() as u32);
        for record in records {
            let mut present = 0;
            if record.selector.is_some() {
                present |= Self::HAS_SELECTOR;
            }
            if record.current.is_some() {
                present |= Self::HAS_CURRENT;
            }
            writer
                .u8(present)
                .u32(record.id as u32)
                .u32(record.selector.unwrap_or_default())
                .u32(record.current.unwrap_or_default())
                .text(&record.name)
                .u32(record.programs.len() as u32);
            for program in &record.programs {
                writer.text(program);
            }
        }
        writer.finish()
    }

    pub fn decode_list(bytes: &[u8]) -> io::Result<Vec<ProgramListRecord>> {
        let mut reader = PayloadReader::new(bytes);
        let count = reader.u32()?;
        let mut records = Vec::with_capacity(bounded_capacity(count, &reader, Self::MINIMUM_BYTES));
        for _ in 0..count {
            let present = reader.u8()?;
            let id = reader.u32()? as i32;
            let selector = reader.u32()?;
            let current = reader.u32()?;
            let name = reader.text()?;
            let program_count = reader.u32()?;
            let mut programs = Vec::with_capacity(bounded_capacity(program_count, &reader, 2));
            for _ in 0..program_count {
                programs.push(reader.text()?);
            }
            records.push(ProgramListRecord {
                id,
                name,
                selector: (present & Self::HAS_SELECTOR != 0).then_some(selector),
                current: (present & Self::HAS_CURRENT != 0).then_some(current),
                programs,
            });
        }
        if !reader.is_empty() {
            return Err(invalid("trailing bytes after the program lists"));
        }
        Ok(records)
    }
}

/// The state a preset file holds: the processor's, and the controller's own if the
/// file has one.  The engine has read and checked the file; the bridge applies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetStateRecord {
    pub component: Vec<u8>,
    pub controller: Option<Vec<u8>>,
}

impl PresetStateRecord {
    /// `None` when a chunk does not fit a control frame.
    pub fn encode(&self) -> Option<Vec<u8>> {
        let mut writer = PayloadWriter::new();
        writer.u8(u8::from(self.controller.is_some()));
        let fits = writer.bytes(&self.component)
            && self
                .controller
                .as_deref()
                .map_or(true, |controller| writer.bytes(controller));
        fits.then(|| writer.finish())
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let mut reader = PayloadReader::new(bytes);
        let has_controller = match reader.u8()? {
            0 => false,
            1 => true,
            _ => return Err(invalid("unknown preset state flag")),
        };
        let component = reader.bytes()?;
        let controller = has_controller.then(|| reader.bytes()).transpose()?;
        if !reader.is_empty() {
            return Err(invalid("trailing bytes after a preset state"));
        }
        Ok(PresetStateRecord {
            component,
            controller,
        })
    }
}

// ── Scan ────────────────────────────────────────────────────────────────────

/// Exit code of a scan process that could not load the file or does not know its format
/// (not a plugin of this machine's architecture, a missing dependency).  Any other
/// failure — a crash, a hang — is the file's fault and blocks it.
pub const SCAN_EXIT_REFUSED: u8 = 5;

/// One plugin found in a file by `plugin_host_bridge --scan`.  The scan process prints
/// them on one line of its standard output; the engine builds its manifests from them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedPluginRecord {
    /// The CLAP id, or the hexadecimal VST3 class id.
    pub native_id: String,
    pub name: String,
    pub vendor: String,
    pub is_instrument: bool,
    /// Auxiliary (sidechain) input buses of an effect; instruments are not probed.
    pub sidechain_inputs: u32,
}

impl ScannedPluginRecord {
    const MINIMUM_BYTES: usize = 11;
    /// Starts the result line, so whatever a plugin prints to the same stream is ignored.
    const RESULT_PREFIX: &'static str = "ECHOES-SCAN-RESULT ";

    pub fn encode_list(records: &[ScannedPluginRecord]) -> Vec<u8> {
        let mut writer = PayloadWriter::new();
        writer.u32(records.len() as u32);
        for record in records {
            writer
                .text(&record.native_id)
                .text(&record.name)
                .text(&record.vendor)
                .u8(u8::from(record.is_instrument))
                .u32(record.sidechain_inputs);
        }
        writer.finish()
    }

    pub fn decode_list(bytes: &[u8]) -> io::Result<Vec<ScannedPluginRecord>> {
        let mut reader = PayloadReader::new(bytes);
        let count = reader.u32()?;
        let mut records = Vec::with_capacity(bounded_capacity(count, &reader, Self::MINIMUM_BYTES));
        for _ in 0..count {
            let native_id = reader.text()?;
            let name = reader.text()?;
            let vendor = reader.text()?;
            let is_instrument = match reader.u8()? {
                0 => false,
                1 => true,
                _ => return Err(invalid("unknown instrument flag")),
            };
            records.push(ScannedPluginRecord {
                native_id,
                name,
                vendor,
                is_instrument,
                sidechain_inputs: reader.u32()?,
            });
        }
        if !reader.is_empty() {
            return Err(invalid("trailing bytes after the scanned plugins"));
        }
        Ok(records)
    }

    /// The line the scan process prints when it has finished.
    pub fn result_line(records: &[ScannedPluginRecord]) -> String {
        let mut line = String::from(Self::RESULT_PREFIX);
        for byte in Self::encode_list(records) {
            line.push_str(&format!("{byte:02x}"));
        }
        line
    }

    /// The plugins in the output of a scan process; an error when it printed no result line.
    pub fn from_scan_output(output: &str) -> io::Result<Vec<ScannedPluginRecord>> {
        let hex = output
            .lines()
            .rev()
            .find_map(|line| line.trim_end().strip_prefix(Self::RESULT_PREFIX))
            .ok_or_else(|| invalid("the scan printed no result"))?;
        if hex.len() % 2 != 0 || !hex.is_ascii() {
            return Err(invalid("the scan result is not hexadecimal"));
        }
        let bytes = (0..hex.len() / 2)
            .map(|index| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(|_| invalid("the scan result is not hexadecimal"))?;
        Self::decode_list(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(direction: PortDirection, role: PortRole, active: bool) -> AudioPortRecord {
        AudioPortRecord {
            direction,
            role,
            channels: 2,
            layout: SpeakerLayout::Stereo,
            active,
        }
    }

    #[test]
    fn a_handshake_roundtrips() {
        let handshake = Handshake {
            param_unit: ParamUnit::Normalised,
            ports: vec![
                port(PortDirection::Input, PortRole::Main, true),
                port(PortDirection::Input, PortRole::Auxiliary, false),
                port(PortDirection::Output, PortRole::Main, true),
            ],
        };
        assert_eq!(Handshake::decode(&handshake.encode()).unwrap(), handshake);
    }

    #[test]
    fn a_block_carries_two_channels_for_each_active_port() {
        let handshake = Handshake {
            param_unit: ParamUnit::Plain,
            ports: vec![
                port(PortDirection::Output, PortRole::Main, true),
                port(PortDirection::Output, PortRole::Auxiliary, true),
                port(PortDirection::Output, PortRole::Auxiliary, false),
                port(PortDirection::Input, PortRole::Main, true),
            ],
        };
        assert_eq!(handshake.active_port_count(PortDirection::Output), 2);
        assert_eq!(handshake.block_channels(PortDirection::Output), 4);
        assert_eq!(handshake.block_channels(PortDirection::Input), 2);
    }

    #[test]
    fn a_garbled_handshake_is_refused() {
        assert!(Handshake::decode(&[]).is_err());
        assert!(Handshake::decode(&[9, 0, 0, 0, 0]).is_err());
        // Declares a million ports, carries none: refused without a big allocation.
        let mut hostile = vec![0u8];
        hostile.extend_from_slice(&1_000_000u32.to_le_bytes());
        assert!(Handshake::decode(&hostile).is_err());
    }

    #[test]
    fn parameters_roundtrip() {
        let records = vec![
            ParamRecord {
                native_id: 4,
                name: "Gain".into(),
                module: "Main/Amp".into(),
                min: 0.0,
                max: 2.0,
                default: 1.0,
                flags: PARAM_AUTOMATABLE | PARAM_MODULATABLE,
            },
            ParamRecord {
                native_id: 9,
                name: String::new(),
                module: String::new(),
                min: -1.5,
                max: 1.5,
                default: 0.0,
                flags: PARAM_HIDDEN,
            },
        ];
        assert_eq!(
            ParamRecord::decode_list(&ParamRecord::encode_list(&records)).unwrap(),
            records
        );
        assert!(ParamRecord::decode_list(&[3, 0, 0, 0]).is_err());
    }

    #[test]
    fn a_track_roundtrips_with_or_without_name_and_color() {
        for track in [
            TrackRecord {
                name: Some("Lead".into()),
                color: Some([1, 2, 3]),
            },
            TrackRecord {
                name: None,
                color: Some([9, 8, 7]),
            },
            TrackRecord {
                name: Some("Bass".into()),
                color: None,
            },
            TrackRecord {
                name: None,
                color: None,
            },
        ] {
            assert_eq!(TrackRecord::decode(&track.encode()).unwrap(), track);
        }
        assert!(TrackRecord::decode(&[1, 2]).is_err());
    }

    #[test]
    fn note_names_roundtrip() {
        let names = vec![
            NoteNameRecord {
                name: "Kick".into(),
                port: -1,
                key: 36,
                channel: -1,
            },
            NoteNameRecord {
                name: "Snare".into(),
                port: 0,
                key: 38,
                channel: 9,
            },
        ];
        assert_eq!(
            NoteNameRecord::decode_list(&NoteNameRecord::encode_list(&names)).unwrap(),
            names
        );
        assert_eq!(
            NoteNameRecord::decode_list(&NoteNameRecord::encode_list(&[])).unwrap(),
            vec![]
        );
    }

    #[test]
    fn program_lists_roundtrip_and_garbage_is_refused() {
        let lists = vec![
            ProgramListRecord {
                id: 4,
                name: "Factory".into(),
                selector: Some(40),
                current: Some(1),
                programs: vec!["Default".into(), String::new(), "Wide".into()],
            },
            ProgramListRecord {
                id: -1,
                name: String::new(),
                selector: None,
                current: None,
                programs: vec![],
            },
        ];
        let bytes = ProgramListRecord::encode_list(&lists);
        assert_eq!(ProgramListRecord::decode_list(&bytes).unwrap(), lists);
        assert!(ProgramListRecord::decode_list(&bytes[..bytes.len() - 1]).is_err());
        let mut trailing = bytes;
        trailing.push(0);
        assert!(ProgramListRecord::decode_list(&trailing).is_err());
        let huge_count = u32::MAX.to_le_bytes();
        assert!(ProgramListRecord::decode_list(&huge_count).is_err());
    }

    fn scanned(native_id: &str, is_instrument: bool, sidechain_inputs: u32) -> ScannedPluginRecord {
        ScannedPluginRecord {
            native_id: native_id.into(),
            name: format!("Plugin {native_id}"),
            vendor: String::new(),
            is_instrument,
            sidechain_inputs,
        }
    }

    #[test]
    fn scanned_plugins_roundtrip_through_the_result_line_and_garbage_is_refused() {
        let plugins = vec![scanned("a.b.c", false, 1), scanned("00FF", true, 0)];
        let line = ScannedPluginRecord::result_line(&plugins);
        assert_eq!(
            ScannedPluginRecord::from_scan_output(&line).unwrap(),
            plugins
        );
        assert_eq!(
            ScannedPluginRecord::from_scan_output(&ScannedPluginRecord::result_line(&[])).unwrap(),
            vec![]
        );

        let noisy = format!("a plugin says hello\r\n{line}\r\nand goodbye\n");
        assert_eq!(
            ScannedPluginRecord::from_scan_output(&noisy).unwrap(),
            plugins
        );

        assert!(ScannedPluginRecord::from_scan_output("").is_err());
        assert!(ScannedPluginRecord::from_scan_output("hello").is_err());
        assert!(ScannedPluginRecord::from_scan_output(&line[..line.len() - 1]).is_err());
        assert!(ScannedPluginRecord::from_scan_output(&format!("{line}zz")).is_err());
        assert!(ScannedPluginRecord::from_scan_output("ECHOES-SCAN-RESULT ffffffff").is_err());
        let bytes = ScannedPluginRecord::encode_list(&plugins);
        assert!(ScannedPluginRecord::decode_list(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn a_preset_state_roundtrips_with_or_without_the_controller_chunk() {
        for controller in [None, Some(vec![9, 8]), Some(vec![])] {
            let state = PresetStateRecord {
                component: vec![1, 2, 3],
                controller,
            };
            assert_eq!(
                PresetStateRecord::decode(&state.encode().unwrap()).unwrap(),
                state
            );
        }
        assert!(PresetStateRecord::decode(&[2, 0, 0, 0, 0]).is_err());
        assert!(PresetStateRecord::decode(&[1, 0, 0, 0, 0]).is_err());
        let mut trailing = PresetStateRecord {
            component: vec![],
            controller: None,
        }
        .encode()
        .unwrap();
        trailing.push(0);
        assert!(PresetStateRecord::decode(&trailing).is_err());
    }

    #[test]
    fn a_preset_roundtrips_and_a_trailing_byte_is_refused() {
        let preset = PresetRecord {
            source: PresetSource::File,
            location: Some("C:/presets/a.fxp".into()),
            load_key: Some("main".into()),
        };
        let mut bytes = preset.encode().unwrap();
        assert_eq!(PresetRecord::decode(&bytes).unwrap(), preset);
        bytes.push(0);
        assert!(PresetRecord::decode(&bytes).is_err());

        let bundled = PresetRecord {
            source: PresetSource::Plugin,
            location: None,
            load_key: Some("loud".into()),
        };
        assert_eq!(
            PresetRecord::decode(&bundled.encode().unwrap()).unwrap(),
            bundled
        );
    }

    #[test]
    fn a_preset_text_too_long_for_the_wire_is_not_encoded() {
        let preset = PresetRecord {
            source: PresetSource::File,
            location: Some("x".repeat(70_000)),
            load_key: None,
        };
        assert_eq!(preset.encode(), None);
    }
}
