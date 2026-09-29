//! CLAP host — scans platform plugin directories and populates the registry.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use crate::plugin_formats::crash_guard;
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
                .is_some_and(|e| e.eq_ignore_ascii_case("clap"))
            {
                self.register_clap(&path, registry);
            } else if path.is_dir() {
                self.scan_dir(&path, registry, depth - 1);
            }
        }
    }

    fn register_clap(&self, dll_path: &Path, registry: &mut PluginRegistry) {
        use crate::plugin_formats::clap::ffi::{make_host, ClapLibrary, CLAP_PLUGIN_FACTORY_ID};

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

        // One throwaway host for every plugin probed out of this DLL (§ below):
        // each probe creates, queries and destroys its own plugin instance before
        // the next one starts, so a single host struct can be reused safely.
        let probe_host = make_host();

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

            // Sidechain foundation (ADR-014) real bus introspection: effects only
            // — an instrument has no meaningful aux *input*, and probing one here
            // would trigger the same wavetable/sample load its normal instantiation
            // does, doubling scan-time cost for nothing this manifest field uses.
            let num_sidechain_inputs = if is_instrument {
                0
            } else {
                probe_aux_input_count(factory, &probe_host, &clap_id)
            };

            registry.add(PluginManifest {
                id: plugin_id,
                name,
                vendor,
                format: DeviceFormat::Clap,
                path: Some(dll_path.to_path_buf()),
                categories,
                num_sidechain_inputs,
                is_instrument,
            });
        }
    }
}

/// Stack context for [`probe_aux_inputs_trampoline`] — no heap allocation.
struct AudioPortsProbeCtx {
    factory: *const crate::plugin_formats::clap::ffi::ClapPluginFactory,
    host: *const crate::plugin_formats::clap::ffi::ClapHost,
    clap_id: *const std::ffi::c_char,
}

/// Create → init → count aux input ports → destroy, all inside one call so the
/// SEH guard below covers the whole cycle. Returns the aux port count as a
/// non-negative `i32` (the `crash_guard` return convention); `CAUGHT` (-1) means
/// an exception was caught.
///
/// # Safety
/// `raw` must point to a live `AudioPortsProbeCtx` for the duration of the call.
unsafe extern "C" fn probe_aux_inputs_trampoline(raw: *mut c_void) -> i32 {
    use crate::plugin_formats::clap::ffi::{ClapAudioPortInfo, ClapPluginAudioPorts, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS};
    use crate::plugin_formats::clap::ffi::ClapPlugin;

    let ctx = &*(raw as *const AudioPortsProbeCtx);
    let plugin = ((*ctx.factory).create_plugin)(ctx.factory, ctx.host, ctx.clap_id);
    if plugin.is_null() {
        return 0;
    }

    // Best-effort: if `init`/`get_extension`/`count`/`get` below panics or SEH-faults,
    // the plugin instance created above is not destroyed — an acceptable one-time
    // scan-time leak (see the SEH-guard note at the call site) rather than risking
    // a double-fault by calling `destroy` from inside an unknown-state exception.
    let destroy = |p: *const ClapPlugin| ((*p).destroy)(p);

    if !((*plugin).init)(plugin) {
        destroy(plugin);
        return 0;
    }

    let ext = ((*plugin).get_extension)(plugin, CLAP_EXT_AUDIO_PORTS.as_ptr() as *const std::ffi::c_char);
    let mut aux: i32 = 0;
    if !ext.is_null() {
        let audio_ports = ext as *const ClapPluginAudioPorts;
        let count = ((*audio_ports).count)(plugin, true); // true = input ports
        for idx in 0..count {
            let mut info: ClapAudioPortInfo = std::mem::zeroed();
            if ((*audio_ports).get)(plugin, idx, true, &mut info)
                && info.flags & CLAP_AUDIO_PORT_IS_MAIN == 0
            {
                aux += 1;
            }
        }
    }
    destroy(plugin);
    aux
}

