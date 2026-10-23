//! VST3 parameter enumeration through the edit controller (ADR-021).
//!
//! VST3 values travel normalised (0..1) end to end, so a parameter's range is
//! reported as `0..1` and its text is asked for a normalised value.

use super::ffi::{
    string128_to_string, IEditController, ParameterInfo, String128, K_RESULT_OK,
    PARAM_CAN_AUTOMATE, PARAM_IS_HIDDEN, PARAM_IS_READ_ONLY,
};
use crate::plugin_system::parameter::PluginParamInfo;

fn param_info(info: &ParameterInfo) -> PluginParamInfo {
    let read_only = info.flags & PARAM_IS_READ_ONLY != 0;
    PluginParamInfo {
        native_id: info.id,
        name: string128_to_string(&info.title),
        module: String::new(),
        min: 0.0,
        max: 1.0,
        default: info.default_normalized_value,
        automatable: info.flags & PARAM_CAN_AUTOMATE != 0 && !read_only,
        // No host modulation in VST3: modulation moves the value (ADR-021).
        modulatable: false,
        hidden: info.flags & PARAM_IS_HIDDEN != 0,
    }
}

/// Lists the controller's parameters.
///
/// # Safety
/// `controller` must be a live, initialised `IEditController`, called off the
/// audio thread with the instance's FFI gate held.
pub unsafe fn read_vst3_params(controller: *mut IEditController) -> Vec<PluginParamInfo> {
    let vtbl = &*(*controller).vtbl;
    (0..(vtbl.get_parameter_count)(controller))
        .filter_map(|index| {
            let mut info: ParameterInfo = std::mem::zeroed();
            ((vtbl.get_parameter_info)(controller, index, &mut info) == K_RESULT_OK)
                .then(|| param_info(&info))
        })
        .collect()
}

/// Current normalised value of one parameter.
///
/// # Safety
/// Same contract as [`read_vst3_params`].
pub unsafe fn read_vst3_param_value(controller: *mut IEditController, native_id: u32) -> f64 {
    ((*(*controller).vtbl).get_param_normalized)(controller, native_id)
}

/// Moves the controller (and its views) to a value that comes from the host.
///
/// # Safety
/// `controller` must be a live, initialised `IEditController`, called on the
/// thread of its editor window.
pub unsafe fn write_vst3_param_value(controller: *mut IEditController, native_id: u32, value: f64) {
    ((*(*controller).vtbl).set_param_normalized)(controller, native_id, value);
}

/// The plugin's text for a normalised value (`-6.0 dB`).
///
/// # Safety
/// Same contract as [`read_vst3_params`].
pub unsafe fn read_vst3_param_text(
    controller: *mut IEditController,
    native_id: u32,
    normalised: f64,
) -> Option<String> {
    let mut text: String128 = [0; 128];
    let r = ((*(*controller).vtbl).get_param_string_by_value)(
        controller,
        native_id,
        normalised,
        text.as_mut_ptr(),
    );
    (r == K_RESULT_OK).then(|| string128_to_string(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(flags: i32) -> ParameterInfo {
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        info.id = 7;
        info.default_normalized_value = 0.25;
        info.flags = flags;
        for (slot, unit) in info.title.iter_mut().zip("Drive".encode_utf16()) {
            *slot = unit;
        }
        info
    }

    #[test]
    fn maps_to_a_normalised_value_modulated_parameter() {
        let param = param_info(&info(PARAM_CAN_AUTOMATE));
        assert_eq!(param.native_id, 7);
        assert_eq!(param.name, "Drive");
        assert_eq!((param.min, param.max, param.default), (0.0, 1.0, 0.25));
        assert!(param.automatable && !param.modulatable && !param.hidden);
    }

    #[test]
    fn read_only_parameters_are_not_automatable() {
        let param = param_info(&info(
            PARAM_CAN_AUTOMATE | PARAM_IS_READ_ONLY | PARAM_IS_HIDDEN,
        ));
        assert!(!param.automatable);
        assert!(param.hidden);
    }
}
