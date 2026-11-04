//! The audio buses of a VST3 plugin (`vst3_host::buses`), read from the hand-made plugin of
//! `fake_plugin.rs` and bound to the graph's `AudioBuffer`s.

use crate::plugin_formats::vst3::fake_plugin::{
    fake_vst3_instance, fake_vst3_instance_with_buses, FakeBus, FakeBuses, FakeListener, MONO,
    STEREO, SURROUND_5_1,
};
use crate::plugin_formats::vst3::ffi::K_INPUT;
use audio_buffer::AudioBuffer;
use vst3_ffi::*;
use vst3_host::buses::*;

#[test]
fn a_plugin_with_one_stereo_output_reports_it() {
    let mut listener = FakeListener::new(false);
    let instance = fake_vst3_instance(&mut listener);
    let (component, _) = instance.vst3_handles().expect("a VST3 instance");

    let buses = unsafe { read_output_buses(component.as_ptr()) };

    assert_eq!(
        buses,
        vec![Vst3OutputBus {
            name: "Stereo Out".into(),
            channel_count: 2,
            control_voltage: false,
            active: false,
        }]
    );
}

#[test]
fn activation_keeps_the_buses_for_the_instance() {
    let mut listener = FakeListener::new(false);
    let instance = fake_vst3_instance(&mut listener);
    assert!(instance.vst3_output_buses().is_empty(), "nothing read yet");

    assert!(instance.vst3_ensure_active(48_000.0, 512));

    let buses = instance.vst3_output_buses();
    assert_eq!(buses.len(), 1);
    assert_eq!(buses[0].channel_count, 2);
}

#[test]
fn an_effect_with_one_bus_each_way_gets_the_request_it_always_got() {
    let mut listener = FakeListener::new(false);
    let mut buses = FakeBuses::new(
        vec![FakeBus::main("In", STEREO)],
        vec![FakeBus::main("Out", STEREO)],
    );
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);

    assert!(instance.vst3_ensure_active(48_000.0, 512));

    assert_eq!(
        buses.arrangement_calls(),
        vec![(vec![STEREO], vec![STEREO])]
    );
    assert_eq!(
        buses.activations(),
        vec![(K_INPUT, 0, true), (K_OUTPUT, 0, true)]
    );
}

#[test]
fn every_bus_of_an_instrument_is_named_in_the_request_but_only_the_main_opens() {
    let mut listener = FakeListener::new(false);
    let mut buses = FakeBuses::new(
        Vec::new(),
        vec![
            FakeBus::main("Main", STEREO),
            FakeBus::aux("Snare", MONO),
            FakeBus::aux("Toms", SURROUND_5_1),
        ],
    );
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);

    assert!(instance.vst3_ensure_active(48_000.0, 512));

    assert_eq!(
        buses.arrangement_calls(),
        vec![(Vec::new(), vec![STEREO, MONO, SURROUND_5_1])],
        "the aux buses keep the layout they had"
    );
    assert_eq!(buses.activations(), vec![(K_OUTPUT, 0, true)]);
    let read = instance.vst3_output_buses();
    let layout: Vec<_> = read
        .iter()
        .map(|bus| (bus.name.as_str(), bus.channel_count))
        .collect();
    assert_eq!(layout, vec![("Main", 2), ("Snare", 1), ("Toms", 6)]);
}

#[test]
fn a_sidechain_input_is_named_in_the_request_and_opened_with_the_main_one() {
    let mut listener = FakeListener::new(false);
    let mut buses = FakeBuses::new(
        vec![FakeBus::main("In", STEREO), FakeBus::aux("Sidechain", MONO)],
        vec![FakeBus::main("Out", STEREO)],
    );
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);

    assert!(instance.vst3_ensure_active(48_000.0, 512));

    assert_eq!(
        buses.arrangement_calls(),
        vec![(vec![STEREO, MONO], vec![STEREO])]
    );
    assert_eq!(
        buses.activations(),
        vec![(K_INPUT, 0, true), (K_INPUT, 1, true), (K_OUTPUT, 0, true)]
    );
    assert_eq!(
        instance.vst3_aux_inputs().to_vec(),
        vec![Vst3AuxInput { channel_count: 1 }]
    );
}

