//! What the host sends a VST3 processor reaches its controller while an editor is
//! open, through a hand-made plugin: the real ones are covered by
//! `ffi_lifecycle_tests`.  The block is run by `fill_param_changes`, the delivery
//! shared by the effect and the instrument.

use std::sync::atomic::AtomicU32;
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::MidiEventList;
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::audio_graph::plugin_params::{PluginParamControl, PluginParamControls};
use crate::plugin_formats::vst3::fake_plugin::{
    fake_vst3_instance_with_units, FakeListener, FakeUnits,
};
use crate::plugin_formats::vst3::ffi::HostParameterChanges;
use crate::plugin_formats::vst3::instrument::VstInstrument;
use crate::plugin_formats::vst3::param_delivery::fill_param_changes;
use crate::plugin_formats::vst3::wrapper::VstWrapperNode;
use crate::plugin_system::instance::PluginInstance;

fn plugin_with(units: &mut FakeUnits) -> (Box<FakeListener>, Arc<PluginInstance>) {
    let mut listener = FakeListener::new(false);
    let instance = fake_vst3_instance_with_units(&mut listener, units);
    (listener, instance)
}

fn control(native_id: u32, base: f32) -> PluginParamControl {
    PluginParamControl::new(native_id, Arc::new(AtomicU32::new(base.to_bits())))
}

/// One audio block, then the tick of the editor window.
fn block_then_tick(instance: &PluginInstance, controls: &PluginParamControls) {
    let mut changes = HostParameterChanges::boxed();
    fill_param_changes(instance, controls, &mut changes);
    instance.mirror_to_controller();
}

fn given_to_controller(units: &FakeUnits) -> Vec<(u32, f64)> {
    units.selections.borrow().clone()
}

fn assert_given(units: &FakeUnits, expected: &[(u32, f64)]) {
    let given = given_to_controller(units);
    assert_eq!(given.len(), expected.len(), "{given:?}");
    for ((id, value), (expected_id, expected_value)) in given.iter().zip(expected) {
        assert_eq!(id, expected_id);
        assert!((value - expected_value).abs() < 1e-6, "{given:?}");
    }
}

#[test]
fn a_value_queued_by_the_host_reaches_the_controller() {
    let mut units = FakeUnits::root_only();
    let (_listener, instance) = plugin_with(&mut units);
    let _watch = instance.watch_vst3_params();

    instance.push_param(5, 0.6);
    block_then_tick(&instance, &PluginParamControls::default());

    assert_given(&units, &[(5, 0.6)]);
}

#[test]
fn a_modulated_parameter_is_given_the_last_point_of_the_block() {
    let mut units = FakeUnits::root_only();
    let (_listener, instance) = plugin_with(&mut units);
    let _watch = instance.watch_vst3_params();
    let mut controls = PluginParamControls::new(vec![control(7, 0.25)]);
    controls.modulate_at(7, 0, 0.1);
    controls.modulate_at(7, 32, 0.3);

    block_then_tick(&instance, &controls);

    assert_given(&units, &[(7, 0.55)]);
}

#[test]
fn an_automated_parameter_is_given_its_value() {
    let mut units = FakeUnits::root_only();
    let (_listener, instance) = plugin_with(&mut units);
    let _watch = instance.watch_vst3_params();
    let mut controls = PluginParamControls::new(vec![control(8, 0.0)]);
    controls.automate_at(8, 0, 0.4);
    controls.automate_at(8, 64, 0.9);

    block_then_tick(&instance, &controls);

    assert_given(&units, &[(8, 0.9)]);
}

#[test]
fn a_value_is_given_once() {
    let mut units = FakeUnits::root_only();
    let (_listener, instance) = plugin_with(&mut units);
    let _watch = instance.watch_vst3_params();

    instance.push_param(5, 0.6);
    block_then_tick(&instance, &PluginParamControls::default());
    block_then_tick(&instance, &PluginParamControls::default());

    assert_eq!(given_to_controller(&units).len(), 1);
}

#[test]
fn nothing_is_given_while_no_editor_is_open() {
    let mut units = FakeUnits::root_only();
    let (_listener, instance) = plugin_with(&mut units);

    instance.push_param(5, 0.6);
    block_then_tick(&instance, &PluginParamControls::default());
    let watch = instance.watch_vst3_params();
    block_then_tick(&instance, &PluginParamControls::default());
    drop(watch);
    instance.push_param(5, 0.7);
    block_then_tick(&instance, &PluginParamControls::default());
    let _watch = instance.watch_vst3_params();
    block_then_tick(&instance, &PluginParamControls::default());

    assert!(given_to_controller(&units).is_empty());
}

#[test]
fn the_edits_of_the_window_are_not_given_back_to_it() {
    let mut units = FakeUnits::root_only();
    let (_listener, instance) = plugin_with(&mut units);
    let _watch = instance.watch_vst3_params();
    let edit_state = instance.vst3_edit_state().unwrap();

    edit_state.perform(7, 0.4);
    block_then_tick(&instance, &PluginParamControls::default());

    assert!(given_to_controller(&units).is_empty());
}

#[test]
fn a_parameter_being_dragged_is_left_alone() {
    let mut units = FakeUnits::root_only();
    let (_listener, instance) = plugin_with(&mut units);
    let _watch = instance.watch_vst3_params();
    instance.vst3_edit_state().unwrap().begin(5);

    instance.push_param(5, 0.6);
    instance.push_param(6, 0.2);
    block_then_tick(&instance, &PluginParamControls::default());

    assert_given(&units, &[(6, 0.2)]);
}

