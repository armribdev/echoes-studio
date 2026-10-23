//! A VST3 plugin with several output buses renders into the graph's output ports
//! (ADR-014 multi-out), in the effect and in the instrument, through a hand-made
//! plugin: each of its buses writes a level of its own.

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::MidiEventList;
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::plugin_formats::vst3::fake_plugin::{
    fake_vst3_instance_with_buses, FakeBus, FakeBuses, FakeListener, MONO, STEREO, SURROUND_5_1,
};
use crate::plugin_formats::vst3::instrument::VstInstrument;
use crate::plugin_formats::vst3::wrapper::VstWrapperNode;
use crate::plugin_system::instance::PluginInstance;

const BLOCK: usize = 64;
const MAIN: f32 = 0.1;
const SNARE: f32 = 0.2;
const HAT: f32 = 0.3;
const TOMS: f32 = 0.4;

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

fn drum_outputs() -> Vec<FakeBus> {
    vec![
        FakeBus::main("Main", STEREO).writing(MAIN),
        FakeBus::aux("Snare", MONO).writing(SNARE),
        FakeBus::aux("Hat", MONO).writing(HAT),
    ]
}

fn plugin(buses: &mut FakeBuses) -> (Box<FakeListener>, Arc<PluginInstance>) {
    let mut listener = FakeListener::new(false);
    let instance = fake_vst3_instance_with_buses(&mut listener, buses);
    (listener, instance)
}

fn graph_outputs(ports: usize) -> Vec<AudioBuffer> {
    (0..ports).map(|_| AudioBuffer::new(2, BLOCK)).collect()
}

fn play_instrument(instrument: &mut VstInstrument, ports: usize) -> Vec<AudioBuffer> {
    let mut outputs = graph_outputs(ports);
    instrument.process_midi(&MidiEventList::new(), &mut outputs, &transport());
    outputs
}

fn run_effect(node: &mut VstWrapperNode, ports: usize) -> Vec<AudioBuffer> {
    let input = AudioBuffer::new(2, BLOCK);
    let mut outputs = graph_outputs(ports);
    node.process(&[&input], &mut outputs, &transport());
    outputs
}

fn is_silent(buffer: &AudioBuffer) -> bool {
    (0..2).all(|channel| buffer.channel(channel).iter().all(|&sample| sample == 0.0))
}

fn holds(buffer: &AudioBuffer, channel: usize, level: f32) -> bool {
    buffer
        .channel(channel)
        .iter()
        .all(|&sample| sample == level)
}

#[test]
fn an_instrument_nothing_reads_but_its_main_bus_renders_as_before() {
    let mut buses = FakeBuses::new(Vec::new(), drum_outputs());
    let (_listener, instance) = plugin(&mut buses);
    let mut instrument = VstInstrument::new(Arc::clone(&instance));
    instrument.prepare(prepare_context());

    let outputs = play_instrument(&mut instrument, 3);

    assert_eq!(instrument.num_output_ports(), 3);
    assert_eq!(buses.process_shapes(), vec![vec![2]], "one bus, as before");
    assert!(holds(&outputs[0], 0, MAIN) && holds(&outputs[0], 1, MAIN));
    assert!(is_silent(&outputs[1]) && is_silent(&outputs[2]));
    assert!(!instance.has_crashed());
}

#[test]
fn an_instrument_renders_the_bus_the_graph_reads_on_its_own_port() {
    let mut buses = FakeBuses::new(Vec::new(), drum_outputs());
    let (_listener, instance) = plugin(&mut buses);
    instance.set_vst3_wanted_outputs(0b100);
    let mut instrument = VstInstrument::new(Arc::clone(&instance));
    instrument.prepare(prepare_context());

    let outputs = play_instrument(&mut instrument, 3);

    assert_eq!(buses.process_shapes(), vec![vec![2, 1, 1]]);
    assert!(holds(&outputs[0], 0, MAIN) && holds(&outputs[0], 1, MAIN));
    assert!(is_silent(&outputs[1]), "the closed bus renders nowhere");
    assert!(
        holds(&outputs[2], 0, HAT) && holds(&outputs[2], 1, HAT),
        "a mono bus is spread over both channels"
    );
    assert!(!instance.has_crashed());
}

