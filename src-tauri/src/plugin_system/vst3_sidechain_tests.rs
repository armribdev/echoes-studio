//! A VST3 effect with an auxiliary input bus is given the signal of the graph
//! port that feeds it, in place of a silent bus (ADR-014 sidechain), through a
//! hand-made plugin that records what it was handed.

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};
use crate::plugin_formats::vst3::fake_plugin::{
    fake_vst3_instance_with_buses, FakeBus, FakeBuses, FakeListener, SeenInput, MONO, STEREO,
};
use crate::plugin_formats::vst3::ffi::{K_INPUT, K_OUTPUT};
use crate::plugin_formats::vst3::wrapper::VstWrapperNode;
use crate::plugin_system::instance::PluginInstance;

const BLOCK: usize = 64;
const MAIN_OUT: f32 = 0.1;

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

fn compressor_with_sidechain(sidechain: FakeBus) -> Box<FakeBuses> {
    FakeBuses::new(
        vec![FakeBus::main("In", STEREO), sidechain],
        vec![FakeBus::main("Out", STEREO).writing(MAIN_OUT)],
    )
}

fn plugin(buses: &mut FakeBuses) -> (Box<FakeListener>, Arc<PluginInstance>) {
    let mut listener = FakeListener::new(false);
    let instance = fake_vst3_instance_with_buses(&mut listener, buses);
    (listener, instance)
}

fn prepared(instance: &Arc<PluginInstance>) -> VstWrapperNode {
    let mut node = VstWrapperNode::new(Arc::clone(instance));
    node.prepare(prepare_context());
    node
}

fn stereo(left: f32, right: f32) -> AudioBuffer {
    let mut buffer = AudioBuffer::new(2, BLOCK);
    buffer.channel_mut(0).fill(left);
    buffer.channel_mut(1).fill(right);
    buffer
}

fn run(node: &mut VstWrapperNode, inputs: &[&AudioBuffer]) -> AudioBuffer {
    let mut outputs = [AudioBuffer::new(2, BLOCK)];
    node.process(inputs, &mut outputs, &transport());
    let [output] = outputs;
    output
}

fn last_seen(buses: &FakeBuses) -> Vec<SeenInput> {
    buses.inputs_seen().pop().expect("the plugin was processed")
}

#[test]
fn the_signal_of_the_port_feeding_a_sidechain_reaches_the_plugin() {
    let mut buses = compressor_with_sidechain(FakeBus::aux("Sidechain", MONO));
    let (_listener, instance) = plugin(&mut buses);
    let mut node = prepared(&instance);
    let (main, kick) = (stereo(0.5, 0.5), stereo(0.75, 0.25));

    run(&mut node, &[&main, &kick]);

    assert_eq!(node.num_input_ports(), 2);
    assert_eq!(
        last_seen(&buses),
        vec![
            SeenInput {
                channels: 2,
                silence_flags: 0,
                first_samples: vec![Some(0.5), Some(0.5)],
            },
            SeenInput {
                channels: 1,
                silence_flags: 0,
                first_samples: vec![Some(0.5)],
            },
        ],
        "the mono sidechain is the average of the source"
    );
    assert!(!instance.has_crashed());
}

#[test]
fn a_stereo_sidechain_gets_both_channels() {
    let mut buses = compressor_with_sidechain(FakeBus::aux("Sidechain", STEREO));
    let (_listener, instance) = plugin(&mut buses);
    let mut node = prepared(&instance);
    let (main, kick) = (stereo(0.5, 0.5), stereo(0.75, 0.25));

    run(&mut node, &[&main, &kick]);

    assert_eq!(
        last_seen(&buses)[1].first_samples,
        vec![Some(0.75), Some(0.25)]
    );
}

#[test]
fn a_sidechain_nobody_chose_is_silence_with_its_flags_set() {
    let mut buses = compressor_with_sidechain(FakeBus::aux("Sidechain", MONO));
    let (_listener, instance) = plugin(&mut buses);
    let mut node = prepared(&instance);
    let (main, scheduler_silence) = (stereo(0.5, 0.5), stereo(0.0, 0.0));

    run(&mut node, &[&main, &scheduler_silence]);

    assert_eq!(
        last_seen(&buses)[1],
        SeenInput {
            channels: 1,
            silence_flags: 0b1,
            first_samples: vec![Some(0.0)],
        }
    );
}

#[test]
fn a_graph_node_sized_without_the_port_still_gives_the_open_bus_valid_silence() {
    let mut buses = compressor_with_sidechain(FakeBus::aux("Sidechain", MONO));
    let (_listener, instance) = plugin(&mut buses);
    let mut node = prepared(&instance);
    let main = stereo(0.5, 0.5);

    run(&mut node, &[&main]);

    let seen = last_seen(&buses);
    assert_eq!(seen.len(), 2, "an opened bus is always handed over");
    assert_eq!(seen[1].silence_flags, 0b1);
    assert_eq!(seen[1].first_samples, vec![Some(0.0)]);
}

#[test]
fn the_sidechain_does_not_change_what_the_plugin_renders_on_its_output() {
    let mut buses = compressor_with_sidechain(FakeBus::aux("Sidechain", MONO));
    let (_listener, instance) = plugin(&mut buses);
    let mut node = prepared(&instance);
    let (main, kick) = (stereo(0.5, 0.5), stereo(0.75, 0.25));

    let output = run(&mut node, &[&main, &kick]);

    assert!(output.channel(0).iter().all(|&sample| sample == MAIN_OUT));
    assert!(output.channel(1).iter().all(|&sample| sample == MAIN_OUT));
}

#[test]
fn an_effect_without_an_aux_input_is_called_exactly_as_before() {
    let mut buses = FakeBuses::new(
        vec![FakeBus::main("In", STEREO)],
        vec![FakeBus::main("Out", STEREO)],
    );
    let (_listener, instance) = plugin(&mut buses);
    let mut node = prepared(&instance);
    let main = stereo(0.5, 0.25);

    run(&mut node, &[&main]);

    assert_eq!(node.num_input_ports(), 1);
    assert_eq!(
        last_seen(&buses),
        vec![SeenInput {
            channels: 2,
            silence_flags: 0,
            first_samples: vec![Some(0.5), Some(0.25)],
        }]
    );
    assert_eq!(
        buses.activations(),
        vec![(K_INPUT, 0, true), (K_OUTPUT, 0, true)]
    );
}

#[test]
fn a_new_wrapper_on_a_live_instance_keeps_the_sidechain() {
    let mut buses = compressor_with_sidechain(FakeBus::aux("Sidechain", MONO));
    let (_listener, instance) = plugin(&mut buses);
    prepared(&instance);
    let mut second = prepared(&instance);
    let (main, kick) = (stereo(0.5, 0.5), stereo(0.75, 0.25));

    run(&mut second, &[&main, &kick]);

    assert_eq!(second.num_input_ports(), 2);
    assert_eq!(last_seen(&buses)[1].first_samples, vec![Some(0.5)]);
    assert_eq!(
        buses
            .activations()
            .iter()
            .filter(|&&(direction, index, _)| direction == K_INPUT && index == 1)
            .count(),
        1,
        "the instance was not reactivated"
    );
}
