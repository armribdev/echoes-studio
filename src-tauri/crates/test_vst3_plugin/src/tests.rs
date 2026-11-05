//! The plugin driven the way a host drives it, through the real `vst3_ffi` calls.

use super::*;
use vst3_ffi::{
    enumerate_classes, instantiate_processor, is_instrument_subcategory, read_component_state,
    set_controller_state, write_component_state,
};

struct Instance {
    component: *mut IComponent,
    processor: *mut IAudioProcessor,
    controller: *mut IEditController,
}

fn instance(class_id: &[u8; 16]) -> Instance {
    unsafe {
        let (component, processor) =
            instantiate_processor(GetPluginFactory(), class_id, std::ptr::null_mut())
                .expect("the class must instantiate");
        let mut controller: *mut c_void = std::ptr::null_mut();
        let result = ((*(*component).vtbl).query_interface)(
            component,
            &IID_IEDIT_CONTROLLER,
            &mut controller,
        );
        assert_eq!(result, K_RESULT_OK);
        Instance {
            component,
            processor,
            controller: controller as *mut IEditController,
        }
    }
}

impl Instance {
    fn set_normalised(&self, id: u32, value: f64) {
        unsafe { ((*(*self.controller).vtbl).set_param_normalized)(self.controller, id, value) };
    }

    fn normalised(&self, id: u32) -> f64 {
        unsafe { ((*(*self.controller).vtbl).get_param_normalized)(self.controller, id) }
    }

    /// One stereo block through the processor; the input is `value` on both
    /// channels. Returns the left output.
    fn process(
        &self,
        value: f32,
        frames: usize,
        changes: Option<&mut HostParameterChanges>,
        events: Option<&mut TestEvents>,
    ) -> Vec<f32> {
        self.process_with_output(value, frames, changes, events, None)
    }

    fn process_with_output(
        &self,
        value: f32,
        frames: usize,
        changes: Option<&mut HostParameterChanges>,
        events: Option<&mut TestEvents>,
        output_changes: Option<&mut HostParameterChanges>,
    ) -> Vec<f32> {
        let mut input = [vec![value; frames], vec![value; frames]];
        let mut output = [vec![0.0f32; frames], vec![0.0f32; frames]];
        let mut input_pointers = [input[0].as_mut_ptr(), input[1].as_mut_ptr()];
        let mut output_pointers = [output[0].as_mut_ptr(), output[1].as_mut_ptr()];
        let mut inputs = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            channel_buffers32: input_pointers.as_mut_ptr(),
        };
        let mut outputs = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            channel_buffers32: output_pointers.as_mut_ptr(),
        };
        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: frames as i32,
            num_inputs: 1,
            num_outputs: 1,
            inputs: &mut inputs,
            outputs: &mut outputs,
            input_param_changes: changes
                .map_or(std::ptr::null_mut(), |c| c as *mut _ as *mut c_void),
            output_param_changes: output_changes
                .map_or(std::ptr::null_mut(), |c| c as *mut _ as *mut c_void),
            input_events: events.map_or(std::ptr::null_mut(), |e| e as *mut _ as *mut c_void),
            output_events: std::ptr::null_mut(),
            process_context: std::ptr::null_mut(),
        };
        let result = unsafe { ((*(*self.processor).vtbl).process)(self.processor, &mut data) };
        assert_eq!(result, K_RESULT_OK);
        output[0].clone()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            ((*(*self.controller).vtbl).release)(self.controller);
            ((*(*self.processor).vtbl).release)(self.processor);
            ((*(*self.component).vtbl).release)(self.component);
        }
    }
}

/// The host's `IEventList`, holding the events of one block.
#[repr(C)]
struct TestEvents {
    vtbl: *const EventListVtbl,
    events: Vec<Event>,
}

unsafe extern "C" fn events_count(this: *mut c_void) -> i32 {
    let events = &(*(this as *mut TestEvents)).events;
    events.len() as i32
}

unsafe extern "C" fn events_get(this: *mut c_void, index: i32, event: *mut Event) -> TResult {
    let events = &(*(this as *mut TestEvents)).events;
    *event = events[index as usize];
    K_RESULT_OK
}

