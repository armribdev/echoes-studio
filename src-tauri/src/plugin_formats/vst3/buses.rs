//! Audio buses of a VST3 plugin (ADR-014 multi-out, VST3 catch-up step 4e).
//!
//! A component declares any number of audio output buses; graph output port `p`
//! is output bus `p`, the main bus being port 0.  The plugin is third-party
//! code, so every count it reports is bounded before it drives a loop.

use std::ffi::c_void;
use std::mem::{offset_of, size_of};

use crate::audio_graph::buffer::AudioBuffer;

use super::ffi::{
    string128_to_string, AudioBusBuffers, IAudioProcessor, IComponent, String128, K_AUDIO, K_INPUT,
    K_OUTPUT, K_RESULT_OK, SPEAKER_STEREO,
};

/// Audio buses read per direction.  The set of output buses a host asks for is a
/// `u32` mask, so no more than 32 can be named.
pub const MAX_AUDIO_BUSES: i32 = 32;

/// Every output bus but the main one (bus 0, which is always open).
pub const AUX_OUTPUT_BUSES: u32 = !1;

/// Channels one bus may declare; a 7th order ambisonic bus has 64.
const MAX_BUS_CHANNELS: i32 = 64;

/// `BusInfo::BusFlags::kIsControlVoltage` (`ivstcomponent.h`).
const BUS_IS_CONTROL_VOLTAGE: u32 = 1 << 1;

/// `BusTypes::kAux` (`ivstcomponent.h`).
const BUS_TYPE_AUX: i32 = 1;

/// `Vst::BusInfo` (`ivstcomponent.h`).
#[repr(C)]
pub struct BusInfo {
    pub media_type: i32,
    pub direction: i32,
    pub channel_count: i32,
    pub name: String128,
    pub bus_type: i32,
    pub flags: u32,
}

const _: () = {
    assert!(size_of::<BusInfo>() == 276, "BusInfo layout drift");
    assert!(offset_of!(BusInfo, bus_type) == 268, "BusInfo layout drift");
};

/// One audio output bus, as the plugin describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vst3OutputBus {
    pub name: String,
    pub channel_count: u32,
    /// Carries control data at sample rate, not sound: never routed to a track.
    pub control_voltage: bool,
    /// Opened by the host at the last activation; an open bus gets its own
    /// sound, a closed one is mixed by the plugin into its main bus.
    pub active: bool,
}

/// The audio output buses of `component`, in bus order.  Reading stops at the
/// first bus the plugin will not describe, so a graph port is always the bus of
/// the same index.
///
/// # Safety
/// `component` must be a live, initialised `IComponent`, called off the audio
/// thread while no `process` call is in flight.
pub unsafe fn read_output_buses(component: *mut IComponent) -> Vec<Vst3OutputBus> {
    describe_buses(component, K_OUTPUT)
        .into_iter()
        .map(|info| Vst3OutputBus {
            name: string128_to_string(&info.name),
            channel_count: info.channel_count.clamp(0, MAX_BUS_CHANNELS) as u32,
            control_voltage: info.flags & BUS_IS_CONTROL_VOLTAGE != 0,
            active: false,
        })
        .collect()
}

/// One auxiliary audio input bus (a sidechain).  Graph input port `p` is input
/// bus `p`, the main bus being port 0, so the first auxiliary input is port 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vst3AuxInput {
    pub channel_count: u32,
}

/// The auxiliary audio input buses of `component`: the buses after the main one
/// that are of type `kAux`, up to the first that is not (the SDK puts every
/// `kMain` bus before the `kAux` ones).  A control-voltage input is not sound,
/// so it ends the run too.
///
/// # Safety
/// As [`read_output_buses`].
pub unsafe fn read_aux_inputs(component: *mut IComponent) -> Vec<Vst3AuxInput> {
    describe_buses(component, K_INPUT)
        .into_iter()
        .skip(1)
        .take_while(|info| {
            info.bus_type == BUS_TYPE_AUX && info.flags & BUS_IS_CONTROL_VOLTAGE == 0
        })
        .map(|info| Vst3AuxInput {
            channel_count: info.channel_count.clamp(0, MAX_BUS_CHANNELS) as u32,
        })
        .collect()
}

