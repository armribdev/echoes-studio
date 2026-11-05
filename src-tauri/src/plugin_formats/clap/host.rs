//! Where CLAP plugins are installed.  The files are listed by `plugin_host_bridge --scan`
//! in a process of their own (ADR-033, step 10), never loaded here.

use std::path::{Path, PathBuf};

/// Bound on vendor-subfolder recursion during a scan (avoids pathological
/// symlink loops); real installs are 1-2 levels deep.
const SCAN_MAX_DEPTH: usize = 4;

/// Every `.clap` file found in the standard CLAP directories.
pub fn find_modules() -> Vec<PathBuf> {
    let mut modules = Vec::new();
    for dir in clap_search_paths() {
        if dir.exists() {
            collect_modules(&dir, &mut modules, SCAN_MAX_DEPTH);
        }
    }
    modules
}

/// Vendors commonly install into a subfolder of the CLAP directory (e.g.
/// `Common Files\CLAP\Surge Synth Team\Surge XT.clap`), so this recurses
/// a bounded number of levels rather than only scanning the top level.
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
            .is_some_and(|ext| ext.eq_ignore_ascii_case("clap"))
        {
            modules.push(path);
        } else if path.is_dir() {
            collect_modules(&path, modules, depth - 1);
        }
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

