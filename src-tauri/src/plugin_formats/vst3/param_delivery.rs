//! Parameter changes sent to a VST3 plugin each block, shared by the effect
//! wrapper and the instrument adapter.

use std::ffi::c_void;
use std::sync::Arc;

use crate::audio_graph::plugin_params::{
    PluginParamControl, PluginParamControls, PluginParamRange, ValueModulatedParam,
};
use crate::plugin_formats::crash_guard;
use crate::plugin_system::crash_marker;
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::param_edits::{write_armed, EditCapture};
use plugin_bridge::protocol::{WireParam, PARAM_GESTURE_BEGIN, PARAM_GESTURE_END, PARAM_VALUE};

use super::component_handler::{EditState, FollowedParam};
use super::ffi::{HostParameterChanges, ProcessData};
use super::wrapper::{vst3_process_trampoline, Vst3ProcessCtx};

/// Where the host's parameter values of a block go: the plugin's own change list
/// in the Echoes process, a wire section for the bridge.  Values are normalised.
pub trait Vst3ParamSink {
    fn push_value(&mut self, offset: u32, native_id: u32, normalised: f64);
}

impl Vst3ParamSink for HostParameterChanges {
    fn push_value(&mut self, offset: u32, native_id: u32, normalised: f64) {
        self.push(native_id, offset as i32, normalised);
    }
}

/// Automation and modulation of one block, for the parameters a route or a lane
/// targets.  VST3 has no host modulation: modulation is added to the tracked base
/// value.  Automation sets the value outright and, on the same parameter, takes
/// precedence.  `is_editing` tells that the user drags a control in the plugin
/// window, which pauses its modulation (and its automation while writing is armed); `record_latest` receives the last value
/// sent for each parameter.
fn emit_control_changes(
    controls: &PluginParamControls,
    is_editing: impl Fn(u32) -> bool,
    mut record_latest: impl FnMut(u32, f32),
    sink: &mut impl Vst3ParamSink,
) {
    for control in controls.iter() {
        let mut latest = None;
        if control.automation.is_empty() {
            if is_editing(control.native_id) {
                continue;
            }
            for (offset, value) in control.modulation.iter() {
                let effective = (control.base_value() + value).clamp(0.0, 1.0);
                sink.push_value(offset, control.native_id, effective as f64);
                latest = Some(effective);
            }
        } else if !(write_armed() && is_editing(control.native_id)) {
            // Writing armed: the lane leaves a dragged parameter alone (ADR-037).
            for (offset, value) in control.automation.iter() {
                let value = value.clamp(0.0, 1.0);
                sink.push_value(offset, control.native_id, value as f64);
                latest = Some(value);
            }
        }
        if let Some(latest) = latest {
            record_latest(control.native_id, latest);
        }
    }
}

/// Refill `changes` for the block (ADR-007, ADR-021).  No allocation: the list
/// is reset and filled in place.
///
/// The controller never reaches its processor on its own, so what the user does
/// in the plugin window (`IComponentHandler`) is forwarded here.
pub fn fill_param_changes(
    instance: &PluginInstance,
    controls: &PluginParamControls,
    capture: &mut EditCapture,
    changes: &mut HostParameterChanges,
) {
    changes.reset();
    let edit_state = instance.vst3_edit_state();
    let mirror = edit_state.map(EditState::mirror);
    instance.drain_params(|change| {
        if let Some(mirror) = mirror {
            mirror.record(change.native_id, f64::from(change.normalised));
        }
        changes.push_value(
            change.sample_offset,
            change.native_id,
            change.normalised as f64,
        );
    });
    if let Some(edit_state) = edit_state {
        // The window's own edits are not mirrored back: its controller has them.
        let dragged = edit_state.active_gestures();
        capture.open_dragged(&dragged);
        edit_state.drain_edits(|change| {
            changes.push_value(
                change.sample_offset,
                change.native_id,
                change.normalised as f64,
            );
            capture.window_value(change.native_id, f64::from(change.normalised));
        });
        capture.close_released(&dragged);
    }
    emit_control_changes(
        controls,
        |native_id| edit_state.is_some_and(|state| state.is_editing(native_id)),
        |native_id, latest| {
            if let Some(mirror) = mirror {
                mirror.record(native_id, f64::from(latest));
            }
        },
        changes,
    );
}

