//! CLAP parameter plumbing shared by the effect wrapper and the instrument
//! adapter (ADR-021): parameter enumeration and per-block parameter events.

use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::sync::Arc;

use crate::audio_graph::plugin_params::{
    PluginParamControl, PluginParamControls, PluginParamRange, ValueModulatedParam,
};
use crate::plugin_formats::clap::ffi::{
    ClapEventBuffer, ClapEventHeader, ClapEventParamGesture, ClapEventParamValue, ClapOutputEvents,
    ClapParamInfo, ClapPlugin, ClapPluginParams, CLAP_CORE_EVENT_SPACE, CLAP_EVENT_PARAM_GESTURE_BEGIN,
    CLAP_EVENT_PARAM_GESTURE_END, CLAP_EVENT_PARAM_VALUE, CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE, mark_plugin_state_edited,
    CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_MODULATABLE, CLAP_PARAM_IS_READONLY,
};
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::parameter::PluginParamInfo;

fn c_text(bytes: &[u8]) -> String {
    CStr::from_bytes_until_nul(bytes)
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn param_info(info: &ClapParamInfo) -> PluginParamInfo {
    let read_only = info.flags & CLAP_PARAM_IS_READONLY != 0;
    PluginParamInfo {
        native_id: info.id,
        name: c_text(&info.name),
        module: c_text(&info.module),
        min: info.min_value,
        max: info.max_value,
        default: info.default_value,
        automatable: info.flags & CLAP_PARAM_IS_AUTOMATABLE != 0 && !read_only,
        modulatable: info.flags & CLAP_PARAM_IS_MODULATABLE != 0 && !read_only,
        hidden: info.flags & CLAP_PARAM_IS_HIDDEN != 0,
    }
}

/// Lists the plugin's parameters through `clap.params`.
///
/// # Safety
/// `plugin` must be a live, initialised CLAP plugin, and the caller must hold
/// the instance's off-RT FFI gate (`clap.params` getters are main-thread only).
pub unsafe fn read_clap_params(plugin: *const ClapPlugin) -> Vec<PluginParamInfo> {
    let extension = ((*plugin).get_extension)(plugin, CLAP_EXT_PARAMS.as_ptr() as *const _);
    if extension.is_null() {
        return Vec::new();
    }
    let params = extension as *const ClapPluginParams;
    (0..((*params).count)(plugin))
        .filter_map(|index| {
            let mut info: ClapParamInfo = std::mem::zeroed();
            ((*params).get_info)(plugin, index, &mut info).then(|| param_info(&info))
        })
        .collect()
}

unsafe fn params_extension(plugin: *const ClapPlugin) -> Option<*const ClapPluginParams> {
    let extension = ((*plugin).get_extension)(plugin, CLAP_EXT_PARAMS.as_ptr() as *const _);
    (!extension.is_null()).then_some(extension as *const ClapPluginParams)
}

/// Current plain value of one parameter.
///
/// # Safety
/// Same contract as [`read_clap_params`].
pub unsafe fn read_clap_param_value(plugin: *const ClapPlugin, native_id: u32) -> Option<f64> {
    let params = params_extension(plugin)?;
    let mut value = 0.0;
    ((*params).get_value)(plugin, native_id, &mut value).then_some(value)
}

unsafe extern "C" fn discard_output_event(_: *const ClapOutputEvents, _: *const ClapEventHeader) -> bool {
    true
}

/// Sets parameters to plain `values` outside `process` (`clap.params.flush`).
///
/// # Safety
/// Same contract as [`read_clap_params`]; the FFI gate keeps `process` from
/// running concurrently even while the plugin is active.
pub unsafe fn flush_clap_param_values(plugin: *const ClapPlugin, values: &[(u32, f64)]) {
    let Some(params) = params_extension(plugin) else {
        return;
    };
    let mut events = ClapEventBuffer::new();
    for &(native_id, value) in values {
        events.push_param_value(0, native_id, value);
    }
    let output = ClapOutputEvents { ctx: std::ptr::null_mut(), try_push: discard_output_event };
    ((*params).flush)(plugin, &events.input_events_vtable, &output);
}

/// Value-modulated parameters of one placement (ADR-021 amendment).  Boxed so
/// the plugin's output-event callback can reach it while `process` runs.
#[derive(Default)]
struct ValueModulation {
    params: Vec<ValueModulatedParam>,
    /// Parameters to put back on their base once (route removed), plain units.
    released: Vec<(u32, f64)>,
}

impl ValueModulation {
    fn find(&self, native_id: u32) -> Option<&ValueModulatedParam> {
        self.params.iter().find(|param| param.native_id == native_id)
    }

    /// A drag in the plugin window pauses the modulation and moves its base.
    fn observe(&mut self, header: &ClapEventHeader) {
        if header.space_id != CLAP_CORE_EVENT_SPACE {
            return;
        }
        match header.r#type {
            CLAP_EVENT_PARAM_GESTURE_BEGIN | CLAP_EVENT_PARAM_GESTURE_END => {
                // SAFETY: the event type guarantees the layout.
                let event = unsafe { &*(header as *const ClapEventHeader as *const ClapEventParamGesture) };
                let is_begin = header.r#type == CLAP_EVENT_PARAM_GESTURE_BEGIN;
                if let Some(param) = self.params.iter_mut().find(|param| param.native_id == event.param_id) {
                    param.editing = is_begin;
                }
                if !is_begin {
                    mark_plugin_state_edited();
                }
            }
            CLAP_EVENT_PARAM_VALUE => {
                // SAFETY: the event type guarantees the layout.
                let event = unsafe { &*(header as *const ClapEventHeader as *const ClapEventParamValue) };
                match self.find(event.param_id) {
                    Some(param) if param.editing => {
                        param.set_base(param.range.normalise(event.value as f32));
                        mark_plugin_state_edited();
                    }
                    // Outside a drag it may echo the value the host just sent.
                    Some(_) => {}
                    None => mark_plugin_state_edited(),
                }
            }
            _ => {}
        }
    }
}

