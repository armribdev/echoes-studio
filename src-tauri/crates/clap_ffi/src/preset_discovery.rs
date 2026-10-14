//! Host side of `clap.preset-discovery-factory`: lists the presets a plugin
//! library offers for one plugin (ADR-031).
//!
//! The factory comes from the library entry, not from a plugin instance.  The
//! host plays the indexer: a provider declares where its presets live, and the
//! host crawls those locations and asks the provider for each file's metadata.
//! Providers, indexer and receiver are single-threaded: [`discover_presets`] runs
//! the whole cycle on the calling thread and keeps nothing alive afterwards.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};

use crate::{ClapPluginEntry, ClapVersion, CLAP_VERSION_1};

pub const CLAP_PRESET_DISCOVERY_FACTORY_ID: &[u8] = b"clap.preset-discovery-factory/2\0";
pub const CLAP_PRESET_DISCOVERY_FACTORY_ID_COMPAT: &[u8] =
    b"clap.preset-discovery-factory/draft-2\0";

pub const CLAP_PRESET_LOCATION_FILE: u32 = 0;
pub const CLAP_PRESET_LOCATION_PLUGIN: u32 = 1;

pub const CLAP_PRESET_IS_FACTORY_CONTENT: u32 = 1 << 0;
pub const CLAP_PRESET_IS_USER_CONTENT: u32 = 1 << 1;
pub const CLAP_PRESET_IS_DEMO_CONTENT: u32 = 1 << 2;
pub const CLAP_PRESET_IS_FAVORITE: u32 = 1 << 3;

/// Deepest directory level crawled below a declared location.
const MAX_DEPTH: usize = 6;
/// Files offered to a provider per declared location.
const MAX_FILES_PER_LOCATION: usize = 10_000;
/// Presets kept per listing, whatever the providers report.
const MAX_PRESETS: usize = 20_000;
/// Longest label kept from a provider (names, descriptions), in characters.
const MAX_TEXT_CHARS: usize = 512;
/// Longest path or load key kept: cutting one would load another preset.
const MAX_KEY_CHARS: usize = 4096;

/// `clap_universal_plugin_id`.
#[repr(C)]
pub struct ClapUniversalPluginId {
    pub abi: *const c_char,
    pub id: *const c_char,
}

/// `clap_preset_discovery_filetype`.
#[repr(C)]
pub struct ClapPresetFiletype {
    pub name: *const c_char,
    pub description: *const c_char,
    pub file_extension: *const c_char,
}

/// `clap_preset_discovery_location`.
#[repr(C)]
pub struct ClapPresetLocation {
    pub flags: u32,
    pub name: *const c_char,
    pub kind: u32,
    pub location: *const c_char,
}

/// `clap_preset_discovery_soundpack`.
#[repr(C)]
pub struct ClapPresetSoundpack {
    pub flags: u32,
    pub id: *const c_char,
    pub name: *const c_char,
    pub description: *const c_char,
    pub homepage_url: *const c_char,
    pub vendor: *const c_char,
    pub image_path: *const c_char,
    pub release_timestamp: u64,
}

/// `clap_preset_discovery_provider_descriptor`.
#[repr(C)]
pub struct ClapPresetProviderDescriptor {
    pub clap_version: ClapVersion,
    pub id: *const c_char,
    pub name: *const c_char,
    pub vendor: *const c_char,
}

/// `clap_preset_discovery_metadata_receiver`.
#[repr(C)]
pub struct ClapPresetMetadataReceiver {
    pub receiver_data: *mut c_void,
    pub on_error: unsafe extern "C" fn(*const ClapPresetMetadataReceiver, i32, *const c_char),
    pub begin_preset: unsafe extern "C" fn(
        *const ClapPresetMetadataReceiver,
        *const c_char,
        *const c_char,
    ) -> bool,
    pub add_plugin_id:
        unsafe extern "C" fn(*const ClapPresetMetadataReceiver, *const ClapUniversalPluginId),
    pub set_soundpack_id: unsafe extern "C" fn(*const ClapPresetMetadataReceiver, *const c_char),
    pub set_flags: unsafe extern "C" fn(*const ClapPresetMetadataReceiver, u32),
    pub add_creator: unsafe extern "C" fn(*const ClapPresetMetadataReceiver, *const c_char),
    pub set_description: unsafe extern "C" fn(*const ClapPresetMetadataReceiver, *const c_char),
    pub set_timestamps: unsafe extern "C" fn(*const ClapPresetMetadataReceiver, u64, u64),
    pub add_feature: unsafe extern "C" fn(*const ClapPresetMetadataReceiver, *const c_char),
    pub add_extra_info:
        unsafe extern "C" fn(*const ClapPresetMetadataReceiver, *const c_char, *const c_char),
}

