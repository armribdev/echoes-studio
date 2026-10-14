//! `clap.preset-load`, both sides.
//!
//! Host side: a plugin says it loaded a preset, or failed to.  A preset the user
//! picks in the plugin's own window changes its state without the host knowing,
//! so `loaded` raises the same unsaved-changes flag as `clap.state.mark_dirty`.
//! An error is logged and its message kept for whoever asked for the load.
//!
//! Plugin side: [`load_preset`] asks a plugin to load one preset (ADR-031).

use std::ffi::{c_char, CStr, CString};

use crate::{host_context, host_is_audio_thread, mark_plugin_state_edited, ClapHost, ClapPlugin};

pub const CLAP_EXT_PRESET_LOAD: &[u8] = b"clap.preset-load/2\0";
pub const CLAP_EXT_PRESET_LOAD_COMPAT: &[u8] = b"clap.preset-load.draft/2\0";

/// `clap_host_preset_load`.  `location` and `load_key` may be null.
#[repr(C)]
pub struct ClapHostPresetLoad {
    pub on_error: unsafe extern "C" fn(
        *const ClapHost,
        location_kind: u32,
        location: *const c_char,
        load_key: *const c_char,
        os_error: i32,
        message: *const c_char,
    ),
    pub loaded:
        unsafe extern "C" fn(*const ClapHost, location_kind: u32, *const c_char, *const c_char),
}

pub(crate) static HOST_PRESET_LOAD: ClapHostPresetLoad = ClapHostPresetLoad {
    on_error: host_preset_load_on_error,
    loaded: host_preset_load_loaded,
};

unsafe extern "C" fn host_preset_load_loaded(
    _: *const ClapHost,
    _: u32,
    _: *const c_char,
    _: *const c_char,
) {
    mark_plugin_state_edited();
}

unsafe extern "C" fn host_preset_load_on_error(
    host: *const ClapHost,
    _: u32,
    location: *const c_char,
    _: *const c_char,
    os_error: i32,
    message: *const c_char,
) {
    // Printing and remembering allocate, which the audio callback must never do.
    if host_is_audio_thread(std::ptr::null()) {
        return;
    }
    let text = |pointer: *const c_char| {
        if pointer.is_null() {
            return String::new();
        }
        CStr::from_ptr(pointer).to_string_lossy().into_owned()
    };
    let (location, message) = (text(location), text(message));
    eprintln!("[clap.preset-load] {location} (os error {os_error}): {message}");
    if let Some(context) = host_context(host) {
        context.note_preset_error(message);
    }
}

/// `clap_plugin_preset_load`, `[main-thread]`.
#[repr(C)]
pub struct ClapPluginPresetLoad {
    pub from_location: unsafe extern "C" fn(
        *const ClapPlugin,
        location_kind: u32,
        location: *const c_char,
        load_key: *const c_char,
    ) -> bool,
}

/// One preset to load: where it lives and the plugin's own key inside it.  A
/// `PLUGIN` location has no path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresetRequest {
    pub location_kind: u32,
    pub location: Option<String>,
    pub load_key: Option<String>,
}

const HAS_LOCATION: u8 = 1 << 0;
const HAS_LOAD_KEY: u8 = 1 << 1;

