//! Program lists of a VST3 plugin (`IUnitInfo`, ADR-031 amendment).
//!
//! A controller that implements `IUnitInfo` describes its units, the program
//! lists they use and the name of every program.  Choosing a program means
//! setting the parameter flagged `kIsProgramChange` on the unit that uses the
//! list.  The plugin is third-party code, so every count it reports is bounded
//! before it drives a loop.

use std::ffi::c_void;
use std::mem::size_of;

use super::ffi::{
    string128_to_string, IEditController, ParameterInfo, String128, TResult, Tuid, IID_IUNIT_INFO,
    K_RESULT_OK, PARAM_IS_PROGRAM_CHANGE,
};

const MAX_UNITS: i32 = 256;
const MAX_PROGRAM_LISTS: i32 = 64;
const MAX_PROGRAMS_PER_LIST: i32 = 4096;
const MAX_PROGRAMS: usize = 20_000;
const MAX_PARAMETERS: i32 = 1 << 16;

/// `Vst::UnitInfo` (`ivstunits.h`).
#[repr(C)]
pub struct UnitInfo {
    pub id: i32,
    pub parent_unit_id: i32,
    pub name: String128,
    pub program_list_id: i32,
}

/// `Vst::ProgramListInfo` (`ivstunits.h`).
#[repr(C)]
pub struct ProgramListInfo {
    pub id: i32,
    pub name: String128,
    pub program_count: i32,
}

const _: () = assert!(size_of::<UnitInfo>() == 268, "UnitInfo layout drift");
const _: () = assert!(
    size_of::<ProgramListInfo>() == 264,
    "ProgramListInfo layout drift"
);

/// `IUnitInfo` in declaration order after `FUnknown`; the methods this module does
/// not call are untyped slots.
#[repr(C)]
pub struct IUnitInfoVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut c_void, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub get_unit_count: unsafe extern "C" fn(*mut c_void) -> i32,
    pub get_unit_info: unsafe extern "C" fn(*mut c_void, i32, *mut UnitInfo) -> TResult,
    pub get_program_list_count: unsafe extern "C" fn(*mut c_void) -> i32,
    pub get_program_list_info:
        unsafe extern "C" fn(*mut c_void, i32, *mut ProgramListInfo) -> TResult,
    pub get_program_name: unsafe extern "C" fn(*mut c_void, i32, i32, *mut u16) -> TResult,
    pub _get_program_info: *const c_void,
    pub _has_program_pitch_names: *const c_void,
    pub _get_program_pitch_name: *const c_void,
    pub _get_selected_unit: *const c_void,
    pub _select_unit: *const c_void,
    pub _get_unit_by_bus: *const c_void,
    pub _set_unit_program_data: *const c_void,
}

const _: () = assert!(
    size_of::<IUnitInfoVtbl>() == 15 * size_of::<usize>(),
    "IUnitInfoVtbl layout drift"
);

/// One program list of a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramList {
    pub id: i32,
    pub name: String,
    /// The `kIsProgramChange` parameter that selects a program of this list;
    /// without it the host has no way to choose one.
    pub program_change_param: Option<u32>,
    /// Program names by index; a plugin may leave one empty.
    pub programs: Vec<String>,
}

/// Reads the program lists of a controller.  A controller without `IUnitInfo`, or
/// one that declares no list, gives an empty vector.
///
/// # Safety
/// `controller` must be a live, initialised `IEditController`, called off the
/// audio thread with the instance's FFI gate held.
pub unsafe fn read_program_lists(controller: *mut IEditController) -> Vec<ProgramList> {
    let mut interface: *mut c_void = std::ptr::null_mut();
    let found =
        ((*(*controller).vtbl).query_interface)(controller, &IID_IUNIT_INFO, &mut interface);
    if found != K_RESULT_OK || interface.is_null() {
        return Vec::new();
    }
    let vtbl = &**(interface as *mut *const IUnitInfoVtbl);
    let unit_lists = read_unit_lists(interface, vtbl);
    let selectors = read_program_change_params(controller);
    let mut lists = read_lists(interface, vtbl);
    (vtbl.release)(interface);
    let list_count = lists.len();
    for list in &mut lists {
        list.program_change_param = program_change_param(list, list_count, &unit_lists, &selectors);
    }
    lists
}

