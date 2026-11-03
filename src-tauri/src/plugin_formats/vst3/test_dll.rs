//! The built `crates/test_vst3_plugin` DLL, for the tests that load a real VST3.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use test_vst3_plugin::{EFFECT_CLASS_ID, INSTRUMENT_CLASS_ID};

use super::host::hex_cid;

/// Built once per test run into its own target directory.
pub(crate) fn test_vst3_dll() -> &'static PathBuf {
    static DLL: OnceLock<PathBuf> = OnceLock::new();
    DLL.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = workspace.join("target").join("vst3-test-plugin");
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "test_vst3_plugin", "--target-dir"])
            .arg(&target)
            .current_dir(workspace)
            .status()
            .expect("run cargo");
        assert!(status.success(), "building test_vst3_plugin failed");
        target.join("debug").join("test_vst3_plugin.dll")
    })
}

/// A copy under another file name: Windows maps it as a separate module, with its own
/// counters, so a test can count what the host did to it alone.
pub(crate) fn copy_of_test_vst3_dll(tag: &str) -> PathBuf {
    let copy = test_vst3_dll().with_file_name(format!("test_vst3_plugin_{tag}.dll"));
    std::fs::copy(test_vst3_dll(), &copy).expect("copy the test plugin");
    copy
}

pub(crate) fn effect_id() -> String {
    format!("vst3:{}", hex_cid(&EFFECT_CLASS_ID))
}

pub(crate) fn instrument_id() -> String {
    format!("vst3:{}", hex_cid(&INSTRUMENT_CLASS_ID))
}
