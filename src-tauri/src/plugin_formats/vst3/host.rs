//! VST3 host implementation.
//!
//! Manages loading, scanning, and lifecycle of VST3 plugin binaries.
//!
//! ## Scanning
//!
//! On Windows, VST3 plugins live in:
//!   - `%PROGRAMFILES%\Common Files\VST3\`
//!   - `%LOCALAPPDATA%\Programs\Common\VST3\`
//!   - `%APPDATA%\VST3\`
//!
//! The scanner looks for `.vst3` directories and calls `GetPluginFactory`
//! to enumerate the contained plugins.
//!
//! ## Instantiation flow
//!
//! 1. `scan()` → populate `PluginRegistry` with `PluginManifest` entries
//! 2. `load(id)` → `LoadLibrary` + `GetPluginFactory` → cache factory
//! 3. `instantiate(id, node_id)` → factory → `IComponent` + `IAudioProcessor`
//! 4. `VstWrapperNode::new(...)` → `Box<dyn AudioNode>`

use crate::plugin_system::registry::PluginRegistry;

pub struct Vst3Host;

impl Vst3Host {
    pub fn new() -> Self { Vst3Host }

    /// Scan platform VST3 directories and register found plugins.
    pub fn scan(&self, registry: &mut PluginRegistry) {
        // TODO: platform-specific scan
        // Windows: iterate %PROGRAMFILES%\Common Files\VST3\
        // macOS:   ~/Library/Audio/Plug-Ins/VST3/
        // Linux:   ~/.vst3/ and /usr/lib/vst3/
        let _ = registry; // suppress unused warning until implemented
    }
}

impl Default for Vst3Host {
    fn default() -> Self { Self::new() }
}