#[test]
fn a_controller_that_reports_the_value_it_is_given_does_not_reach_the_processor() {
    let mut units = FakeUnits::root_only();
    units.echoes_values = true;
    let (_listener, instance) = plugin_with(&mut units);
    let _watch = instance.watch_vst3_params();
    let edit_state = instance.vst3_edit_state().unwrap();

    instance.push_param(5, 0.6);
    block_then_tick(&instance, &PluginParamControls::default());

    assert_given(&units, &[(5, 0.6)]);
    let mut echoed = Vec::new();
    edit_state.drain_edits(|change| echoed.push(change.native_id));
    assert!(echoed.is_empty(), "{echoed:?}");
    assert!(!edit_state.is_editing(5), "its gesture was not opened");
}

#[test]
fn a_crashed_plugin_is_not_called() {
    let mut units = FakeUnits::root_only();
    let (_listener, instance) = plugin_with(&mut units);
    let _watch = instance.watch_vst3_params();
    instance.push_param(5, 0.6);
    let mut changes = HostParameterChanges::boxed();
    fill_param_changes(&instance, &PluginParamControls::default(), &mut changes);
    instance.set_lifecycle(crate::plugin_system::lifecycle::LifecycleState::Error);

    instance.mirror_to_controller();

    assert!(given_to_controller(&units).is_empty());
}

const BLOCK: usize = 64;

fn prepare_context() -> PrepareContext {
    PrepareContext {
        sample_rate: 48_000.0,
        max_block_size: 512,
        num_channels: 2,
        live: true,
    }
}

fn transport() -> TransportInfo {
    TransportInfo {
        block_size: BLOCK,
        ..TransportInfo::stopped(48_000.0)
    }
}

fn effect(instance: &Arc<PluginInstance>) -> VstWrapperNode {
    let mut node = VstWrapperNode::new(Arc::clone(instance));
    node.prepare(prepare_context());
    node
}

fn run_effect_block(node: &mut VstWrapperNode) {
    let input = AudioBuffer::new(2, BLOCK);
    let mut outputs = vec![AudioBuffer::new(2, BLOCK)];
    node.process(&[&input], &mut outputs, &transport());
}

#[test]
fn a_value_the_plugin_writes_reaches_the_controller() {
    let mut units = FakeUnits::root_only();
    units.outputs_to_write = vec![(5, 0.75)];
    let (_listener, instance) = plugin_with(&mut units);
    let mut node = effect(&instance);
    let _watch = instance.watch_vst3_params();

    run_effect_block(&mut node);
    instance.mirror_to_controller();

    assert_given(&units, &[(5, 0.75)]);
    assert_eq!(
        instance.state_edit_count(),
        0,
        "the plugin's own changes do not mark the project unsaved"
    );
    let mut edits = 0;
    instance
        .vst3_edit_state()
        .unwrap()
        .drain_edits(|_| edits += 1);
    assert_eq!(edits, 0, "nor do they come back to the processor");
}

#[test]
fn the_last_point_a_plugin_writes_is_the_one_given() {
    let mut units = FakeUnits::root_only();
    units.outputs_to_write = vec![(5, 0.2), (5, 0.6)];
    let (_listener, instance) = plugin_with(&mut units);
    let mut node = effect(&instance);
    let _watch = instance.watch_vst3_params();

    run_effect_block(&mut node);
    instance.mirror_to_controller();

    assert_given(&units, &[(5, 0.6)]);
}

#[test]
fn the_list_is_empty_again_at_each_block() {
    let mut units = FakeUnits::root_only();
    units.outputs_to_write = vec![(5, 0.75)];
    let (_listener, instance) = plugin_with(&mut units);
    let mut node = effect(&instance);
    let _watch = instance.watch_vst3_params();

    run_effect_block(&mut node);
    instance.mirror_to_controller();
    units.outputs_to_write.clear();
    run_effect_block(&mut node);
    instance.mirror_to_controller();

    assert_given(&units, &[(5, 0.75)]);
}

#[test]
fn nothing_a_plugin_writes_is_kept_while_no_editor_is_open() {
    let mut units = FakeUnits::root_only();
    units.outputs_to_write = vec![(5, 0.75)];
    let (_listener, instance) = plugin_with(&mut units);
    let mut node = effect(&instance);

    run_effect_block(&mut node);
    let _watch = instance.watch_vst3_params();
    instance.mirror_to_controller();

    assert!(given_to_controller(&units).is_empty());
}

#[test]
fn a_plugin_that_writes_more_parameters_than_the_list_holds_is_not_cut_short() {
    let mut units = FakeUnits::root_only();
    units.outputs_to_write = (0..130).map(|id| (id, 0.5)).collect();
    let (_listener, instance) = plugin_with(&mut units);
    let mut node = effect(&instance);
    let _watch = instance.watch_vst3_params();

    run_effect_block(&mut node);
    instance.mirror_to_controller();

    assert_eq!(given_to_controller(&units).len(), 128);
    assert!(!instance.has_crashed());
}

#[test]
fn an_instrument_hands_the_values_it_writes_to_the_controller_too() {
    let mut units = FakeUnits::root_only();
    units.outputs_to_write = vec![(5, 0.75)];
    let (_listener, instance) = plugin_with(&mut units);
    let mut instrument = VstInstrument::new(Arc::clone(&instance));
    instrument.prepare(prepare_context());
    let _watch = instance.watch_vst3_params();

    let mut outputs = vec![AudioBuffer::new(2, BLOCK)];
    instrument.process_midi(&MidiEventList::new(), &mut outputs, &transport());
    instance.mirror_to_controller();

    assert_given(&units, &[(5, 0.75)]);
}
