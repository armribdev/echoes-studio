//! Host side of `clap.preset-load`: a plugin says it loaded a preset, or failed to.
//!
//! A preset the user picks in the plugin's own window changes its state without
//! the host knowing, so `loaded` raises the same unsaved-changes flag as
//! `clap.state.mark_dirty`.  An error is only logged.

use std::ffi::{c_char, CStr};

use crate::{host_is_audio_thread, mark_plugin_state_edited, ClapHost};

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
    _: *const ClapHost,
    _: u32,
    location: *const c_char,
    _: *const c_char,
    os_error: i32,
    message: *const c_char,
) {
    // Printing allocates and does I/O, which the audio callback must never do.
    if host_is_audio_thread(std::ptr::null()) {
        return;
    }
    let text = |pointer: *const c_char| {
        if pointer.is_null() {
            return String::new();
        }
        CStr::from_ptr(pointer).to_string_lossy().into_owned()
    };
    eprintln!(
        "[clap.preset-load] {} (os error {os_error}): {}",
        text(location),
        text(message)
    );
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
