//! Program lists of a VST3 plugin (`IUnitInfo`, ADR-031 amendment).
//!
//! A controller that implements `IUnitInfo` describes its units, the program
//! lists they use and the name of every program.  Choosing a program means
//! setting the parameter flagged `kIsProgramChange` on the unit that uses the
//! list.  The plugin is third-party code, so every count it reports is bounded
//! before it drives a loop.

use std::ffi::c_void;
use std::mem::size_of;

use vst3_ffi::{
    string128_to_string, IEditController, ParameterInfo, String128, TResult, Tuid, IID_IUNIT_INFO,
    K_RESULT_OK, PARAM_IS_PROGRAM_CHANGE,
};

const MAX_UNITS: i32 = 256;
const MAX_PROGRAM_LISTS: i32 = 64;
pub const MAX_PROGRAMS_PER_LIST: i32 = 4096;
pub const MAX_PROGRAMS: usize = 20_000;
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
    pub has_program_pitch_names: unsafe extern "C" fn(*mut c_void, i32, i32) -> TResult,
    pub get_program_pitch_name:
        unsafe extern "C" fn(*mut c_void, i32, i32, i16, *mut u16) -> TResult,
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
    /// The program the selector says is active, if it names one of `programs`.
    pub current_program: Option<usize>,
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
        list.program_change_param = program_change_param(
            list.id,
            list.programs.len(),
            list_count,
            &unit_lists,
            &selectors,
        );
        list.current_program = list
            .program_change_param
            .and_then(|selector| current_program(controller, selector, list.programs.len()));
    }
    lists
}

/// The program a selector is set to, as the plugin's own conversion reads it.
unsafe fn current_program(
    controller: *mut IEditController,
    selector: u32,
    program_count: usize,
) -> Option<usize> {
    let vtbl = &*(*controller).vtbl;
    let normalised = (vtbl.get_param_normalized)(controller, selector);
    let index = (vtbl.normalized_param_to_plain)(controller, selector, normalised).round();
    (0.0..program_count as f64)
        .contains(&index)
        .then_some(index as usize)
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
    list_id: i32,
    program_count: usize,
    list_count: usize,
    unit_lists: &[(i32, i32)],
    selectors: &[ProgramChangeParam],
) -> Option<u32> {
    let of_a_unit_using_the_list = unit_lists
        .iter()
        .filter(|&&(_, unit_list)| unit_list == list_id)
        .find_map(|&(unit_id, _)| selectors.iter().find(|param| param.unit_id == unit_id));
    match (of_a_unit_using_the_list, selectors) {
        (Some(param), _) => Some(param.id),
        (None, [only]) if list_count == 1 && only.step_count as i64 + 1 == program_count as i64 => {
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

/// The program lists a plugin declares, their program count bounded.
unsafe fn read_list_headers(interface: *mut c_void, vtbl: &IUnitInfoVtbl) -> Vec<ProgramListInfo> {
    let count = (vtbl.get_program_list_count)(interface).clamp(0, MAX_PROGRAM_LISTS);
    (0..count)
        .filter_map(|index| {
            let mut info: ProgramListInfo = std::mem::zeroed();
            let read = (vtbl.get_program_list_info)(interface, index, &mut info) == K_RESULT_OK;
            info.program_count = info.program_count.clamp(0, MAX_PROGRAMS_PER_LIST);
            read.then_some(info)
        })
        .collect()
}

unsafe fn read_lists(interface: *mut c_void, vtbl: &IUnitInfoVtbl) -> Vec<ProgramList> {
    let mut budget = MAX_PROGRAMS;
    read_list_headers(interface, vtbl)
        .into_iter()
        .map(|info| {
            let programs = (info.program_count as usize).min(budget);
            budget -= programs;
            ProgramList {
                id: info.id,
                name: string128_to_string(&info.name),
                program_change_param: None,
                programs: (0..programs as i32)
                    .map(|program| read_program_name(interface, vtbl, info.id, program))
                    .collect(),
                current_program: None,
            }
        })
        .collect()
}

/// A name a plugin gives to a note, e.g. a pad of a drum kit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PitchName {
    pub pitch: u8,
    pub name: String,
}

const MIDI_PITCH_COUNT: i16 = 128;

/// The names the plugin gives to the notes of its current program (`IUnitInfo`
/// pitch names).  A plugin has them per program, so the program a selector is
/// set to decides; with several lists the first one that names notes wins.
/// Empty when the plugin has no `IUnitInfo`, no selector, or no names.
///
/// # Safety
/// `controller` must be a live, initialised `IEditController`, called off the
/// audio thread with the instance's FFI gate held.
pub unsafe fn read_pitch_names(controller: *mut IEditController) -> Vec<PitchName> {
    let mut interface: *mut c_void = std::ptr::null_mut();
    let found =
        ((*(*controller).vtbl).query_interface)(controller, &IID_IUNIT_INFO, &mut interface);
    if found != K_RESULT_OK || interface.is_null() {
        return Vec::new();
    }
    let vtbl = &**(interface as *mut *const IUnitInfoVtbl);
    let unit_lists = read_unit_lists(interface, vtbl);
    let selectors = read_program_change_params(controller);
    let lists = read_list_headers(interface, vtbl);
    let names = lists
        .iter()
        .find_map(|list| {
            let program_count = list.program_count as usize;
            let selector =
                program_change_param(list.id, program_count, lists.len(), &unit_lists, &selectors)?;
            let program = current_program(controller, selector, program_count)?;
            read_program_pitch_names(interface, vtbl, list.id, program as i32)
        })
        .unwrap_or_default();
    (vtbl.release)(interface);
    names
}

/// The pitch names of one program, `None` when it has none.
unsafe fn read_program_pitch_names(
    interface: *mut c_void,
    vtbl: &IUnitInfoVtbl,
    list_id: i32,
    program: i32,
) -> Option<Vec<PitchName>> {
    if (vtbl.has_program_pitch_names)(interface, list_id, program) != K_RESULT_OK {
        return None;
    }
    let names: Vec<PitchName> = (0..MIDI_PITCH_COUNT)
        .filter_map(|pitch| {
            let mut name: String128 = [0; 128];
            let read = (vtbl.get_program_pitch_name)(
                interface,
                list_id,
                program,
                pitch,
                name.as_mut_ptr(),
            ) == K_RESULT_OK;
            let name = string128_to_string(&name);
            (read && !name.is_empty()).then_some(PitchName {
                pitch: pitch as u8,
                name,
            })
        })
        .collect();
    (!names.is_empty()).then_some(names)
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

/// Identifies a program in its preset: the parameter that selects it and its
/// index.  Only the host writes it, and a load accepts only what a listing offered.
pub fn program_key(param_id: u32, index: usize) -> String {
    format!("{param_id}:{index}")
}

pub fn parse_program_key(key: &str) -> Option<(u32, i32)> {
    let (param_id, index) = key.split_once(':')?;
    let index = index.parse::<i32>().ok().filter(|index| *index >= 0)?;
    Some((param_id.parse().ok()?, index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_key_reads_back_and_garbage_does_not() {
        assert_eq!(
            parse_program_key(&program_key(1_886_548_852, 72)),
            Some((1_886_548_852, 72))
        );
        for key in [
            "",
            "5",
            ":",
            "5:",
            ":5",
            "x:1",
            "5:x",
            "5:-1",
            "5:1:2",
            "4294967296:1",
            "5:2147483648",
        ] {
            assert_eq!(parse_program_key(key), None, "{key:?}");
        }
    }
}
