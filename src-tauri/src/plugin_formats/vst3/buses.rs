//! Audio buses of a VST3 plugin (ADR-014 multi-out, VST3 catch-up step 4e).
//!
//! A component declares any number of audio output buses; graph output port `p`
//! is output bus `p`, the main bus being port 0.  The plugin is third-party
//! code, so every count it reports is bounded before it drives a loop.

use std::ffi::c_void;
use std::mem::{offset_of, size_of};

use super::ffi::{string128_to_string, IComponent, String128, K_AUDIO, K_OUTPUT, K_RESULT_OK};

/// Output buses read from a plugin.  The set of buses a host asks for is a `u32`
/// mask, so no more than 32 can be named.
pub const MAX_OUTPUT_BUSES: i32 = 32;

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
    let declared = (vtbl.get_bus_count)(component, K_AUDIO, K_OUTPUT).clamp(0, MAX_OUTPUT_BUSES);
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::plugin_formats::vst3::fake_plugin::{fake_vst3_instance, FakeListener};

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
}
