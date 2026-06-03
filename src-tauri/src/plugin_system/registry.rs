//! Plugin registry — discovers and catalogues available plugins.
//!
//! The registry is populated at startup by scanning well-known plugin
//! directories.  It maps device IDs to `PluginManifest` structs that describe
//! the plugin without instantiating it.
//!
//! The actual instantiation happens in `PluginHost::instantiate`.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::plugin_system::wrapper::DeviceFormat;

/// Static metadata about a discovered plugin.
///
/// Populated by the scanner without loading the plugin binary.
#[derive(Debug, Clone)]
pub struct PluginManifest {
    /// Stable unique ID.  For VST3: the FUID string.  For internal plugins:
    /// a human-readable slug like `"echoes.gain"`.
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub format: DeviceFormat,
    /// Path to the plugin binary / bundle (empty for built-in devices).
    pub path: Option<PathBuf>,
    /// Category tags (e.g. `["Filter", "EQ"]`).
    pub categories: Vec<String>,
    pub num_audio_inputs: usize,
    pub num_audio_outputs: usize,
    pub has_midi_input: bool,
    pub has_midi_output: bool,
    /// Number of **auxiliary** (sidechain) input ports/buses, separate from the
    /// main input bus (Phase 5d, P0-3 / P1-5).  Cached at scan time; the graph
    /// compiler allocates this many extra input ports on the plugin node so a
    /// sidechain source can be routed in.  `0` for plugins with no aux bus →
    /// the compiler sizes the node exactly as before (bit-identical, C-2).
    pub num_sidechain_inputs: usize,
}

impl PluginManifest {
    /// Create a manifest for a built-in (no-path) device.
    pub fn internal(
        id: impl Into<String>,
        name: impl Into<String>,
        vendor: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            vendor: vendor.into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            format: DeviceFormat::Internal,
            path: None,
            categories: vec!["Internal".to_string()],
            num_audio_inputs: 2,
            num_audio_outputs: 2,
            has_midi_input: false,
            has_midi_output: false,
            num_sidechain_inputs: 0,
        }
    }
}

pub struct PluginRegistry {
    manifests: HashMap<String, PluginManifest>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            manifests: HashMap::new(),
        };
        registry.register_builtins();
        registry
    }

    fn register_builtins(&mut self) {
        // These match the `NodeKind` entries the compiler can build.
        let builtins = [
            ("echoes.gain", "Gain", "Echoes Studio"),
            ("echoes.pan", "Pan", "Echoes Studio"),
            ("echoes.bus", "Bus", "Echoes Studio"),
            ("echoes.send", "Send", "Echoes Studio"),
            ("echoes.meter", "Meter", "Echoes Studio"),
            ("echoes.master", "Master", "Echoes Studio"),
        ];

        for (id, name, vendor) in builtins {
            self.add(PluginManifest::internal(id, name, vendor));
        }
    }

    pub fn add(&mut self, manifest: PluginManifest) {
        self.manifests.insert(manifest.id.clone(), manifest);
    }

    /// Scan a directory for VST3 bundles / AU components.
    ///
    /// Currently a stub — real scanner will be added per platform.
    pub fn scan_directory(&mut self, _path: &std::path::Path) {
        // TODO: platform-specific plugin scanner
        // Windows: scan *.vst3 bundles
        // macOS:   scan ~/Library/Audio/Plug-Ins/VST3 and .component
    }

    pub fn get(&self, id: &str) -> Option<&PluginManifest> {
        self.manifests.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = &PluginManifest> {
        self.manifests.values()
    }

    pub fn by_format(&self, format: DeviceFormat) -> impl Iterator<Item = &PluginManifest> {
        self.manifests.values().filter(move |m| m.format == format)
    }

    pub fn search(&self, query: &str) -> Vec<&PluginManifest> {
        let q = query.to_lowercase();
        self.manifests
            .values()
            .filter(|m| {
                m.name.to_lowercase().contains(&q)
                    || m.vendor.to_lowercase().contains(&q)
                    || m.categories.iter().any(|c| c.to_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.manifests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}
