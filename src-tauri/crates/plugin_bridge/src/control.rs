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

/// Control operations.  Payloads are little-endian; the records they carry are
/// format-neutral and defined in [`crate::records`].
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
    /// → the parameter list ([`crate::records::ParamRecord`]s).
    Params = 4,
    /// `native_id:u32` → `value:f64`.
    ParamValue = 5,
    /// `[native_id:u32, value:f64]*` → ().  Sets parameters outside `process`.
    FlushValues = 6,
    /// `title:utf8` → `window:u64` (a native window handle).  Brings an
    /// already open editor to the front.  The engine closes it by posting
    /// `WM_CLOSE` to that window.
    OpenEditor = 7,
    /// `native_id:u32, value:f64` → the plugin's text for that value, empty
    /// when it has none.
    ParamText = 8,
    /// A [`crate::records::TrackRecord`] → ().  The bridge tells the plugin when
    /// it differs from the last one.
    TrackInfo = 9,
    /// → the plugin's note names ([`crate::records::NoteNameRecord`]s).
    NoteNames = 10,
    /// A [`crate::records::PresetRecord`] → ().  The plugin loads the preset; a
    /// refusal is the plugin's own message.
    PresetLoad = 11,
    /// → the [`crate::records::Handshake`]: the unit of the plugin's parameter
    /// values and its audio port table.  Asked once, right after the start.
    Handshake = 12,
    /// → the program lists the plugin offers ([`crate::records::ProgramListRecord`]s).
    /// Empty for a plugin without any.
    PresetList = 13,
    /// A [`crate::records::PresetStateRecord`] → ().  The plugin takes the state of a
    /// preset file the engine has read and checked.
    PresetLoadState = 14,
    /// → the [`crate::records::PresetRecord`] a CLAP plugin last reported as loaded
    /// (`clap.preset-load`), empty when it never did.
    LoadedPreset = 15,
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
            12 => Op::Handshake,
            13 => Op::PresetList,
            14 => Op::PresetLoadState,
            15 => Op::LoadedPreset,
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

    pub fn u8(&mut self) -> io::Result<u8> {
        self.take::<1>().map(|[byte]| byte)
    }

    pub fn u16(&mut self) -> io::Result<u16> {
        self.take().map(u16::from_le_bytes)
    }

    pub fn i16(&mut self) -> io::Result<i16> {
        self.take().map(i16::from_le_bytes)
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

    /// Text written by [`PayloadWriter::text`]: `length:u16`, then UTF-8.
    pub fn text(&mut self) -> io::Result<String> {
        let length = self.u16()? as usize;
        let (head, rest) = self
            .bytes
            .split_at_checked(length)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short control payload"))?;
        self.bytes = rest;
        String::from_utf8(head.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "control text is not UTF-8"))
    }

    /// Bytes written by [`PayloadWriter::bytes`]: `length:u32`, then the bytes.
    pub fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let length = self.u32()? as usize;
        let (head, rest) = self
            .bytes
            .split_at_checked(length)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short control payload"))?;
        self.bytes = rest;
        Ok(head.to_vec())
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Bytes not read yet.
    pub fn remaining(&self) -> usize {
        self.bytes.len()
    }
}

/// Builds a payload the [`PayloadReader`] reads back.
#[derive(Default)]
pub struct PayloadWriter {
    bytes: Vec<u8>,
}

impl PayloadWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u8(&mut self, value: u8) -> &mut Self {
        self.bytes.push(value);
        self
    }

    pub fn u16(&mut self, value: u16) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn i16(&mut self, value: i16) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn u32(&mut self, value: u32) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn f64(&mut self, value: f64) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// `length:u16` then UTF-8; a text that does not fit is cut at a character.
    pub fn text(&mut self, text: &str) -> &mut Self {
        let mut end = text.len().min(u16::MAX as usize);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.u16(end as u16);
        self.bytes.extend_from_slice(&text.as_bytes()[..end]);
        self
    }

    /// `length:u32` then the bytes; `false` (nothing written) when the length does
    /// not fit a frame.
    pub fn bytes(&mut self, bytes: &[u8]) -> bool {
        let Ok(length) = u32::try_from(bytes.len()) else {
            return false;
        };
        if length > MAX_FRAME_BYTES {
            return false;
        }
        self.u32(length);
        self.bytes.extend_from_slice(bytes);
        true
    }

    /// Like [`Self::text`], but `false` (nothing written) when it would be cut.
    pub fn text_exact(&mut self, text: &str) -> bool {
        if text.len() > u16::MAX as usize {
            return false;
        }
        self.text(text);
        true
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
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
            Op::PresetLoad,
            Op::Handshake,
            Op::PresetList,
            Op::PresetLoadState,
            Op::LoadedPreset,
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

    #[test]
    fn the_writer_and_the_reader_agree() {
        let mut writer = PayloadWriter::new();
        writer.u8(7).u16(513).i16(-1).u32(9).f64(0.5).text("café");
        let bytes = writer.finish();
        let mut reader = PayloadReader::new(&bytes);
        assert_eq!(reader.u8().unwrap(), 7);
        assert_eq!(reader.u16().unwrap(), 513);
        assert_eq!(reader.i16().unwrap(), -1);
        assert_eq!(reader.u32().unwrap(), 9);
        assert_eq!(reader.f64().unwrap(), 0.5);
        assert_eq!(reader.text().unwrap(), "café");
        assert!(reader.is_empty());
    }

    #[test]
    fn a_text_longer_than_its_prefix_is_cut_or_refused() {
        let long = "é".repeat(40_000);
        let mut cut = PayloadWriter::new();
        cut.text(&long);
        let bytes = cut.finish();
        let text = PayloadReader::new(&bytes).text().unwrap();
        assert!(text.len() <= u16::MAX as usize && long.starts_with(&text));
        assert!(!PayloadWriter::new().text_exact(&long));
    }

    #[test]
    fn bytes_roundtrip_and_a_short_payload_is_an_error() {
        let mut writer = PayloadWriter::new();
        assert!(writer.bytes(&[1, 2, 3]));
        let bytes = writer.finish();
        assert_eq!(PayloadReader::new(&bytes).bytes().unwrap(), [1, 2, 3]);
        assert!(PayloadReader::new(&bytes[..bytes.len() - 1])
            .bytes()
            .is_err());
        assert!(PayloadReader::new(&u32::MAX.to_le_bytes()).bytes().is_err());
    }

    #[test]
    fn a_truncated_or_foreign_text_is_an_error() {
        assert!(PayloadReader::new(&[5, 0, b'a']).text().is_err());
        assert!(PayloadReader::new(&[1, 0, 0xFF]).text().is_err());
    }
}
