//! A state save of a VST3 plugin whose parameters are modulated by value, through a
//! hand-made plugin: the real ones are covered by `ffi_lifecycle_tests`.

use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Duration;

use crate::plugin_formats::vst3::component_handler::FollowedParam;
use crate::plugin_formats::vst3::fake_plugin::{
    fake_vst3_instance_with_units, FakeListener, FakeUnits, ProcessCall,
};
use crate::plugin_system::instance::PluginInstance;

fn active_plugin(units: &mut FakeUnits) -> (Box<FakeListener>, Arc<PluginInstance>) {
    let mut listener = FakeListener::new(false);
    let instance = fake_vst3_instance_with_units(&mut listener, units);
    assert!(instance.vst3_ensure_active(48_000.0, 512));
    (listener, instance)
}

fn follow(instance: &PluginInstance, bases: &[(u32, f32)]) {
    let params = bases
        .iter()
        .map(|&(native_id, base)| {
            let base = Arc::new(AtomicU32::new(base.to_bits()));
            FollowedParam::new(native_id, base)
        })
        .collect();
    instance.vst3_edit_state().unwrap().replace_followed(params);
}

fn save(instance: &PluginInstance) -> Option<Vec<u8>> {
    let _gate = instance
        .offrt_try_acquire(Duration::from_millis(50))
        .expect("the plugin is idle");
    instance.save_state_blob()
}

#[test]
fn a_state_save_puts_the_modulated_parameters_back_on_their_base_first() {
    let mut units = FakeUnits::root_only();
    units.processor_state = b"state".to_vec();
    let (_listener, instance) = active_plugin(&mut units);
    follow(&instance, &[(7, 0.25), (9, 0.5)]);

    let state = save(&instance);

    assert_eq!(state.as_deref(), Some(&b"state"[..]));
    assert_eq!(
        *units.process_calls.borrow(),
        [ProcessCall {
            num_samples: 0,
            changes: vec![(7, 0.25), (9, 0.5)],
            state_reads: 0,
        }],
        "one flush, before the state is read"
    );
    assert_eq!(units.state_reads.get(), 1);
}

#[test]
fn the_base_is_the_one_the_user_last_set() {
    let mut units = FakeUnits::root_only();
    let (_listener, instance) = active_plugin(&mut units);
    follow(&instance, &[(7, 0.25)]);
    instance
        .vst3_edit_state()
        .unwrap()
        .rebase_followed(|_| Some(0.8));

    save(&instance);

    let calls = units.process_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].changes.len(), 1);
    assert!((calls[0].changes[0].1 - 0.8).abs() < 1e-6);
}

#[test]
fn a_plugin_without_modulated_parameters_is_not_processed_by_a_save() {
    let mut units = FakeUnits::root_only();
    let (_listener, instance) = active_plugin(&mut units);

    save(&instance);

    assert!(units.process_calls.borrow().is_empty());
    assert_eq!(units.state_reads.get(), 1);
}

#[test]
fn a_plugin_that_is_not_active_is_not_processed_by_a_save() {
    let mut units = FakeUnits::root_only();
    let mut listener = FakeListener::new(false);
    let instance = fake_vst3_instance_with_units(&mut listener, &mut units);
    follow(&instance, &[(7, 0.25)]);

    save(&instance);

    assert!(units.process_calls.borrow().is_empty());
    assert_eq!(units.state_reads.get(), 1);
}
