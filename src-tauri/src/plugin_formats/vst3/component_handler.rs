//! The host's `IComponentHandler` of a VST3 edit controller (`vst3_host::component_handler`),
//! wired to the engine's process-wide plugin signals: the same ones the CLAP host
//! callbacks raise (unsaved flag, restart request, parameter rescan).

use std::sync::Arc;

use crate::plugin_formats::clap::ffi::{
    mark_plugin_params_rescan, mark_plugin_restart_requested, mark_plugin_state_edited,
    CLAP_PARAM_RESCAN_INFO, CLAP_PARAM_RESCAN_TEXT,
};

pub use vst3_host::component_handler::{ComponentHandler, EditState, FollowedParam, HostSignals};

struct EngineSignals;

impl HostSignals for EngineSignals {
    fn state_edited(&self) {
        mark_plugin_state_edited();
    }

    fn restart_requested(&self) {
        mark_plugin_restart_requested();
    }

    fn params_changed(&self, titles_too: bool) {
        let info = if titles_too {
            CLAP_PARAM_RESCAN_INFO
        } else {
            0
        };
        mark_plugin_params_rescan(CLAP_PARAM_RESCAN_TEXT | info);
    }
}

/// A new handler for one plugin instance, reporting to the engine.
pub fn engine_component_handler() -> Box<ComponentHandler> {
    ComponentHandler::boxed(Arc::new(EngineSignals))
}
