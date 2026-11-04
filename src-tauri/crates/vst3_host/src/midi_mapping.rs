//! MIDI controllers for a VST3 instrument (`IMidiMapping`).
//!
//! VST3 has no raw MIDI input: pitch bend, control changes and channel
//! aftertouch reach a plugin as changes of the parameters its controller maps
//! them to.  The table is read off the audio thread (`prepare`); each block only
//! looks it up.

use std::ffi::c_void;

use vst3_ffi::{
    HostParameterChanges, IEditController, TResult, Tuid, IID_IMIDI_MAPPING, K_RESULT_OK,
};

/// `ControllerNumbers` (`ivstmidicontrollers.h`): 0..127 are the MIDI CCs.
pub const CTRL_AFTER_TOUCH: i16 = 128;
pub const CTRL_PITCH_BEND: i16 = 129;
const CONTROLLER_COUNT: usize = 130;

const MIDI_CONTROL_CHANGE: u8 = 0xB0;
const MIDI_CHANNEL_PRESSURE: u8 = 0xD0;
const MIDI_PITCH_BEND: u8 = 0xE0;
const PITCH_BEND_MAX: f64 = 16383.0;
const SEVEN_BIT_MAX: f64 = 127.0;

#[repr(C)]
struct IMidiMappingVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const Tuid, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    get_midi_controller_assignment:
        unsafe extern "C" fn(*mut c_void, i32, i16, i16, *mut u32) -> TResult,
}

/// The parameter each MIDI controller drives, if the plugin maps it.
pub struct MidiControllerMap {
    params: [Option<u32>; CONTROLLER_COUNT],
}

impl MidiControllerMap {
    pub fn empty() -> Self {
        MidiControllerMap {
            params: [None; CONTROLLER_COUNT],
        }
    }

    /// `assignment` answers for one controller number.
    pub fn from_assignments(mut assignment: impl FnMut(i16) -> Option<u32>) -> Self {
        let mut map = Self::empty();
        for (controller, slot) in map.params.iter_mut().enumerate() {
            *slot = assignment(controller as i16);
        }
        map
    }

    pub fn param_for(&self, controller: i16) -> Option<u32> {
        usize::try_from(controller)
            .ok()
            .and_then(|index| self.params.get(index).copied().flatten())
    }
}

impl Default for MidiControllerMap {
    fn default() -> Self {
        Self::empty()
    }
}

/// Asks the controller which parameter each MIDI controller drives, on the main
/// event bus and channel.  A controller without `IMidiMapping` gives an empty map.
///
/// # Safety
/// `controller` must be a live, initialised `IEditController`, called off the
/// audio thread with the instance's FFI gate held.
pub unsafe fn read_midi_mapping(controller: *mut IEditController) -> MidiControllerMap {
    let mut interface: *mut c_void = std::ptr::null_mut();
    let found =
        ((*(*controller).vtbl).query_interface)(controller, &IID_IMIDI_MAPPING, &mut interface);
    if found != K_RESULT_OK || interface.is_null() {
        return MidiControllerMap::empty();
    }
    let vtbl = &**(interface as *mut *const IMidiMappingVtbl);
    let map = MidiControllerMap::from_assignments(|number| {
        let mut param_id = 0u32;
        let answer = (vtbl.get_midi_controller_assignment)(interface, 0, 0, number, &mut param_id);
        (answer == K_RESULT_OK).then_some(param_id)
    });
    (vtbl.release)(interface);
    map
}

/// A raw MIDI control as a VST3 controller number and a normalised value, or
/// `None` for a message VST3 does not carry this way (polyphonic aftertouch has
/// its own event).
pub fn controller_value(message: [u8; 3]) -> Option<(i16, f64)> {
    match message[0] & 0xF0 {
        MIDI_CONTROL_CHANGE => Some((message[1] as i16, message[2] as f64 / SEVEN_BIT_MAX)),
        MIDI_CHANNEL_PRESSURE => Some((CTRL_AFTER_TOUCH, message[1] as f64 / SEVEN_BIT_MAX)),
        MIDI_PITCH_BEND => {
            let bend = (u16::from(message[2]) << 7) | u16::from(message[1]);
            Some((CTRL_PITCH_BEND, f64::from(bend) / PITCH_BEND_MAX))
        }
        _ => None,
    }
}

