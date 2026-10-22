//! Where `.vstpreset` files of a plugin live, and listing them (ADR-031 amendment).
//!
//! Steinberg's convention: `<root>\VST3 Presets\<vendor>\<plugin name>\…`, with
//! the roots `Documents` (the user's own presets), `%APPDATA%` and
//! `%PROGRAMDATA%` (presets that ship with the plugin).  The scan does not know
//! the vendor folder, so every vendor folder is looked into for one named like the
//! plugin.  Files come from the user's disk: the walk is bounded in depth and
//! number, links are not followed, and only the 48-byte header of a file is read
//! to see whether it belongs to the plugin.

use std::path::{Path, PathBuf};

use super::preset_file::is_preset_for;
use crate::plugin_system::presets::{PluginPreset, PresetOrigin};

const PRESETS_FOLDER: &str = "VST3 Presets";
const EXTENSION: &str = "vstpreset";

const MAX_DEPTH: usize = 6;
const MAX_VENDOR_FOLDERS: usize = 1_000;
const MAX_ENTRIES_PER_FOLDER: usize = 10_000;
const MAX_PRESETS: usize = 20_000;

/// A folder that holds the presets of one plugin, and whose they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetFolder {
    pub path: PathBuf,
    pub origin: PresetOrigin,
}

/// The roots a plugin's presets may sit under, as the host found them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresetRoots {
    pub documents: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
    pub program_data: Option<PathBuf>,
}

impl PresetRoots {
    /// The Windows folders of the current user; `Documents` may have been moved
    /// (OneDrive), so it comes from the shell, not from the profile path.
    pub fn of_this_machine() -> Self {
        Self {
            documents: documents_folder(),
            app_data: std::env::var_os("APPDATA").map(PathBuf::from),
            program_data: std::env::var_os("PROGRAMDATA").map(PathBuf::from),
        }
    }

    fn entries(&self) -> [(&Option<PathBuf>, PresetOrigin); 3] {
        [
            (&self.documents, PresetOrigin::User),
            (&self.app_data, PresetOrigin::Factory),
            (&self.program_data, PresetOrigin::Factory),
        ]
    }
}

#[cfg(windows)]
fn documents_folder() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};

    let mut raw: *mut u16 = std::ptr::null_mut();
    // SAFETY: `raw` receives a NUL-terminated string that the shell allocates and
    // this function frees, whatever the result.
    unsafe {
        let result = SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut raw);
        let path = (result >= 0 && !raw.is_null()).then(|| {
            let length = (0..).take_while(|&i| *raw.add(i) != 0).count();
            PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(
                raw, length,
            )))
        });
        CoTaskMemFree(raw.cast());
        path
    }
}

#[cfg(not(windows))]
fn documents_folder() -> Option<PathBuf> {
    None
}

/// A plugin name as a folder name: the characters Windows forbids in one become `_`.
fn folder_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if "\\/:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// The folders named like the plugin under each vendor folder of each root.
pub fn preset_folders(roots: &PresetRoots, plugin_name: &str) -> Vec<PresetFolder> {
    let wanted = folder_name(plugin_name.trim());
    if wanted.is_empty() || wanted == "." || wanted == ".." {
        return Vec::new();
    }
    let mut folders = Vec::new();
    for (root, origin) in roots.entries() {
        let Some(root) = root else { continue };
        for vendor in sub_folders(&root.join(PRESETS_FOLDER), MAX_VENDOR_FOLDERS) {
            for folder in sub_folders(&vendor, MAX_ENTRIES_PER_FOLDER) {
                let is_plugin = folder
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case(&wanted));
                if is_plugin {
                    folders.push(PresetFolder {
                        path: folder,
                        origin,
                    });
                }
            }
        }
    }
    folders
}

/// The folders inside `folder`, by name; links are not followed.
fn sub_folders(folder: &Path, limit: usize) -> Vec<PathBuf> {
    let mut found = entries(folder, limit, |kind| kind.is_dir());
    found.sort();
    found
}