#[test]
fn a_plugin_that_keeps_its_own_layout_is_read_as_it_answered() {
    let mut listener = FakeListener::new(false);
    let mut buses =
        FakeBuses::new(Vec::new(), vec![FakeBus::main("Out", MONO)]).keeping_its_layout();
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);

    assert!(instance.vst3_ensure_active(48_000.0, 512));

    assert_eq!(buses.arrangement_calls(), vec![(Vec::new(), vec![STEREO])]);
    let read = instance.vst3_output_buses();
    assert_eq!(read.len(), 1);
    assert_eq!(
        read[0].channel_count, 1,
        "the plugin's answer, not the request"
    );
}

#[test]
fn a_control_voltage_bus_is_flagged() {
    let mut listener = FakeListener::new(false);
    let mut buses = FakeBuses::new(
        Vec::new(),
        vec![
            FakeBus::main("Main", STEREO),
            FakeBus::aux("Gate", MONO).control_voltage(),
        ],
    );
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);

    assert!(instance.vst3_ensure_active(48_000.0, 512));

    let read = instance.vst3_output_buses();
    assert_eq!(
        read.iter()
            .map(|bus| bus.control_voltage)
            .collect::<Vec<_>>(),
        vec![false, true]
    );
}

fn aux_inputs_of(inputs: Vec<FakeBus>) -> Vec<Vst3AuxInput> {
    let mut listener = FakeListener::new(false);
    let mut buses = FakeBuses::new(inputs, vec![FakeBus::main("Out", STEREO)]);
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);
    let (component, _) = instance.vst3_handles().expect("a VST3 instance");
    unsafe { read_aux_inputs(component.as_ptr()) }
}

fn aux_input(channel_count: u32) -> Vst3AuxInput {
    Vst3AuxInput { channel_count }
}

#[test]
fn a_sidechain_after_the_main_input_is_an_aux_input() {
    let read = aux_inputs_of(vec![
        FakeBus::main("In", STEREO),
        FakeBus::aux("Sidechain", MONO),
    ]);

    assert_eq!(read, vec![aux_input(1)]);
}

#[test]
fn an_effect_with_only_a_main_input_and_a_plugin_with_none_have_no_aux_input() {
    assert!(aux_inputs_of(vec![FakeBus::main("In", STEREO)]).is_empty());
    assert!(aux_inputs_of(Vec::new()).is_empty());
}

#[test]
fn every_aux_input_is_read_with_its_own_channel_count() {
    let read = aux_inputs_of(vec![
        FakeBus::main("In", STEREO),
        FakeBus::aux("Key", MONO),
        FakeBus::aux("Detector", STEREO),
    ]);

    assert_eq!(read, vec![aux_input(1), aux_input(2)]);
}

#[test]
fn a_control_voltage_input_is_not_a_sidechain_and_ends_the_run() {
    let read = aux_inputs_of(vec![
        FakeBus::main("In", STEREO),
        FakeBus::aux("Key", MONO),
        FakeBus::aux("Cv", MONO).control_voltage(),
        FakeBus::aux("Late", MONO),
    ]);

    assert_eq!(read, vec![aux_input(1)]);
}

#[test]
fn a_second_main_input_is_not_an_aux_input() {
    let read = aux_inputs_of(vec![
        FakeBus::main("In", STEREO),
        FakeBus::main("In 2", STEREO),
        FakeBus::aux("Key", MONO),
    ]);

    assert!(
        read.is_empty(),
        "port p stays input bus p: no gap is skipped"
    );
}

fn three_outputs() -> Box<FakeBuses> {
    FakeBuses::new(
        Vec::new(),
        vec![
            FakeBus::main("Main", STEREO),
            FakeBus::aux("Snare", MONO),
            FakeBus::aux("Hat", MONO),
        ],
    )
}