/// `clap_preset_discovery_indexer`.
#[repr(C)]
pub struct ClapPresetIndexer {
    pub clap_version: ClapVersion,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub url: *const c_char,
    pub version: *const c_char,
    pub indexer_data: *mut c_void,
    pub declare_filetype:
        unsafe extern "C" fn(*const ClapPresetIndexer, *const ClapPresetFiletype) -> bool,
    pub declare_location:
        unsafe extern "C" fn(*const ClapPresetIndexer, *const ClapPresetLocation) -> bool,
    pub declare_soundpack:
        unsafe extern "C" fn(*const ClapPresetIndexer, *const ClapPresetSoundpack) -> bool,
    pub get_extension:
        unsafe extern "C" fn(*const ClapPresetIndexer, *const c_char) -> *const c_void,
}

/// `clap_preset_discovery_provider`.
#[repr(C)]
pub struct ClapPresetProvider {
    pub desc: *const ClapPresetProviderDescriptor,
    pub provider_data: *mut c_void,
    pub init: unsafe extern "C" fn(*const ClapPresetProvider) -> bool,
    pub destroy: unsafe extern "C" fn(*const ClapPresetProvider),
    pub get_metadata: unsafe extern "C" fn(
        *const ClapPresetProvider,
        u32,
        *const c_char,
        *const ClapPresetMetadataReceiver,
    ) -> bool,
    pub get_extension:
        unsafe extern "C" fn(*const ClapPresetProvider, *const c_char) -> *const c_void,
}

/// `clap_preset_discovery_factory`.
#[repr(C)]
pub struct ClapPresetDiscoveryFactory {
    pub count: unsafe extern "C" fn(*const ClapPresetDiscoveryFactory) -> u32,
    pub get_descriptor: unsafe extern "C" fn(
        *const ClapPresetDiscoveryFactory,
        u32,
    ) -> *const ClapPresetProviderDescriptor,
    pub create: unsafe extern "C" fn(
        *const ClapPresetDiscoveryFactory,
        *const ClapPresetIndexer,
        *const c_char,
    ) -> *const ClapPresetProvider,
}

/// A preset a provider reported for the plugin being listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresetEntry {
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

struct DeclaredLocation {
    flags: u32,
    kind: u32,
    location: Option<String>,
}

struct DeclaredSoundpack {
    id: String,
    name: String,
}

/// What a provider declared while it was initialised.
#[derive(Default)]
struct Declarations {
    extensions: Vec<String>,
    locations: Vec<DeclaredLocation>,
    soundpacks: Vec<DeclaredSoundpack>,
}

/// Reads a provider's C string, cut to [`MAX_TEXT_CHARS`].  Null is `None`.
unsafe fn read_text(pointer: *const c_char) -> Option<String> {
    read_clipped(pointer, MAX_TEXT_CHARS)
}

/// Reads a provider's path or load key, cut to [`MAX_KEY_CHARS`].  Null is `None`.
unsafe fn read_key(pointer: *const c_char) -> Option<String> {
    read_clipped(pointer, MAX_KEY_CHARS)
}

unsafe fn read_clipped(pointer: *const c_char, max_chars: usize) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let text = CStr::from_ptr(pointer).to_string_lossy();
    Some(text.chars().take(max_chars).collect())
}

static INDEXER_NAME: &[u8] = b"Echoes Studio\0";

unsafe extern "C" fn indexer_declare_filetype(
    indexer: *const ClapPresetIndexer,
    filetype: *const ClapPresetFiletype,
) -> bool {
    let (Some(declarations), Some(filetype)) = (
        ((*indexer).indexer_data as *mut Declarations).as_mut(),
        filetype.as_ref(),
    ) else {
        return false;
    };
    // An empty extension matches every file.
    let extension = read_text(filetype.file_extension).unwrap_or_default();
    declarations
        .extensions
        .push(extension.trim_start_matches('.').to_ascii_lowercase());
    true
}

unsafe extern "C" fn indexer_declare_location(
    indexer: *const ClapPresetIndexer,
    location: *const ClapPresetLocation,
) -> bool {
    let (Some(declarations), Some(location)) = (
        ((*indexer).indexer_data as *mut Declarations).as_mut(),
        location.as_ref(),
    ) else {
        return false;
    };
    let is_file = location.kind == CLAP_PRESET_LOCATION_FILE;
    let is_plugin = location.kind == CLAP_PRESET_LOCATION_PLUGIN;
    let path = read_key(location.location);
    if !(is_plugin || (is_file && path.is_some())) {
        return false;
    }
    declarations.locations.push(DeclaredLocation {
        flags: location.flags,
        kind: location.kind,
        location: path,
    });
    true
}