/// `clap_output_events.try_push` handed to CLAP plugins; `ctx` comes from
/// [`ClapParamEvents::output_context`].  Other output events are discarded.
pub unsafe extern "C" fn observe_output_event(
    list: *const ClapOutputEvents,
    header: *const ClapEventHeader,
) -> bool {
    let modulation = (*list).ctx as *mut ValueModulation;
    if !modulation.is_null() && !header.is_null() {
        (*modulation).observe(&*header);
    }
    true
}

/// Parameter state of one CLAP placement and the events it sends each block.
pub struct ClapParamEvents {
    ranges: HashMap<u32, PluginParamRange>,
    controls: PluginParamControls,
    /// Parameters whose host modulation must be reset once (route removed).
    released_modulation: Vec<u32>,
    value_modulation: Box<ValueModulation>,
    pub buffer: Box<ClapEventBuffer>,
}

impl ClapParamEvents {
    pub fn new() -> Self {
        Self {
            ranges: HashMap::new(),
            controls: PluginParamControls::default(),
            released_modulation: Vec::new(),
            value_modulation: Box::default(),
            buffer: ClapEventBuffer::new(),
        }
    }

    /// Off the audio thread, after `read_clap_params`.
    pub fn set_ranges(&mut self, params: &[PluginParamInfo]) {
        self.ranges = params
            .iter()
            .map(|param| {
                let range = PluginParamRange {
                    min: param.min as f32,
                    max: param.max as f32,
                    modulatable: param.modulatable,
                    automatable: param.automatable,
                };
                (param.native_id, range)
            })
            .collect();
    }

    fn range(&self, native_id: u32) -> PluginParamRange {
        self.ranges.get(&native_id).copied().unwrap_or(PluginParamRange::UNKNOWN)
    }

