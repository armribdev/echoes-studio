//! Choosing a program of a VST3 plugin, through a hand-made plugin: the real ones
//! are covered by `ffi_lifecycle_tests`.  The process-wide "unsaved" flag is
//! shared by every test, so these tests count the marks of their own instance.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::plugin_formats::vst3::component_handler::FollowedParam;
use crate::plugin_formats::clap::ffi::{
    CLAP_PRESET_IS_FACTORY_CONTENT, CLAP_PRESET_LOCATION_PLUGIN,
};
use crate::plugin_formats::vst3::fake_plugin::{
    fake_vst3_instance, fake_vst3_instance_with_units, FakeListener, FakeUnits,
};
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::param_queue::ParamChange;
use crate::plugin_system::presets::PluginPreset;

const SELECTOR: u32 = 40;

/// A plugin like TDR Nova: four programs in one list, chosen by one parameter.
fn four_programs() -> Box<FakeUnits> {
    let mut units = FakeUnits::root_only();
    units.add_list(4, "Factory", &["Default", "Warm", "Bright", "Wide"]);
    units.program_change_params = vec![(0, SELECTOR, 3)];
    units
}

fn plugin_with(units: &mut FakeUnits) -> (Box<FakeListener>, Arc<PluginInstance>) {
    let mut listener = FakeListener::new(false);
    let instance = fake_vst3_instance_with_units(&mut listener, units);
    (listener, instance)
}

fn program_named(instance: &PluginInstance, name: &str) -> PluginPreset {
    instance
        .vst3_program_presets()
        .into_iter()
        .find(|preset| preset.name == name)
        .unwrap_or_else(|| panic!("no program named {name}"))
}

fn queued(instance: &PluginInstance) -> Vec<(u32, f32)> {
    let mut changes = Vec::new();
    instance.drain_params(
        |ParamChange {
             native_id,
             normalised,
             ..
         }| {
            changes.push((native_id, normalised));
        },
    );
    changes
}

#[test]
fn the_programs_of_a_plugin_are_listed_in_its_own_order() {
    let mut units = four_programs();
    let (_listener, instance) = plugin_with(&mut units);

    let presets = instance.vst3_program_presets();

    let names: Vec<_> = presets.iter().map(|preset| preset.name.as_str()).collect();
    assert_eq!(names, ["Default", "Warm", "Bright", "Wide"]);
    assert_eq!(presets[2].load_key.as_deref(), Some("40:2"));
    assert_eq!(presets[2].location_kind, CLAP_PRESET_LOCATION_PLUGIN);
    assert_eq!(presets[2].location, None);
    assert_eq!(presets[2].flags, CLAP_PRESET_IS_FACTORY_CONTENT);
    assert_eq!(units.references(), 0, "no reference is left behind");
}

fn active_name(instance: &PluginInstance) -> Option<String> {
    let active: Vec<_> = instance
        .vst3_program_presets()
        .into_iter()
        .filter(|preset| preset.active)
        .map(|preset| preset.name)
        .collect();
    assert!(
        active.len() <= 1,
        "at most one program is active: {active:?}"
    );
    active.into_iter().next()
}

#[test]
fn the_active_program_is_the_one_the_plugin_is_set_to() {
    let mut units = four_programs();
    let (_listener, instance) = plugin_with(&mut units);
    assert_eq!(active_name(&instance).as_deref(), Some("Default"));

    for name in ["Bright", "Wide", "Warm"] {
        instance
            .load_preset(&program_named(&instance, name).request())
            .unwrap();
        assert_eq!(active_name(&instance).as_deref(), Some(name));
    }
}

#[test]
fn a_program_the_plugin_refuses_does_not_become_active() {
    let mut units = four_programs();
    let (_listener, instance) = plugin_with(&mut units);
    let warm = program_named(&instance, "Warm").request();
    units.refuses_selection = true;

    assert!(instance.load_preset(&warm).is_err());

    assert_eq!(active_name(&instance).as_deref(), Some("Default"));
}

#[test]
fn a_plugin_without_selectable_programs_lists_nothing() {
    let mut without_interface = FakeUnits::absent();
    let (_listener, instance) = plugin_with(&mut without_interface);
    assert!(instance.vst3_program_presets().is_empty());

    let mut without_selector = FakeUnits::with_list(1, "A", &["a1", "a2"]);
    let (_listener, instance) = plugin_with(&mut without_selector);
    assert!(instance.vst3_program_presets().is_empty());

    let mut listener = FakeListener::new(false);
    assert!(fake_vst3_instance(&mut listener)
        .vst3_program_presets()
        .is_empty());
}

#[test]
fn a_busy_plugin_lists_nothing_instead_of_waiting_for_ever() {
    let mut units = four_programs();
    let (_listener, instance) = plugin_with(&mut units);
    let gate = instance
        .offrt_try_acquire(Duration::from_millis(50))
        .unwrap();

    assert!(instance.vst3_program_presets().is_empty());

    drop(gate);
    assert_eq!(instance.vst3_program_presets().len(), 4);
}

#[test]
fn choosing_a_program_sets_the_controller_and_queues_it_for_the_processor() {
    let mut units = four_programs();
    let (_listener, instance) = plugin_with(&mut units);
    let bright = program_named(&instance, "Bright");

    instance.load_preset(&bright.request()).unwrap();

    let expected = 2.0 / 3.0;
    assert_eq!(*units.selections.borrow(), [(SELECTOR, expected)]);
    assert_eq!(queued(&instance), [(SELECTOR, expected as f32)]);
}