fn entries(
    folder: &Path,
    limit: usize,
    wanted: impl Fn(&std::fs::FileType) -> bool,
) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    read.flatten()
        .take(limit)
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|kind| !kind.is_symlink() && wanted(&kind))
        })
        .map(|entry| entry.path())
        .collect()
}

/// The `.vstpreset` files of these folders that belong to the plugin with this
/// `class_id` (see `preset_file::class_id_text`), by name within each origin.
pub fn list_preset_files(folders: &[PresetFolder], class_id: &str) -> Vec<PluginPreset> {
    let mut presets = Vec::new();
    for folder in folders {
        let mut pending = vec![(folder.path.clone(), 0)];
        while let Some((directory, depth)) = pending.pop() {
            for file in entries(&directory, MAX_ENTRIES_PER_FOLDER, |kind| kind.is_file()) {
                if presets.len() >= MAX_PRESETS {
                    return presets;
                }
                if let Some(preset) = file_preset(&file, folder.origin, class_id) {
                    presets.push(preset);
                }
            }
            if depth < MAX_DEPTH {
                pending.extend(
                    sub_folders(&directory, MAX_ENTRIES_PER_FOLDER)
                        .into_iter()
                        .map(|sub| (sub, depth + 1)),
                );
            }
        }
    }
    presets.sort_by_cached_key(|preset| (preset.name.to_lowercase(), preset.location.clone()));
    presets
}