fn open_flags(instance: &crate::plugin_system::instance::PluginInstance) -> Vec<bool> {
    instance
        .vst3_output_buses()
        .iter()
        .map(|bus| bus.active)
        .collect()
}

#[test]
fn nothing_is_open_but_the_main_bus_until_the_graph_reads_another() {
    let mut listener = FakeListener::new(false);
    let mut buses = three_outputs();
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);

    assert!(instance.vst3_ensure_active(48_000.0, 512));

    assert_eq!(buses.activations(), vec![(K_OUTPUT, 0, true)]);
    assert_eq!(open_flags(&instance), vec![true, false, false]);
}

#[test]
fn a_bus_the_graph_reads_is_opened_with_the_main_one() {
    let mut listener = FakeListener::new(false);
    let mut buses = three_outputs();
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);
    instance.set_vst3_wanted_outputs(0b100);

    assert!(instance.vst3_ensure_active(48_000.0, 512));

    assert_eq!(
        buses.activations(),
        vec![(K_OUTPUT, 0, true), (K_OUTPUT, 2, true)]
    );
    assert_eq!(open_flags(&instance), vec![true, false, true]);
}

#[test]
fn a_live_plugin_is_reactivated_to_open_and_to_close_a_bus() {
    let mut listener = FakeListener::new(false);
    let mut buses = three_outputs();
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);
    assert!(instance.vst3_ensure_active(48_000.0, 512));

    instance.set_vst3_wanted_outputs(0b10);
    assert!(instance.vst3_ensure_active(48_000.0, 512));
    assert_eq!(open_flags(&instance), vec![true, true, false]);

    instance.set_vst3_wanted_outputs(0b100);
    assert!(instance.vst3_ensure_active(48_000.0, 512));
    assert_eq!(open_flags(&instance), vec![true, false, true]);
    assert_eq!(
        buses.activations()[buses.activations().len() - 3..],
        [
            (K_OUTPUT, 0, true),
            (K_OUTPUT, 1, false),
            (K_OUTPUT, 2, true)
        ],
        "bus 1 is closed explicitly: a bus stays open across a reactivation"
    );
}

#[test]
fn the_same_demand_does_not_reactivate_the_plugin() {
    let mut listener = FakeListener::new(false);
    let mut buses = three_outputs();
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);
    instance.set_vst3_wanted_outputs(0b10);
    assert!(instance.vst3_ensure_active(48_000.0, 512));
    let activations = buses.activations();

    assert!(instance.vst3_ensure_active(48_000.0, 512));

    assert_eq!(buses.activations(), activations);
    assert_eq!(buses.arrangement_calls().len(), 1);
}

#[test]
fn the_main_bus_a_missing_bus_and_a_control_voltage_bus_are_not_asked_for() {
    let mut listener = FakeListener::new(false);
    let mut buses = FakeBuses::new(
        Vec::new(),
        vec![
            FakeBus::main("Main", STEREO),
            FakeBus::aux("Gate", MONO).control_voltage(),
        ],
    );
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);
    instance.set_vst3_wanted_outputs(0b1 | 0b10 | 1 << 5);

    assert!(instance.vst3_ensure_active(48_000.0, 512));

    assert_eq!(buses.activations(), vec![(K_OUTPUT, 0, true)]);
    assert_eq!(open_flags(&instance), vec![true, false]);
}

#[test]
fn a_restart_that_changes_the_buses_reads_them_again() {
    let mut listener = FakeListener::new(false);
    let mut buses = three_outputs();
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);
    instance.set_vst3_wanted_outputs(0b100);
    assert!(instance.vst3_ensure_active(48_000.0, 512));
    assert_eq!(instance.vst3_output_buses().len(), 3);

    buses.limit_outputs_to(1);
    instance
        .vst3_edit_state()
        .expect("a VST3 instance")
        .request_reactivation();
    assert!(instance.vst3_ensure_active(48_000.0, 512));

    assert_eq!(open_flags(&instance), vec![true]);
}

