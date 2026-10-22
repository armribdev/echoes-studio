//! Loading and listing `.vstpreset` files for a VST3 plugin, through a hand-made
//! plugin: the real ones are covered by `ffi_lifecycle_tests`.  The process-wide
//! "unsaved" flag is shared by every test, so these tests count the marks of their
//! own instance.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::audio_graph::plugin_params::{PluginParamRange, ValueModulatedParam};
use crate::plugin_formats::vst3::fake_plugin::{
    fake_vst3_instance_with_units, FakeListener, FakeUnits, FAKE_PLUGIN_ID,
};
use crate::plugin_formats::vst3::host::decode_plugin_id;
use crate::plugin_formats::vst3::preset_file::class_id_text;
use crate::plugin_formats::vst3::preset_file::tests::preset_file;
use crate::plugin_formats::vst3::preset_locations::PresetRoots;
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::presets::{PluginPreset, PresetListing, Vst3PresetFiles};

const PLUGIN_NAME: &str = "Fake Synth";

fn class_id() -> String {
    class_id_text(&decode_plugin_id(FAKE_PLUGIN_ID).unwrap())
}

/// A folder of the test's own that is removed with it.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("echoes-vst3-presets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> String {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path.to_str().unwrap().to_owned()
    }

    fn preset(&self, relative: &str, component: &[u8], controller: Option<&[u8]>) -> String {
        let mut chunks: Vec<(&[u8; 4], &[u8])> = vec![(b"Comp", component)];
        if let Some(controller) = controller {
            chunks.push((b"Cont", controller));
        }
        self.write(relative, &preset_file(&class_id(), &chunks))
    }

    fn roots(&self) -> PresetRoots {
        PresetRoots {
            documents: Some(self.0.join("docs")),
            app_data: None,
            program_data: Some(self.0.join("programdata")),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn plugin_with(units: &mut FakeUnits) -> (Box<FakeListener>, Arc<PluginInstance>) {
    let mut listener = FakeListener::new(false);
    let instance = fake_vst3_instance_with_units(&mut listener, units);
    (listener, instance)
}

fn file_request(path: &str) -> PluginPreset {
    PluginPreset::vst3_file(
        "x".into(),
        path.into(),
        crate::plugin_system::presets::PresetOrigin::User,
    )
}

fn load(instance: &PluginInstance, path: &str) -> Result<(), String> {
    instance.load_preset(&file_request(path).request())
}

#[test]
fn a_preset_file_sets_the_state_of_the_processor_and_of_the_controller() {
    let scratch = Scratch::new("load");
    let path = scratch.preset("a.vstpreset", b"processor sound", Some(b"controller view"));
    let mut units = FakeUnits::absent();
    let (_listener, instance) = plugin_with(&mut units);

    load(&instance, &path).unwrap();

    assert_eq!(
        *units.component_loads.borrow(),
        [b"processor sound".to_vec()]
    );
    assert_eq!(
        *units.controller_loads.borrow(),
        [b"controller view".to_vec()]
    );
    assert_eq!(
        instance.state_edit_count(),
        1,
        "the project reads as unsaved"
    );
    assert!(
        instance.offrt_try_acquire(Duration::ZERO).is_some(),
        "gate freed"
    );
}

#[test]
fn a_file_without_controller_state_leaves_the_controller_alone() {
    let scratch = Scratch::new("no-controller-state");
    let path = scratch.preset("a.vstpreset", b"sound", None);
    let mut units = FakeUnits::absent();
    let (_listener, instance) = plugin_with(&mut units);

    load(&instance, &path).unwrap();

    assert_eq!(units.component_loads.borrow().len(), 1);
    assert!(units.controller_loads.borrow().is_empty());
}

#[test]
fn a_controller_that_refuses_its_own_state_does_not_fail_the_load() {
    let scratch = Scratch::new("controller-refuses");
    let path = scratch.preset("a.vstpreset", b"sound", Some(b"view"));
    let mut units = FakeUnits::absent();
    units.refuses_controller_state = true;
    let (_listener, instance) = plugin_with(&mut units);

    load(&instance, &path).unwrap();

    assert_eq!(instance.state_edit_count(), 1);
}

#[test]
fn a_processor_that_refuses_the_state_fails_the_load_and_leaves_the_rest_alone() {
    let scratch = Scratch::new("processor-refuses");
    let path = scratch.preset("a.vstpreset", b"sound", Some(b"view"));
    let mut units = FakeUnits::absent();
    units.refuses_component_state = true;
    let (_listener, instance) = plugin_with(&mut units);

    let error = load(&instance, &path).unwrap_err();

    assert!(error.contains("could not load"), "{error}");
    assert!(units.controller_loads.borrow().is_empty());
    assert_eq!(instance.state_edit_count(), 0);
    assert!(
        instance.offrt_try_acquire(Duration::ZERO).is_some(),
        "gate freed"
    );
}

#[test]
fn a_file_that_cannot_be_used_is_refused_before_the_plugin_is_touched() {
    let scratch = Scratch::new("unusable");
    let other_plugin = scratch.write(
        "other.vstpreset",
        &preset_file(&"0".repeat(32), &[(b"Comp", b"x")]),
    );
    let mut valid = preset_file(&class_id(), &[(b"Comp", b"sound")]);
    let garbage = scratch.write("garbage.vstpreset", b"not a preset");
    valid.truncate(valid.len() - 5);
    let truncated = scratch.write("truncated.vstpreset", &valid);
    let missing = scratch
        .0
        .join("missing.vstpreset")
        .to_str()
        .unwrap()
        .to_owned();
    let mut units = FakeUnits::absent();
    let (_listener, instance) = plugin_with(&mut units);

    for (path, expected) in [
        (&other_plugin, "another plugin"),
        (&garbage, "not a VST3 preset"),
        (&truncated, "truncated"),
        (&missing, "cannot be read"),
    ] {
        let error = load(&instance, path).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    let no_path = PluginPreset {
        location: None,
        ..file_request("")
    };
    assert!(instance.load_preset(&no_path.request()).is_err());
    assert!(units.component_loads.borrow().is_empty());
    assert_eq!(instance.state_edit_count(), 0);
}

#[test]
fn a_busy_plugin_refuses_the_load_and_is_left_alone() {
    let scratch = Scratch::new("busy");
    let path = scratch.preset("a.vstpreset", b"sound", None);
    let mut units = FakeUnits::absent();
    let (_listener, instance) = plugin_with(&mut units);
    let gate = instance
        .offrt_try_acquire(Duration::from_millis(50))
        .unwrap();

    let error = load(&instance, &path).unwrap_err();

    assert!(error.contains("busy"), "{error}");
    assert!(units.component_loads.borrow().is_empty());
    assert_eq!(instance.state_edit_count(), 0);

    drop(gate);
    load(&instance, &path).unwrap();
    assert_eq!(units.component_loads.borrow().len(), 1);
}

#[test]
fn the_base_of_a_value_modulated_parameter_follows_the_file() {
    let scratch = Scratch::new("base");
    let path = scratch.preset("a.vstpreset", b"sound", None);
    let mut units = FakeUnits::absent();
    units.values.borrow_mut().insert(7, 0.6);
    let (_listener, instance) = plugin_with(&mut units);
    let base = Arc::new(AtomicU32::new(0.1f32.to_bits()));
    instance
        .vst3_edit_state()
        .unwrap()
        .replace_followed(vec![ValueModulatedParam::new(
            7,
            PluginParamRange::UNKNOWN,
            Arc::clone(&base),
        )]);

    load(&instance, &path).unwrap();

    assert_eq!(f32::from_bits(base.load(Ordering::Relaxed)), 0.6);
}

fn listing(instance: &Arc<PluginInstance>, scratch: &Scratch) -> PresetListing {
    PresetListing::Vst3 {
        instance: Arc::clone(instance),
        files: Some(
            Vst3PresetFiles::new(PLUGIN_NAME, FAKE_PLUGIN_ID)
                .unwrap()
                .under(scratch.roots()),
        ),
    }
}

#[test]
fn the_listing_offers_programs_then_factory_files_then_user_files() {
    let scratch = Scratch::new("listing");
    scratch.preset(
        "programdata/VST3 Presets/V/Fake Synth/Zeta.vstpreset",
        b"z",
        None,
    );
    scratch.preset(
        "programdata/VST3 Presets/V/Fake Synth/Alpha.vstpreset",
        b"a",
        None,
    );
    scratch.preset("docs/VST3 Presets/V/Fake Synth/Mine.vstpreset", b"m", None);
    scratch.write(
        "docs/VST3 Presets/V/Fake Synth/Theirs.vstpreset",
        &preset_file(&"0".repeat(32), &[(b"Comp", b"t")]),
    );
    scratch.preset(
        "docs/VST3 Presets/V/Other Plugin/Wrong.vstpreset",
        b"w",
        None,
    );
    let mut units = FakeUnits::root_only();
    units.add_list(4, "Factory", &["Default", "Warm"]);
    units.program_change_params = vec![(0, 40, 1)];
    let (_listener, instance) = plugin_with(&mut units);

    let presets = listing(&instance, &scratch).list();

    let names: Vec<_> = presets.iter().map(|preset| preset.name.as_str()).collect();
    assert_eq!(names, ["Default", "Warm", "Alpha", "Zeta", "Mine"]);
    assert_eq!(presets[2].provider_id, "vst3:file");
    assert_eq!(presets[4].flags, 2, "the user's own come last");
}

#[test]
fn a_plugin_with_no_program_and_no_file_lists_nothing() {
    let scratch = Scratch::new("nothing");
    let mut units = FakeUnits::absent();
    let (_listener, instance) = plugin_with(&mut units);

    assert!(listing(&instance, &scratch).list().is_empty());
}

#[test]
fn a_listing_without_files_still_offers_the_programs() {
    let mut units = FakeUnits::root_only();
    units.add_list(4, "Factory", &["Default"]);
    units.program_change_params = vec![(0, 40, 0)];
    let (_listener, instance) = plugin_with(&mut units);

    let presets = PresetListing::Vst3 {
        instance,
        files: None,
    }
    .list();

    assert_eq!(presets.len(), 1);
}