fn file_preset(file: &Path, origin: PresetOrigin, class_id: &str) -> Option<PluginPreset> {
    let has_extension = file
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION));
    // A path the UI could not send back unchanged is not offered.
    let location = file.to_str()?;
    if !has_extension || !is_preset_for(file, class_id) {
        return None;
    }
    let name = file.file_stem()?.to_str()?.to_owned();
    Some(PluginPreset::vst3_file(name, location.to_owned(), origin))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_formats::vst3::preset_file::tests::preset_file;

    const CLASS: &str = "E831FF31F2D54301928EBBEE25697802";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "echoes-vst3-locations-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            path
        }

        fn preset(&self, relative: &str, class_id: &str) -> PathBuf {
            self.write(relative, &preset_file(class_id, &[(b"Comp", b"state")]))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(presets: &[PluginPreset]) -> Vec<&str> {
        presets.iter().map(|preset| preset.name.as_str()).collect()
    }

    fn folder(scratch: &Scratch, relative: &str, origin: PresetOrigin) -> PresetFolder {
        PresetFolder {
            path: scratch.0.join(relative),
            origin,
        }
    }

    #[test]
    fn a_name_is_made_a_valid_folder_name() {
        assert_eq!(folder_name("A/B:C*D?\"E<F>G|H\\I"), "A_B_C_D__E_F_G_H_I");
        assert_eq!(folder_name("TDR Nova"), "TDR Nova");
    }

    #[test]
    fn folders_are_found_under_every_vendor_of_every_root() {
        let scratch = Scratch::new("folders");
        scratch.preset(
            "docs/VST3 Presets/Tokyo Dawn Labs/TDR Nova/a.vstpreset",
            CLASS,
        );
        scratch.preset("data/VST3 Presets/Other Vendor/tdr nova/b.vstpreset", CLASS);
        scratch.preset(
            "data/VST3 Presets/Tokyo Dawn Labs/Another/c.vstpreset",
            CLASS,
        );
        scratch.preset("programdata/VST3 Presets/V/TDR Nova/d.vstpreset", CLASS);
        let roots = PresetRoots {
            documents: Some(scratch.0.join("docs")),
            app_data: Some(scratch.0.join("data")),
            program_data: Some(scratch.0.join("programdata")),
        };

        let folders = preset_folders(&roots, "TDR Nova");

        let origins: Vec<_> = folders.iter().map(|folder| folder.origin).collect();
        assert_eq!(
            origins,
            [
                PresetOrigin::User,
                PresetOrigin::Factory,
                PresetOrigin::Factory
            ]
        );
        assert!(folders[0].path.ends_with("Tokyo Dawn Labs\\TDR Nova"));
    }

    #[test]
    fn missing_roots_and_unusable_names_give_no_folder() {
        assert!(preset_folders(&PresetRoots::default(), "TDR Nova").is_empty());
        let scratch = Scratch::new("names");
        scratch.preset("docs/VST3 Presets/V/.vstpreset", CLASS);
        let roots = PresetRoots {
            documents: Some(scratch.0.join("docs")),
            ..PresetRoots::default()
        };
        for name in ["", "  ", ".", ".."] {
            assert!(preset_folders(&roots, name).is_empty(), "{name:?}");
        }
    }

    #[test]
    fn only_the_files_of_the_plugin_are_listed() {
        let scratch = Scratch::new("filter");
        scratch.preset("p/Warm.vstpreset", CLASS);
        scratch.preset("p/Other plugin.vstpreset", &"0".repeat(32));
        scratch.write("p/Garbage.vstpreset", b"not a preset");
        scratch.write("p/notes.txt", b"hello");
        scratch.preset("p/No extension", CLASS);

        let presets = list_preset_files(&[folder(&scratch, "p", PresetOrigin::User)], CLASS);

        assert_eq!(names(&presets), ["Warm"]);
        assert_eq!(presets[0].location_kind, 0);
        assert_eq!(presets[0].load_key, None);
        assert_eq!(
            presets[0].location.as_deref().map(Path::new),
            Some(scratch.0.join("p").join("Warm.vstpreset").as_path())
        );
    }

    #[test]
    fn the_extension_is_case_insensitive_and_names_drop_it() {
        let scratch = Scratch::new("case");
        scratch.preset("p/Loud.VSTPRESET", CLASS);
        scratch.preset("p/with.dots.in.name.vstpreset", CLASS);

        let presets = list_preset_files(&[folder(&scratch, "p", PresetOrigin::User)], CLASS);

        assert_eq!(names(&presets), ["Loud", "with.dots.in.name"]);
    }

    #[test]
    fn nested_folders_are_walked_down_to_a_bound() {
        let scratch = Scratch::new("depth");
        scratch.preset("p/Bass/Sub/deep.vstpreset", CLASS);
        let mut relative = String::from("p");
        for level in 0..=MAX_DEPTH {
            relative.push_str(&format!("/d{level}"));
            scratch.preset(&format!("{relative}/level{level}.vstpreset"), CLASS);
        }

        let presets = list_preset_files(&[folder(&scratch, "p", PresetOrigin::User)], CLASS);

        let found = names(&presets);
        assert!(found.contains(&"deep"));
        assert!(found.contains(&format!("level{}", MAX_DEPTH - 1).as_str()));
        assert!(
            !found.contains(&format!("level{MAX_DEPTH}").as_str()),
            "{found:?}"
        );
    }

    #[test]
    fn presets_are_sorted_by_name_within_the_folders_order() {
        let scratch = Scratch::new("sort");
        scratch.preset("p/b.vstpreset", CLASS);
        scratch.preset("p/A.vstpreset", CLASS);
        scratch.preset("p/c.vstpreset", CLASS);

        let presets = list_preset_files(&[folder(&scratch, "p", PresetOrigin::Factory)], CLASS);

        assert_eq!(names(&presets), ["A", "b", "c"]);
        assert!(presets
            .iter()
            .all(|preset| preset.provider_id == "vst3:file"));
    }

    #[test]
    fn a_missing_folder_gives_nothing() {
        let scratch = Scratch::new("missing");
        let none = list_preset_files(&[folder(&scratch, "nowhere", PresetOrigin::User)], CLASS);
        assert!(none.is_empty());
    }

    #[test]
    fn a_folder_that_is_a_file_gives_nothing() {
        let scratch = Scratch::new("file");
        scratch.preset("p.vstpreset", CLASS);
        let none = list_preset_files(
            &[folder(&scratch, "p.vstpreset", PresetOrigin::User)],
            CLASS,
        );
        assert!(none.is_empty());
    }

    #[test]
    fn this_machine_has_roots_without_failing() {
        let roots = PresetRoots::of_this_machine();
        if let Some(documents) = roots.documents {
            assert!(documents.is_absolute());
        }
    }
}