/// The range of a VST3 parameter in the units of `ValueModulatedParam`: values
/// are normalised end to end.
const NORMALISED_RANGE: PluginParamRange = PluginParamRange {
    min: 0.0,
    max: 1.0,
    modulatable: false,
    automatable: true,
};

/// Parameter state of one placement whose plugin runs in the bridge, and the
/// values it sends each block (ADR-021, ADR-033 step 6).  The bridge converts and
/// computes nothing: it gets the effective normalised values and reports what the
/// plugin window does.
///
/// An edit reaches this side one block late, so a drag pauses the modulation one
/// block after it started; the bridge applies the edit to its plugin at once and
/// puts it last in the block, so the user's value wins meanwhile.
pub struct Vst3ParamEvents {
    controls: PluginParamControls,
    /// Parameters modulated by value; an edit in the plugin window moves their base.
    followed: Vec<FollowedParam>,
    /// Parameters being dragged in the plugin window right now.
    editing: Vec<u32>,
}

impl Default for Vst3ParamEvents {
    fn default() -> Self {
        Self::new()
    }
}

impl Vst3ParamEvents {
    pub fn new() -> Self {
        Self {
            controls: PluginParamControls::default(),
            followed: Vec::new(),
            editing: Vec::with_capacity(vst3_host::component_handler::MAX_GESTURES),
        }
    }

    pub fn controls_mut(&mut self) -> &mut PluginParamControls {
        &mut self.controls
    }

    /// Off the audio thread, at compile time (ADR-021): the parameters modulated by
    /// value from now on, whose bases a state save restores.
    pub fn set_controls(&mut self, controls: Vec<PluginParamControl>, instance: &PluginInstance) {
        let followed = followed_params(&controls);
        let previous = instance.replace_value_modulated_params(
            followed
                .iter()
                .map(|param| {
                    ValueModulatedParam::new(
                        param.native_id,
                        NORMALISED_RANGE,
                        Arc::clone(&param.base),
                    )
                })
                .collect(),
        );
        let previous: Vec<FollowedParam> = previous
            .into_iter()
            .map(|param| FollowedParam::new(param.native_id, param.base))
            .collect();
        settle_followed(instance, &followed, &previous);
        self.followed = followed;
        self.controls = PluginParamControls::new(controls);
    }

    /// Audio thread: this block's values, queued edits first.  `sink` is the
    /// caller's to clear.  No allocation.
    pub fn fill_into(&mut self, instance: &PluginInstance, sink: &mut impl Vst3ParamSink) {
        instance.drain_params(|change| {
            sink.push_value(
                change.sample_offset,
                change.native_id,
                change.normalised as f64,
            );
        });
        emit_control_changes(
            &self.controls,
            |native_id| self.editing.contains(&native_id),
            |_, _| {},
            sink,
        );
    }

    /// Audio thread: what the plugin window did to one of its parameters.  An edit
    /// moves the base of a parameter modulated by value, as `EditState::perform`
    /// does in the Echoes process.
    pub fn observe_edit(&mut self, edit: &WireParam) {
        match edit.kind {
            PARAM_VALUE => {
                if let Some(param) = self
                    .followed
                    .iter()
                    .find(|param| param.native_id == edit.native_id)
                {
                    param.set_base(edit.value as f32);
                }
            }
            PARAM_GESTURE_BEGIN
                if !self.editing.contains(&edit.native_id)
                    && self.editing.len() < self.editing.capacity() =>
            {
                self.editing.push(edit.native_id);
            }
            PARAM_GESTURE_END => self
                .editing
                .retain(|&native_id| native_id != edit.native_id),
            _ => {}
        }
    }
}

/// Empties the list the plugin writes its own parameter changes to
/// (`outputParameterChanges`) and returns it for `ProcessData`.
pub fn begin_output_changes(changes: &mut HostParameterChanges) -> *mut c_void {
    changes.reset();
    changes as *mut HostParameterChanges as *mut c_void
}

