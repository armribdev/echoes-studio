//! Plugin state serialisation — persists device state in project files.
//!
//! Each device provides a `DeviceState` blob that is stored in the project
//! file alongside project-structural events.  On project load, `load_state`
//! is called to restore the exact plugin settings.
//!
//! ## Format
//!
//! `DeviceState` is an opaque byte buffer chosen by the plugin.  The host
//! treats it as a black box.  For internal devices, the convention is JSON.
//! For VST3/AU, it is the raw binary chunk from the plugin's `getState` call.

use serde::{Deserialize, Serialize};

/// Serialisable state snapshot for one device instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceState {
    /// Device ID that produced this state (used for validation on restore).
    pub device_id: String,
    /// Format tag so the host knows how to restore the state.
    pub format: StateFormat,
    /// Raw state payload.
    pub data: StateData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StateFormat {
    /// JSON object — used by internal devices.
    Json,
    /// Binary chunk — used by VST3 / AU plugins.
    Binary,
    /// Empty state (device has no persistent state).
    Empty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StateData {
    Json(serde_json::Value),
    Binary(Vec<u8>),
    Empty,
}

impl DeviceState {
    /// Create an empty state (device has no persistent data).
    pub fn empty(device_id: &str) -> Self {
        DeviceState {
            device_id: device_id.to_string(),
            format: StateFormat::Empty,
            data: StateData::Empty,
        }
    }

    /// Create a JSON state from a serialisable value.
    pub fn from_json(device_id: &str, value: serde_json::Value) -> Self {
        DeviceState {
            device_id: device_id.to_string(),
            format: StateFormat::Json,
            data: StateData::Json(value),
        }
    }

    /// Create a binary state (e.g. VST3 chunk).
    pub fn from_binary(device_id: &str, bytes: Vec<u8>) -> Self {
        DeviceState {
            device_id: device_id.to_string(),
            format: StateFormat::Binary,
            data: StateData::Binary(bytes),
        }
    }

    pub fn as_json(&self) -> Option<&serde_json::Value> {
        match &self.data {
            StateData::Json(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_binary(&self) -> Option<&[u8]> {
        match &self.data {
            StateData::Binary(b) => Some(b),
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self.format, StateFormat::Empty)
    }
}