/// Adds the block's mapped controls to the parameter changes, at their sample
/// offsets.  `controls` are the raw MIDI control messages of the block, each with
/// its offset in frames; a control the plugin does not map is dropped.
pub fn push_mapped_controls(
    controls: impl IntoIterator<Item = (u32, [u8; 3])>,
    map: &MidiControllerMap,
    changes: &mut HostParameterChanges,
) {
    for (offset, message) in controls {
        let Some((controller, value)) = controller_value(message) else {
            continue;
        };
        if let Some(param_id) = map.param_for(controller) {
            changes.push(param_id, offset as i32, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(param id, [(offset, value)])` of every queue.
    fn queues(changes: &HostParameterChanges) -> Vec<(u32, Vec<(i32, f64)>)> {
        changes.queues[..changes.count as usize]
            .iter()
            .map(|queue| {
                let points = (0..queue.count as usize)
                    .map(|i| (queue.offsets[i], queue.values[i]))
                    .collect();
                (queue.param_id, points)
            })
            .collect()
    }

    #[test]
    fn a_control_change_is_scaled_to_a_normalised_value() {
        assert_eq!(controller_value([0xB0, 1, 127]), Some((1, 1.0)));
        assert_eq!(controller_value([0xB0, 64, 0]), Some((64, 0.0)));
        let (_, half) = controller_value([0xB0, 7, 64]).unwrap();
        assert!((half - 64.0 / 127.0).abs() < 1e-12);
    }

    #[test]
    fn pitch_bend_uses_its_14_bits() {
        assert_eq!(
            controller_value([0xE0, 0x00, 0x00]),
            Some((CTRL_PITCH_BEND, 0.0))
        );
        assert_eq!(
            controller_value([0xE0, 0x7F, 0x7F]),
            Some((CTRL_PITCH_BEND, 1.0))
        );
        let (_, centre) = controller_value([0xE0, 0x00, 0x40]).unwrap();
        assert!((centre - 8192.0 / 16383.0).abs() < 1e-12);
    }

    #[test]
    fn channel_pressure_has_one_data_byte() {
        assert_eq!(
            controller_value([0xD0, 127, 0]),
            Some((CTRL_AFTER_TOUCH, 1.0))
        );
    }

    #[test]
    fn other_messages_are_not_controllers() {
        assert_eq!(controller_value([0xA0, 60, 100]), None);
        assert_eq!(controller_value([0xC0, 5, 0]), None);
    }

    #[test]
    fn the_map_answers_only_for_what_the_plugin_assigned() {
        let map = MidiControllerMap::from_assignments(|number| match number {
            1 => Some(500),
            CTRL_PITCH_BEND => Some(501),
            _ => None,
        });
        assert_eq!(map.param_for(1), Some(500));
        assert_eq!(map.param_for(CTRL_PITCH_BEND), Some(501));
        assert_eq!(map.param_for(64), None);
        assert_eq!(map.param_for(-1), None);
        assert_eq!(map.param_for(500), None);
    }

    #[test]
    fn mapped_controls_become_parameter_points_at_their_offset() {
        let map = MidiControllerMap::from_assignments(|number| match number {
            1 => Some(500),
            CTRL_PITCH_BEND => Some(501),
            _ => None,
        });
        let controls = [
            (10, [0xE0, 0x00, 0x60]),
            (20, [0xB0, 1, 127]),
            (30, [0xB0, 74, 10]),
            (40, [0xE0, 0x00, 0x20]),
        ];

        let mut changes = HostParameterChanges::boxed();
        push_mapped_controls(controls, &map, &mut changes);

        let found = queues(&changes);
        assert_eq!(found.len(), 2, "the unmapped CC 74 adds nothing");
        assert_eq!(found[0].0, 501);
        assert_eq!(found[0].1[0].0, 10);
        assert_eq!(found[0].1[1].0, 40);
        assert!(found[0].1[0].1 > 0.5 && found[0].1[1].1 < 0.5);
        assert_eq!(found[1], (500, vec![(20, 1.0)]));
    }

    #[test]
    fn without_a_mapping_no_control_reaches_the_plugin() {
        let mut changes = HostParameterChanges::boxed();
        push_mapped_controls(
            [(0, [0xE0, 0x00, 0x60])],
            &MidiControllerMap::empty(),
            &mut changes,
        );
        assert_eq!(changes.count, 0);
    }
}