/// The audio buses of one direction as the plugin describes them, up to the
/// first one it will not describe.
unsafe fn describe_buses(component: *mut IComponent, direction: i32) -> Vec<BusInfo> {
    let vtbl = &*(*component).vtbl;
    let declared = (vtbl.get_bus_count)(component, K_AUDIO, direction).clamp(0, MAX_AUDIO_BUSES);
    (0..declared)
        .map_while(|index| {
            let mut info: BusInfo = std::mem::zeroed();
            let described = (vtbl.get_bus_info)(
                component,
                K_AUDIO,
                direction,
                index,
                &mut info as *mut BusInfo as *mut c_void,
            );
            (described == K_RESULT_OK).then_some(info)
        })
        .collect()
}

/// Opens the main output bus, and the auxiliary ones `wanted` names (bit `n` =
/// bus `n`) unless they carry control voltage.  A bus opened by an earlier
/// activation (`previous`) and no longer wanted is closed, because a bus stays
/// open until the host says otherwise.  Marks each bus of `buses` accordingly.
///
/// # Safety
/// As [`read_output_buses`], with the plugin not active.
pub unsafe fn open_output_buses(
    component: *mut IComponent,
    buses: &mut [Vst3OutputBus],
    wanted: u32,
    previous: u32,
) {
    let activate_bus = (*(*component).vtbl).activate_bus;
    activate_bus(component, K_AUDIO, K_OUTPUT, 0, 1);
    for (index, bus) in buses.iter_mut().enumerate() {
        let bit = 1u32 << index;
        bus.active = index == 0 || (wanted & bit != 0 && !bus.control_voltage);
        if index == 0 {
            continue;
        }
        if bus.active {
            activate_bus(component, K_AUDIO, K_OUTPUT, index as i32, 1);
        } else if previous & bit != 0 {
            activate_bus(component, K_AUDIO, K_OUTPUT, index as i32, 0);
        }
    }
}

/// Tells the plugin the arrangement of every audio bus it declared, as
/// `setBusArrangements` requires (one per bus, inputs and outputs).  The main bus
/// of each direction is asked for stereo; any other bus is offered the
/// arrangement it already has, so a mono or surround bus is not forced.  The
/// plugin may answer that it prefers another layout: the buses are read again
/// afterwards, so its answer is not needed here.
///
/// Returns whether the plugin has an audio input bus.
///
/// # Safety
/// As [`read_output_buses`], with `processor` the same plugin's `IAudioProcessor`,
/// before the plugin is activated.
pub unsafe fn propose_arrangements(
    component: *mut IComponent,
    processor: *mut IAudioProcessor,
) -> bool {
    let mut inputs = proposed_arrangements(component, processor, K_INPUT);
    let mut outputs = proposed_arrangements(component, processor, K_OUTPUT);
    ((*(*processor).vtbl).set_bus_arrangements)(
        processor,
        arrangements_ptr(&mut inputs),
        inputs.len() as i32,
        arrangements_ptr(&mut outputs),
        outputs.len() as i32,
    );
    !inputs.is_empty()
}

unsafe fn proposed_arrangements(
    component: *mut IComponent,
    processor: *mut IAudioProcessor,
    direction: i32,
) -> Vec<u64> {
    let declared = ((*(*component).vtbl).get_bus_count)(component, K_AUDIO, direction)
        .clamp(0, MAX_AUDIO_BUSES);
    (0..declared)
        .map(|index| match index {
            0 => SPEAKER_STEREO,
            _ => current_arrangement(processor, direction, index),
        })
        .collect()
}

/// The arrangement a bus has now; stereo when the plugin will not say.
unsafe fn current_arrangement(processor: *mut IAudioProcessor, direction: i32, index: i32) -> u64 {
    let mut arrangement = 0;
    let answered =
        ((*(*processor).vtbl).get_bus_arrangement)(processor, direction, index, &mut arrangement);
    if answered == K_RESULT_OK {
        arrangement
    } else {
        SPEAKER_STEREO
    }
}

/// A plugin with no bus in a direction is given a null array, not a dangling one.
fn arrangements_ptr(arrangements: &mut [u64]) -> *mut u64 {
    if arrangements.is_empty() {
        std::ptr::null_mut()
    } else {
        arrangements.as_mut_ptr()
    }
}

/// One graph output port and the bus behind it.
struct OutputSlot {
    /// Channels the bus declared; 0 when the plugin did not say.
    declared: usize,
    open: bool,
}