/// After `process`: what the plugin changed by itself (a program, a macro, a
/// meter) is kept for its controller.  The processor already has these values and
/// they neither mark the project unsaved nor move a modulation base.
pub fn publish_output_changes(instance: &PluginInstance, changes: &HostParameterChanges) {
    if let Some(edit_state) = instance.vst3_edit_state() {
        for (native_id, value) in changes.latest_points() {
            edit_state.mirror().record(native_id, value);
        }
    }
}

/// Off the audio thread, FFI gate held (ADR-010): puts each parameter modulated by
/// value back on its base with a zero-sample `process`, the parameter flush that
/// `ProcessData` allows, so a state save captures the user's setting and not a
/// point of the modulation.  The next block modulates again.
pub fn flush_param_bases(instance: &PluginInstance) {
    let (Some(edit_state), Some((_, processor))) =
        (instance.vst3_edit_state(), instance.vst3_handles())
    else {
        return;
    };
    let bases = edit_state.followed_bases();
    if bases.is_empty() {
        return;
    }
    let mut changes = HostParameterChanges::boxed();
    for (native_id, base) in bases {
        changes.push(native_id, 0, base);
    }
    let mut data = ProcessData {
        process_mode: 0,
        symbolic_sample_size: 0,
        num_samples: 0,
        num_inputs: 0,
        num_outputs: 0,
        inputs: std::ptr::null_mut(),
        outputs: std::ptr::null_mut(),
        input_param_changes: changes.as_mut() as *mut HostParameterChanges as *mut c_void,
        output_param_changes: std::ptr::null_mut(),
        input_events: std::ptr::null_mut(),
        output_events: std::ptr::null_mut(),
        process_context: std::ptr::null_mut(),
    };
    let mut ctx = Vst3ProcessCtx {
        proc_ptr: processor.as_ptr(),
        data_ptr: &mut data,
    };
    // SAFETY: live VST3 processor owned by this instance; the caller holds the FFI gate.
    let outcome = unsafe {
        crash_guard::guarded(
            vst3_process_trampoline,
            &mut ctx as *mut Vst3ProcessCtx as *mut c_void,
        )
    };
    if outcome == crash_guard::CAUGHT {
        instance.set_lifecycle(LifecycleState::Error);
        crash_marker::note_crash();
    }
}

/// Off the audio thread (ADR-021): a newly modulated parameter starts from the
/// plugin's current value; one no longer modulated is put back on its base.
pub fn track_value_modulation(instance: &PluginInstance, controls: &[PluginParamControl]) {
    let Some(edit_state) = instance.vst3_edit_state() else {
        return;
    };
    let params = followed_params(controls);
    let previous = edit_state.replace_followed(params.clone());
    settle_followed(instance, &params, &previous);
}

fn followed_params(controls: &[PluginParamControl]) -> Vec<FollowedParam> {
    controls
        .iter()
        .filter(|control| control.modulated)
        .map(|control| FollowedParam::new(control.native_id, Arc::clone(&control.base)))
        .collect()
}

