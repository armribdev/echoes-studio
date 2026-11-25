//! `clap.remote-controls`: the pages of parameters a plugin considers its most
//! important, read on demand.
//!
//! The host offers no `clap_host_remote_controls`: a plugin that changes its pages
//! is read again the next time the app asks, and no page is ever suggested.

use std::ffi::{c_char, CStr};

use crate::{ClapPlugin, CLAP_INVALID_ID, CLAP_NAME_SIZE};

pub const CLAP_EXT_REMOTE_CONTROLS: &[u8] = b"clap.remote-controls/2\0";
pub const CLAP_EXT_REMOTE_CONTROLS_COMPAT: &[u8] = b"clap.remote-controls.draft/2\0";

const CLAP_REMOTE_CONTROLS_COUNT: usize = 8;

/// `clap_remote_controls_page`.
#[repr(C)]
pub struct ClapRemoteControlsPage {
    pub section_name: [u8; CLAP_NAME_SIZE],
    pub page_id: u32,
    pub page_name: [u8; CLAP_NAME_SIZE],
    pub param_ids: [u32; CLAP_REMOTE_CONTROLS_COUNT],
    pub is_for_preset: bool,
}

/// `clap_plugin_remote_controls`, `[main-thread]`.
#[repr(C)]
pub struct ClapPluginRemoteControls {
    pub count: unsafe extern "C" fn(*const ClapPlugin) -> u32,
    pub get: unsafe extern "C" fn(*const ClapPlugin, u32, *mut ClapRemoteControlsPage) -> bool,
}

/// A page of up to eight parameters, in the order the plugin gives them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePage {
    pub section: String,
    pub name: String,
    pub param_ids: Vec<u32>,
}

/// A plugin has a handful of pages per section; this only bounds a broken answer.
const MAX_REMOTE_PAGES: u32 = 512;

/// The plugin's pages without an empty slot; empty when it has no `clap.remote-controls`.
///
/// # Safety
/// `plugin` must be live and the call must run where CLAP allows main-thread calls on it.
pub unsafe fn read_remote_pages(plugin: *const ClapPlugin) -> Vec<RemotePage> {
    let extension = remote_controls_extension(plugin);
    let Some(extension) = extension.as_ref() else {
        return Vec::new();
    };
    let count = (extension.count)(plugin).min(MAX_REMOTE_PAGES);
    let mut pages = Vec::new();
    for index in 0..count {
        let mut raw: ClapRemoteControlsPage = std::mem::zeroed();
        if !(extension.get)(plugin, index, &mut raw) {
            continue;
        }
        raw.section_name[CLAP_NAME_SIZE - 1] = 0;
        raw.page_name[CLAP_NAME_SIZE - 1] = 0;
        let param_ids: Vec<u32> = raw
            .param_ids
            .into_iter()
            .filter(|&id| id != CLAP_INVALID_ID)
            .collect();
        if param_ids.is_empty() {
            continue;
        }
        pages.push(RemotePage {
            section: text_of(&raw.section_name),
            name: text_of(&raw.page_name),
            param_ids,
        });
    }
    pages
}

unsafe fn remote_controls_extension(plugin: *const ClapPlugin) -> *const ClapPluginRemoteControls {
    let current =
        ((*plugin).get_extension)(plugin, CLAP_EXT_REMOTE_CONTROLS.as_ptr() as *const c_char);
    if !current.is_null() {
        return current as *const ClapPluginRemoteControls;
    }
    ((*plugin).get_extension)(
        plugin,
        CLAP_EXT_REMOTE_CONTROLS_COMPAT.as_ptr() as *const c_char,
    ) as *const ClapPluginRemoteControls
}

unsafe fn text_of(name: &[u8; CLAP_NAME_SIZE]) -> String {
    CStr::from_ptr(name.as_ptr() as *const c_char)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_has_the_layout_of_the_clap_header() {
        assert_eq!(std::mem::offset_of!(ClapRemoteControlsPage, page_id), 256);
        assert_eq!(std::mem::offset_of!(ClapRemoteControlsPage, page_name), 260);
        assert_eq!(std::mem::offset_of!(ClapRemoteControlsPage, param_ids), 516);
        assert_eq!(
            std::mem::offset_of!(ClapRemoteControlsPage, is_for_preset),
            548
        );
    }
}
