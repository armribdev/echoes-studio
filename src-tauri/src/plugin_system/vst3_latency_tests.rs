//! A VST3 effect and a VST3 instrument hand their plugin's latency to the graph
//! compiler (ADR-006 PDC), and read it again after a `kLatencyChanged` restart,
//! through a hand-made plugin whose latency the test sets.

use std::sync::Arc;

use crate::audio_graph::node::{AudioNode, PrepareContext};
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::plugin_formats::vst3::fake_plugin::{
    fake_vst3_instance_with_buses, FakeBus, FakeBuses, FakeListener, STEREO,
};
use crate::plugin_formats::vst3::instrument::VstInstrument;
use crate::plugin_formats::vst3::wrapper::VstWrapperNode;
use crate::plugin_system::instance::{PluginInstance, FFI_GATE_HARVEST_BUDGET};

fn prepare_context() -> PrepareContext {
    PrepareContext {
        sample_rate: 48_000.0,
        max_block_size: 512,
        num_channels: 2,
        live: true,
    }
}

fn plugin(buses: &mut FakeBuses) -> (Box<FakeListener>, Arc<PluginInstance>) {
    let mut listener = FakeListener::new(false);
    let instance = fake_vst3_instance_with_buses(&mut listener, buses);
    (listener, instance)
}

fn instrument_buses(latency: u32) -> Box<FakeBuses> {
    let buses = FakeBuses::new(Vec::new(), vec![FakeBus::main("Main", STEREO)]);
    buses.set_latency(latency);
    buses
}

/// As the plugin does from `restartComponent(kLatencyChanged)`; the handler's own
/// part is pinned by `component_handler.rs`.
fn plugin_asks_for_a_restart(instance: &PluginInstance) {
    instance
        .vst3_edit_state()
        .expect("a VST3 instance")
        .request_reactivation();
}

#[test]
fn an_instrument_reports_the_latency_of_its_plugin() {
    let mut buses = instrument_buses(96);
    let (_listener, instance) = plugin(&mut buses);
    let mut instrument = VstInstrument::new(instance);

    assert_eq!(
        instrument.latency_samples(),
        0,
        "nothing is read before prepare"
    );
    instrument.prepare(prepare_context());

    assert_eq!(instrument.latency_samples(), 96);
}

#[test]
fn an_effect_still_reports_the_latency_of_its_plugin() {
    let mut buses = FakeBuses::new(
        vec![FakeBus::main("In", STEREO)],
        vec![FakeBus::main("Out", STEREO)],
    );
    buses.set_latency(40);
    let (_listener, instance) = plugin(&mut buses);
    let mut node = VstWrapperNode::new(instance);

    node.prepare(prepare_context());

    assert_eq!(node.latency_samples(), 40);
}

#[test]
fn an_instrument_reads_the_new_latency_once_its_plugin_is_reactivated() {
    let mut buses = instrument_buses(64);
    let (_listener, instance) = plugin(&mut buses);
    let mut instrument = VstInstrument::new(Arc::clone(&instance));
    instrument.prepare(prepare_context());
    let activations = buses.activations().len();

    buses.set_latency(128);
    plugin_asks_for_a_restart(&instance);
    instrument.prepare(prepare_context());

    assert_eq!(instrument.latency_samples(), 128);
    assert!(
        buses.activations().len() > activations,
        "the plugin was reactivated, not just asked again"
    );
}

#[test]
fn a_block_in_flight_keeps_the_previous_latency_until_the_next_prepare() {
    let mut buses = instrument_buses(64);
    let (_listener, instance) = plugin(&mut buses);
    let mut instrument = VstInstrument::new(Arc::clone(&instance));
    instrument.prepare(prepare_context());
    let activations = buses.activations().len();

    buses.set_latency(128);
    plugin_asks_for_a_restart(&instance);
    let block_in_flight = instance
        .offrt_try_acquire(FFI_GATE_HARVEST_BUDGET)
        .expect("an idle instance gives its gate");
    instrument.prepare(prepare_context());

    assert_eq!(
        instrument.latency_samples(),
        64,
        "a busy gate changes nothing"
    );
    assert_eq!(
        buses.activations().len(),
        activations,
        "no reactivation either"
    );

    drop(block_in_flight);
    instrument.prepare(prepare_context());

    assert_eq!(instrument.latency_samples(), 128, "the request was kept");
    assert!(buses.activations().len() > activations);
}