fn bus(channels: u32, active: bool) -> Vst3OutputBus {
    Vst3OutputBus {
        name: String::new(),
        channel_count: channels,
        control_voltage: false,
        active,
    }
}

fn blocks(count: usize) -> Vec<AudioBuffer> {
    (0..count).map(|_| AudioBuffer::new(2, 8)).collect()
}

/// The bus table the plugin sees, as `(channels, first sample pointer)`.
fn seen_by_the_plugin(table: *mut AudioBusBuffers, count: i32) -> Vec<(i32, *mut f32)> {
    (0..count as usize)
        .map(|index| unsafe {
            let bus = &*table.add(index);
            (bus.num_channels, *bus.channel_buffers32)
        })
        .collect()
}

#[test]
fn a_plugin_with_no_known_bus_has_the_single_main_bus() {
    let mut buses = Vst3OutputBuses::new(&[]);
    buses.prepare(2, 8);
    let mut outputs = blocks(1);
    let main = outputs[0].channel_mut(0).as_mut_ptr();

    let (table, count) = buses.bind(&mut outputs);

    assert_eq!((buses.num_ports(), count), (1, 1));
    assert_eq!(seen_by_the_plugin(table, count), vec![(2, main)]);
}

#[test]
fn closed_buses_after_the_last_open_one_are_not_handed_over() {
    let mut buses = Vst3OutputBuses::new(&[bus(2, true), bus(1, false), bus(1, false)]);
    buses.prepare(2, 8);
    let mut outputs = blocks(3);

    let (_, count) = buses.bind(&mut outputs);

    assert_eq!(buses.num_ports(), 3, "the graph still has a port per bus");
    assert_eq!(count, 1, "the plugin sees what it saw before multi-out");
}

#[test]
fn a_closed_bus_between_open_ones_is_valid_memory_the_open_ones_are_bound() {
    let mut buses = Vst3OutputBuses::new(&[bus(2, true), bus(1, false), bus(1, true)]);
    buses.prepare(2, 8);
    let mut outputs = blocks(3);
    let main = outputs[0].channel_mut(0).as_mut_ptr();
    let third = outputs[2].channel_mut(0).as_mut_ptr();

    let (table, count) = buses.bind(&mut outputs);

    let seen = seen_by_the_plugin(table, count);
    assert_eq!(count, 3);
    assert_eq!((seen[0], seen[2]), ((2, main), (1, third)));
    assert_eq!(seen[1].0, 1, "the closed bus keeps its channel count");
    assert!(!seen[1].1.is_null());
    assert_ne!(seen[1].1, outputs[1].channel_mut(0).as_mut_ptr());
}

#[test]
fn a_bus_with_more_channels_than_the_engine_gets_valid_pointers_for_all() {
    let mut buses = Vst3OutputBuses::new(&[bus(2, true), bus(6, true)]);
    buses.prepare(2, 8);
    let mut outputs = blocks(2);
    let left = outputs[1].channel_mut(0).as_mut_ptr();
    let right = outputs[1].channel_mut(1).as_mut_ptr();

    let (table, _) = buses.bind(&mut outputs);

    unsafe {
        let surround = &*table.add(1);
        assert_eq!(surround.num_channels, 6);
        let channels = std::slice::from_raw_parts(surround.channel_buffers32, 6);
        assert_eq!((channels[0], channels[1]), (left, right));
        assert!(channels.iter().all(|channel| !channel.is_null()));
    }
}

#[test]
fn a_mono_bus_is_spread_over_both_channels_and_the_main_bus_is_left_alone() {
    let mut buses = Vst3OutputBuses::new(&[bus(2, true), bus(1, true)]);
    buses.prepare(2, 8);
    let mut outputs = blocks(2);
    buses.bind(&mut outputs);
    outputs[0].channel_mut(0).fill(0.5);
    outputs[1].channel_mut(0).fill(0.25);

    buses.finish(&mut outputs);

    assert!(outputs[1].channel(1).iter().all(|&sample| sample == 0.25));
    assert!(outputs[0].channel(1).iter().all(|&sample| sample == 0.0));
}