struct ProgramChangeParam {
    unit_id: i32,
    id: u32,
    step_count: i32,
}

/// The parameter that selects a program of `list`: the one of a unit that uses
/// the list, as the SDK describes.  Some plugins (TDR Nova) leave their unit
/// unlinked from their only list; their only program-change parameter is then
/// taken if its step count matches the number of programs.
fn program_change_param(
    list: &ProgramList,
    list_count: usize,
    unit_lists: &[(i32, i32)],
    selectors: &[ProgramChangeParam],
) -> Option<u32> {
    let of_a_unit_using_the_list = unit_lists
        .iter()
        .filter(|&&(_, unit_list)| unit_list == list.id)
        .find_map(|&(unit_id, _)| selectors.iter().find(|param| param.unit_id == unit_id));
    match (of_a_unit_using_the_list, selectors) {
        (Some(param), _) => Some(param.id),
        (None, [only])
            if list_count == 1 && only.step_count as i64 + 1 == list.programs.len() as i64 =>
        {
            Some(only.id)
        }
        _ => None,
    }
}

/// `(unit id, program list id)` of every unit.
unsafe fn read_unit_lists(interface: *mut c_void, vtbl: &IUnitInfoVtbl) -> Vec<(i32, i32)> {
    let count = (vtbl.get_unit_count)(interface).clamp(0, MAX_UNITS);
    (0..count)
        .filter_map(|index| {
            let mut info: UnitInfo = std::mem::zeroed();
            ((vtbl.get_unit_info)(interface, index, &mut info) == K_RESULT_OK)
                .then_some((info.id, info.program_list_id))
        })
        .collect()
}

unsafe fn read_program_change_params(controller: *mut IEditController) -> Vec<ProgramChangeParam> {
    let vtbl = &*(*controller).vtbl;
    let count = (vtbl.get_parameter_count)(controller).clamp(0, MAX_PARAMETERS);
    (0..count)
        .filter_map(|index| {
            let mut info: ParameterInfo = std::mem::zeroed();
            let read = (vtbl.get_parameter_info)(controller, index, &mut info) == K_RESULT_OK;
            (read && info.flags & PARAM_IS_PROGRAM_CHANGE != 0).then_some(ProgramChangeParam {
                unit_id: info.unit_id,
                id: info.id,
                step_count: info.step_count,
            })
        })
        .collect()
}

unsafe fn read_lists(interface: *mut c_void, vtbl: &IUnitInfoVtbl) -> Vec<ProgramList> {
    let count = (vtbl.get_program_list_count)(interface).clamp(0, MAX_PROGRAM_LISTS);
    let mut budget = MAX_PROGRAMS;
    let mut lists = Vec::new();
    for index in 0..count {
        let mut info: ProgramListInfo = std::mem::zeroed();
        if (vtbl.get_program_list_info)(interface, index, &mut info) != K_RESULT_OK {
            continue;
        }
        let programs = info.program_count.clamp(0, MAX_PROGRAMS_PER_LIST) as usize;
        let programs = programs.min(budget);
        budget -= programs;
        lists.push(ProgramList {
            id: info.id,
            name: string128_to_string(&info.name),
            program_change_param: None,
            programs: (0..programs as i32)
                .map(|program| read_program_name(interface, vtbl, info.id, program))
                .collect(),
        });
    }
    lists
}

unsafe fn read_program_name(
    interface: *mut c_void,
    vtbl: &IUnitInfoVtbl,
    list_id: i32,
    program: i32,
) -> String {
    let mut name: String128 = [0; 128];
    if (vtbl.get_program_name)(interface, list_id, program, name.as_mut_ptr()) == K_RESULT_OK {
        string128_to_string(&name)
    } else {
        String::new()
    }
}