/// Probe `clap.audio-ports` for the number of auxiliary (non-main) input buses
/// a plugin declares, via a throwaway instance created and destroyed on the
/// spot. Runs once at scan time (off the audio thread, off the command thread —
/// this is process startup), so the create/init/destroy cost is a one-time
/// scan expense, not a hot path.
///
/// SEH-guarded ([`crash_guard`]): a plugin whose `create_plugin`/`init` crashes
/// during the probe cannot take the scanner down with it — it just gets `0`
/// aux inputs, the same fallback as a plugin with no `clap.audio-ports` support.
/// It was already discovered via the descriptor-only path above, independent of
/// this probe.
fn probe_aux_input_count(
    factory: *const crate::plugin_formats::clap::ffi::ClapPluginFactory,
    host: &crate::plugin_formats::clap::ffi::ClapHost,
    clap_id: &str,
) -> usize {
    let clap_id_cstr: Vec<u8> = clap_id.bytes().chain(std::iter::once(0)).collect();
    let ctx = AudioPortsProbeCtx {
        factory,
        host: host as *const _,
        clap_id: clap_id_cstr.as_ptr() as *const std::ffi::c_char,
    };
    let result = unsafe {
        crash_guard::guarded(
            probe_aux_inputs_trampoline,
            &ctx as *const AudioPortsProbeCtx as *mut c_void,
        )
    };
    if result == crash_guard::CAUGHT {
        eprintln!("[clap] scan: audio-ports probe crashed for '{clap_id}' — treating as 0 aux inputs");
        0
    } else {
        result.max(0) as usize
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

/// Real-FFI proof that the scanner reads `clap.audio-ports` instead of always
/// caching `0` (ADR-014 follow-up: "bus introspection not wired"). Companion
/// to the process-routing test in `plugin_formats/clap/wrapper.rs`, which
/// proves the audio actually reaches the plugin once the count is known; this
/// proves the scanner reads the count correctly in the first place.
#[cfg(all(test, windows))]
mod audio_ports_probe_tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::OnceLock;

    use super::*;

    /// Built once per test run — mirrors `plugin_system::sandbox_tests::artifacts`.
    fn test_plugin_dll() -> &'static PathBuf {
        static DLL: OnceLock<PathBuf> = OnceLock::new();
        DLL.get_or_init(|| {
            let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
            let target = workspace.join("target").join("clap-audio-ports-probe-test");
            let status = Command::new(env!("CARGO"))
                .args(["build", "--quiet", "-p", "test_clap_plugin", "--target-dir"])
                .arg(&target)
                .current_dir(workspace)
                .status()
                .expect("run cargo");
            assert!(status.success(), "building test_clap_plugin failed");
            target.join("debug").join("test_clap_plugin.dll")
        })
    }

    /// `test_clap_plugin` declares one main + one aux input port. The scanner
    /// must report exactly one aux input — not `0` (the pre-introspection
    /// default) and not `2` (miscounting the main port as auxiliary too).
    #[test]
    fn scan_reads_the_real_aux_input_count_from_clap_audio_ports() {
        let dll = test_plugin_dll();
        let host = ClapHost::new();
        let mut registry = PluginRegistry::new();

        host.register_clap(dll, &mut registry);

        let manifest = registry
            .get("clap:echoes.test.gain")
            .expect("register_clap must add the descriptor-scanned manifest");
        assert_eq!(
            manifest.num_sidechain_inputs, 1,
            "the plugin declares exactly one non-main input port"
        );
    }

    /// `create_plugin` returning null (unknown id — the ordinary "not found"
    /// outcome, not a crash) must fall back to `0` aux inputs rather than
    /// dereferencing a null plugin pointer.
    #[test]
    fn an_unknown_plugin_id_falls_back_to_zero_without_crashing() {
        use crate::plugin_formats::clap::ffi::{ClapLibrary, CLAP_PLUGIN_FACTORY_ID};

        let dll = test_plugin_dll();
        let (_lib, entry) = ClapLibrary::load(dll).expect("load test_clap_plugin.dll");
        let factory_ptr = unsafe {
            ((*entry).get_factory)(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const std::ffi::c_char)
        };
        let factory = factory_ptr as *const crate::plugin_formats::clap::ffi::ClapPluginFactory;
        let host = crate::plugin_formats::clap::ffi::make_host();

        assert_eq!(
            probe_aux_input_count(factory, &host, "no.such.plugin.id"),
            0,
            "an unknown plugin id must fall back to 0, not crash"
        );
    }
}
