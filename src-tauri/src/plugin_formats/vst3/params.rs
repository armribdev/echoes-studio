//! VST3 parameter enumeration through the edit controller (ADR-021).
//!
//! The controller is read by `vst3_host::params`; this adapter gives the engine its
//! own `PluginParamInfo`.

use crate::plugin_system::parameter::PluginParamInfo;
use vst3_ffi::IEditController;

pub use vst3_host::params::{read_vst3_param_text, read_vst3_param_value, write_vst3_param_value};

/// Lists the controller's parameters.
///
/// # Safety
/// `controller` must be a live, initialised `IEditController`, called off the
/// audio thread with the instance's FFI gate held.
pub unsafe fn read_vst3_params(controller: *mut IEditController) -> Vec<PluginParamInfo> {
    vst3_host::params::read_vst3_params(controller)
        .into_iter()
        .map(|info| PluginParamInfo {
            native_id: info.native_id,
            name: info.name,
            module: info.module,
            min: info.min,
            max: info.max,
            default: info.default,
            automatable: info.automatable,
            modulatable: info.modulatable,
            hidden: info.hidden,
        })
        .collect()
}