/// Chooses a program of the list a program-change parameter drives, as a host
/// does: the controller takes the value (the plugin's own `plainParamToNormalized`
/// turns the index into it) and the caller forwards it to the processor.  Returns
/// the normalised value, or `None` when the plugin does not accept it.
///
/// # Safety
/// `controller` must be a live, initialised `IEditController`, called off the
/// audio thread with the instance's FFI gate held.
pub unsafe fn select_program(
    controller: *mut IEditController,
    param_id: u32,
    index: i32,
) -> Option<f64> {
    let vtbl = &*(*controller).vtbl;
    let normalised = (vtbl.plain_param_to_normalized)(controller, param_id, f64::from(index));
    if !normalised.is_finite() {
        return None;
    }
    let normalised = normalised.clamp(0.0, 1.0);
    ((vtbl.set_param_normalized)(controller, param_id, normalised) == K_RESULT_OK)
        .then_some(normalised)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_formats::vst3::fake_plugin::{
        fake_vst3_instance_with_units, FakeListener, FakeUnits,
    };

    fn lists_of(units: &mut FakeUnits) -> Vec<ProgramList> {
        let mut listener = FakeListener::new(false);
        let instance = fake_vst3_instance_with_units(&mut listener, units);
        let controller = instance
            .vst3_controller()
            .expect("the fake has a controller");
        unsafe { read_program_lists(controller.as_ptr()) }
    }

    #[test]
    fn a_controller_without_unit_info_has_no_lists() {
        assert!(lists_of(&mut FakeUnits::absent()).is_empty());
    }

    #[test]
    fn a_plugin_with_only_a_root_unit_has_no_lists() {
        assert!(lists_of(&mut FakeUnits::root_only()).is_empty());
    }

    #[test]
    fn one_list_is_read_with_its_programs_and_its_selector() {
        let mut units = FakeUnits::with_list(7, "Factory", &["Piano", "Strings", "Bass"]);
        units.program_change_params = vec![(0, 99, 2)];
        let lists = lists_of(&mut units);
        assert_eq!(
            lists,
            vec![ProgramList {
                id: 7,
                name: "Factory".into(),
                program_change_param: Some(99),
                programs: vec!["Piano".into(), "Strings".into(), "Bass".into()],
            }]
        );
    }

    #[test]
    fn several_lists_are_read_in_order_each_with_its_own_selector() {
        let mut units = FakeUnits::with_list(1, "A", &["a1", "a2"]);
        units.add_list(2, "B", &["b1"]);
        units.add_unit(5, 0, "Part 2", 2);
        units.program_change_params = vec![(5, 51, 0), (0, 50, 1), (3, 77, 9)];
        let lists = lists_of(&mut units);
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].program_change_param, Some(50));
        assert_eq!(lists[1].program_change_param, Some(51));
        assert_eq!(lists[1].programs, vec!["b1"]);
    }

    /// A plugin like TDR Nova: its root unit uses no list, and one parameter
    /// selects the programs of the only list.
    fn unlinked_unit(programs: &[&str], steps: i32) -> Box<FakeUnits> {
        let mut units = FakeUnits::root_only();
        units.add_list(4, "Factory", programs);
        units.program_change_params = vec![(0, 40, steps)];
        units
    }

    #[test]
    fn the_only_program_change_parameter_selects_the_only_list_when_the_steps_match() {
        let lists = lists_of(&mut unlinked_unit(&["a", "b", "c"], 2));
        assert_eq!(lists[0].program_change_param, Some(40));
    }

    #[test]
    fn a_parameter_whose_steps_do_not_match_the_programs_is_not_taken() {
        for steps in [0, 1, 3, -1, i32::MAX] {
            let lists = lists_of(&mut unlinked_unit(&["a", "b", "c"], steps));
            assert_eq!(lists[0].program_change_param, None, "steps {steps}");
        }
    }

    #[test]
    fn the_fallback_needs_exactly_one_list_and_one_parameter() {
        let mut two_lists = unlinked_unit(&["a", "b", "c"], 2);
        two_lists.add_list(5, "User", &["x", "y", "z"]);
        assert!(lists_of(&mut two_lists)
            .iter()
            .all(|list| list.program_change_param.is_none()));

        let mut two_parameters = unlinked_unit(&["a", "b", "c"], 2);
        two_parameters.program_change_params.push((0, 41, 2));
        assert_eq!(lists_of(&mut two_parameters)[0].program_change_param, None);
    }

    #[test]
    fn a_unit_that_uses_the_list_wins_over_the_fallback() {
        let mut units = unlinked_unit(&["a", "b", "c"], 2);
        units.add_unit(2, 0, "Part", 4);
        units.program_change_params.push((2, 42, 7));
        assert_eq!(lists_of(&mut units)[0].program_change_param, Some(42));
    }

    #[test]
    fn a_list_nobody_can_select_is_still_read_but_has_no_selector() {
        let lists = lists_of(&mut FakeUnits::with_list(1, "A", &["a1"]));
        assert_eq!(lists[0].program_change_param, None);
    }

    #[test]
    fn a_parameter_that_is_not_a_program_change_is_not_a_selector() {
        let mut units = FakeUnits::with_list(1, "A", &["a1"]);
        units.plain_params = vec![(0, 10)];
        assert_eq!(lists_of(&mut units)[0].program_change_param, None);
    }

    #[test]
    fn a_program_without_a_name_is_read_as_empty() {
        let lists = lists_of(&mut FakeUnits::with_list(1, "A", &["First", "", "Third"]));
        assert_eq!(lists[0].programs, vec!["First", "", "Third"]);
    }

    #[test]
    fn a_very_long_name_stops_at_the_128_characters_of_a_string128() {
        let long = "x".repeat(300);
        let lists = lists_of(&mut FakeUnits::with_list(1, &long, &[&long]));
        assert_eq!(lists[0].name.chars().count(), 128);
        assert_eq!(lists[0].programs[0].chars().count(), 128);
    }

    #[test]
    fn a_name_that_fails_to_read_is_empty() {
        let mut units = FakeUnits::with_list(1, "A", &["a1", "a2"]);
        units.failing_program = Some(1);
        assert_eq!(lists_of(&mut units)[0].programs, vec!["a1", ""]);
    }

    #[test]
    fn a_list_or_unit_that_fails_to_read_is_skipped() {
        let mut units = FakeUnits::with_list(1, "A", &["a1"]);
        units.add_list(2, "B", &["b1"]);
        units.failing_list = Some(0);
        units.failing_unit = Some(0);
        let lists = lists_of(&mut units);
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].id, 2);
    }

    #[test]
    fn counts_the_plugin_reports_are_bounded() {
        let mut units = FakeUnits::with_list(1, "A", &["a1"]);
        for id in 2..=10 {
            units.add_list(id, "more", &[]);
        }
        units.reported_unit_count = Some(i32::MAX);
        units.reported_list_count = Some(i32::MAX);
        units.reported_program_count = Some(i32::MAX);
        let lists = lists_of(&mut units);
        assert_eq!(lists.len(), 10, "only the lists that exist are read");
        assert!(lists
            .iter()
            .all(|list| list.programs.len() <= MAX_PROGRAMS_PER_LIST as usize));
        let programs: usize = lists.iter().map(|list| list.programs.len()).sum();
        assert_eq!(programs, MAX_PROGRAMS, "the total is capped");
    }

    #[test]
    fn negative_counts_read_nothing() {
        let mut units = FakeUnits::with_list(1, "A", &["a1"]);
        units.reported_list_count = Some(-5);
        assert!(lists_of(&mut units).is_empty());
        let mut units = FakeUnits::with_list(1, "A", &["a1"]);
        units.reported_program_count = Some(-1);
        assert!(lists_of(&mut units)[0].programs.is_empty());
    }

    #[test]
    fn the_interface_is_released_after_reading() {
        let mut units = FakeUnits::with_list(1, "A", &["a1"]);
        lists_of(&mut units);
        assert_eq!(units.references(), 0, "every query is released");
    }
}