#[test]
fn a_graph_output_that_is_missing_gets_a_spare_buffer_not_null() {
    let mut buses = Vst3OutputBuses::new(&[bus(2, true), bus(2, true)]);
    buses.prepare(2, 8);
    let mut outputs = blocks(1);

    let (table, count) = buses.bind(&mut outputs);

    assert_eq!(count, 2);
    assert!(!seen_by_the_plugin(table, count)[1].1.is_null());
}

const FRAMES: usize = 8;

fn input_buses(aux_channels: &[u32]) -> Vst3InputBuses {
    let aux: Vec<_> = aux_channels
        .iter()
        .map(|&channel_count| Vst3AuxInput { channel_count })
        .collect();
    let mut buses = Vst3InputBuses::new(&aux);
    buses.prepare(2, FRAMES);
    buses
}

fn stereo(left: f32, right: f32) -> AudioBuffer {
    let mut buffer = AudioBuffer::new(2, FRAMES);
    buffer.channel_mut(0).fill(left);
    buffer.channel_mut(1).fill(right);
    buffer
}

/// The bus `index` as the plugin sees it: channels, silence flags, first
/// sample of each channel.
fn seen_input(table: *mut AudioBusBuffers, index: usize) -> (i32, u64, Vec<f32>) {
    unsafe {
        let bus = &*table.add(index);
        let channels = std::slice::from_raw_parts(bus.channel_buffers32, bus.num_channels as usize);
        let first = channels
            .iter()
            .map(|&channel| {
                assert!(!channel.is_null(), "an auxiliary bus has no null channel");
                *channel
            })
            .collect();
        (bus.num_channels, bus.silence_flags, first)
    }
}

#[test]
fn a_plugin_with_no_aux_input_has_the_single_main_bus_as_before() {
    let mut buses = input_buses(&[]);
    let main = stereo(0.5, 0.25);

    let (table, count) = buses.bind(&[&main], FRAMES);

    assert_eq!((buses.num_ports(), count), (1, 1));
    assert_eq!(seen_input(table, 0), (2, 0, vec![0.5, 0.25]));
}

#[test]
fn a_main_bus_with_no_input_is_flagged_silent() {
    let mut buses = input_buses(&[]);

    let (table, count) = buses.bind(&[], FRAMES);

    assert_eq!(count, 1);
    assert_eq!(unsafe { (*table).silence_flags }, u64::MAX);
}

#[test]
fn a_stereo_aux_input_gets_the_channels_of_its_source() {
    let mut buses = input_buses(&[2]);
    let (main, source) = (stereo(0.5, 0.5), stereo(0.25, -0.75));

    let (table, count) = buses.bind(&[&main, &source], FRAMES);

    assert_eq!((buses.num_ports(), count), (2, 2));
    assert_eq!(seen_input(table, 1), (2, 0, vec![0.25, -0.75]));
}

#[test]
fn an_aux_input_nothing_feeds_is_zeros_with_every_channel_flagged() {
    let mut buses = input_buses(&[2]);
    let main = stereo(0.5, 0.5);

    let (table, count) = buses.bind(&[&main], FRAMES);

    assert_eq!(
        count, 2,
        "the opened bus is handed over though no port feeds it"
    );
    assert_eq!(seen_input(table, 1), (2, 0b11, vec![0.0, 0.0]));
}

#[test]
fn a_source_with_no_signal_is_flagged_silent_per_channel() {
    let mut buses = input_buses(&[2]);
    let (main, source) = (stereo(0.5, 0.5), stereo(0.0, 0.1));

    let (table, _) = buses.bind(&[&main, &source], FRAMES);

    assert_eq!(
        seen_input(table, 1).1,
        0b01,
        "only the left channel is zero"
    );
}

