//! Off-real-time control channel between the engine and the bridge (ADR-015).
//!
//! Requests that are not per-block audio — activation, state save/load,
//! parameter introspection, the editor window — travel over the child's
//! stdin/stdout pipes as length-prefixed frames:
//!
//! ```text
//!   request:  [len:u32 LE][op:u8][payload]        (len counts op + payload)
//!   response: [len:u32 LE][status:u8][payload]    (status 0 = ok, 1 = error text)
//! ```
//!
//! Strictly request → response, one at a time; the engine side serialises
//! callers.  Never used on the audio thread.

use std::io::{self, Read, Write};

/// Largest frame either side accepts — a corrupt or hostile length prefix must
/// not trigger a huge allocation.
const MAX_FRAME_BYTES: u32 = 256 * 1024 * 1024;

pub const STATUS_OK: u8 = 0;
pub const STATUS_ERROR: u8 = 1;

/// Control operations.  Payloads are little-endian.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `sample_rate:f64, max_frames:u32` → `latency:u32`.  Idempotent; a new
    /// configuration deactivates and reactivates the plugin.
    Activate = 1,
    /// → the plugin's state bytes.
    SaveState = 2,
    /// state bytes → ().
    LoadState = 3,
    /// → the parameter list (format-native records, see the bridge host).
    Params = 4,
    /// `native_id:u32` → `value:f64`.
    ParamValue = 5,
    /// `[native_id:u32, value:f64]*` → ().  Sets parameters outside `process`.
    FlushValues = 6,
    /// `title:utf8` → `window:u64` (a native window handle).  Brings an
    /// already open editor to the front.  The engine closes it by posting
    /// `WM_CLOSE` to that window.
    OpenEditor = 7,
    /// `native_id:u32, value:f64` → the plugin's text for that plain value, empty
    /// when it has none.
    ParamText = 8,
    /// The plugin's track (`clap.track-info`, encoded by the CLAP bindings) → ().
    /// The bridge calls the plugin's `changed` when it differs from the last one.
    TrackInfo = 9,
    /// → the plugin's note names (`clap.note-name`, encoded by the CLAP bindings).
    NoteNames = 10,
    /// A `clap_ffi::PresetRequest` in its wire form → ().  The plugin loads the
    /// preset (`clap.preset-load`); a refusal is the plugin's own message.
    PresetLoad = 11,
}

impl Op {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => Op::Activate,
            2 => Op::SaveState,
            3 => Op::LoadState,
            4 => Op::Params,
            5 => Op::ParamValue,
            6 => Op::FlushValues,
            7 => Op::OpenEditor,
            8 => Op::ParamText,
            9 => Op::TrackInfo,
            10 => Op::NoteNames,
            11 => Op::PresetLoad,
            _ => return None,
        })
    }
}

/// Write one frame: `tag` is an [`Op`] (request) or a status (response).
pub fn write_frame(writer: &mut impl Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len() + 1)
        .ok()
        .filter(|&len| len <= MAX_FRAME_BYTES)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "control frame too large"))?;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&[tag])?;
    writer.write_all(payload)?;
    writer.flush()
}

/// Read one frame, returning `(tag, payload)`.
pub fn read_frame(reader: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut len = [0u8; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad control frame length",
        ));
    }
    let mut tag = [0u8; 1];
    reader.read_exact(&mut tag)?;
    let mut payload = vec![0u8; len as usize - 1];
    reader.read_exact(&mut payload)?;
    Ok((tag[0], payload))
}

/// Sequential little-endian reader over a payload.
pub struct PayloadReader<'a> {
    bytes: &'a [u8],
}

impl<'a> PayloadReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        PayloadReader { bytes }
    }

    fn take<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let (head, rest) = self
            .bytes
            .split_first_chunk::<N>()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short control payload"))?;
        self.bytes = rest;
        Ok(*head)
    }

    pub fn u32(&mut self) -> io::Result<u32> {
        self.take().map(u32::from_le_bytes)
    }

    pub fn u64(&mut self) -> io::Result<u64> {
        self.take().map(u64::from_le_bytes)
    }

    pub fn f64(&mut self) -> io::Result<f64> {
        self.take().map(f64::from_le_bytes)
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip() {
        let mut wire = Vec::new();
        write_frame(&mut wire, Op::LoadState as u8, b"state").unwrap();
        write_frame(&mut wire, STATUS_OK, &[]).unwrap();

        let mut reader = wire.as_slice();
        assert_eq!(
            read_frame(&mut reader).unwrap(),
            (Op::LoadState as u8, b"state".to_vec())
        );
        assert_eq!(read_frame(&mut reader).unwrap(), (STATUS_OK, Vec::new()));
    }

    #[test]
    fn oversized_length_prefix_is_rejected() {
        let mut wire = (MAX_FRAME_BYTES + 1).to_le_bytes().to_vec();
        wire.push(STATUS_OK);
        assert!(read_frame(&mut wire.as_slice()).is_err());
    }

    #[test]
    fn ops_roundtrip_through_u8() {
        for op in [
            Op::Activate,
            Op::SaveState,
            Op::LoadState,
            Op::Params,
            Op::ParamValue,
            Op::FlushValues,
            Op::OpenEditor,
            Op::ParamText,
            Op::TrackInfo,
            Op::NoteNames,
        ] {
            assert_eq!(Op::from_u8(op as u8), Some(op));
        }
        assert_eq!(Op::from_u8(0), None);
    }

    #[test]
    fn payload_reader_decodes_in_order() {
        let mut bytes = 48_000.0f64.to_le_bytes().to_vec();
        bytes.extend_from_slice(&512u32.to_le_bytes());
        let mut reader = PayloadReader::new(&bytes);
        assert_eq!(reader.f64().unwrap(), 48_000.0);
        assert_eq!(reader.u32().unwrap(), 512);
        assert!(reader.is_empty());
        assert!(reader.u32().is_err());
    }
}
