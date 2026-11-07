//! Preset listing (ADR-031): CLAP presets, and the programs of VST3 plugins.
//!
//! A CLAP library's preset providers are third-party code that crawls the user's
//! files, so the whole cycle runs on a worker thread, outside the engine lock,
//! behind the same SEH guard as the scan's probes.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::plugin_formats::clap::ffi::{
    discover_presets, ClapLibrary, ClapPluginEntry, PresetEntry, PresetRequest,
    CLAP_PRESET_IS_FACTORY_CONTENT, CLAP_PRESET_IS_USER_CONTENT, CLAP_PRESET_LOCATION_FILE,
    CLAP_PRESET_LOCATION_PLUGIN,
};
use crate::plugin_formats::crash_guard;
use crate::plugin_formats::vst3::host::decode_plugin_id;
use crate::plugin_formats::vst3::preset_file::class_id_text;
use crate::plugin_formats::vst3::preset_locations::{
    list_preset_files, preset_folders, PresetRoots,
};
use crate::plugin_formats::vst3::units::{program_key, ProgramList};
use crate::plugin_system::instance::PluginInstance;

/// Where to ask for the presets of one hosted plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetSource {
    pub library: PathBuf,
    pub clap_plugin_id: String,
}

/// Whose a preset is, which decides the group it is shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetOrigin {
    Factory,
    User,
}

impl PresetOrigin {
    fn flags(self) -> u32 {
        match self {
            Self::Factory => CLAP_PRESET_IS_FACTORY_CONTENT,
            Self::User => CLAP_PRESET_IS_USER_CONTENT,
        }
    }
}

/// The `.vstpreset` files a VST3 plugin may have on disk.  The folders are only
/// looked at when listing, on the worker thread.
pub struct Vst3PresetFiles {
    plugin_name: String,
    class_id: String,
    roots: Option<PresetRoots>,
}

impl Vst3PresetFiles {
    /// `None` when the plugin id is not a VST3 class id.
    pub fn new(plugin_name: &str, plugin_id: &str) -> Option<Self> {
        let class_id = decode_plugin_id(plugin_id)?;
        Some(Self {
            plugin_name: plugin_name.to_owned(),
            class_id: class_id_text(&class_id),
            roots: None,
        })
    }

    /// Looks under these roots instead of the machine's own.
    #[cfg(test)]
    pub fn under(mut self, roots: PresetRoots) -> Self {
        self.roots = Some(roots);
        self
    }

    fn list(&self) -> Vec<PluginPreset> {
        let roots = self
            .roots
            .clone()
            .unwrap_or_else(PresetRoots::of_this_machine);
        list_preset_files(&preset_folders(&roots, &self.plugin_name), &self.class_id)
    }
}

/// Where the presets of one placement come from.
pub enum PresetListing {
    /// `loaded` is the preset the plugin last reported through `clap.preset-load`.
    Clap {
        source: PresetSource,
        loaded: Option<PresetRequest>,
    },
    Vst3 {
        instance: Arc<PluginInstance>,
        files: Option<Vst3PresetFiles>,
    },
}

impl PresetListing {
    /// Runs the listing; meant for a worker thread, outside the engine lock.  A
    /// VST3 plugin's own programs come first within their group, then its files.
    pub fn list(&self) -> Vec<PluginPreset> {
        match self {
            Self::Clap { source, loaded } => {
                let mut presets = list_presets(source);
                if let Some(loaded) = loaded {
                    mark_active(&mut presets, loaded);
                }
                presets
            }
            Self::Vst3 { instance, files } => {
                let mut presets = instance.vst3_program_presets();
                presets.extend(files.iter().flat_map(Vst3PresetFiles::list));
                presets.sort_by_key(|preset| group_rank(preset.flags));
                presets
            }
        }
    }

    /// What the host's list of offered presets is kept under.
    pub fn whitelist_key(&self) -> String {
        match self {
            Self::Clap { source, .. } => source.clap_plugin_id.clone(),
            Self::Vst3 { instance, .. } => instance.device_id.clone(),
        }
    }
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
    /// The plugin says this is the preset in use; only a VST3 program can tell.
    pub active: bool,
}