/// A newly modulated parameter starts from the plugin's current value; one no
/// longer modulated is put back on its base.
fn settle_followed(
    instance: &PluginInstance,
    params: &[FollowedParam],
    previous: &[FollowedParam],
) {
    let was_modulated = |native_id: u32| previous.iter().any(|param| param.native_id == native_id);

    for param in params
        .iter()
        .filter(|param| !was_modulated(param.native_id))
    {
        if let Some(value) = instance.param_value(param.native_id) {
            param.set_base(value as f32);
        }
    }
    for released in previous
        .iter()
        .filter(|old| !params.iter().any(|param| param.native_id == old.native_id))
    {
        instance.push_param(released.native_id, released.base_value());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    #[derive(Default)]
    struct RecordedSink(Vec<(u32, u32, f64)>);

    impl Vst3ParamSink for RecordedSink {
        fn push_value(&mut self, offset: u32, native_id: u32, normalised: f64) {
            self.0.push((offset, native_id, normalised));
        }
    }

    fn control(native_id: u32, base: f32) -> PluginParamControl {
        PluginParamControl::new(native_id, Arc::new(AtomicU32::new(base.to_bits())))
    }

    fn emitted(
        controls: &mut PluginParamControls,
        is_editing: impl Fn(u32) -> bool,
    ) -> Vec<(u32, u32, f64)> {
        let mut sink = RecordedSink::default();
        emit_control_changes(controls, is_editing, |_, _| {}, &mut sink);
        sink.0
    }

    #[test]
    fn modulation_moves_around_the_base_and_stays_in_range() {
        let mut controls = PluginParamControls::new(vec![control(3, 0.5)]);
        controls.modulate_at(3, 0, 0.25);
        controls.modulate_at(3, 16, 0.75);
        assert_eq!(
            emitted(&mut controls, |_| false),
            [(0, 3, 0.75), (16, 3, 1.0)]
        );
    }

    #[test]
    fn automation_sets_the_value_outright_and_wins_over_modulation() {
        let mut controls = PluginParamControls::new(vec![control(3, 0.5)]);
        controls.modulate_at(3, 0, 0.25);
        controls.automate_at(3, 0, 0.125);
        controls.automate_at(3, 8, 2.0);
        assert_eq!(
            emitted(&mut controls, |_| false),
            [(0, 3, 0.125), (8, 3, 1.0)]
        );
    }

    #[test]
    fn a_drag_pauses_the_modulation_but_not_the_automation() {
        let mut controls = PluginParamControls::new(vec![control(3, 0.5), control(4, 0.5)]);
        controls.modulate_at(3, 0, 0.25);
        controls.automate_at(4, 0, 0.25);
        assert_eq!(
            emitted(&mut controls, |_| false),
            [(0, 3, 0.75), (0, 4, 0.25)]
        );
        assert_eq!(emitted(&mut controls, |id| id == 3), [(0, 4, 0.25)]);
        assert_eq!(
            emitted(&mut controls, |id| id == 4),
            [(0, 3, 0.75), (0, 4, 0.25)]
        );
    }

    #[test]
    fn the_latest_value_of_each_parameter_is_recorded() {
        let mut controls = PluginParamControls::new(vec![control(3, 0.5)]);
        controls.modulate_at(3, 0, 0.25);
        controls.modulate_at(3, 16, 0.125);
        let mut latest = Vec::new();
        emit_control_changes(
            &controls,
            |_| false,
            |native_id, value| latest.push((native_id, value)),
            &mut RecordedSink::default(),
        );
        assert_eq!(latest, [(3, 0.625)]);
    }

    #[test]
    fn an_edit_of_the_plugin_window_moves_the_base_of_a_followed_parameter_only() {
        let base = Arc::new(AtomicU32::new(0.5f32.to_bits()));
        let mut events = Vst3ParamEvents::new();
        events.followed = vec![FollowedParam::new(3, Arc::clone(&base))];

        events.observe_edit(&WireParam::new(PARAM_VALUE, 0, 3, 0.8));
        events.observe_edit(&WireParam::new(PARAM_VALUE, 0, 9, 0.1));
        assert_eq!(f32::from_bits(base.load(Ordering::Relaxed)), 0.8);
    }

    #[test]
    fn a_gesture_lasts_from_its_begin_to_its_end_and_the_list_never_grows() {
        let mut events = Vst3ParamEvents::new();
        let capacity = events.editing.capacity();
        events.observe_edit(&WireParam::new(PARAM_GESTURE_BEGIN, 0, 3, 0.0));
        events.observe_edit(&WireParam::new(PARAM_GESTURE_BEGIN, 0, 3, 0.0));
        assert_eq!(events.editing, [3]);
        events.observe_edit(&WireParam::new(PARAM_GESTURE_END, 0, 3, 0.0));
        assert!(events.editing.is_empty());

        for native_id in 0..capacity as u32 + 4 {
            events.observe_edit(&WireParam::new(PARAM_GESTURE_BEGIN, 0, native_id, 0.0));
        }
        assert_eq!(
            (events.editing.len(), events.editing.capacity()),
            (capacity, capacity)
        );
    }
}
