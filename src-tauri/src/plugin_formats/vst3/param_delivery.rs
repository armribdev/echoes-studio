//! Parameter changes sent to a VST3 plugin each block, shared by the effect
//! wrapper and the instrument adapter.

use std::sync::Arc;

use crate::audio_graph::plugin_params::{
    PluginParamControl, PluginParamControls, ValueModulatedParam,
};
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::param_queue::ParamChange;

use super::ffi::HostParameterChanges;

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
    let mut push_change = |change: ParamChange| {
        changes.push(
            change.native_id,
            change.sample_offset as i32,
            change.normalised as f64,
        );
    };
    instance.drain_params(&mut push_change);
    if let Some(edit_state) = edit_state {
        edit_state.drain_edits(&mut push_change);
    }
    for control in controls.iter() {
        if control.automation.is_empty() {
            // A drag in the plugin window pauses the modulation.
            if edit_state.is_some_and(|state| state.is_editing(control.native_id)) {
                continue;
            }
            for (offset, value) in control.modulation.iter() {
                let effective = (control.base_value() + value).clamp(0.0, 1.0);
                changes.push(control.native_id, offset as i32, effective as f64);
            }
        } else {
            for (offset, value) in control.automation.iter() {
                changes.push(
                    control.native_id,
                    offset as i32,
                    value.clamp(0.0, 1.0) as f64,
                );
            }
        }
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
