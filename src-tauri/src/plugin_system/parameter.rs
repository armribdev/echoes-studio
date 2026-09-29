//! Plugin parameter metadata.
//!
//! Parameter values travel through the SPSC `param_queue` (ADR-008) and the
//! graph's `ParameterBlock` (ADR-009); this module only describes what a
//! plugin reports about its parameters.

use serde::Serialize;

/// One native parameter as the plugin reports it (format-agnostic, ADR-021).
/// Values are in the plugin's plain units.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginParamInfo {
    pub native_id: u32,
    pub name: String,
    /// Group path inside the plugin, e.g. `Oscillators/Osc 1`; may be empty.
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub automatable: bool,
    /// Accepts non-destructive host modulation (LFO, envelope…).
    pub modulatable: bool,
    /// Internal parameter the plugin asks hosts not to show.
    pub hidden: bool,
}
