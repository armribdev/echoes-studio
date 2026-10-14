//! Cross-process wire protocol for the plugin bridge (ADR-015).
//!
//! One audio block travels as one ring message:
//!
//! ```text
//!   [BlockHeader 40 B][events: header.event_bytes][samples: channels × frames f32 LE]
//! ```
//!
//! `events` is an opaque byte stream of format-native events (CLAP event structs
//! for a CLAP plugin): the engine writes the block's transport, parameter and
//! note events; the bridge answers with the plugin's output events.  This crate
//! never interprets them, so it stays format-agnostic.
//!
//! ABI rules: `#[repr(C)]`, fixed-width fields only.  Any layout change is a
//! protocol break → bump [`BRIDGE_PROTOCOL_VERSION`].

/// Wire-format version, checked at handshake (control block) and per block.
pub const BRIDGE_PROTOCOL_VERSION: u32 = 2;

/// Stamped at the start of every block so a torn / desynced region is detected.
/// ASCII `"EsBr"` (Echoes Bridge), little-endian.
pub const BLOCK_MAGIC: u32 = u32::from_le_bytes(*b"EsBr");

/// Engine → bridge: the block carries input audio (an effect).  Absent for a
/// generator with no audio input.
pub const BLOCK_HAS_INPUT: u32 = 1 << 0;
/// Engine → bridge: clear the plugin's DSP state (tails, envelopes) before
/// this block.
pub const BLOCK_RESET: u32 = 1 << 3;
/// Bridge → engine: the plugin reported a state change since the last block
/// (edit in its window, `clap.state.mark_dirty`).
pub const BLOCK_PLUGIN_EDITED: u32 = 1 << 1;
/// Bridge → engine: the plugin could not process this block; the samples are a
/// copy of the input.
pub const BLOCK_PASSED_THROUGH: u32 = 1 << 2;
/// Bridge → engine: the plugin requested a restart (e.g. its latency changed);
/// the next `Activate` reactivates it and replies with its new latency.
pub const BLOCK_RESTART_REQUESTED: u32 = 1 << 4;
/// Bridge → engine: the plugin's `clap.params.rescan` flags since the last
/// block (`CLAP_PARAM_RESCAN_*`, four bits) sit at this shift.
pub const BLOCK_PARAMS_RESCAN_SHIFT: u32 = 8;
pub const BLOCK_PARAMS_RESCAN_MASK: u32 = 0xF << BLOCK_PARAMS_RESCAN_SHIFT;
/// Bridge → engine: the plugin reported new note names (`clap.note-name.changed`)
/// since the last block.
pub const BLOCK_NOTE_NAMES_CHANGED: u32 = 1 << 12;

/// Header prefixed to every audio block, in both directions.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockHeader {
    /// [`BLOCK_MAGIC`].
    pub magic: u32,
    /// [`BRIDGE_PROTOCOL_VERSION`].
    pub version: u32,
    /// Frames in this block (≤ the layout's `max_frames`).
    pub frames: u32,
    /// Audio channels following the events.
    pub channel_count: u32,
    /// `BLOCK_*` flags.
    pub flags: u32,
    /// Bytes of event data between the header and the samples.
    pub event_bytes: u32,
    /// Monotonic block counter; a reply echoes its request's value.
    pub sequence: u64,
    /// Sample clock of the block's first frame (CLAP `steady_time`).
    pub steady_time: i64,
}

/// Serialised size of a [`BlockHeader`].
pub const BLOCK_HEADER_BYTES: usize = 40;

const _: () = assert!(core::mem::size_of::<BlockHeader>() == BLOCK_HEADER_BYTES);

impl BlockHeader {
    pub fn new(
        frames: u32,
        channel_count: u32,
        flags: u32,
        event_bytes: u32,
        sequence: u64,
        steady_time: i64,
    ) -> Self {
        BlockHeader {
            magic: BLOCK_MAGIC,
            version: BRIDGE_PROTOCOL_VERSION,
            frames,
            channel_count,
            flags,
            event_bytes,
            sequence,
            steady_time,
        }
    }

    pub fn to_bytes(&self) -> [u8; BLOCK_HEADER_BYTES] {
        // SAFETY: `#[repr(C)]` POD of exactly BLOCK_HEADER_BYTES (const-asserted).
        unsafe { core::mem::transmute_copy::<BlockHeader, [u8; BLOCK_HEADER_BYTES]>(self) }
    }

    /// `None` when `bytes` is too short or the magic/version do not match.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let raw: [u8; BLOCK_HEADER_BYTES] = bytes.get(..BLOCK_HEADER_BYTES)?.try_into().ok()?;
        // SAFETY: equal-size by-value transmute onto an owned POD value.
        let header = unsafe { core::mem::transmute::<[u8; BLOCK_HEADER_BYTES], BlockHeader>(raw) };
        (header.magic == BLOCK_MAGIC && header.version == BRIDGE_PROTOCOL_VERSION).then_some(header)
    }
}

/// Encode `samples` as little-endian bytes into `out` (same number of samples).
pub fn write_samples(samples: &[f32], out: &mut [u8]) {
    for (sample, dst) in samples.iter().zip(out.chunks_exact_mut(4)) {
        dst.copy_from_slice(&sample.to_le_bytes());
    }
}

/// Decode little-endian `bytes` into `out`; missing samples become silence.
pub fn read_samples(bytes: &[u8], out: &mut [f32]) {
    let mut chunks = bytes.chunks_exact(4);
    for sample in out.iter_mut() {
        *sample = chunks
            .next()
            .map_or(0.0, |c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_header_abi_is_stable() {
        assert_eq!(core::mem::size_of::<BlockHeader>(), 40);
        assert_eq!(core::mem::align_of::<BlockHeader>(), 8);
    }

    #[test]
    fn magic_is_readable_ascii() {
        assert_eq!(&BLOCK_MAGIC.to_le_bytes(), b"EsBr");
    }

    #[test]
    fn block_header_bytes_roundtrip() {
        let header = BlockHeader::new(256, 2, BLOCK_HAS_INPUT, 120, 42, -7);
        assert_eq!(BlockHeader::from_bytes(&header.to_bytes()), Some(header));
    }

    #[test]
    fn from_bytes_rejects_short_or_foreign_input() {
        assert_eq!(BlockHeader::from_bytes(&[0u8; 8]), None);
        let mut bytes = BlockHeader::new(1, 1, 0, 0, 0, 0).to_bytes();
        bytes[0] ^= 0xFF;
        assert_eq!(BlockHeader::from_bytes(&bytes), None);
    }

    #[test]
    fn samples_roundtrip_and_pad_with_silence() {
        let samples = [0.5, -1.0, 0.25];
        let mut bytes = [0u8; 12];
        write_samples(&samples, &mut bytes);
        let mut decoded = [9.0f32; 4];
        read_samples(&bytes, &mut decoded);
        assert_eq!(decoded, [0.5, -1.0, 0.25, 0.0]);
    }
}
