//! The program lists of a VST3 controller (`vst3_host::units`), read from the hand-made
//! plugin of `fake_plugin.rs`.
use crate::plugin_formats::vst3::fake_plugin::{
    fake_vst3_instance_with_units, FakeListener, FakeUnits,
};
use vst3_host::units::*;

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
            current_program: Some(0),
        }]
    );
}

#[test]
fn the_current_program_is_the_one_the_selector_is_set_to() {
    let mut units = unlinked_unit(&["a", "b", "c", "d"], 3);
    for (value, expected) in [(0.0, 0), (1.0 / 3.0, 1), (2.0 / 3.0, 2), (1.0, 3)] {
        units.values.borrow_mut().insert(40, value);
        assert_eq!(lists_of(&mut units)[0].current_program, Some(expected));
    }
}

#[test]
fn a_selector_between_two_programs_is_rounded_and_one_outside_the_list_is_unknown() {
    let mut units = unlinked_unit(&["a", "b", "c", "d"], 3);
    units.values.borrow_mut().insert(40, 0.49);
    assert_eq!(lists_of(&mut units)[0].current_program, Some(1));
    for value in [-1.0, 2.0, f64::NAN] {
        units.values.borrow_mut().insert(40, value);
        assert_eq!(lists_of(&mut units)[0].current_program, None, "{value}");
    }
}

#[test]
fn a_list_without_a_selector_has_no_current_program() {
    let lists = lists_of(&mut FakeUnits::with_list(1, "A", &["a1", "a2"]));
    assert_eq!(lists[0].current_program, None);
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