#[test]
fn a_silence_flag_follows_the_block_not_the_last_one() {
    let mut buses = input_buses(&[2]);
    let main = stereo(0.5, 0.5);
    buses.bind(&[&main, &stereo(0.0, 0.0)], FRAMES);

    let (table, _) = buses.bind(&[&main, &stereo(0.3, 0.3)], FRAMES);

    assert_eq!(seen_input(table, 1).1, 0);
}

#[test]
fn a_mono_aux_input_gets_the_average_of_a_stereo_source() {
    let mut buses = input_buses(&[1]);
    let (main, source) = (stereo(0.5, 0.5), stereo(0.5, -0.25));

    let (table, _) = buses.bind(&[&main, &source], FRAMES);

    assert_eq!(seen_input(table, 1), (1, 0, vec![0.125]));
    assert_eq!(
        seen_input(table, 0).2,
        vec![0.5, 0.5],
        "the main bus is left alone"
    );
}

#[test]
fn a_mono_mix_of_opposite_signals_is_flagged_silent() {
    let mut buses = input_buses(&[1]);
    let (main, source) = (stereo(0.5, 0.5), stereo(0.5, -0.5));

    let (table, _) = buses.bind(&[&main, &source], FRAMES);

    assert_eq!(seen_input(table, 1), (1, 0b1, vec![0.0]));
}

#[test]
fn two_mono_aux_inputs_keep_their_own_mix() {
    let mut buses = input_buses(&[1, 1]);
    let main = stereo(0.5, 0.5);
    let (first, second) = (stereo(0.2, 0.4), stereo(1.0, 0.0));

    let (table, count) = buses.bind(&[&main, &first, &second], FRAMES);

    assert_eq!(count, 3);
    assert!((seen_input(table, 1).2[0] - 0.3).abs() < 1e-6);
    assert_eq!(seen_input(table, 2).2, vec![0.5]);
}

#[test]
fn an_aux_bus_wider_than_the_engine_gets_zeros_on_the_extra_channels() {
    let mut buses = input_buses(&[6]);
    let (main, source) = (stereo(0.5, 0.5), stereo(0.25, 0.75));

    let (table, _) = buses.bind(&[&main, &source], FRAMES);

    let (channels, flags, first) = seen_input(table, 1);
    assert_eq!(channels, 6);
    assert_eq!(first, vec![0.25, 0.75, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(flags, 0b111100);
}

#[test]
fn an_aux_bus_that_declares_no_channel_takes_the_engine_layout() {
    let mut buses = input_buses(&[0]);
    let (main, source) = (stereo(0.5, 0.5), stereo(0.25, 0.75));

    let (table, _) = buses.bind(&[&main, &source], FRAMES);

    assert_eq!(seen_input(table, 1), (2, 0, vec![0.25, 0.75]));
}

#[test]
fn a_block_shorter_than_the_buffers_is_judged_on_its_own_frames() {
    let mut buses = input_buses(&[2]);
    let main = stereo(0.5, 0.5);
    let mut source = stereo(0.0, 0.0);
    source.channel_mut(0)[FRAMES - 1] = 0.9;

    let (table, _) = buses.bind(&[&main, &source], FRAMES / 2);

    assert_eq!(
        seen_input(table, 1).1,
        0b11,
        "the late sample is not in this block"
    );
}

#[test]
fn a_plugin_that_will_not_describe_a_bus_ends_the_list() {
    let mut listener = FakeListener::new(false);
    let mut buses = FakeBuses::new(Vec::new(), vec![FakeBus::main("Main", STEREO)]);
    buses.declare_more_than_it_describes(2);
    let instance = fake_vst3_instance_with_buses(&mut listener, &mut buses);
    let (component, _) = instance.vst3_handles().expect("a VST3 instance");

    let read = unsafe { read_output_buses(component.as_ptr()) };

    assert_eq!(read.len(), 1, "reading stops at the first refusal");
}