    /// Off the audio thread, at compile time, after `set_ranges`.
    pub fn set_controls(&mut self, mut controls: Vec<PluginParamControl>, instance: &PluginInstance) {
        for control in &mut controls {
            control.range = self.range(control.native_id);
        }
        let modulated: Vec<u32> = controls
            .iter()
            .filter(|control| control.modulated && control.range.modulatable)
            .map(|control| control.native_id)
            .collect();
        self.released_modulation = instance.replace_modulated_params(&modulated);
        self.value_modulation = Box::new(value_modulation(&controls, instance));
        self.controls = PluginParamControls::new(controls);
    }

    /// `ctx` for [`observe_output_event`]; stable until the next `set_controls`.
    pub fn output_context(&mut self) -> *mut c_void {
        &mut *self.value_modulation as *mut ValueModulation as *mut c_void
    }

    pub fn controls_mut(&mut self) -> &mut PluginParamControls {
        &mut self.controls
    }

    /// Audio thread: refills `buffer` with this block's parameter events —
    /// queued edits, released modulation, modulation, automation.  The caller
    /// may add notes, then must call `buffer.sort()`.  No allocation.
    pub fn fill(&mut self, instance: &PluginInstance) {
        self.buffer.clear();
        let buffer = &mut self.buffer;
        let ranges = &self.ranges;

        instance.drain_params(|change| {
            let range = ranges.get(&change.native_id).copied().unwrap_or(PluginParamRange::UNKNOWN);
            buffer.push_param_value(change.sample_offset, change.native_id, range.denormalise(change.normalised) as f64);
        });

        for native_id in self.released_modulation.drain(..) {
            buffer.push_param_mod(0, native_id, 0.0);
        }
        for (native_id, base) in self.value_modulation.released.drain(..) {
            buffer.push_param_value(0, native_id, base);
        }

        for control in self.controls.iter() {
            if control.range.modulatable {
                for (offset, value) in control.modulation.iter() {
                    buffer.push_param_mod(offset, control.native_id, control.range.plain_offset(value) as f64);
                }
            } else if control.automation.is_empty() {
                let value_modulated = self.value_modulation.find(control.native_id);
                if let Some(param) = value_modulated.filter(|param| !param.editing) {
                    for (offset, value) in control.modulation.iter() {
                        buffer.push_param_value(offset, control.native_id, param.modulated_plain(value) as f64);
                    }
                }
            }
            for (offset, value) in control.automation.iter() {
                let plain = control.range.denormalise(value.clamp(0.0, 1.0));
                buffer.push_param_value(offset, control.native_id, plain as f64);
            }
        }
    }
}