static TEST_EVENTS_VTBL: Shared<EventListVtbl> = Shared(EventListVtbl {
    query_interface: 0,
    add_ref: 0,
    release: 0,
    get_event_count: events_count,
    get_event: events_get,
    add_event: 0,
});

fn note_events(events: Vec<Event>) -> TestEvents {
    TestEvents {
        vtbl: &TEST_EVENTS_VTBL.0,
        events,
    }
}

#[test]
fn the_factory_lists_an_effect_and_an_instrument() {
    let classes = unsafe { enumerate_classes(GetPluginFactory()) };
    let sub_categories: Vec<_> = classes
        .iter()
        .map(|class| {
            (
                class.name.as_str(),
                class.sub_categories.as_deref().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        sub_categories,
        [
            ("Echoes Test Gain", "Fx"),
            ("Echoes Test Synth", "Instrument|Synth")
        ]
    );
    assert!(!is_instrument_subcategory("Fx"));
    assert!(is_instrument_subcategory("Instrument|Synth"));
}

#[test]
fn an_unknown_interface_is_refused() {
    let plugin = instance(&EFFECT_CLASS_ID);
    let mut object: *mut c_void = std::ptr::null_mut();
    let result = unsafe {
        ((*(*plugin.component).vtbl).query_interface)(plugin.component, &[7; 16], &mut object)
    };
    assert_eq!((result, object.is_null()), (K_NO_INTERFACE, true));
}

#[test]
fn the_effect_multiplies_its_input_by_the_gain() {
    let plugin = instance(&EFFECT_CLASS_ID);
    assert_eq!(plugin.process(0.5, 8, None, None), vec![0.5; 8]);
    plugin.set_normalised(GAIN, 0.25);
    assert_eq!(plugin.process(0.5, 8, None, None), vec![0.25; 8]);
}

#[test]
fn a_parameter_point_applies_at_its_sample_offset() {
    let plugin = instance(&EFFECT_CLASS_ID);
    let mut changes = HostParameterChanges::boxed();
    changes.push(GAIN, 4, 1.0);
    let output = plugin.process(1.0, 8, Some(&mut changes), None);
    assert_eq!(output, [1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
}

#[test]
fn the_instrument_plays_the_velocity_of_the_held_note() {
    let plugin = instance(&INSTRUMENT_CLASS_ID);
    let mut notes = note_events(vec![
        Event::note_on(2, 60, 0.5, -1),
        Event::note_off(6, 60, -1),
    ]);
    let output = plugin.process(0.0, 8, None, Some(&mut notes));
    assert_eq!(output, [0.0, 0.0, 0.5, 0.5, 0.5, 0.5, 0.0, 0.0]);
}

/// What the instrument's `IMidiMapping` answers for `controller` on bus 0, channel 0.
fn mapped_parameter(plugin: &Instance, controller: i16) -> Option<u32> {
    let mut interface: *mut c_void = std::ptr::null_mut();
    let found = unsafe {
        ((*(*plugin.controller).vtbl).query_interface)(
            plugin.controller,
            &IID_IMIDI_MAPPING,
            &mut interface,
        )
    };
    if found != K_RESULT_OK {
        return None;
    }
    let mapping = interface as *mut IMidiMapping;
    let mut id = 0u32;
    let answer = unsafe {
        ((*(*mapping).vtbl).get_midi_controller_assignment)(mapping, 0, 0, controller, &mut id)
    };
    unsafe { ((*(*mapping).vtbl).release)(mapping) };
    (answer == K_RESULT_OK).then_some(id)
}

#[test]
fn only_the_instrument_maps_the_mod_wheel_and_pitch_bend() {
    let instrument = instance(&INSTRUMENT_CLASS_ID);
    assert_eq!(mapped_parameter(&instrument, 1), Some(MOD_WHEEL));
    assert_eq!(mapped_parameter(&instrument, 129), Some(BEND));
    assert_eq!(mapped_parameter(&instrument, 7), None);
    assert_eq!(mapped_parameter(&instance(&EFFECT_CLASS_ID), 1), None);
}

#[test]
fn the_mod_wheel_and_the_bend_shape_the_instrument_and_rest_neutral() {
    let plugin = instance(&INSTRUMENT_CLASS_ID);
    let play = |plugin: &Instance| {
        let mut notes = note_events(vec![Event::note_on(0, 62, 0.5, -1)]);
        plugin.process(0.0, 4, None, Some(&mut notes))[3]
    };
    assert_eq!(play(&plugin), 0.5, "the defaults leave the sound alone");
    plugin.set_normalised(MOD_WHEEL, 1.0);
    assert_eq!(play(&plugin), 1.0);
    plugin.set_normalised(BEND, 0.75);
    assert_eq!(play(&plugin), 1.0 * 1.25);
    assert_eq!(plugin.normalised(LAST_PITCH) * DIAGNOSTIC_RANGE, 62.0);
}

#[test]
fn the_state_carries_the_gain() {
    let plugin = instance(&EFFECT_CLASS_ID);
    plugin.set_normalised(GAIN, 0.25);
    let state =
        unsafe { read_component_state(plugin.component) }.expect("the plugin saves its state");
    let restored = instance(&EFFECT_CLASS_ID);
    assert!(unsafe { write_component_state(restored.component, &state) });
    assert_eq!(restored.normalised(GAIN), 0.25);
}

#[test]
fn it_reports_its_latency_and_counts_its_blocks() {
    let plugin = instance(&EFFECT_CLASS_ID);
    let latency = unsafe { ((*(*plugin.processor).vtbl).get_latency_samples)(plugin.processor) };
    assert_eq!(latency, LATENCY);
    plugin.process(0.0, 4, None, None);
    plugin.process(0.0, 4, None, None);
    assert_eq!(plugin.normalised(PROCESS_CALLS) * DIAGNOSTIC_RANGE, 2.0);
}

#[test]
fn the_buses_follow_the_class() {
    let count = |plugin: &Instance, media, direction| unsafe {
        ((*(*plugin.component).vtbl).get_bus_count)(plugin.component, media, direction)
    };
    let effect = instance(&EFFECT_CLASS_ID);
    let instrument = instance(&INSTRUMENT_CLASS_ID);
    assert_eq!(
        (
            count(&effect, K_AUDIO, K_INPUT),
            count(&effect, K_EVENT, K_INPUT)
        ),
        (2, 0)
    );
    assert_eq!(
        (
            count(&instrument, K_AUDIO, K_INPUT),
            count(&instrument, K_EVENT, K_INPUT)
        ),
        (0, 1)
    );
    assert_eq!(count(&instrument, K_AUDIO, K_OUTPUT), 2);
    assert_eq!(count(&effect, K_AUDIO, K_OUTPUT), 2);
}

#[test]
fn the_effect_has_a_mono_auxiliary_sidechain_input() {
    let plugin = instance(&EFFECT_CLASS_ID);
    let info = |index| unsafe {
        let mut info: BusInfo = zeroed();
        let result = ((*(*plugin.component).vtbl).get_bus_info)(
            plugin.component,
            K_AUDIO,
            K_INPUT,
            index,
            &mut info as *mut BusInfo as *mut c_void,
        );
        (result, info.channel_count, info.bus_type, info.flags)
    };
    assert_eq!(info(0), (K_RESULT_OK, 2, 0, BUS_FLAGS_DEFAULT_ACTIVE));
    assert_eq!(info(1), (K_RESULT_OK, 1, 1, 0));
    assert_eq!(info(2).0, K_INVALID_ARGUMENT);
}

#[test]
fn the_second_output_is_an_auxiliary_bus_the_host_opens_on_demand() {
    let plugin = instance(&INSTRUMENT_CLASS_ID);
    let info = |index| unsafe {
        let mut info: BusInfo = zeroed();
        let result = ((*(*plugin.component).vtbl).get_bus_info)(
            plugin.component,
            K_AUDIO,
            K_OUTPUT,
            index,
            &mut info as *mut BusInfo as *mut c_void,
        );
        (result, info.bus_type, info.flags)
    };
    assert_eq!(info(0), (K_RESULT_OK, 0, BUS_FLAGS_DEFAULT_ACTIVE));
    assert_eq!(info(1), (K_RESULT_OK, 1, 0));
    assert_eq!(info(2).0, K_INVALID_ARGUMENT);
}

#[test]
fn the_processor_and_the_controller_each_keep_their_own_self_value() {
    let plugin = instance(&EFFECT_CLASS_ID);
    let mut changes = HostParameterChanges::boxed();
    changes.push(SELF_VALUE, 0, 0.7);
    plugin.process(0.0, 4, Some(&mut changes), None);
    assert_eq!(plugin.normalised(PROCESSOR_SELF), 0.7);
    assert_eq!(
        plugin.normalised(SELF_VALUE),
        0.0,
        "the controller is not told what the processor got"
    );
    plugin.set_normalised(SELF_VALUE, 0.7);
    assert_eq!(plugin.normalised(SELF_VALUE), 0.7);
}

#[test]
fn write_output_puts_a_new_self_value_in_the_output_list() {
    let plugin = instance(&EFFECT_CLASS_ID);
    let mut output = HostParameterChanges::boxed();
    plugin.process_with_output(0.0, 4, None, None, Some(&mut output));
    assert_eq!(output.latest_points().count(), 0, "nothing by itself");

    let mut changes = HostParameterChanges::boxed();
    changes.push(WRITE_OUTPUT, 0, 1.0);
    plugin.process_with_output(0.0, 4, Some(&mut changes), None, Some(&mut output));
    assert_eq!(
        output.latest_points().collect::<Vec<_>>(),
        [(SELF_VALUE, WRITTEN_VALUE)]
    );
    assert_eq!(plugin.normalised(PROCESSOR_SELF), WRITTEN_VALUE);

    output.reset();
    plugin.process_with_output(0.0, 4, None, None, Some(&mut output));
    assert_eq!(output.latest_points().count(), 0, "written once");
}

#[test]
fn a_new_latency_applies_at_the_next_activation() {
    let plugin = instance(&EFFECT_CLASS_ID);
    let latency = |plugin: &Instance| unsafe {
        ((*(*plugin.processor).vtbl).get_latency_samples)(plugin.processor)
    };
    let set_active = |plugin: &Instance, state| unsafe {
        ((*(*plugin.component).vtbl).set_active)(plugin.component, state)
    };
    let mut changes = HostParameterChanges::boxed();
    changes.push(LOOKAHEAD, 0, 1.0);
    plugin.process(0.0, 4, Some(&mut changes), None);
    assert_eq!(
        latency(&plugin),
        LATENCY,
        "an active plugin keeps its latency"
    );
    set_active(&plugin, 0);
    set_active(&plugin, 1);
    assert_eq!(latency(&plugin), LOOKAHEAD_LATENCY);
}

#[test]
fn renaming_changes_the_title_the_controller_reports() {
    let plugin = instance(&EFFECT_CLASS_ID);
    let title = |plugin: &Instance| unsafe {
        let mut info: ParameterInfo = zeroed();
        let index = PARAMS.iter().position(|p| p.id == SELF_VALUE).unwrap() as i32;
        ((*(*plugin.controller).vtbl).get_parameter_info)(plugin.controller, index, &mut info);
        vst3_ffi::string128_to_string(&info.title)
    };
    assert_eq!(title(&plugin), SELF_VALUE_TITLE);
    let mut changes = HostParameterChanges::boxed();
    changes.push(RENAME, 0, 1.0);
    plugin.process(0.0, 4, Some(&mut changes), None);
    assert_eq!(title(&plugin), RENAMED_TITLE);
}

/// A host frame that accepts every size and counts the requests.
#[repr(C)]
struct TestFrame {
    vtbl: *const IPlugFrameVtbl,
    requests: std::cell::RefCell<Vec<(i32, i32)>>,
}

unsafe extern "C" fn frame_query(_: *mut FUnknown, _: *const Tuid, _: *mut *mut c_void) -> TResult {
    K_NO_INTERFACE
}

unsafe extern "C" fn frame_reference(_: *mut FUnknown) -> u32 {
    1
}

unsafe extern "C" fn frame_resize(
    this: *mut FUnknown,
    _: *mut IPlugView,
    rect: *mut ViewRect,
) -> TResult {
    let frame = &*(this as *const TestFrame);
    frame
        .requests
        .borrow_mut()
        .push(((*rect).right - (*rect).left, (*rect).bottom - (*rect).top));
    K_RESULT_OK
}

static FRAME_VTBL: IPlugFrameVtbl = IPlugFrameVtbl {
    query_interface: frame_query,
    add_ref: frame_reference,
    release: frame_reference,
    resize_view: frame_resize,
};

fn test_frame() -> TestFrame {
    TestFrame {
        vtbl: &FRAME_VTBL,
        requests: std::cell::RefCell::new(Vec::new()),
    }
}

impl Instance {
    fn create_view(&self, name: &[u8]) -> *mut IPlugView {
        unsafe { ((*(*self.controller).vtbl).create_view)(self.controller, name.as_ptr()) }
    }

    fn view_calls(&self) -> u32 {
        (self.normalised(VIEW_CALLS) * DIAGNOSTIC_RANGE).round() as u32
    }

    fn plain(&self, id: u32) -> u32 {
        (self.normalised(id) * DIAGNOSTIC_RANGE).round() as u32
    }
}

#[test]
fn only_the_editor_view_is_offered() {
    let plugin = instance(&EFFECT_CLASS_ID);
    assert!(plugin.create_view(b"remote\0").is_null());
    let view = plugin.create_view(b"editor\0");
    assert!(!view.is_null());
    unsafe { ((*(*view).vtbl).release)(view) };
}

#[test]
fn the_view_records_what_the_host_does_in_order() {
    let plugin = instance(&EFFECT_CLASS_ID);
    let mut frame = test_frame();
    let frame_pointer = &mut frame as *mut TestFrame as *mut FUnknown;
    let view = plugin.create_view(b"editor\0");
    unsafe {
        let table = &*(*view).vtbl;
        assert_eq!(
            (table.is_platform_type_supported)(view, c"HWND".as_ptr().cast()),
            K_RESULT_OK
        );
        assert_eq!(
            (table.is_platform_type_supported)(view, c"NSView".as_ptr().cast()),
            K_RESULT_FALSE
        );
        (table.set_frame)(view, frame_pointer);
        assert_eq!(
            (table.attached)(view, 0x10 as *mut c_void, c"HWND".as_ptr().cast()),
            K_RESULT_OK
        );

        let mut size = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        (table.get_size)(view, &mut size);
        assert_eq!((size.right, size.bottom), VIEW_INITIAL_SIZE);
        size = ViewRect {
            left: 0,
            top: 0,
            right: 640,
            bottom: 480,
        };
        (table.on_size)(view, &mut size);

        (table.set_frame)(view, std::ptr::null_mut());
        (table.removed)(view);
        (table.release)(view);
    }
    // setFrame, attached, onSize, setFrame(null), removed, released.
    assert_eq!(plugin.view_calls(), 134_259);
    assert_eq!(
        (plugin.plain(VIEW_WIDTH), plugin.plain(VIEW_HEIGHT)),
        (640, 480)
    );
}

#[test]
fn a_view_that_is_asked_to_resize_while_attaching_asks_the_host() {
    let plugin = instance(&EFFECT_CLASS_ID);
    plugin.set_normalised(VIEW_ASK, 700_500.0 / DIAGNOSTIC_RANGE);
    let mut frame = test_frame();
    let view = plugin.create_view(b"editor\0");
    unsafe {
        let table = &*(*view).vtbl;
        (table.set_frame)(view, &mut frame as *mut TestFrame as *mut FUnknown);
        (table.attached)(view, 0x10 as *mut c_void, c"HWND".as_ptr().cast());
        (table.release)(view);
    }
    assert_eq!(*frame.requests.borrow(), [(700, 500)]);
}

#[test]
fn the_content_scale_is_kept_as_a_percentage() {
    let plugin = instance(&EFFECT_CLASS_ID);
    let view = plugin.create_view(b"editor\0");
    unsafe {
        let mut support: *mut c_void = std::ptr::null_mut();
        let found = ((*(*view).vtbl).query_interface)(
            view,
            &IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT,
            &mut support,
        );
        assert_eq!(found, K_RESULT_OK);
        let support = support as *mut IPlugViewContentScaleSupport;
        ((*(*support).vtbl).set_content_scale_factor)(support, 1.5);
        ((*(*support).vtbl).release)(support);
        ((*(*view).vtbl).release)(view);
    }
    assert_eq!(plugin.plain(VIEW_SCALE), 150);
}

fn unit_info(plugin: &Instance) -> *mut IUnitInfo {
    let mut found: *mut c_void = std::ptr::null_mut();
    let result = unsafe {
        ((*(*plugin.controller).vtbl).query_interface)(
            plugin.controller,
            &IID_IUNIT_INFO,
            &mut found,
        )
    };
    assert_eq!(result, K_RESULT_OK);
    found as *mut IUnitInfo
}

#[test]
fn both_classes_offer_one_program_list_with_three_programs() {
    for class in [&EFFECT_CLASS_ID, &INSTRUMENT_CLASS_ID] {
        let plugin = instance(class);
        let units = unit_info(&plugin);
        let table = unsafe { &*(*units).vtbl };
        unsafe {
            assert_eq!((table.get_program_list_count)(units), 1);
            let mut list: ProgramListInfo = zeroed();
            assert_eq!(
                (table.get_program_list_info)(units, 0, &mut list),
                K_RESULT_OK
            );
            assert_eq!(list.program_count, 3);
            let names: Vec<String> = (0..3)
                .map(|program| {
                    let mut name = [0u16; 128];
                    assert_eq!(
                        (table.get_program_name)(units, list.id, program, name.as_mut_ptr()),
                        K_RESULT_OK
                    );
                    String::from_utf16_lossy(&name[..name.iter().position(|&c| c == 0).unwrap()])
                })
                .collect();
            assert_eq!(names, PROGRAM_NAMES);
            (table.release)(units);
        }
    }
}

#[test]
fn the_selector_is_flagged_and_belongs_to_the_unit_that_uses_the_list() {
    let plugin = instance(&EFFECT_CLASS_ID);
    let units = unit_info(&plugin);
    let table = unsafe { &*(*units).vtbl };
    let mut unit: UnitInfo = unsafe { zeroed() };
    assert_eq!(
        unsafe { (table.get_unit_info)(units, 1, &mut unit) },
        K_RESULT_OK
    );
    unsafe { (table.release)(units) };
    let count = unsafe { ((*(*plugin.controller).vtbl).get_parameter_count)(plugin.controller) };
    let selector = (0..count)
        .map(|index| {
            let mut info: ParameterInfo = unsafe { zeroed() };
            unsafe {
                ((*(*plugin.controller).vtbl).get_parameter_info)(
                    plugin.controller,
                    index,
                    &mut info,
                )
            };
            info
        })
        .find(|info| info.flags & PARAM_IS_PROGRAM_CHANGE != 0)
        .expect("one parameter selects a program");
    assert_eq!(selector.id, PROGRAM);
    assert_eq!(selector.unit_id, unit.id);
    assert_eq!(selector.step_count, 2);
    assert_eq!(plugin.normalised(PROGRAM), 0.5, "Unity is the default");
}

#[test]
fn only_the_processor_applies_a_program() {
    let plugin = instance(&EFFECT_CLASS_ID);
    plugin.set_normalised(PROGRAM, 1.0);
    assert_eq!(
        plugin.normalised(GAIN),
        0.5,
        "the controller alone changes no sound"
    );
    let mut changes = HostParameterChanges::boxed();
    changes.push(PROGRAM, 0, 1.0);
    let heard = plugin.process(0.5, 4, Some(&mut changes), None);
    assert_eq!(heard, vec![0.5 * 1.75; 4]);
    assert_eq!(plugin.normalised(GAIN), 1.75 / 2.0);
}

#[test]
fn the_controller_state_is_the_self_value() {
    let plugin = instance(&EFFECT_CLASS_ID);
    assert!(unsafe { set_controller_state(plugin.controller, &0.8f64.to_le_bytes()) });
    assert_eq!(plugin.normalised(SELF_VALUE), 0.8);
    assert!(!unsafe { set_controller_state(plugin.controller, &[1, 2]) });
}
