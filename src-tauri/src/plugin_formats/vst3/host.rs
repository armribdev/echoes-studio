//! VST3 host — scans platform plugin directories and populates the registry.

use std::path::{Path, PathBuf};

use crate::plugin_system::registry::{PluginManifest, PluginRegistry};
use crate::plugin_system::wrapper::DeviceFormat;

pub struct Vst3Host;

impl Vst3Host {
    pub fn new() -> Self { Vst3Host }

    /// Scan all standard VST3 directories and register found plugins.
    pub fn scan(&self, registry: &mut PluginRegistry) {
        for dir in vst3_search_paths() {
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
            if let Some(ext) = path.extension() {
                if ext.eq_ignore_ascii_case("vst3") {
                    self.register_bundle(&path, registry);
                }
            }
        }
    }

    fn register_bundle(&self, bundle: &Path, registry: &mut PluginRegistry) {
        let dll = match resolve_vst3_dll(bundle) {
            Some(p) => p,
            None => return,
        };

        use crate::plugin_formats::vst3::ffi::Vst3Library;
        let (lib, factory) = match Vst3Library::load(&dll) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[vst3] scan: {} — {e}", bundle.display());
                return;
            }
        };
        let _ = lib; // keep DLL alive through enumeration scope

        let classes = unsafe { crate::plugin_formats::vst3::ffi::enumerate_classes(factory) };
        for (cid, name) in classes {
            // Encode CID into the plugin_id so no extra storage is needed.
            // Format: "vst3:<32-hex-chars>"
            let plugin_id = format!("vst3:{}", hex_cid(&cid));
            if registry.get(&plugin_id).is_some() { continue; }

            registry.add(PluginManifest {
                id:   plugin_id,
                name: name.clone(),
                vendor: String::new(),
                version: String::new(),
                format: DeviceFormat::Vst3,
                path:  Some(dll.clone()),
                categories: vec!["VST3".into()],
                num_audio_inputs:  2,
                num_audio_outputs: 2,
                has_midi_input:  false,
                has_midi_output: false,
            });
        }

        // Release factory ref — library stays loaded until Arc drops.
        unsafe {
            ((*(*factory).vtbl).release)(factory);
        }
    }
}

impl Default for Vst3Host {
    fn default() -> Self { Self::new() }
}

// ── Path helpers ───────────────────────────────────────────────────────────────

fn vst3_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(pf) = std::env::var("PROGRAMFILES") {
        paths.push(PathBuf::from(pf).join("Common Files").join("VST3"));
    }
    if let Ok(pf) = std::env::var("PROGRAMFILES(X86)") {
        paths.push(PathBuf::from(pf).join("Common Files").join("VST3"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        paths.push(PathBuf::from(local).join("Programs").join("Common").join("VST3"));
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        paths.push(PathBuf::from(appdata).join("VST3"));
    }
    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/VST3"));
        }
    }
    paths
}

/// Resolve the actual DLL inside a .vst3 bundle (directory or single-file).
fn resolve_vst3_dll(bundle: &Path) -> Option<PathBuf> {
    if bundle.is_file() {
        return Some(bundle.to_path_buf());
    }
    let arch = if cfg!(target_arch = "x86_64") { "x86_64-win" } else { "x86-win" };
    let contents = bundle.join("Contents").join(arch);
    if contents.exists() {
        for entry in std::fs::read_dir(&contents).ok()?.flatten() {
            let p = entry.path();
            if let Some(ext) = p.extension() {
                if ext.eq_ignore_ascii_case("vst3") || ext.eq_ignore_ascii_case("dll") {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Encode 16-byte class ID as a 32-char uppercase hex string.
pub fn hex_cid(cid: &[u8; 16]) -> String {
    cid.iter().map(|b| format!("{b:02X}")).collect()
}

/// Decode a `vst3:<32hex>` plugin_id back to a 16-byte CID.
pub fn decode_plugin_id(plugin_id: &str) -> Option<[u8; 16]> {
    let hex = plugin_id.strip_prefix("vst3:")?;
    if hex.len() != 32 { return None; }
    let mut cid = [0u8; 16];
    for (i, b) in cid.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(cid)
}
