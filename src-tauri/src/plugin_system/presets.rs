//! Preset listing for CLAP plugins (ADR-031).
//!
//! A library's preset providers are third-party code that crawls the user's
//! files, so the whole cycle runs on a worker thread, outside the engine lock,
//! behind the same SEH guard as the scan's probes.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::plugin_formats::clap::ffi::{
    discover_presets, ClapLibrary, ClapPluginEntry, PresetEntry, PresetRequest,
    CLAP_PRESET_IS_FACTORY_CONTENT, CLAP_PRESET_IS_USER_CONTENT,
};
use crate::plugin_formats::crash_guard;

/// Where to ask for the presets of one hosted plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetSource {
    pub library: PathBuf,
    pub clap_plugin_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PluginPreset {
    pub name: String,
    pub load_key: Option<String>,
    pub location_kind: u32,
    pub location: Option<String>,
    pub provider_id: String,
    pub flags: u32,
    pub creator: Option<String>,
    pub description: Option<String>,
    pub features: Vec<String>,
    pub soundpack: Option<String>,
}

impl PluginPreset {
    pub fn request(&self) -> PresetRequest {
        PresetRequest {
            location_kind: self.location_kind,
            location: self.location.clone(),
            load_key: self.load_key.clone(),
        }
    }
}

impl From<PresetEntry> for PluginPreset {
    fn from(entry: PresetEntry) -> Self {
        Self {
            name: entry.name,
            load_key: entry.load_key,
            location_kind: entry.location_kind,
            location: entry.location,
            provider_id: entry.provider_id,
            flags: entry.flags,
            creator: entry.creator,
            description: entry.description,
            features: entry.features,
            soundpack: entry.soundpack,
        }
    }
}

/// Providers are not thread-safe; two listings never run at once.
static DISCOVERY: Mutex<()> = Mutex::new(());

struct Cycle<'a> {
    entry: *const ClapPluginEntry,
    clap_plugin_id: &'a str,
    presets: Vec<PresetEntry>,
}

unsafe extern "C" fn run_cycle(raw: *mut c_void) -> i32 {
    let cycle = &mut *(raw as *mut Cycle);
    cycle.presets = discover_presets(cycle.entry, cycle.clap_plugin_id);
    0
}

/// Factory presets first, then the user's, each group by name.  Empty when the
/// library offers none, cannot be loaded, or a provider faults.
pub fn list_presets(source: &PresetSource) -> Vec<PluginPreset> {
    let Ok((_library, entry)) = ClapLibrary::load(&source.library) else {
        return Vec::new();
    };
    let _one_at_a_time = DISCOVERY.lock().unwrap_or_else(|e| e.into_inner());
    let mut cycle = Cycle {
        entry,
        clap_plugin_id: &source.clap_plugin_id,
        presets: Vec::new(),
    };
    // SAFETY: `entry` belongs to `_library`, alive for the whole call, and the
    // lock above keeps providers of this process from overlapping.
    let outcome =
        unsafe { crash_guard::guarded(run_cycle, &mut cycle as *mut Cycle as *mut c_void) };
    if outcome == crash_guard::CAUGHT {
        eprintln!(
            "[clap] presets: the provider of '{}' crashed — listing none",
            source.clap_plugin_id
        );
        return Vec::new();
    }
    let mut presets: Vec<PluginPreset> = cycle.presets.into_iter().map(Into::into).collect();
    presets.sort_by_cached_key(|preset| (group_rank(preset.flags), preset.name.to_lowercase()));
    presets
}

fn group_rank(flags: u32) -> u8 {
    if flags & CLAP_PRESET_IS_FACTORY_CONTENT != 0 {
        0
    } else if flags & CLAP_PRESET_IS_USER_CONTENT != 0 {
        1
    } else {
        2
    }
}
