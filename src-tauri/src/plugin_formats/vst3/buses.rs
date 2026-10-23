//! Audio buses of a VST3 plugin (ADR-014 multi-out, VST3 catch-up step 4e).
//!
//! A component declares any number of audio output buses; graph output port `p`
//! is output bus `p`, the main bus being port 0.  The plugin is third-party
//! code, so every count it reports is bounded before it drives a loop.

use std::ffi::c_void;
use std::mem::{offset_of, size_of};

use super::ffi::{
    string128_to_string, IAudioProcessor, IComponent, String128, K_AUDIO, K_INPUT, K_OUTPUT,
    K_RESULT_OK, SPEAKER_STEREO,
};

/// Audio buses read per direction.  The set of output buses a host asks for is a
/// `u32` mask, so no more than 32 can be named.
pub const MAX_AUDIO_BUSES: i32 = 32;

/// Channels one bus may declare; a 7th order ambisonic bus has 64.
const MAX_BUS_CHANNELS: i32 = 64;

/// `BusInfo::BusFlags::kIsControlVoltage` (`ivstcomponent.h`).
const BUS_IS_CONTROL_VOLTAGE: u32 = 1 << 1;

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
}

/// The audio output buses of `component`, in bus order.  Reading stops at the
/// first bus the plugin will not describe, so a graph port is always the bus of
/// the same index.
///
/// # Safety
/// `component` must be a live, initialised `IComponent`, called off the audio
/// thread while no `process` call is in flight.
pub unsafe fn read_output_buses(component: *mut IComponent) -> Vec<Vst3OutputBus> {
    let vtbl = &*(*component).vtbl;
    let declared = (vtbl.get_bus_count)(component, K_AUDIO, K_OUTPUT).clamp(0, MAX_AUDIO_BUSES);
    (0..declared)
        .map_while(|index| {
            let mut info: BusInfo = std::mem::zeroed();
            let described = (vtbl.get_bus_info)(
                component,
                K_AUDIO,
                K_OUTPUT,
                index,
                &mut info as *mut BusInfo as *mut c_void,
            );
            (described == K_RESULT_OK).then(|| Vst3OutputBus {
                name: string128_to_string(&info.name),
                channel_count: info.channel_count.clamp(0, MAX_BUS_CHANNELS) as u32,
                control_voltage: info.flags & BUS_IS_CONTROL_VOLTAGE != 0,
            })
        })
        .collect()
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