#[test]
fn a_new_wrapper_on_a_live_instrument_follows_the_new_demand() {
    let mut buses = FakeBuses::new(Vec::new(), drum_outputs());
    let (_listener, instance) = plugin(&mut buses);
    let mut first = VstInstrument::new(Arc::clone(&instance));
    first.prepare(prepare_context());
    play_instrument(&mut first, 3);

    instance.set_vst3_wanted_outputs(0b10);
    let mut second = VstInstrument::new(Arc::clone(&instance));
    second.prepare(prepare_context());
    let outputs = play_instrument(&mut second, 3);

    assert_eq!(buses.process_shapes(), vec![vec![2], vec![2, 1]]);
    assert!(holds(&outputs[1], 1, SNARE));
    assert!(is_silent(&outputs[2]));
}

#[test]
fn an_effect_renders_the_bus_the_graph_reads_on_its_own_port() {
    let mut buses = FakeBuses::new(
        vec![FakeBus::main("In", STEREO)],
        vec![
            FakeBus::main("Main", STEREO).writing(MAIN),
            FakeBus::aux("Wet", MONO).writing(SNARE),
        ],
    );
    let (_listener, instance) = plugin(&mut buses);
    instance.set_vst3_wanted_outputs(0b10);
    let mut node = VstWrapperNode::new(Arc::clone(&instance));
    node.prepare(prepare_context());

    let outputs = run_effect(&mut node, 2);

    assert_eq!(node.num_output_ports(), 2);
    assert_eq!(buses.process_shapes(), vec![vec![2, 1]]);
    assert!(holds(&outputs[0], 1, MAIN));
    assert!(holds(&outputs[1], 0, SNARE) && holds(&outputs[1], 1, SNARE));
}

#[test]
fn an_effect_nothing_reads_but_its_main_bus_renders_as_before() {
    let mut buses = FakeBuses::new(
        vec![FakeBus::main("In", STEREO)],
        vec![
            FakeBus::main("Main", STEREO).writing(MAIN),
            FakeBus::aux("Wet", MONO).writing(SNARE),
        ],
    );
    let (_listener, instance) = plugin(&mut buses);
    let mut node = VstWrapperNode::new(Arc::clone(&instance));
    node.prepare(prepare_context());

    let outputs = run_effect(&mut node, 2);

    assert_eq!(buses.process_shapes(), vec![vec![2]]);
    assert!(holds(&outputs[0], 0, MAIN));
    assert!(is_silent(&outputs[1]));
}

#[test]
fn a_surround_bus_is_handed_over_whole_and_the_engine_hears_its_first_two_channels() {
    let mut buses = FakeBuses::new(
        Vec::new(),
        vec![
            FakeBus::main("Main", STEREO).writing(MAIN),
            FakeBus::aux("Toms", SURROUND_5_1).writing(TOMS),
        ],
    );
    let (_listener, instance) = plugin(&mut buses);
    instance.set_vst3_wanted_outputs(0b10);
    let mut instrument = VstInstrument::new(Arc::clone(&instance));
    instrument.prepare(prepare_context());

    let outputs = play_instrument(&mut instrument, 2);

    assert_eq!(buses.process_shapes(), vec![vec![2, 6]]);
    assert!(holds(&outputs[1], 0, TOMS) && holds(&outputs[1], 1, TOMS));
    assert!(!instance.has_crashed());
}

#[test]
fn a_graph_without_the_aux_port_still_gives_the_plugin_valid_buffers() {
    let mut buses = FakeBuses::new(Vec::new(), drum_outputs());
    let (_listener, instance) = plugin(&mut buses);
    instance.set_vst3_wanted_outputs(0b100);
    let mut instrument = VstInstrument::new(Arc::clone(&instance));
    instrument.prepare(prepare_context());

    let outputs = play_instrument(&mut instrument, 1);

    assert!(holds(&outputs[0], 0, MAIN));
    assert!(!instance.has_crashed());
}
