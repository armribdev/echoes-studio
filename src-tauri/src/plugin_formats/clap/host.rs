//! CLAP host — scans platform plugin directories and populates the registry.

use std::path::{Path, PathBuf};

use crate::plugin_system::registry::{PluginManifest, PluginRegistry};
use crate::plugin_system::wrapper::DeviceFormat;

/// Bound on vendor-subfolder recursion during a scan (avoids pathological
/// symlink loops); real installs are 1-2 levels deep.
const SCAN_MAX_DEPTH: usize = 4;

pub struct ClapHost;

impl ClapHost {
    pub fn new() -> Self {
        ClapHost
    }

    pub fn scan(&self, registry: &mut PluginRegistry) {
        for dir in clap_search_paths() {
            if dir.exists() {
                self.scan_dir(&dir, registry, SCAN_MAX_DEPTH);
            }
        }
    }

    /// Vendors commonly install into a subfolder of the CLAP directory (e.g.
    /// `Common Files\CLAP\Surge Synth Team\Surge XT.clap`), so this recurses
    /// a bounded number of levels rather than only scanning the top level.
    fn scan_dir(&self, dir: &Path, registry: &mut PluginRegistry, depth: usize) {
        if depth == 0 {
            return;
        }
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
            } else if path.is_dir() {
                self.scan_dir(&path, registry, depth - 1);
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
            let features = clap_descriptor_features(desc);
            let is_instrument = features.iter().any(|f| f == "instrument");
            let categories: Vec<String> = if is_instrument {
                vec!["Instrument".into(), "CLAP".into()]
            } else {
                vec!["CLAP".into()]
            };

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
                categories,
                num_audio_inputs: if is_instrument { 0 } else { 2 },
                num_audio_outputs: 2,
                has_midi_input: is_instrument,
                has_midi_output: false,
                num_sidechain_inputs: 0,
                is_instrument,
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

fn clap_descriptor_features(desc: &crate::plugin_formats::clap::ffi::ClapPluginDescriptor) -> Vec<String> {
    use std::ffi::CStr;

    if desc.features.is_null() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    loop {
        let ptr = unsafe { *desc.features.add(i) };
        if ptr.is_null() {
            break;
        }
        if let Ok(s) = unsafe { CStr::from_ptr(ptr) }.to_str() {
            out.push(s.to_string());
        }
        i += 1;
    }
    out
}