/// Off the audio thread: the value-modulated parameters of a new control set.
/// A newly modulated parameter starts from the plugin's current value; one no
/// longer modulated is put back on its base.
fn value_modulation(controls: &[PluginParamControl], instance: &PluginInstance) -> ValueModulation {
    let params: Vec<ValueModulatedParam> = controls
        .iter()
        .filter(|control| control.modulated && !control.range.modulatable && control.range.automatable)
        .map(|control| ValueModulatedParam::new(control.native_id, control.range, Arc::clone(&control.base)))
        .collect();
    let previous = instance.replace_value_modulated_params(params.clone());
    let was_modulated = |native_id: u32| previous.iter().any(|param| param.native_id == native_id);

    for param in params.iter().filter(|param| !was_modulated(param.native_id)) {
        if let Some(plain) = instance.clap_param_value(param.native_id) {
            param.set_base(param.range.normalise(plain as f32));
        }
    }
    let released = previous
        .iter()
        .filter(|old| !params.iter().any(|param| param.native_id == old.native_id))
        .map(|old| (old.native_id, old.base_plain() as f64))
        .collect();
    ValueModulation { params, released }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_formats::clap::ffi::{
        ClapEventHeader, ClapEventParamValue, CLAP_EVENT_PARAM_MOD, CLAP_EVENT_PARAM_VALUE,
    };
    use std::sync::atomic::AtomicU32;
    use std::sync::Arc;

    fn info(native_id: u32, modulatable: bool) -> PluginParamInfo {
        PluginParamInfo {
            native_id,
            name: format!("p{native_id}"),
            module: String::new(),
            min: 100.0,
            max: 300.0,
            default: 200.0,
            automatable: true,
            modulatable,
            hidden: false,
        }
    }

    fn read_only_info(native_id: u32) -> PluginParamInfo {
        PluginParamInfo { automatable: false, ..info(native_id, false) }
    }

    fn header(r#type: u16, size: usize) -> ClapEventHeader {
        ClapEventHeader { size: size as u32, time: 0, space_id: CLAP_CORE_EVENT_SPACE, r#type, flags: 0 }
    }

    fn gesture(r#type: u16, param_id: u32) -> ClapEventParamGesture {
        ClapEventParamGesture { header: header(r#type, std::mem::size_of::<ClapEventParamGesture>()), param_id }
    }

    fn value_event(param_id: u32, value: f64) -> ClapEventParamValue {
        ClapEventParamValue {
            header: header(CLAP_EVENT_PARAM_VALUE, std::mem::size_of::<ClapEventParamValue>()),
            param_id,
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            _pad: 0,
            value,
        }
    }

    fn push_output(params: &mut ClapParamEvents, event: &ClapEventHeader) {
        let output = ClapOutputEvents { ctx: params.output_context(), try_push: observe_output_event };
        assert!(unsafe { observe_output_event(&output, event) });
    }

    fn control(native_id: u32, modulated: bool) -> PluginParamControl {
        let mut control = PluginParamControl::new(native_id, Arc::new(AtomicU32::new(0)));
        control.modulated = modulated;
        control
    }

    fn events(params: &mut ClapParamEvents) -> Vec<(u32, u16, u32, f64)> {
        params.buffer.sort();
        (0..params.buffer.len())
            .map(|i| {
                let header = params.buffer.header(i).unwrap();
                let event = unsafe { &*(header as *const ClapEventHeader as *const ClapEventParamValue) };
                (header.time, header.r#type, event.param_id, event.value)
            })
            .collect()
    }

    #[test]
    fn modulation_becomes_a_plain_offset_and_automation_a_plain_value() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut params = ClapParamEvents::new();
        params.set_ranges(&[info(1, true), info(2, true)]);
        params.set_controls(vec![control(1, true), control(2, false)], &instance);

        params.controls_mut().modulate(1, 0.25);
        params.controls_mut().modulate_at(1, 32, -0.5);
        params.controls_mut().automate_at(2, 64, 0.5);
        params.fill(&instance);

        assert_eq!(
            events(&mut params),
            vec![
                (0, CLAP_EVENT_PARAM_MOD, 1, 50.0),
                (32, CLAP_EVENT_PARAM_MOD, 1, -100.0),
                (64, CLAP_EVENT_PARAM_VALUE, 2, 200.0),
            ]
        );
    }

    #[test]
    fn automatable_only_parameter_is_modulated_by_value_around_its_base() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut params = ClapParamEvents::new();
        params.set_ranges(&[info(1, false)]);
        let modulated = control(1, true);
        modulated.base.store(0.5f32.to_bits(), std::sync::atomic::Ordering::Relaxed);
        params.set_controls(vec![modulated], &instance);

        params.controls_mut().modulate(1, 0.25);
        params.controls_mut().modulate_at(1, 32, -1.0);
        params.fill(&instance);

        assert_eq!(
            events(&mut params),
            vec![(0, CLAP_EVENT_PARAM_VALUE, 1, 250.0), (32, CLAP_EVENT_PARAM_VALUE, 1, 100.0)]
        );
    }

    #[test]
    fn parameter_neither_modulatable_nor_automatable_ignores_modulation() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut params = ClapParamEvents::new();
        params.set_ranges(&[read_only_info(1)]);
        params.set_controls(vec![control(1, true)], &instance);

        params.controls_mut().modulate(1, 0.25);
        params.fill(&instance);

        assert!(events(&mut params).is_empty());
    }

    #[test]
    fn dragging_in_the_plugin_window_pauses_value_modulation_and_moves_the_base() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut params = ClapParamEvents::new();
        params.set_ranges(&[info(1, false)]);
        params.set_controls(vec![control(1, true)], &instance);

        push_output(&mut params, &gesture(CLAP_EVENT_PARAM_GESTURE_BEGIN, 1).header);
        push_output(&mut params, &value_event(1, 150.0).header);
        params.controls_mut().modulate(1, 0.25);
        params.fill(&instance);
        assert!(events(&mut params).is_empty(), "no modulation while the user drags");

        push_output(&mut params, &gesture(CLAP_EVENT_PARAM_GESTURE_END, 1).header);
        params.controls_mut().clear_block();
        params.controls_mut().modulate(1, 0.25);
        params.fill(&instance);
        assert_eq!(events(&mut params), vec![(0, CLAP_EVENT_PARAM_VALUE, 1, 200.0)]);
    }

    #[test]
    fn plugin_values_outside_a_drag_do_not_move_the_base() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut params = ClapParamEvents::new();
        params.set_ranges(&[info(1, false)]);
        params.set_controls(vec![control(1, true)], &instance);

        push_output(&mut params, &value_event(1, 300.0).header);
        params.controls_mut().modulate(1, 0.0);
        params.fill(&instance);
        assert_eq!(events(&mut params), vec![(0, CLAP_EVENT_PARAM_VALUE, 1, 100.0)]);
    }

    #[test]
    fn plugin_window_edits_mark_the_project_unsaved() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut params = ClapParamEvents::new();
        params.set_ranges(&[info(1, false), info(2, false)]);
        params.set_controls(vec![control(1, true)], &instance);

        push_output(&mut params, &value_event(2, 150.0).header);
        assert!(crate::plugin_formats::clap::ffi::take_plugin_state_edited());
    }

    #[test]
    fn removed_value_modulation_restores_the_base_once() {
        let instance = PluginInstance::new_internal("test", "Test");
        let base = Arc::new(AtomicU32::new(0.25f32.to_bits()));
        let mut first = ClapParamEvents::new();
        first.set_ranges(&[info(1, false)]);
        let mut modulated = PluginParamControl::new(1, Arc::clone(&base));
        modulated.modulated = true;
        first.set_controls(vec![modulated], &instance);

        let mut second = ClapParamEvents::new();
        second.set_ranges(&[info(1, false)]);
        second.set_controls(Vec::new(), &instance);

        second.fill(&instance);
        assert_eq!(events(&mut second), vec![(0, CLAP_EVENT_PARAM_VALUE, 1, 150.0)]);
        second.fill(&instance);
        assert!(events(&mut second).is_empty(), "the restore is sent only once");
    }

    #[test]
    fn removed_modulation_is_reset_once() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut first = ClapParamEvents::new();
        first.set_ranges(&[info(1, true)]);
        first.set_controls(vec![control(1, true)], &instance);

        let mut second = ClapParamEvents::new();
        second.set_ranges(&[info(1, true)]);
        second.set_controls(Vec::new(), &instance);

        second.fill(&instance);
        assert_eq!(events(&mut second), vec![(0, CLAP_EVENT_PARAM_MOD, 1, 0.0)]);
        second.fill(&instance);
        assert!(events(&mut second).is_empty(), "the reset is sent only once");
    }

    #[test]
    fn queued_edits_are_denormalised() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut params = ClapParamEvents::new();
        params.set_ranges(&[info(1, true)]);
        assert!(instance.push_param(1, 0.5));

        params.fill(&instance);
        assert_eq!(events(&mut params), vec![(0, CLAP_EVENT_PARAM_VALUE, 1, 200.0)]);
    }
}
