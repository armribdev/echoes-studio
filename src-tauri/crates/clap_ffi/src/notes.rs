//! `clap.note-ports` and `clap.note-name`: what the host answers, and how it reads
//! a plugin's note names.
//!
//! The host does not read the plugin's note ports: it only says which dialects
//! it sends (note events and raw MIDI) and treats a rescan of all ports as a
//! restart request.  The plugin's note names are read on demand (`read_note_names`)
//! and its `changed` is recorded for the app to poll.

use std::ffi::{c_char, CStr};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{host_context, ClapHost, ClapPlugin, CLAP_NAME_SIZE};

pub const CLAP_EXT_NOTE_PORTS: &[u8] = b"clap.note-ports\0";
pub const CLAP_EXT_NOTE_NAME: &[u8] = b"clap.note-name\0";

pub const CLAP_NOTE_DIALECT_CLAP: u32 = 1 << 0;
pub const CLAP_NOTE_DIALECT_MIDI: u32 = 1 << 1;
pub const CLAP_NOTE_DIALECT_MIDI_MPE: u32 = 1 << 2;
pub const CLAP_NOTE_DIALECT_MIDI2: u32 = 1 << 3;

pub const CLAP_NOTE_PORTS_RESCAN_ALL: u32 = 1 << 0;
pub const CLAP_NOTE_PORTS_RESCAN_NAMES: u32 = 1 << 1;

/// What the host sends a plugin: `CLAP_EVENT_NOTE_*` and raw `CLAP_EVENT_MIDI`
/// (pitch bend, CC, aftertouch).  No MPE, no MIDI 2.
pub const HOST_NOTE_DIALECTS: u32 = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;

/// `clap_host_note_ports`.
#[repr(C)]
pub struct ClapHostNotePorts {
    pub supported_dialects: unsafe extern "C" fn(*const ClapHost) -> u32,
    pub rescan: unsafe extern "C" fn(*const ClapHost, u32),
}

pub(crate) static HOST_NOTE_PORTS: ClapHostNotePorts = ClapHostNotePorts {
    supported_dialects: host_note_ports_supported_dialects,
    rescan: host_note_ports_rescan,
};

unsafe extern "C" fn host_note_ports_supported_dialects(_: *const ClapHost) -> u32 {
    HOST_NOTE_DIALECTS
}

// The ports are not read, so a changed list has nothing to refresh; a restart
// still makes the plugin reactivate, which is what CLAP asks of a host for an
// active plugin.  A renamed port (`NAMES`) shows nowhere.
unsafe extern "C" fn host_note_ports_rescan(host: *const ClapHost, flags: u32) {
    if flags & CLAP_NOTE_PORTS_RESCAN_ALL == 0 {
        return;
    }
    if let Some(context) = host_context(host) {
        context.request_restart();
    }
}

/// `clap_note_name`: `-1` in `port`, `key` or `channel` means every one.
#[repr(C)]
pub struct ClapNoteName {
    pub name: [u8; CLAP_NAME_SIZE],
    pub port: i16,
    pub key: i16,
    pub channel: i16,
}

/// `clap_plugin_note_name`, `[main-thread]`.
#[repr(C)]
pub struct ClapPluginNoteName {
    pub count: unsafe extern "C" fn(*const ClapPlugin) -> u32,
    pub get: unsafe extern "C" fn(*const ClapPlugin, u32, *mut ClapNoteName) -> bool,
}

/// `clap_host_note_name`.
#[repr(C)]
pub struct ClapHostNoteName {
    pub changed: unsafe extern "C" fn(*const ClapHost),
}

pub(crate) static HOST_NOTE_NAME: ClapHostNoteName = ClapHostNoteName {
    changed: host_note_name_changed,
};

/// Set when any hosted plugin reports new note names; the app polls it.
static PLUGIN_NOTE_NAMES_CHANGED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn host_note_name_changed(_: *const ClapHost) {
    mark_plugin_note_names_changed();
}

/// Lock-free: callable from any thread.
pub fn mark_plugin_note_names_changed() {
    PLUGIN_NOTE_NAMES_CHANGED.store(true, Ordering::Release);
}

/// Whether a plugin reported new note names since the last call, and clears it.
pub fn take_plugin_note_names_changed() -> bool {
    PLUGIN_NOTE_NAMES_CHANGED.swap(false, Ordering::AcqRel)
}

/// A name the plugin gives to notes: `-1` in `port`, `key` or `channel` means every one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteName {
    pub name: String,
    pub port: i16,
    pub key: i16,
    pub channel: i16,
}

/// A plugin cannot name more notes than 128 keys on 16 channels and a few ports.
const MAX_NOTE_NAMES: u32 = 4096;