impl PluginPreset {
    /// A `.vstpreset` file the host found: it is loaded by its path.
    pub fn vst3_file(name: String, path: String, origin: PresetOrigin) -> Self {
        Self {
            name,
            load_key: None,
            location_kind: CLAP_PRESET_LOCATION_FILE,
            location: Some(path),
            provider_id: "vst3:file".into(),
            flags: origin.flags(),
            creator: None,
            description: None,
            features: Vec::new(),
            soundpack: None,
            active: false,
        }
    }

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
            active: false,
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

const MAX_STORED_TEXT_CHARS: usize = 4096;

/// The preset a placement was last known to use, as the project file keeps it
/// (ADR-035).  A hint compared with the plugin's listing, never loaded from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredPreset {
    pub device_id: String,
    pub location_kind: u32,
    pub location: Option<String>,
    pub load_key: Option<String>,
}

impl StoredPreset {
    pub fn of(device_id: &str, request: &PresetRequest) -> Self {
        Self {
            device_id: device_id.to_owned(),
            location_kind: request.location_kind,
            location: request.location.clone(),
            load_key: request.load_key.clone(),
        }
    }

    pub fn request(&self) -> PresetRequest {
        PresetRequest {
            location_kind: self.location_kind,
            location: self.location.clone(),
            load_key: self.load_key.clone(),
        }
    }

    /// A project file is user data: texts past the bound are not trusted.
    pub fn is_within_bounds(&self) -> bool {
        [&self.location, &self.load_key]
            .into_iter()
            .flatten()
            .all(|text| text.chars().count() <= MAX_STORED_TEXT_CHARS)
    }
}

/// Marks the presets the plugin reported as loaded.
fn mark_active(presets: &mut [PluginPreset], loaded: &PresetRequest) {
    for preset in presets {
        preset.active = same_preset(&preset.request(), loaded);
    }
}

