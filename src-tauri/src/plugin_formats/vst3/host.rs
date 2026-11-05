//! Where VST3 plugins are installed.  The files are listed by `plugin_host_bridge --scan`
//! in a process of their own (ADR-033, step 10), never loaded here.

use std::path::{Path, PathBuf};

/// Bound on vendor-subfolder recursion during a scan (avoids pathological
/// symlink loops); real installs are 1-2 levels deep.
const SCAN_MAX_DEPTH: usize = 4;

/// The module (DLL) of every `.vst3` found in the standard VST3 directories.
pub fn find_modules() -> Vec<PathBuf> {
    let mut modules = Vec::new();
    for dir in vst3_search_paths() {
        if dir.exists() {
            collect_modules(&dir, &mut modules, SCAN_MAX_DEPTH);
        }
    }
    modules
}

/// Vendors commonly install into a subfolder of the VST3 directory, so
/// this recurses a bounded number of levels rather than only scanning
/// the top level.
fn collect_modules(dir: &Path, modules: &mut Vec<PathBuf>, depth: usize) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("vst3"))
        {
            modules.extend(resolve_vst3_dll(&path));
        } else if path.is_dir() {
            collect_modules(&path, modules, depth - 1);
        }
    }
}

fn vst3_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(pf) = std::env::var("PROGRAMFILES") {
        paths.push(PathBuf::from(pf).join("Common Files").join("VST3"));
    }
    if let Ok(pf) = std::env::var("PROGRAMFILES(X86)") {
        paths.push(PathBuf::from(pf).join("Common Files").join("VST3"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        paths.push(
            PathBuf::from(local)
                .join("Programs")
                .join("Common")
                .join("VST3"),
        );
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
pub fn resolve_vst3_dll(bundle: &Path) -> Option<PathBuf> {
    if bundle.is_file() {
        return Some(bundle.to_path_buf());
    }
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64-win"
    } else {
        "x86-win"
    };
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
#[cfg(test)]
pub fn hex_cid(cid: &[u8; 16]) -> String {
    cid.iter().map(|b| format!("{b:02X}")).collect()
}

/// Decode a `vst3:<32hex>` plugin_id back to a 16-byte CID.
pub fn decode_plugin_id(plugin_id: &str) -> Option<[u8; 16]> {
    let hex = plugin_id.strip_prefix("vst3:")?;
    if hex.len() != 32 {
        return None;
    }
    let mut cid = [0u8; 16];
    for (i, b) in cid.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(cid)
}