/// Graph port → bus mapping plus the per-block tables handed to
/// `ProcessData::outputs` (ADR-014 multi-out), shared by the effect and the
/// instrument.  Port `p` is output bus `p`; the main bus keeps the engine's
/// channel count, an auxiliary bus its declared one.
///
/// ## Real-time contract
///
/// Everything is allocated in `prepare`; `bind` and `finish` only write the
/// pointer tables and copy samples.
pub struct Vst3OutputBuses {
    slots: Vec<OutputSlot>,
    /// Sample pointers of each bus: the graph buffer's channels, or `spare` for a
    /// channel the graph has no room for and for a closed bus.
    channel_ptrs: Vec<Vec<*mut f32>>,
    /// The `AudioBusBuffers` of every bus up to the last open one.
    table: Vec<AudioBusBuffers>,
    spare: AudioBuffer,
    num_channels: usize,
}

impl Vst3OutputBuses {
    /// A plugin whose buses are not known (none read yet, or none declared) has
    /// the single main bus, as before multi-out.
    pub fn new(buses: &[Vst3OutputBus]) -> Self {
        let slots = if buses.is_empty() {
            vec![OutputSlot {
                declared: 0,
                open: true,
            }]
        } else {
            buses
                .iter()
                .map(|bus| OutputSlot {
                    declared: bus.channel_count as usize,
                    open: bus.active,
                })
                .collect()
        };
        Vst3OutputBuses {
            slots,
            channel_ptrs: Vec::new(),
            table: Vec::new(),
            spare: AudioBuffer::silence(),
            num_channels: 0,
        }
    }

    /// Output ports the graph node exposes: every declared bus, open or not.
    pub fn num_ports(&self) -> usize {
        self.slots.len()
    }

    pub fn prepare(&mut self, num_channels: usize, max_block_size: usize) {
        self.num_channels = num_channels;
        self.spare = AudioBuffer::new(num_channels, max_block_size);
        let spare = self.spare.channel_mut(0).as_mut_ptr();
        self.channel_ptrs = (0..self.slots.len())
            .map(|port| vec![spare; self.plugin_channels(port)])
            .collect();
        let open_buses = self
            .slots
            .iter()
            .rposition(|slot| slot.open)
            .map_or(1, |last| last + 1);
        let mut table = Vec::with_capacity(open_buses);
        for port in 0..open_buses {
            table.push(AudioBusBuffers {
                num_channels: self.plugin_channels(port) as i32,
                silence_flags: 0,
                channel_buffers32: self.channel_ptrs[port].as_mut_ptr(),
            });
        }
        self.table = table;
    }

    /// Channels the plugin writes on port `port`; an auxiliary bus may have more
    /// than the engine, the plugin being told the layout it declared.
    fn plugin_channels(&self, port: usize) -> usize {
        match self.slots[port].declared {
            declared if port == 0 || declared == 0 => self.num_channels,
            declared => declared,
        }
    }

    /// Point every open bus at its graph output.  Returns the table and its
    /// length, for `ProcessData::outputs` / `numOutputs`: the buses up to the last
    /// open one, so a closed bus between two open ones is still a valid entry.
    pub fn bind(&mut self, outputs: &mut [AudioBuffer]) -> (*mut AudioBusBuffers, i32) {
        for port in 0..self.table.len() {
            if !self.slots[port].open {
                continue;
            }
            let bound = self.plugin_channels(port).min(self.num_channels);
            let target = match outputs.get_mut(port) {
                Some(buffer) => buffer,
                None => &mut self.spare,
            };
            for channel in 0..bound {
                self.channel_ptrs[port][channel] = target.channel_mut(channel).as_mut_ptr();
            }
        }
        (self.table.as_mut_ptr(), self.table.len() as i32)
    }

    /// Spread a mono auxiliary bus over every engine channel, so a receiving
    /// track hears it centred rather than left only.
    pub fn finish(&self, outputs: &mut [AudioBuffer]) {
        for port in 1..self.table.len().min(outputs.len()) {
            if self.slots[port].open && self.plugin_channels(port) == 1 {
                outputs[port].mono_to_all_channels();
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::plugin_formats::vst3::fake_plugin::{
        fake_vst3_instance, fake_vst3_instance_with_buses, FakeBus, FakeBuses, FakeListener, MONO,
        STEREO, SURROUND_5_1,
    };
    use crate::plugin_formats::vst3::ffi::K_INPUT;

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
    fn a_sidechain_input_is_named_in_the_request_and_stays_closed() {
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
            vec![(K_INPUT, 0, true), (K_OUTPUT, 0, true)]
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
}
