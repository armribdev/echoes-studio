//! CLAP host — scans platform plugin directories and populates the registry.

use std::path::{Path, PathBuf};

use crate::plugin_system::registry::{PluginManifest, PluginRegistry};
use crate::plugin_system::wrapper::DeviceFormat;

pub struct ClapHost;

impl ClapHost {
    pub fn new() -> Self {
        ClapHost
    }

    pub fn scan(&self, registry: &mut PluginRegistry) {
        for dir in clap_search_paths() {
            if dir.exists() {
                self.scan_dir(&dir, registry);
            }
        }
    }

    fn scan_dir(&self, dir: &Path, registry: &mut PluginRegistry) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .map_or(false, |e| e.eq_ignore_ascii_case("clap"))
            {
                self.register_clap(&path, registry);
            }
        }
    }

    fn register_clap(&self, dll_path: &Path, registry: &mut PluginRegistry) {
        use crate::plugin_formats::clap::ffi::{ClapLibrary, CLAP_PLUGIN_FACTORY_ID};

        // `ClapLibrary::load` caches loaded DLLs process-wide and calls
        // `clap_entry.init` exactly once per DLL — no need to call init here.
        let (_lib, entry) = match ClapLibrary::load(dll_path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[clap] scan: {} — {e}", dll_path.display());
                return;
            }
        };

        // Get the plugin factory.
        let factory_ptr = unsafe {
            ((*entry).get_factory)(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const std::ffi::c_char)
        };
        if factory_ptr.is_null() {
            return;
        }

        let factory = factory_ptr as *const crate::plugin_formats::clap::ffi::ClapPluginFactory;
        let count = unsafe { ((*factory).get_plugin_count)(factory) };

        for i in 0..count {
            let desc = unsafe { ((*factory).get_plugin_descriptor)(factory, i) };
            if desc.is_null() {
                continue;
            }
            let desc = unsafe { &*desc };

            let clap_id = desc.id_str().to_string();
            let name = desc.name_str().to_string();
            let vendor = desc.vendor_str().to_string();
            let plugin_id = format!("clap:{clap_id}");

            if registry.get(&plugin_id).is_some() {
                continue;
            }

            registry.add(PluginManifest {
                id: plugin_id,
                name,
                vendor,
                version: String::new(),
                format: DeviceFormat::Clap,
                path: Some(dll_path.to_path_buf()),
                categories: vec!["CLAP".into()],
                num_audio_inputs: 2,
                num_audio_outputs: 2,
                has_midi_input: false,
                has_midi_output: false,
                // No bus introspection at scan yet (descriptor-only); a lazy
                // `clap.audio-ports` probe will populate this (Phase 5d follow-up).
                num_sidechain_inputs: 0,
            });
        }
    }
}

impl Default for ClapHost {
    fn default() -> Self {
        Self::new()
    }
}

fn clap_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(pf) = std::env::var("PROGRAMFILES") {
        paths.push(PathBuf::from(pf).join("Common Files").join("CLAP"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        paths.push(
            PathBuf::from(local)
                .join("Programs")
                .join("Common")
                .join("CLAP"),
        );
    }
    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
        }
    }
    paths
}

/// Decode a `clap:<clap-plugin-id>` plugin_id back to the raw CLAP id string.
pub fn decode_plugin_id(plugin_id: &str) -> Option<&str> {
    plugin_id.strip_prefix("clap:")
}
