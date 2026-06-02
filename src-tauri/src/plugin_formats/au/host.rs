//! Audio Unit (AU) host — macOS only.
//!
//! Status: **stub — macOS implementation pending**.
//!
//! AU plugins use the `AudioComponentFindNext` / `AudioComponentInstanceNew`
//! CoreAudio API.  This requires:
//!   - `#[cfg(target_os = "macos")]`
//!   - `extern "C"` bindings or the `coreaudio-sys` crate
//!   - The `AudioUnit` / `AUGraph` APIs for render callbacks

use crate::plugin_system::registry::PluginRegistry;

pub struct AuHost;

impl AuHost {
    pub fn new() -> Self {
        AuHost
    }

    /// Scan the system for installed AU components.
    ///
    /// macOS: `AudioComponentFindNext(NULL, &desc)` iterates all AUs.
    pub fn scan(&self, registry: &mut PluginRegistry) {
        // TODO: #[cfg(target_os = "macos")] implementation
        let _ = registry;
    }
}

impl Default for AuHost {
    fn default() -> Self {
        Self::new()
    }
}