/// The plugin's note names; empty when it has no `clap.note-name`.
///
/// # Safety
/// `plugin` must be live and the call must run where CLAP allows main-thread calls on it.
pub unsafe fn read_note_names(plugin: *const ClapPlugin) -> Vec<NoteName> {
    let extension = ((*plugin).get_extension)(plugin, CLAP_EXT_NOTE_NAME.as_ptr() as *const c_char)
        as *const ClapPluginNoteName;
    let Some(extension) = extension.as_ref() else {
        return Vec::new();
    };
    let count = (extension.count)(plugin).min(MAX_NOTE_NAMES);
    let mut names = Vec::new();
    for index in 0..count {
        let mut raw: ClapNoteName = std::mem::zeroed();
        if !(extension.get)(plugin, index, &mut raw) {
            continue;
        }
        raw.name[CLAP_NAME_SIZE - 1] = 0;
        names.push(NoteName {
            name: CStr::from_ptr(raw.name.as_ptr() as *const c_char)
                .to_string_lossy()
                .into_owned(),
            port: raw.port,
            key: raw.key,
            channel: raw.channel,
        });
    }
    names
}

/// Wire form for the bridge: per name `[port:i16, key:i16, channel:i16, length:u16, name…]`.
pub fn encode_note_names(names: &[NoteName]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for entry in names {
        let name = &entry.name.as_bytes()[..entry.name.len().min(CLAP_NAME_SIZE)];
        bytes.extend_from_slice(&entry.port.to_le_bytes());
        bytes.extend_from_slice(&entry.key.to_le_bytes());
        bytes.extend_from_slice(&entry.channel.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(name);
    }
    bytes
}

pub fn decode_note_names(mut bytes: &[u8]) -> Option<Vec<NoteName>> {
    let mut names = Vec::new();
    while !bytes.is_empty() {
        let (head, rest) = bytes.split_first_chunk::<8>()?;
        let number = |at: usize| i16::from_le_bytes([head[at], head[at + 1]]);
        let length = u16::from_le_bytes([head[6], head[7]]) as usize;
        let name = std::str::from_utf8(rest.get(..length)?).ok()?;
        names.push(NoteName {
            name: name.to_owned(),
            port: number(0),
            key: number(2),
            channel: number(4),
        });
        bytes = &rest[length..];
    }
    Some(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{host_get_extension, make_host};
    use std::mem::{offset_of, size_of};

    fn host_table<T>(host: &ClapHost, id: &[u8]) -> &'static T {
        let extension = unsafe { host_get_extension(host, id.as_ptr() as *const c_char) };
        unsafe { (extension as *const T).as_ref() }.expect("extension answered")
    }

    #[test]
    fn the_note_name_layout_follows_the_spec() {
        assert_eq!(offset_of!(ClapNoteName, name), 0);
        assert_eq!(offset_of!(ClapNoteName, port), CLAP_NAME_SIZE);
        assert_eq!(offset_of!(ClapNoteName, key), CLAP_NAME_SIZE + 2);
        assert_eq!(offset_of!(ClapNoteName, channel), CLAP_NAME_SIZE + 4);
        assert_eq!(size_of::<ClapNoteName>(), CLAP_NAME_SIZE + 6);
    }

    #[test]
    fn the_host_says_which_dialects_it_sends() {
        let host = make_host();
        let ports: &ClapHostNotePorts = host_table(&host, CLAP_EXT_NOTE_PORTS);
        let dialects = unsafe { (ports.supported_dialects)(&*host) };

        assert_eq!(dialects, CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI);
        assert_eq!(dialects & CLAP_NOTE_DIALECT_MIDI_MPE, 0);
        assert_eq!(dialects & CLAP_NOTE_DIALECT_MIDI2, 0);
    }

    #[test]
    fn note_names_changed_is_reported_once() {
        let host = make_host();
        let names: &ClapHostNoteName = host_table(&host, CLAP_EXT_NOTE_NAME);
        take_plugin_note_names_changed();

        unsafe { (names.changed)(&*host) };
        assert!(take_plugin_note_names_changed());
        assert!(!take_plugin_note_names_changed());
    }

    #[test]
    fn note_names_round_trip_on_the_wire() {
        let names = vec![
            NoteName {
                name: "Kick".into(),
                port: -1,
                key: 36,
                channel: -1,
            },
            NoteName {
                name: "Caisse claire é".into(),
                port: 0,
                key: 38,
                channel: 9,
            },
        ];
        assert_eq!(decode_note_names(&encode_note_names(&names)), Some(names));
        assert_eq!(decode_note_names(&[]), Some(Vec::new()));
        assert_eq!(decode_note_names(&[0; 5]), None);
        assert_eq!(decode_note_names(&[0, 0, 0, 0, 0, 0, 9, 0, b'a']), None);
    }
}
