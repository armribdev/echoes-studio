//! The plugin driven the way a host drives it, through the real `vst3_ffi` calls.

use super::*;
use vst3_ffi::{
    enumerate_classes, instantiate_processor, is_instrument_subcategory, read_component_state,
    write_component_state,
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
        (1, 0)
    );
    assert_eq!(
        (
            count(&instrument, K_AUDIO, K_INPUT),
            count(&instrument, K_EVENT, K_INPUT)
        ),
        (0, 1)
    );
    assert_eq!(count(&instrument, K_AUDIO, K_OUTPUT), 1);
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
