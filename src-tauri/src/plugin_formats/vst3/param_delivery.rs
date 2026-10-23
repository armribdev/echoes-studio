//! Parameter changes sent to a VST3 plugin each block, shared by the effect
//! wrapper and the instrument adapter.

use std::ffi::c_void;
use std::sync::Arc;

use crate::audio_graph::plugin_params::{
    PluginParamControl, PluginParamControls, ValueModulatedParam,
};
use crate::plugin_formats::crash_guard;
use crate::plugin_system::crash_marker;
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::param_queue::ParamChange;

use super::component_handler::EditState;
use super::ffi::{HostParameterChanges, ProcessData};
use super::wrapper::{vst3_process_trampoline, Vst3ProcessCtx};

/// Refill `changes` for the block (ADR-007, ADR-021).  No allocation: the list
/// is reset and filled in place.
///
/// The controller never reaches its processor on its own, so what the user does
/// in the plugin window (`IComponentHandler`) is forwarded here.  VST3 has no
/// host modulation: modulation is added to the tracked base value.  Automation
/// sets the value outright and, on the same parameter, takes precedence.
pub fn fill_param_changes(
    instance: &PluginInstance,
    controls: &PluginParamControls,
    changes: &mut HostParameterChanges,
) {
    changes.reset();
    let edit_state = instance.vst3_edit_state();
    let mirror = edit_state.map(EditState::mirror);
    let mut push_change = |change: ParamChange| {
        changes.push(
            change.native_id,
            change.sample_offset as i32,
            change.normalised as f64,
        );
    };
    instance.drain_params(|change| {
        if let Some(mirror) = mirror {
            mirror.record(change.native_id, f64::from(change.normalised));
        }
        push_change(change);
    });
    if let Some(edit_state) = edit_state {
        // The window's own edits are not mirrored back: its controller has them.
        edit_state.drain_edits(&mut push_change);
    }
    for control in controls.iter() {
        let mut latest = None;
        if control.automation.is_empty() {
            // A drag in the plugin window pauses the modulation.
            if edit_state.is_some_and(|state| state.is_editing(control.native_id)) {
                continue;
            }
            for (offset, value) in control.modulation.iter() {
                let effective = (control.base_value() + value).clamp(0.0, 1.0);
                changes.push(control.native_id, offset as i32, effective as f64);
                latest = Some(effective);
            }
        } else {
            for (offset, value) in control.automation.iter() {
                let value = value.clamp(0.0, 1.0);
                changes.push(control.native_id, offset as i32, value as f64);
                latest = Some(value);
            }
        }
        if let (Some(mirror), Some(latest)) = (mirror, latest) {
            mirror.record(control.native_id, f64::from(latest));
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
    let params: Vec<ValueModulatedParam> = controls
        .iter()
        .filter(|control| control.modulated)
        .map(|control| {
            ValueModulatedParam::new(control.native_id, control.range, Arc::clone(&control.base))
        })
        .collect();
    let previous = edit_state.replace_followed(params.clone());
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
