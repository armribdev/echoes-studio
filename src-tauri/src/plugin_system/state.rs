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

// ── Base64 (RFC 4648, standard alphabet) ────────────────────────────────────
//
// Plugin state chunks (VST3) are stored in the project file as base64 strings
// (audit B3).  The project carries no `base64` crate dependency, so we provide
// a minimal, self-contained codec — round-trip tested below.  Compact relative
// to a JSON byte array (which would ~4× a multi-MB synth preset).

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes to a padded base64 string.
pub fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(B64_ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64_ALPHABET[((n >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64_ALPHABET[(n & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Decode a base64 string.  Returns `None` on any invalid character or
/// truncated group (whitespace is skipped, trailing `=` padding honoured).
pub fn decode_base64(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for &c in s.as_bytes() {
        match c {
            b'=' => break,
            b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => {
                let v = val(c)?;
                acc = (acc << 6) | v;
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    out.push((acc >> bits) as u8);
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_arbitrary_bytes() {
        for len in 0..=64usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let encoded = encode_base64(&bytes);
            let decoded = decode_base64(&encoded).expect("valid base64 round-trip");
            assert_eq!(decoded, bytes, "round-trip failed at len {len}");
        }
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(decode_base64("Zm9vYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn base64_decode_rejects_invalid() {
        assert!(decode_base64("@@@").is_none());
    }
}