unsafe extern "C" fn indexer_declare_soundpack(
    indexer: *const ClapPresetIndexer,
    soundpack: *const ClapPresetSoundpack,
) -> bool {
    let (Some(declarations), Some(soundpack)) = (
        ((*indexer).indexer_data as *mut Declarations).as_mut(),
        soundpack.as_ref(),
    ) else {
        return false;
    };
    let Some(id) = read_text(soundpack.id) else {
        return false;
    };
    let name = read_text(soundpack.name).unwrap_or_else(|| id.clone());
    declarations.soundpacks.push(DeclaredSoundpack { id, name });
    true
}

unsafe extern "C" fn indexer_get_extension(
    _: *const ClapPresetIndexer,
    _: *const c_char,
) -> *const c_void {
    std::ptr::null()
}

/// One call to `get_metadata`: the presets it reports, kept when they name the
/// plugin being listed.
struct Collector<'a> {
    plugin_id: &'a str,
    provider_id: &'a str,
    soundpacks: &'a [DeclaredSoundpack],
    kind: u32,
    location: Option<&'a str>,
    location_flags: u32,
    presets: &'a mut Vec<PresetEntry>,
    current: Option<(PresetEntry, bool)>,
}

impl Collector<'_> {
    fn finish_current(&mut self) {
        let Some((mut preset, names_the_plugin)) = self.current.take() else {
            return;
        };
        if !names_the_plugin {
            return;
        }
        if preset.name.is_empty() {
            preset.name = self.fallback_name(&preset);
        }
        preset.soundpack = preset.soundpack.map(|id| {
            self.soundpacks
                .iter()
                .find(|soundpack| soundpack.id == id)
                .map_or(id, |soundpack| soundpack.name.clone())
        });
        self.presets.push(preset);
    }

    /// A preset without a name takes its file's name, else its key.
    fn fallback_name(&self, preset: &PresetEntry) -> String {
        self.location
            .and_then(|path| Path::new(path).file_stem())
            .map(|stem| stem.to_string_lossy().into_owned())
            .or_else(|| preset.load_key.clone())
            .unwrap_or_default()
    }
}

unsafe fn collector<'a>(
    receiver: *const ClapPresetMetadataReceiver,
) -> Option<&'a mut Collector<'a>> {
    ((*receiver).receiver_data as *mut Collector<'a>).as_mut()
}

unsafe extern "C" fn receiver_on_error(
    _: *const ClapPresetMetadataReceiver,
    os_error: i32,
    message: *const c_char,
) {
    eprintln!(
        "[clap.preset-discovery] os error {os_error}: {}",
        read_text(message).unwrap_or_default()
    );
}

unsafe extern "C" fn receiver_begin_preset(
    receiver: *const ClapPresetMetadataReceiver,
    name: *const c_char,
    load_key: *const c_char,
) -> bool {
    let Some(collector) = collector(receiver) else {
        return false;
    };
    collector.finish_current();
    if collector.presets.len() >= MAX_PRESETS {
        return false;
    }
    collector.current = Some((
        PresetEntry {
            name: read_text(name).unwrap_or_default(),
            load_key: read_key(load_key),
            location_kind: collector.kind,
            location: collector.location.map(str::to_owned),
            provider_id: collector.provider_id.to_owned(),
            flags: collector.location_flags,
            ..PresetEntry::default()
        },
        false,
    ));
    true
}

unsafe extern "C" fn receiver_add_plugin_id(
    receiver: *const ClapPresetMetadataReceiver,
    plugin_id: *const ClapUniversalPluginId,
) {
    let (Some(collector), Some(plugin_id)) = (collector(receiver), plugin_id.as_ref()) else {
        return;
    };
    let is_clap = read_text(plugin_id.abi).is_some_and(|abi| abi.eq_ignore_ascii_case("clap"));
    let names_the_plugin =
        is_clap && read_text(plugin_id.id).as_deref() == Some(collector.plugin_id);
    if let Some((_, flagged)) = collector.current.as_mut() {
        *flagged |= names_the_plugin;
    }
}

unsafe extern "C" fn receiver_set_soundpack_id(
    receiver: *const ClapPresetMetadataReceiver,
    soundpack_id: *const c_char,
) {
    if let Some((preset, _)) = collector(receiver).and_then(|collector| collector.current.as_mut())
    {
        preset.soundpack = read_text(soundpack_id);
    }
}

unsafe extern "C" fn receiver_set_flags(receiver: *const ClapPresetMetadataReceiver, flags: u32) {
    if let Some((preset, _)) = collector(receiver).and_then(|collector| collector.current.as_mut())
    {
        preset.flags = flags;
    }
}

unsafe extern "C" fn receiver_add_creator(
    receiver: *const ClapPresetMetadataReceiver,
    creator: *const c_char,
) {
    if let Some((preset, _)) = collector(receiver).and_then(|collector| collector.current.as_mut())
    {
        preset.creator = read_text(creator);
    }
}

unsafe extern "C" fn receiver_set_description(
    receiver: *const ClapPresetMetadataReceiver,
    description: *const c_char,
) {
    if let Some((preset, _)) = collector(receiver).and_then(|collector| collector.current.as_mut())
    {
        preset.description = read_text(description);
    }
}

unsafe extern "C" fn receiver_set_timestamps(_: *const ClapPresetMetadataReceiver, _: u64, _: u64) {
}

unsafe extern "C" fn receiver_add_feature(
    receiver: *const ClapPresetMetadataReceiver,
    feature: *const c_char,
) {
    if let (Some((preset, _)), Some(feature)) = (
        collector(receiver).and_then(|collector| collector.current.as_mut()),
        read_text(feature),
    ) {
        preset.features.push(feature);
    }
}

unsafe extern "C" fn receiver_add_extra_info(
    _: *const ClapPresetMetadataReceiver,
    _: *const c_char,
    _: *const c_char,
) {
}

/// Asks the provider for the presets of one file, or of the library for a
/// `PLUGIN` location (no path).
unsafe fn read_metadata(
    provider: *const ClapPresetProvider,
    declared: &DeclaredLocation,
    path: Option<&str>,
    soundpacks: &[DeclaredSoundpack],
    plugin_id: &str,
    provider_id: &str,
    presets: &mut Vec<PresetEntry>,
) {
    let path_text = match path.map(CString::new) {
        Some(Ok(text)) => Some(text),
        Some(Err(_)) => return,
        None => None,
    };
    let mut collector = Collector {
        plugin_id,
        provider_id,
        soundpacks,
        kind: declared.kind,
        location: path,
        location_flags: declared.flags,
        presets,
        current: None,
    };
    let receiver = ClapPresetMetadataReceiver {
        receiver_data: &mut collector as *mut Collector as *mut c_void,
        on_error: receiver_on_error,
        begin_preset: receiver_begin_preset,
        add_plugin_id: receiver_add_plugin_id,
        set_soundpack_id: receiver_set_soundpack_id,
        set_flags: receiver_set_flags,
        add_creator: receiver_add_creator,
        set_description: receiver_set_description,
        set_timestamps: receiver_set_timestamps,
        add_feature: receiver_add_feature,
        add_extra_info: receiver_add_extra_info,
    };
    let location_pointer = path_text
        .as_ref()
        .map_or(std::ptr::null(), |text| text.as_ptr());
    ((*provider).get_metadata)(provider, declared.kind, location_pointer, &receiver);
    collector.finish_current();
}

/// Files below `root` that a declared extension matches, in name order, within
/// the depth and count bounds.
fn matching_files(root: &Path, extensions: &[String]) -> Vec<PathBuf> {
    fn walk(directory: &Path, depth: usize, extensions: &[String], found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            if found.len() >= MAX_FILES_PER_LOCATION {
                return;
            }
            if path.is_dir() {
                if depth < MAX_DEPTH {
                    walk(&path, depth + 1, extensions, found);
                }
            } else if matches_extension(&path, extensions) {
                found.push(path);
            }
        }
    }

    let mut found = Vec::new();
    walk(root, 0, extensions, &mut found);
    found
}