#[test]
fn a_loaded_program_marks_the_project_unsaved() {
    let mut units = four_programs();
    let (_listener, instance) = plugin_with(&mut units);
    assert_eq!(instance.state_edit_count(), 0);

    instance
        .load_preset(&program_named(&instance, "Warm").request())
        .unwrap();

    assert_eq!(instance.state_edit_count(), 1);
}

#[test]
fn the_gate_is_free_again_after_a_load() {
    let mut units = four_programs();
    let (_listener, instance) = plugin_with(&mut units);

    instance
        .load_preset(&program_named(&instance, "Warm").request())
        .unwrap();

    assert!(instance.offrt_try_acquire(Duration::ZERO).is_some());
}

#[test]
fn a_program_the_plugin_refuses_changes_nothing() {
    let mut units = four_programs();
    units.refuses_selection = true;
    let (_listener, instance) = plugin_with(&mut units);
    let request = program_named(&instance, "Warm").request();

    let error = instance.load_preset(&request).unwrap_err();

    assert!(error.contains("could not load"), "{error}");
    assert!(units.selections.borrow().is_empty());
    assert!(queued(&instance).is_empty());
    assert_eq!(instance.state_edit_count(), 0);
}

#[test]
fn a_busy_plugin_refuses_the_load_and_is_left_alone() {
    let mut units = four_programs();
    let (_listener, instance) = plugin_with(&mut units);
    let request = program_named(&instance, "Warm").request();
    let gate = instance
        .offrt_try_acquire(Duration::from_millis(50))
        .unwrap();

    let error = instance.load_preset(&request).unwrap_err();

    assert!(error.contains("busy"), "{error}");
    assert!(units.selections.borrow().is_empty());
    assert!(queued(&instance).is_empty());
    assert_eq!(instance.state_edit_count(), 0);

    drop(gate);
    instance.load_preset(&request).unwrap();
    assert_eq!(units.selections.borrow().len(), 1);
}

#[test]
fn a_request_that_is_not_a_program_is_refused() {
    let mut units = four_programs();
    let (_listener, instance) = plugin_with(&mut units);
    let mut request = program_named(&instance, "Warm").request();

    for key in [
        None,
        Some(""),
        Some("40"),
        Some("x:1"),
        Some("40:-1"),
        Some("40:1:2"),
    ] {
        request.load_key = key.map(String::from);
        assert!(instance.load_preset(&request).is_err(), "{key:?}");
    }
    assert!(units.selections.borrow().is_empty());
    assert_eq!(instance.state_edit_count(), 0);
}

#[test]
fn the_controller_is_given_the_processors_state_so_the_knobs_follow() {
    let mut units = four_programs();
    units.processor_state = b"the processor state".to_vec();
    units.selection_side_effects = vec![(7, 0.9)];
    let (_listener, instance) = plugin_with(&mut units);
    let base = Arc::new(AtomicU32::new(0.1f32.to_bits()));
    instance
        .vst3_edit_state()
        .unwrap()
        .replace_followed(vec![FollowedParam::new(7, Arc::clone(&base))]);

    instance.sync_vst3_controller_with_processor(instance.vst3_controller().unwrap());

    assert_eq!(
        *units.component_states.borrow(),
        [b"the processor state".to_vec()]
    );
    assert_eq!(
        f32::from_bits(base.load(Ordering::Relaxed)),
        0.0,
        "the bases are read again after the controller moved"
    );
    assert!(instance.offrt_try_acquire(Duration::ZERO).is_some());
}

#[test]
fn a_busy_plugin_is_not_given_its_state() {
    let mut units = four_programs();
    units.processor_state = b"state".to_vec();
    let (_listener, instance) = plugin_with(&mut units);
    let gate = instance
        .offrt_try_acquire(Duration::from_millis(50))
        .unwrap();

    instance.sync_vst3_controller_with_processor(instance.vst3_controller().unwrap());

    assert!(units.component_states.borrow().is_empty());
    drop(gate);
}

#[test]
fn a_plugin_that_is_not_processing_is_not_waited_for_nor_synced() {
    let mut units = four_programs();
    units.processor_state = b"state".to_vec();
    let (_listener, instance) = plugin_with(&mut units);
    let started = std::time::Instant::now();

    instance
        .load_preset(&program_named(&instance, "Warm").request())
        .unwrap();

    assert!(started.elapsed() < Duration::from_millis(150));
    assert!(units.component_states.borrow().is_empty());
}

#[test]
fn the_base_of_a_value_modulated_parameter_follows_the_program() {
    let mut units = four_programs();
    units.selection_side_effects = vec![(7, 0.9)];
    let (_listener, instance) = plugin_with(&mut units);
    let base = Arc::new(AtomicU32::new(0.1f32.to_bits()));
    let untouched = Arc::new(AtomicU32::new(0.3f32.to_bits()));
    instance.vst3_edit_state().unwrap().replace_followed(vec![
        FollowedParam::new(7, Arc::clone(&base)),
        FollowedParam::new(8, Arc::clone(&untouched)),
    ]);

    instance
        .load_preset(&program_named(&instance, "Wide").request())
        .unwrap();

    assert_eq!(f32::from_bits(base.load(Ordering::Relaxed)), 0.9);
    assert_eq!(
        f32::from_bits(untouched.load(Ordering::Relaxed)),
        0.0,
        "every followed parameter is read again, from the controller"
    );
}