/// A plugin may spell a file's path differently in its report and in its
/// listing (separators, and case on Windows); the key must match exactly.
fn same_preset(a: &PresetRequest, b: &PresetRequest) -> bool {
    let path = |location: &Option<String>| {
        location.as_deref().map(|text| {
            let text = text.replace('\\', "/");
            let text = text.trim_end_matches('/');
            if cfg!(windows) {
                text.to_lowercase()
            } else {
                text.to_owned()
            }
        })
    };
    a.location_kind == b.location_kind
        && a.load_key == b.load_key
        && path(&a.location) == path(&b.location)
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

/// The programs of the lists a host can select, in the plugin's own order, as
/// factory presets.  With several lists each name starts with its list's name.
pub fn programs_as_presets(lists: &[ProgramList]) -> Vec<PluginPreset> {
    let selectable: Vec<_> = lists
        .iter()
        .filter(|list| !list.programs.is_empty())
        .filter_map(|list| Some((list, list.program_change_param?)))
        .collect();
    let several = selectable.len() > 1;
    selectable
        .into_iter()
        .flat_map(|(list, param_id)| {
            list.programs
                .iter()
                .enumerate()
                .map(move |(index, program)| {
                    let program = if program.is_empty() {
                        format!("Program {}", index + 1)
                    } else {
                        program.clone()
                    };
                    let name = if several && !list.name.is_empty() {
                        format!("{} / {program}", list.name)
                    } else {
                        program
                    };
                    PluginPreset {
                        name,
                        load_key: Some(program_key(param_id, index)),
                        location_kind: CLAP_PRESET_LOCATION_PLUGIN,
                        location: None,
                        provider_id: "vst3:program".into(),
                        flags: CLAP_PRESET_IS_FACTORY_CONTENT,
                        creator: None,
                        description: None,
                        features: Vec::new(),
                        soundpack: None,
                        active: list.current_program == Some(index),
                    }
                })
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn list(id: i32, name: &str, selector: Option<u32>, programs: &[&str]) -> ProgramList {
        ProgramList {
            id,
            name: name.into(),
            program_change_param: selector,
            programs: programs.iter().map(|program| program.to_string()).collect(),
            current_program: None,
        }
    }

    fn names(presets: &[PluginPreset]) -> Vec<&str> {
        presets.iter().map(|preset| preset.name.as_str()).collect()
    }

    #[test]
    fn no_lists_no_presets() {
        assert!(programs_as_presets(&[]).is_empty());
    }

    #[test]
    fn a_list_is_offered_in_the_plugins_order_as_factory_presets() {
        let presets = programs_as_presets(&[list(1, "Factory", Some(5), &["Zebra", "Apple"])]);
        assert_eq!(names(&presets), ["Zebra", "Apple"]);
        assert!(presets
            .iter()
            .all(|preset| preset.flags == CLAP_PRESET_IS_FACTORY_CONTENT));
        assert_eq!(presets[1].load_key.as_deref(), Some("5:1"));
        assert_eq!(presets[1].provider_id, "vst3:program");
    }

    #[test]
    fn a_list_the_host_cannot_select_or_without_programs_is_not_offered() {
        let presets = programs_as_presets(&[
            list(1, "No selector", None, &["a"]),
            list(2, "Empty", Some(6), &[]),
        ]);
        assert!(presets.is_empty());
    }

    #[test]
    fn a_program_without_a_name_is_numbered() {
        let presets = programs_as_presets(&[list(1, "F", Some(5), &["First", "", "Third"])]);
        assert_eq!(names(&presets), ["First", "Program 2", "Third"]);
    }

    #[test]
    fn several_lists_are_told_apart_by_their_names() {
        let presets = programs_as_presets(&[
            list(1, "Pads", Some(5), &["Warm"]),
            list(2, "", Some(6), &["Bass"]),
            list(3, "No selector", None, &["Ignored"]),
        ]);
        assert_eq!(names(&presets), ["Pads / Warm", "Bass"]);
        assert_eq!(presets[0].load_key.as_deref(), Some("5:0"));
        assert_eq!(presets[1].load_key.as_deref(), Some("6:0"));
    }

    #[test]
    fn the_preset_the_plugin_reported_is_the_only_active_one() {
        let origin = PresetOrigin::Factory;
        let mut presets = vec![
            PluginPreset::vst3_file("Warm".into(), "/p/warm".into(), origin),
            PluginPreset::vst3_file("Cold".into(), "/p/cold".into(), origin),
        ];
        let loaded = presets[1].request();

        mark_active(&mut presets, &loaded);

        let active: Vec<_> = presets.iter().map(|preset| preset.active).collect();
        assert_eq!(active, [false, true]);
    }

    #[test]
    fn a_path_spelled_differently_is_still_the_same_preset() {
        let listed = PluginPreset::vst3_file(
            "Warm".into(),
            r"C:\Presets\Warm.fxp".into(),
            PresetOrigin::Factory,
        );
        let reported = PresetRequest {
            location: Some("c:/presets/warm.fxp".into()),
            ..listed.request()
        };
        let other_key = PresetRequest {
            load_key: Some("b".into()),
            ..reported.clone()
        };

        assert_eq!(same_preset(&listed.request(), &reported), cfg!(windows));
        assert!(!same_preset(&listed.request(), &other_key));
    }

    #[test]
    fn only_the_current_program_is_active() {
        let mut pads = list(1, "Pads", Some(5), &["Warm", "Cold", "Wide"]);
        pads.current_program = Some(1);
        let active: Vec<_> = programs_as_presets(&[pads])
            .into_iter()
            .map(|preset| preset.active)
            .collect();
        assert_eq!(active, [false, true, false]);

        let unknown = list(1, "Pads", Some(5), &["Warm", "Cold"]);
        assert!(programs_as_presets(&[unknown])
            .iter()
            .all(|preset| !preset.active));
    }
}