fn matches_extension(path: &Path, extensions: &[String]) -> bool {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    extensions
        .iter()
        .any(|wanted| wanted.is_empty() || *wanted == extension)
}

unsafe fn crawl_provider(
    provider: *const ClapPresetProvider,
    declarations: &Declarations,
    plugin_id: &str,
    provider_id: &str,
    presets: &mut Vec<PresetEntry>,
) {
    for declared in &declarations.locations {
        let args = (&declarations.soundpacks[..], plugin_id, provider_id);
        match (declared.kind, declared.location.as_deref()) {
            (CLAP_PRESET_LOCATION_PLUGIN, _) => {
                read_metadata(provider, declared, None, args.0, args.1, args.2, presets)
            }
            (_, Some(path)) if Path::new(path).is_dir() => {
                for file in matching_files(Path::new(path), &declarations.extensions) {
                    let file = file.to_string_lossy();
                    read_metadata(
                        provider,
                        declared,
                        Some(&file),
                        args.0,
                        args.1,
                        args.2,
                        presets,
                    );
                }
            }
            (_, Some(path)) if Path::new(path).is_file() => read_metadata(
                provider,
                declared,
                Some(path),
                args.0,
                args.1,
                args.2,
                presets,
            ),
            _ => {}
        }
    }
}