impl PresetRequest {
    /// Wire form for the bridge: `kind:u32, present:u8`, then each present text as
    /// `length:u16, bytes`.  `None` when a text is too long for that.
    pub fn encode(&self) -> Option<Vec<u8>> {
        let mut present = 0;
        if self.location.is_some() {
            present |= HAS_LOCATION;
        }
        if self.load_key.is_some() {
            present |= HAS_LOAD_KEY;
        }
        let mut bytes = self.location_kind.to_le_bytes().to_vec();
        bytes.push(present);
        for text in [&self.location, &self.load_key].into_iter().flatten() {
            bytes.extend_from_slice(&u16::try_from(text.len()).ok()?.to_le_bytes());
            bytes.extend_from_slice(text.as_bytes());
        }
        Some(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (head, mut rest) = bytes.split_first_chunk::<5>()?;
        let location_kind = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        let mut text = |wanted: bool| -> Option<Option<String>> {
            if !wanted {
                return Some(None);
            }
            let (length, tail) = rest.split_first_chunk::<2>()?;
            let length = usize::from(u16::from_le_bytes(*length));
            let value = std::str::from_utf8(tail.get(..length)?).ok()?.to_owned();
            rest = &tail[length..];
            Some(Some(value))
        };
        let location = text(head[4] & HAS_LOCATION != 0)?;
        let load_key = text(head[4] & HAS_LOAD_KEY != 0)?;
        rest.is_empty().then_some(Self {
            location_kind,
            location,
            load_key,
        })
    }
}

/// Asks the plugin to load a preset through its `clap.preset-load`.  False when
/// it has no such extension, when a text holds a NUL, or when the plugin refuses
/// (its `on_error` then tells the host why).
///
/// # Safety
/// `plugin` must be live and the call must run where CLAP allows main-thread calls on it.
pub unsafe fn load_preset(plugin: *const ClapPlugin, request: &PresetRequest) -> bool {
    let extension = [CLAP_EXT_PRESET_LOAD, CLAP_EXT_PRESET_LOAD_COMPAT]
        .iter()
        .map(|id| ((*plugin).get_extension)(plugin, id.as_ptr() as *const c_char))
        .find(|pointer| !pointer.is_null())
        .unwrap_or(std::ptr::null());
    let Some(extension) = (extension as *const ClapPluginPresetLoad).as_ref() else {
        return false;
    };
    let text = |text: &Option<String>| match text.as_deref().map(CString::new) {
        Some(Ok(text)) => Some(Some(text)),
        Some(Err(_)) => None,
        None => Some(None),
    };
    let (Some(location), Some(load_key)) = (text(&request.location), text(&request.load_key))
    else {
        return false;
    };
    let pointer = |text: &Option<CString>| text.as_ref().map_or(std::ptr::null(), |t| t.as_ptr());
    (extension.from_location)(
        plugin,
        request.location_kind,
        pointer(&location),
        pointer(&load_key),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{host_get_extension, make_host, take_plugin_state_edited};

    fn host_table(host: &ClapHost, id: &[u8]) -> &'static ClapHostPresetLoad {
        let extension = unsafe { host_get_extension(host, id.as_ptr() as *const c_char) };
        unsafe { (extension as *const ClapHostPresetLoad).as_ref() }.expect("extension answered")
    }

    #[test]
    fn both_names_of_the_extension_are_answered_with_the_same_table() {
        let host = make_host();
        let current = host_table(&host, CLAP_EXT_PRESET_LOAD);
        let compat = host_table(&host, CLAP_EXT_PRESET_LOAD_COMPAT);

        assert!(std::ptr::eq(current, compat));
    }

    #[test]
    fn a_failed_load_keeps_the_plugins_message_for_the_caller_once() {
        let host = make_host();
        let context = host.context().unwrap();
        let table = host_table(&host, CLAP_EXT_PRESET_LOAD);
        assert_eq!(context.take_preset_error(), None);

        unsafe {
            (table.on_error)(
                &*host,
                0,
                c"/presets/broken".as_ptr(),
                std::ptr::null(),
                2,
                c"file not found".as_ptr(),
            )
        };

        assert_eq!(
            context.take_preset_error().as_deref(),
            Some("file not found")
        );
        assert_eq!(context.take_preset_error(), None);
    }

    #[test]
    fn a_request_round_trips_on_the_wire() {
        let requests = [
            PresetRequest {
                location_kind: 1,
                location: None,
                load_key: Some("loud".into()),
            },
            PresetRequest {
                location_kind: 0,
                location: Some("C:\\Presets\\Chaleur é.fxp".into()),
                load_key: None,
            },
            PresetRequest::default(),
            PresetRequest {
                location_kind: 0,
                location: Some("a".into()),
                load_key: Some("b".into()),
            },
        ];
        for request in requests {
            assert_eq!(
                PresetRequest::decode(&request.encode().unwrap()),
                Some(request)
            );
        }
    }

    #[test]
    fn a_malformed_request_is_refused() {
        let good = PresetRequest {
            location_kind: 0,
            location: Some("abc".into()),
            load_key: Some("k".into()),
        }
        .encode()
        .unwrap();

        assert_eq!(PresetRequest::decode(&[]), None);
        assert_eq!(PresetRequest::decode(&good[..good.len() - 1]), None);
        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(PresetRequest::decode(&trailing), None);
        let mut not_utf8 = good;
        not_utf8[7] = 0xFF;
        assert_eq!(PresetRequest::decode(&not_utf8), None);

        let too_long = PresetRequest {
            location: Some("x".repeat(70_000)),
            ..PresetRequest::default()
        };
        assert_eq!(too_long.encode(), None);
    }

    #[test]
    fn a_loaded_preset_marks_the_project_unsaved_and_an_error_does_not() {
        let host = make_host();
        let table = host_table(&host, CLAP_EXT_PRESET_LOAD);
        take_plugin_state_edited();

        unsafe {
            (table.on_error)(
                &*host,
                0,
                c"/presets/broken".as_ptr(),
                std::ptr::null(),
                2,
                c"gone".as_ptr(),
            );
            (table.on_error)(
                &*host,
                1,
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
            );
        }
        assert!(!take_plugin_state_edited(), "a failed load changed nothing");

        unsafe { (table.loaded)(&*host, 0, c"/presets/warm".as_ptr(), std::ptr::null()) };
        assert!(take_plugin_state_edited());
        assert!(!take_plugin_state_edited(), "reported once");
    }
}
