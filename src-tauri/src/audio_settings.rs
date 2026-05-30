//! Audio settings types — shared between the Tauri commands and the engine.
//!
//! These structs are serialised / deserialised by Tauri's IPC layer so the
//! frontend can read available devices and push new settings to the backend.
//! They contain **no** CPAL types; keep this module free of cpal imports.

use serde::{Deserialize, Serialize};

// ── Device list (sent to frontend) ────────────────────────────────────────────

/// One audio device entry in the device picker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceInfo {
    /// Machine-readable ID used to select the device (same as name for CPAL).
    pub id: String,
    /// Human-readable display name shown in the UI.
    pub name: String,
}

/// Full list of available input and output devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceList {
    pub inputs: Vec<AudioDeviceInfo>,
    pub outputs: Vec<AudioDeviceInfo>,
}

// ── Audio settings (received from frontend) ───────────────────────────────────

/// Engine configuration sent from the Settings panel.
///
/// `output_device` / `input_device` match the `id` field of `AudioDeviceInfo`
/// (i.e. the CPAL device name), or the literal string `"default"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSettings {
    /// Output device name, or `"default"` for the system default.
    pub output_device: String,
    /// Input device name (reserved — not yet used for recording).
    pub input_device: String,
    /// Requested output sample rate in Hz (e.g. 44100, 48000, 96000).
    pub sample_rate: u32,
    /// Requested buffer size in frames (e.g. 64, 128, 256, 512).
    pub buffer_size: u32,
    /// Number of output channels to use (1 = mono downmix, 2 = stereo, …).
    /// Must be ≤ the device's native channel count.
    /// `0` means "use all device channels" (default behaviour).
    pub output_channels: u16,
    /// When true, the master bus applies tanh soft-clip before device output.
    #[serde(default)]
    pub master_soft_clip: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings {
            output_device: "default".to_string(),
            input_device: "default".to_string(),
            sample_rate: 48_000,
            buffer_size: 256,
            output_channels: 0,
            master_soft_clip: false,
        }
    }
}