/// The presets the library offers for `plugin_id` (the CLAP plugin id): empty
/// when it has no preset-discovery factory.  Runs every provider's whole cycle
/// (create, init, crawl, destroy) on the calling thread and may take a while.
///
/// # Safety
/// `entry` must be a live, initialised `clap_entry` (see `ClapLibrary::load`),
/// and the call must not overlap another use of that library's providers.
pub unsafe fn discover_presets(entry: *const ClapPluginEntry, plugin_id: &str) -> Vec<PresetEntry> {
    let mut factory =
        ((*entry).get_factory)(CLAP_PRESET_DISCOVERY_FACTORY_ID.as_ptr() as *const c_char);
    if factory.is_null() {
        factory = ((*entry).get_factory)(
            CLAP_PRESET_DISCOVERY_FACTORY_ID_COMPAT.as_ptr() as *const c_char
        );
    }
    let factory = factory as *const ClapPresetDiscoveryFactory;
    let mut presets = Vec::new();
    if factory.is_null() {
        return presets;
    }

    for index in 0..((*factory).count)(factory) {
        let descriptor = ((*factory).get_descriptor)(factory, index);
        let Some(provider_id) = descriptor
            .as_ref()
            .and_then(|descriptor| read_text(descriptor.id))
        else {
            continue;
        };
        let Ok(provider_id_text) = CString::new(provider_id.as_str()) else {
            continue;
        };

        let mut declarations = Declarations::default();
        let indexer = ClapPresetIndexer {
            clap_version: CLAP_VERSION_1,
            name: INDEXER_NAME.as_ptr() as *const c_char,
            vendor: std::ptr::null(),
            url: std::ptr::null(),
            version: std::ptr::null(),
            indexer_data: &mut declarations as *mut Declarations as *mut c_void,
            declare_filetype: indexer_declare_filetype,
            declare_location: indexer_declare_location,
            declare_soundpack: indexer_declare_soundpack,
            get_extension: indexer_get_extension,
        };
        let provider = ((*factory).create)(factory, &indexer, provider_id_text.as_ptr());
        if provider.is_null() {
            continue;
        }
        if ((*provider).init)(provider) {
            crawl_provider(
                provider,
                &declarations,
                plugin_id,
                &provider_id,
                &mut presets,
            );
        }
        ((*provider).destroy)(provider);
    }
    presets
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::mem::offset_of;

    const PLUGIN: &str = "test.plugin";

    #[test]
    fn the_receiver_follows_the_spec_order() {
        let order = [
            offset_of!(ClapPresetMetadataReceiver, receiver_data),
            offset_of!(ClapPresetMetadataReceiver, on_error),
            offset_of!(ClapPresetMetadataReceiver, begin_preset),
            offset_of!(ClapPresetMetadataReceiver, add_plugin_id),
            offset_of!(ClapPresetMetadataReceiver, set_soundpack_id),
            offset_of!(ClapPresetMetadataReceiver, set_flags),
            offset_of!(ClapPresetMetadataReceiver, add_creator),
            offset_of!(ClapPresetMetadataReceiver, set_description),
            offset_of!(ClapPresetMetadataReceiver, set_timestamps),
            offset_of!(ClapPresetMetadataReceiver, add_feature),
            offset_of!(ClapPresetMetadataReceiver, add_extra_info),
        ];
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn the_indexer_provider_and_factory_follow_the_spec_order() {
        let indexer = [
            offset_of!(ClapPresetIndexer, clap_version),
            offset_of!(ClapPresetIndexer, name),
            offset_of!(ClapPresetIndexer, vendor),
            offset_of!(ClapPresetIndexer, url),
            offset_of!(ClapPresetIndexer, version),
            offset_of!(ClapPresetIndexer, indexer_data),
            offset_of!(ClapPresetIndexer, declare_filetype),
            offset_of!(ClapPresetIndexer, declare_location),
            offset_of!(ClapPresetIndexer, declare_soundpack),
            offset_of!(ClapPresetIndexer, get_extension),
        ];
        let provider = [
            offset_of!(ClapPresetProvider, desc),
            offset_of!(ClapPresetProvider, provider_data),
            offset_of!(ClapPresetProvider, init),
            offset_of!(ClapPresetProvider, destroy),
            offset_of!(ClapPresetProvider, get_metadata),
            offset_of!(ClapPresetProvider, get_extension),
        ];
        let factory = [
            offset_of!(ClapPresetDiscoveryFactory, count),
            offset_of!(ClapPresetDiscoveryFactory, get_descriptor),
            offset_of!(ClapPresetDiscoveryFactory, create),
        ];
        for order in [&indexer[..], &provider[..], &factory[..]] {
            assert!(order.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }

    #[test]
    fn the_declaration_structs_follow_the_spec() {
        assert_eq!(offset_of!(ClapPresetLocation, flags), 0);
        assert!(offset_of!(ClapPresetLocation, name) < offset_of!(ClapPresetLocation, kind));
        assert!(offset_of!(ClapPresetLocation, kind) < offset_of!(ClapPresetLocation, location));
        assert!(
            offset_of!(ClapPresetFiletype, name) < offset_of!(ClapPresetFiletype, description)
                && offset_of!(ClapPresetFiletype, description)
                    < offset_of!(ClapPresetFiletype, file_extension)
        );
        assert!(
            offset_of!(ClapPresetSoundpack, image_path)
                < offset_of!(ClapPresetSoundpack, release_timestamp)
        );
        assert!(
            offset_of!(ClapPresetProviderDescriptor, clap_version)
                < offset_of!(ClapPresetProviderDescriptor, id)
        );
    }

    // A provider written the way a plugin would, driven by thread-locals so each
    // test (one thread) picks its own directory and behaviour.
    thread_local! {
        static DIRECTORY: RefCell<String> = const { RefCell::new(String::new()) };
        static INIT_FAILS: Cell<bool> = const { Cell::new(false) };
        static ONLY_COMPAT_ID: Cell<bool> = const { Cell::new(false) };
        static NO_FACTORY: Cell<bool> = const { Cell::new(false) };
        static METADATA_CALLS: Cell<u32> = const { Cell::new(0) };
    }

    static PROVIDER_ID: &[u8] = b"test.provider\0";
    static FILETYPE_NAME: &[u8] = b"Test preset\0";
    static FILETYPE_EXTENSION: &[u8] = b".TPRESET\0";
    static LOCATION_NAME: &[u8] = b"Bundled\0";
    static USER_LOCATION_NAME: &[u8] = b"User\0";
    static SOUNDPACK_ID: &[u8] = b"pack.one\0";
    static SOUNDPACK_NAME: &[u8] = b"Pack One\0";

    static DESCRIPTOR: ClapPresetProviderDescriptor = ClapPresetProviderDescriptor {
        clap_version: CLAP_VERSION_1,
        id: PROVIDER_ID.as_ptr() as *const c_char,
        name: PROVIDER_ID.as_ptr() as *const c_char,
        vendor: std::ptr::null(),
    };

    static PROVIDER: ClapPresetProvider = ClapPresetProvider {
        desc: &DESCRIPTOR,
        provider_data: std::ptr::null_mut(),
        init: provider_init,
        destroy: provider_destroy,
        get_metadata: provider_get_metadata,
        get_extension: provider_get_extension,
    };

    // SAFETY: the statics hold pointers to immutable literals only.
    unsafe impl Sync for ClapPresetProvider {}
    unsafe impl Sync for ClapPresetProviderDescriptor {}

    static FACTORY: ClapPresetDiscoveryFactory = ClapPresetDiscoveryFactory {
        count: factory_count,
        get_descriptor: factory_get_descriptor,
        create: factory_create,
    };

    unsafe extern "C" fn factory_count(_: *const ClapPresetDiscoveryFactory) -> u32 {
        1
    }

    unsafe extern "C" fn factory_get_descriptor(
        _: *const ClapPresetDiscoveryFactory,
        _: u32,
    ) -> *const ClapPresetProviderDescriptor {
        &DESCRIPTOR
    }

    unsafe extern "C" fn factory_create(
        _: *const ClapPresetDiscoveryFactory,
        indexer: *const ClapPresetIndexer,
        _: *const c_char,
    ) -> *const ClapPresetProvider {
        // The provider keeps the indexer, as plugins do; it declares in `init`.
        INDEXER.with(|slot| slot.set(indexer));
        &PROVIDER
    }

    thread_local! {
        static INDEXER: Cell<*const ClapPresetIndexer> = const { Cell::new(std::ptr::null()) };
    }

    unsafe extern "C" fn provider_init(_: *const ClapPresetProvider) -> bool {
        if INIT_FAILS.get() {
            return false;
        }
        let indexer = &*INDEXER.get();
        let filetype = ClapPresetFiletype {
            name: FILETYPE_NAME.as_ptr() as *const c_char,
            description: std::ptr::null(),
            file_extension: FILETYPE_EXTENSION.as_ptr() as *const c_char,
        };
        assert!((indexer.declare_filetype)(indexer, &filetype));

        let bundled = ClapPresetLocation {
            flags: CLAP_PRESET_IS_FACTORY_CONTENT,
            name: LOCATION_NAME.as_ptr() as *const c_char,
            kind: CLAP_PRESET_LOCATION_PLUGIN,
            location: std::ptr::null(),
        };
        assert!((indexer.declare_location)(indexer, &bundled));

        let directory =
            CString::new(DIRECTORY.with(|directory| directory.borrow().clone())).unwrap();
        let user = ClapPresetLocation {
            flags: CLAP_PRESET_IS_USER_CONTENT,
            name: USER_LOCATION_NAME.as_ptr() as *const c_char,
            kind: CLAP_PRESET_LOCATION_FILE,
            location: directory.as_ptr(),
        };
        assert!((indexer.declare_location)(indexer, &user));

        let invalid = ClapPresetLocation {
            flags: 0,
            name: std::ptr::null(),
            kind: CLAP_PRESET_LOCATION_FILE,
            location: std::ptr::null(),
        };
        assert!(
            !(indexer.declare_location)(indexer, &invalid),
            "a file location needs a path"
        );

        let soundpack = ClapPresetSoundpack {
            flags: 0,
            id: SOUNDPACK_ID.as_ptr() as *const c_char,
            name: SOUNDPACK_NAME.as_ptr() as *const c_char,
            description: std::ptr::null(),
            homepage_url: std::ptr::null(),
            vendor: std::ptr::null(),
            image_path: std::ptr::null(),
            release_timestamp: 0,
        };
        assert!((indexer.declare_soundpack)(indexer, &soundpack));
        true
    }

    unsafe extern "C" fn provider_destroy(_: *const ClapPresetProvider) {}

    unsafe extern "C" fn provider_get_extension(
        _: *const ClapPresetProvider,
        _: *const c_char,
    ) -> *const c_void {
        std::ptr::null()
    }

    unsafe fn name_plugin(receiver: *const ClapPresetMetadataReceiver, plugin: &CStr) {
        let id = ClapUniversalPluginId {
            abi: c"clap".as_ptr(),
            id: plugin.as_ptr(),
        };
        ((*receiver).add_plugin_id)(receiver, &id);
    }

    unsafe extern "C" fn provider_get_metadata(
        _: *const ClapPresetProvider,
        kind: u32,
        location: *const c_char,
        receiver: *const ClapPresetMetadataReceiver,
    ) -> bool {
        METADATA_CALLS.set(METADATA_CALLS.get() + 1);
        let r = &*receiver;
        let this_plugin = c"test.plugin";
        if kind == CLAP_PRESET_LOCATION_PLUGIN {
            assert!(location.is_null());
            assert!((r.begin_preset)(
                receiver,
                c"Bundled A".as_ptr(),
                c"a".as_ptr()
            ));
            name_plugin(receiver, this_plugin);
            (r.set_soundpack_id)(receiver, SOUNDPACK_ID.as_ptr() as *const c_char);
            (r.begin_preset)(receiver, c"For another plugin".as_ptr(), c"b".as_ptr());
            name_plugin(receiver, c"someone.else");
            (r.begin_preset)(receiver, c"Names nobody".as_ptr(), c"c".as_ptr());
            return true;
        }
        let path = CStr::from_ptr(location).to_string_lossy().to_string();
        if path.ends_with("bank.tpreset") {
            (r.begin_preset)(receiver, c"First".as_ptr(), c"1".as_ptr());
            name_plugin(receiver, this_plugin);
            (r.begin_preset)(receiver, c"Second".as_ptr(), c"2".as_ptr());
            name_plugin(receiver, this_plugin);
            (r.set_flags)(receiver, CLAP_PRESET_IS_FAVORITE);
            return true;
        }
        (r.begin_preset)(receiver, std::ptr::null(), std::ptr::null());
        name_plugin(receiver, this_plugin);
        (r.add_creator)(receiver, c"Someone".as_ptr());
        (r.set_description)(receiver, c"Warm and round".as_ptr());
        (r.add_feature)(receiver, c"pad".as_ptr());
        (r.add_feature)(receiver, c"warm".as_ptr());
        (r.set_timestamps)(receiver, 0, 0);
        (r.add_extra_info)(receiver, c"k".as_ptr(), c"v".as_ptr());
        true
    }

    unsafe extern "C" fn entry_init(_: *const c_char) -> bool {
        true
    }

    unsafe extern "C" fn entry_deinit() {}

    unsafe extern "C" fn entry_get_factory(id: *const c_char) -> *const c_void {
        let id = CStr::from_ptr(id).to_bytes_with_nul();
        let current = id == CLAP_PRESET_DISCOVERY_FACTORY_ID && !ONLY_COMPAT_ID.get();
        let compat = id == CLAP_PRESET_DISCOVERY_FACTORY_ID_COMPAT;
        if NO_FACTORY.get() || !(current || compat) {
            return std::ptr::null();
        }
        &FACTORY as *const ClapPresetDiscoveryFactory as *const c_void
    }

    static ENTRY: ClapPluginEntry = ClapPluginEntry {
        clap_version: CLAP_VERSION_1,
        init: entry_init,
        deinit: entry_deinit,
        get_factory: entry_get_factory,
    };

    /// A scratch directory removed when dropped.
    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("clap_ffi_presets_{}_{unique}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn directory_with(files: &[&str]) -> TestDirectory {
        let directory = TestDirectory::new();
        for file in files {
            let path = directory.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        DIRECTORY.with(|slot| *slot.borrow_mut() = directory.path().to_string_lossy().into_owned());
        directory
    }

    fn discover() -> Vec<PresetEntry> {
        unsafe { discover_presets(&ENTRY, PLUGIN) }
    }

    fn names(presets: &[PresetEntry]) -> Vec<&str> {
        presets.iter().map(|preset| preset.name.as_str()).collect()
    }

    #[test]
    fn only_the_presets_that_name_the_plugin_are_listed() {
        let _directory = directory_with(&[]);
        let presets = discover();

        assert_eq!(names(&presets), ["Bundled A"]);
        assert_eq!(presets[0].load_key.as_deref(), Some("a"));
        assert_eq!(presets[0].location_kind, CLAP_PRESET_LOCATION_PLUGIN);
        assert_eq!(presets[0].location, None);
        assert_eq!(presets[0].provider_id, "test.provider");
        assert_eq!(presets[0].flags, CLAP_PRESET_IS_FACTORY_CONTENT);
        assert_eq!(presets[0].soundpack.as_deref(), Some("Pack One"));
    }

    #[test]
    fn a_directory_is_crawled_by_extension_in_name_order_below_the_top_level() {
        let _directory = directory_with(&[
            "warm.tpreset",
            "notes.txt",
            "LOUD.TPRESET",
            "nested/deeper/deep.tpreset",
        ]);
        let presets = discover();

        assert_eq!(names(&presets), ["Bundled A", "LOUD", "deep", "warm"]);
        let warm = presets.last().unwrap();
        assert_eq!(warm.location_kind, CLAP_PRESET_LOCATION_FILE);
        assert!(warm.location.as_deref().unwrap().ends_with("warm.tpreset"));
        assert_eq!(warm.flags, CLAP_PRESET_IS_USER_CONTENT);
        assert_eq!(warm.load_key, None);
        assert_eq!(warm.creator.as_deref(), Some("Someone"));
        assert_eq!(warm.description.as_deref(), Some("Warm and round"));
        assert_eq!(warm.features, ["pad", "warm"]);
    }

    #[test]
    fn a_file_can_hold_several_presets_and_set_its_own_flags() {
        let _directory = directory_with(&["bank.tpreset"]);
        let presets = discover();

        assert_eq!(names(&presets), ["Bundled A", "First", "Second"]);
        assert_eq!(presets[1].flags, CLAP_PRESET_IS_USER_CONTENT);
        assert_eq!(presets[2].flags, CLAP_PRESET_IS_FAVORITE);
        assert_eq!(presets[1].load_key.as_deref(), Some("1"));
        assert_eq!(presets[2].load_key.as_deref(), Some("2"));
        assert_eq!(presets[1].location, presets[2].location);
    }

    #[test]
    fn a_missing_directory_lists_what_the_other_locations_hold() {
        let directory = directory_with(&[]);
        DIRECTORY.with(|slot| {
            *slot.borrow_mut() = directory.path().join("gone").to_string_lossy().into_owned()
        });
        METADATA_CALLS.set(0);

        assert_eq!(names(&discover()), ["Bundled A"]);
        assert_eq!(
            METADATA_CALLS.get(),
            1,
            "the provider is not asked about a path that does not exist"
        );
    }

    #[test]
    fn a_library_without_the_factory_lists_nothing() {
        let _directory = directory_with(&["warm.tpreset"]);
        NO_FACTORY.set(true);

        assert!(discover().is_empty());
        NO_FACTORY.set(false);
    }

    #[test]
    fn the_compat_factory_id_is_used_when_the_current_one_is_missing() {
        let _directory = directory_with(&[]);
        ONLY_COMPAT_ID.set(true);

        assert_eq!(names(&discover()), ["Bundled A"]);
        ONLY_COMPAT_ID.set(false);
    }

    #[test]
    fn a_provider_that_fails_to_initialise_lists_nothing() {
        let _directory = directory_with(&["warm.tpreset"]);
        INIT_FAILS.set(true);

        assert!(discover().is_empty());
        INIT_FAILS.set(false);
    }

    #[test]
    fn the_file_bounds_stop_a_crawl_that_never_ends() {
        let directory = TestDirectory::new();
        let mut deep = directory.path().to_path_buf();
        for level in 0..=MAX_DEPTH + 1 {
            deep = deep.join(format!("level{level}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("too_deep.tpreset"), b"x").unwrap();
        std::fs::write(directory.path().join("top.tpreset"), b"x").unwrap();

        let found = matching_files(directory.path(), &["tpreset".to_owned()]);

        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("top.tpreset"));
    }

    #[test]
    fn labels_are_cut_but_paths_and_keys_are_not() {
        let long = CString::new("x".repeat(MAX_TEXT_CHARS * 2)).unwrap();
        unsafe {
            assert_eq!(read_text(long.as_ptr()).unwrap().len(), MAX_TEXT_CHARS);
            assert_eq!(read_key(long.as_ptr()).unwrap().len(), MAX_TEXT_CHARS * 2);
            assert_eq!(read_key(std::ptr::null()), None);
        }
    }

    #[test]
    fn an_empty_extension_matches_every_file_and_none_matches_nothing() {
        let path = Path::new("anything.xyz");
        assert!(matches_extension(path, &[String::new()]));
        assert!(matches_extension(path, &["XYZ".to_ascii_lowercase()]));
        assert!(!matches_extension(path, &["abc".to_owned()]));
        assert!(!matches_extension(path, &[]));
    }
}
