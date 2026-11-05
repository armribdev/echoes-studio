//! Cross-process wire protocol for the plugin bridge (ADR-015, ADR-033 step 4).
//!
//! One audio block travels as one ring message:
//!
//! ```text
//!   [BlockHeader 48 B]
//!   [WireTransport 64 B]                    only with BLOCK_HAS_TRANSPORT
//!   [notes:  header.note_count  × WireNote  12 B]
//!   [params: header.param_count × WireParam 24 B]
//!   [samples: channel_count × frames f32 LE]
//! ```
//!
//! `channel_count` is the channels of the samples **in this message**: in a request
//! those of every input port, in a reply those of every output port, each port
//! taking [`crate::records::PORT_CHANNELS`] channels in the order of the port table
//! ([`crate::records::Handshake`], active ports only).  A bus that is not open
//! plays silence.
//!
//! Engine → bridge, the sections are the block's transport, its notes and raw MIDI
//! (one ordered list), and its parameter events (value, modulation offset, gesture
//! begin / end) with their sample offset.  Bridge → engine, `params` holds what the
//! plugin did to its own parameters (edits and gestures) and `notes` the MIDI it
//! produced.  Nothing here depends on a plugin format: an adapter in the bridge
//! turns the sections into CLAP events (or, later, VST3 structures).
//!
//! Parameter values are never converted by the bridge.  The handshake
//! ([`crate::records::Handshake`]) declares the unit of the format and the engine
//! sends that unit.
//!
//! ABI rules: `#[repr(C)]`, fixed-width fields only, **no implicit padding** (every
//! gap is a named reserved field, so a record can be copied as bytes).  Any layout
//! change is a protocol break → bump [`BRIDGE_PROTOCOL_VERSION`].

use core::marker::PhantomData;
use core::mem::size_of;

/// Wire-format version, checked at handshake (control block) and per block.
pub const BRIDGE_PROTOCOL_VERSION: u32 = 4;

/// Stamped at the start of every block so a torn / desynced region is detected.
/// ASCII `"EsBr"` (Echoes Bridge), little-endian.
pub const BLOCK_MAGIC: u32 = u32::from_le_bytes(*b"EsBr");

/// Engine → bridge: the block carries input audio (an effect).  Absent for a
/// generator with no audio input.
pub const BLOCK_HAS_INPUT: u32 = 1 << 0;
/// Engine → bridge: clear the plugin's DSP state (tails, envelopes) before
/// this block.
pub const BLOCK_RESET: u32 = 1 << 3;
/// Engine → bridge: a [`WireTransport`] section follows the header.
pub const BLOCK_HAS_TRANSPORT: u32 = 1 << 5;
/// Bridge → engine: the plugin reported a state change since the last block
/// (edit in its window, dirty flag).
pub const BLOCK_PLUGIN_EDITED: u32 = 1 << 1;
/// Bridge → engine: the plugin could not process this block; the samples are a
/// copy of the input.
pub const BLOCK_PASSED_THROUGH: u32 = 1 << 2;
/// Bridge → engine: the plugin requested a restart (e.g. its latency changed);
/// the next `Activate` reactivates it and replies with its new latency.
pub const BLOCK_RESTART_REQUESTED: u32 = 1 << 4;
/// Bridge → engine: what the plugin's parameter list needs since the last block
/// (the `PARAMS_*` bits) sit at this shift.
pub const BLOCK_PARAMS_RESCAN_SHIFT: u32 = 8;
pub const BLOCK_PARAMS_RESCAN_MASK: u32 = 0xF << BLOCK_PARAMS_RESCAN_SHIFT;
/// Bridge → engine: the plugin reported new note names since the last block.
pub const BLOCK_NOTE_NAMES_CHANGED: u32 = 1 << 12;

/// Parameter values changed (the UI reads them again).
pub const PARAMS_VALUES: u32 = 1 << 0;
/// Parameter texts changed.
pub const PARAMS_TEXT: u32 = 1 << 1;
/// Names, ranges or flags changed.
pub const PARAMS_INFO: u32 = 1 << 2;
/// The list itself changed: the project must be recompiled.
pub const PARAMS_LIST: u32 = 1 << 3;

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
    /// Audio channels following the sections.
    pub channel_count: u32,
    /// `BLOCK_*` flags.
    pub flags: u32,
    /// [`WireNote`] records between the transport and the parameter events.
    pub note_count: u32,
    /// [`WireParam`] records between the notes and the samples.
    pub param_count: u32,
    pub reserved: u32,
    /// Monotonic block counter; a reply echoes its request's value.
    pub sequence: u64,
    /// Sample clock of the block's first frame.
    pub steady_time: i64,
}

/// Serialised size of a [`BlockHeader`].
pub const BLOCK_HEADER_BYTES: usize = 48;

/// The sections that follow a [`BlockHeader`] and the samples they precede.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockShape {
    pub frames: u32,
    pub channel_count: u32,
    pub flags: u32,
    pub note_count: u32,
    pub param_count: u32,
    pub sequence: u64,
    pub steady_time: i64,
}

impl BlockHeader {
    pub fn new(shape: BlockShape) -> Self {
        BlockHeader {
            magic: BLOCK_MAGIC,
            version: BRIDGE_PROTOCOL_VERSION,
            frames: shape.frames,
            channel_count: shape.channel_count,
            flags: shape.flags,
            note_count: shape.note_count,
            param_count: shape.param_count,
            reserved: 0,
            sequence: shape.sequence,
            steady_time: shape.steady_time,
        }
    }

    /// `None` when `bytes` is too short or the magic/version do not match.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let header = <BlockHeader as WireRecord>::read(bytes.get(..BLOCK_HEADER_BYTES)?);
        (header.magic == BLOCK_MAGIC && header.version == BRIDGE_PROTOCOL_VERSION).then_some(header)
    }

    /// Bytes of the transport section this header announces.
    pub fn transport_bytes(&self) -> usize {
        if self.flags & BLOCK_HAS_TRANSPORT != 0 {
            TRANSPORT_BYTES
        } else {
            0
        }
    }

    /// Bytes of every section between the header and the samples.
    pub fn section_bytes(&self) -> usize {
        self.transport_bytes()
            + self.note_count as usize * NOTE_BYTES
            + self.param_count as usize * PARAM_BYTES
    }
}

// ── Records ─────────────────────────────────────────────────────────────────

/// A plain-old-data record copied to and from the ring as bytes.
///
/// # Safety
/// Implementors are `#[repr(C)]`, contain only integers and floats, and have no
/// padding (their size is the sum of their fields, const-asserted where defined).
pub unsafe trait WireRecord: Copy {
    fn write(&self, out: &mut [u8]) {
        let bytes = unsafe {
            core::slice::from_raw_parts(self as *const Self as *const u8, size_of::<Self>())
        };
        out[..size_of::<Self>()].copy_from_slice(bytes);
    }

    /// `bytes` must hold at least `size_of::<Self>()` bytes.
    fn read(bytes: &[u8]) -> Self {
        assert!(bytes.len() >= size_of::<Self>());
        // SAFETY: length checked; any bit pattern is a valid integer / float.
        unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Self) }
    }
}

unsafe impl WireRecord for BlockHeader {}
const _: () = assert!(size_of::<BlockHeader>() == BLOCK_HEADER_BYTES);

/// `WireTransport::flags`: the transport is rolling.
pub const TRANSPORT_PLAYING: u32 = 1 << 0;

/// Where the host transport is, once per block.  Positions are the engine's:
/// the adapter converts them to the units of its format.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WireTransport {
    /// `TRANSPORT_*`.
    pub flags: u32,
    pub bar_number: i32,
    pub sample_rate: f64,
    pub position_samples: i64,
    pub position_seconds: f64,
    /// Musical position in quarter notes.
    pub position_beats: f64,
    /// Start of the current bar, in quarter notes.
    pub bar_start_beats: f64,
    pub tempo: f64,
    pub time_signature_numerator: u16,
    pub time_signature_denominator: u16,
    pub reserved: u32,
}

pub const TRANSPORT_BYTES: usize = 64;
unsafe impl WireRecord for WireTransport {}
const _: () = assert!(size_of::<WireTransport>() == TRANSPORT_BYTES);

pub const NOTE_ON: u8 = 0;
pub const NOTE_OFF: u8 = 1;
/// Raw MIDI: `data` is the status byte and its two data bytes.
pub const MIDI: u8 = 2;

/// A note or a raw MIDI message, ordered with the others of its block.
/// `data` is `[pitch, velocity, 0]` for a note on (velocity 0..127),
/// `[pitch, 0, 0]` for a note off.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireNote {
    /// Sample offset within the block.
    pub offset: u32,
    pub kind: u8,
    pub channel: u8,
    pub data: [u8; 3],
    pub reserved: [u8; 3],
}

pub const NOTE_BYTES: usize = 12;
unsafe impl WireRecord for WireNote {}
const _: () = assert!(size_of::<WireNote>() == NOTE_BYTES);

impl WireNote {
    pub fn new(offset: u32, kind: u8, data: [u8; 3]) -> Self {
        WireNote {
            offset,
            kind,
            channel: 0,
            data,
            reserved: [0; 3],
        }
    }
}

/// Set the parameter to `value` (the unit the handshake declared).
pub const PARAM_VALUE: u32 = 0;
/// Offset the parameter by `value` without moving its base (a modulation).
pub const PARAM_MODULATION: u32 = 1;
/// The user (or the plugin window) starts holding the parameter.
pub const PARAM_GESTURE_BEGIN: u32 = 2;
pub const PARAM_GESTURE_END: u32 = 3;

/// A parameter event with its sample offset.  `value` is meaningless for a gesture.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WireParam {
    /// Sample offset within the block.
    pub offset: u32,
    pub native_id: u32,
    pub value: f64,
    /// `PARAM_*`.
    pub kind: u32,
    pub reserved: u32,
}

pub const PARAM_BYTES: usize = 24;
unsafe impl WireRecord for WireParam {}
const _: () = assert!(size_of::<WireParam>() == PARAM_BYTES);

impl WireParam {
    pub fn new(kind: u32, offset: u32, native_id: u32, value: f64) -> Self {
        WireParam {
            offset,
            native_id,
            value,
            kind,
            reserved: 0,
        }
    }
}

/// A run of equal records inside a received message, read without alignment.
#[derive(Clone, Copy)]
pub struct Section<'a, T> {
    bytes: &'a [u8],
    marker: PhantomData<T>,
}

impl<'a, T: WireRecord + 'a> Section<'a, T> {
    /// The first `count` records of `bytes` (fewer if the message is short).
    pub fn new(bytes: &'a [u8], count: usize) -> Self {
        let available = bytes.len() / size_of::<T>();
        Section {
            bytes: &bytes[..available.min(count) * size_of::<T>()],
            marker: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.bytes.len() / size_of::<T>()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = T> + 'a {
        self.bytes.chunks_exact(size_of::<T>()).map(T::read)
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

/// Appends `records` to `out` at `offset`; returns the offset after them.
pub fn write_records<T: WireRecord>(records: &[T], out: &mut [u8], offset: usize) -> usize {
    let mut end = offset;
    for record in records {
        record.write(&mut out[end..end + size_of::<T>()]);
        end += size_of::<T>();
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of};

    fn shape() -> BlockShape {
        BlockShape {
            frames: 256,
            channel_count: 2,
            flags: BLOCK_HAS_INPUT,
            note_count: 3,
            param_count: 5,
            sequence: 42,
            steady_time: -7,
        }
    }

    #[test]
    fn the_records_have_a_frozen_layout() {
        assert_eq!(size_of::<BlockHeader>(), 48);
        assert_eq!(align_of::<BlockHeader>(), 8);
        assert_eq!(offset_of!(BlockHeader, magic), 0);
        assert_eq!(offset_of!(BlockHeader, version), 4);
        assert_eq!(offset_of!(BlockHeader, frames), 8);
        assert_eq!(offset_of!(BlockHeader, channel_count), 12);
        assert_eq!(offset_of!(BlockHeader, flags), 16);
        assert_eq!(offset_of!(BlockHeader, note_count), 20);
        assert_eq!(offset_of!(BlockHeader, param_count), 24);
        assert_eq!(offset_of!(BlockHeader, sequence), 32);
        assert_eq!(offset_of!(BlockHeader, steady_time), 40);

        assert_eq!(size_of::<WireTransport>(), 64);
        assert_eq!(offset_of!(WireTransport, flags), 0);
        assert_eq!(offset_of!(WireTransport, bar_number), 4);
        assert_eq!(offset_of!(WireTransport, sample_rate), 8);
        assert_eq!(offset_of!(WireTransport, position_samples), 16);
        assert_eq!(offset_of!(WireTransport, position_seconds), 24);
        assert_eq!(offset_of!(WireTransport, position_beats), 32);
        assert_eq!(offset_of!(WireTransport, bar_start_beats), 40);
        assert_eq!(offset_of!(WireTransport, tempo), 48);
        assert_eq!(offset_of!(WireTransport, time_signature_numerator), 56);
        assert_eq!(offset_of!(WireTransport, time_signature_denominator), 58);

        assert_eq!(size_of::<WireNote>(), 12);
        assert_eq!(align_of::<WireNote>(), 4);
        assert_eq!(offset_of!(WireNote, offset), 0);
        assert_eq!(offset_of!(WireNote, kind), 4);
        assert_eq!(offset_of!(WireNote, channel), 5);
        assert_eq!(offset_of!(WireNote, data), 6);

        assert_eq!(size_of::<WireParam>(), 24);
        assert_eq!(align_of::<WireParam>(), 8);
        assert_eq!(offset_of!(WireParam, offset), 0);
        assert_eq!(offset_of!(WireParam, native_id), 4);
        assert_eq!(offset_of!(WireParam, value), 8);
        assert_eq!(offset_of!(WireParam, kind), 16);
    }

    #[test]
    fn the_flag_and_kind_values_are_frozen() {
        assert_eq!(BRIDGE_PROTOCOL_VERSION, 4);
        assert_eq!(
            [
                BLOCK_HAS_INPUT,
                BLOCK_PLUGIN_EDITED,
                BLOCK_PASSED_THROUGH,
                BLOCK_RESET
            ],
            [1, 2, 4, 8]
        );
        assert_eq!(
            [
                BLOCK_RESTART_REQUESTED,
                BLOCK_HAS_TRANSPORT,
                BLOCK_NOTE_NAMES_CHANGED
            ],
            [16, 32, 4096]
        );
        assert_eq!(BLOCK_PARAMS_RESCAN_MASK, 0xF00);
        assert_eq!(
            [PARAMS_VALUES, PARAMS_TEXT, PARAMS_INFO, PARAMS_LIST],
            [1, 2, 4, 8]
        );
        assert_eq!([NOTE_ON, NOTE_OFF, MIDI], [0, 1, 2]);
        assert_eq!(
            [
                PARAM_VALUE,
                PARAM_MODULATION,
                PARAM_GESTURE_BEGIN,
                PARAM_GESTURE_END
            ],
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn magic_is_readable_ascii() {
        assert_eq!(&BLOCK_MAGIC.to_le_bytes(), b"EsBr");
    }

    #[test]
    fn a_header_survives_the_ring() {
        let header = BlockHeader::new(shape());
        let mut bytes = [0u8; BLOCK_HEADER_BYTES];
        header.write(&mut bytes);
        assert_eq!(BlockHeader::from_bytes(&bytes), Some(header));
    }

    #[test]
    fn from_bytes_rejects_short_foreign_or_old_input() {
        assert_eq!(BlockHeader::from_bytes(&[0u8; 8]), None);
        let mut bytes = [0u8; BLOCK_HEADER_BYTES];
        BlockHeader::new(shape()).write(&mut bytes);
        let mut foreign = bytes;
        foreign[0] ^= 0xFF;
        assert_eq!(BlockHeader::from_bytes(&foreign), None);
        let mut old = bytes;
        old[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(BlockHeader::from_bytes(&old), None);
    }

    #[test]
    fn the_section_sizes_follow_the_header() {
        let header = BlockHeader::new(BlockShape {
            flags: BLOCK_HAS_TRANSPORT,
            ..shape()
        });
        assert_eq!(header.section_bytes(), 64 + 3 * 12 + 5 * 24);
        assert_eq!(BlockHeader::new(shape()).section_bytes(), 3 * 12 + 5 * 24);
    }

    #[test]
    fn records_roundtrip_through_unaligned_bytes() {
        let notes = [
            WireNote::new(0, NOTE_ON, [60, 100, 0]),
            WireNote::new(31, MIDI, [0xE0, 1, 2]),
        ];
        let params = [
            WireParam::new(PARAM_VALUE, 5, 7, 0.25),
            WireParam::new(PARAM_GESTURE_END, 9, 7, 0.0),
        ];
        let mut bytes = vec![0u8; 1 + 2 * NOTE_BYTES + 2 * PARAM_BYTES];
        let end = write_records(&notes, &mut bytes, 1);
        assert_eq!(end, 1 + 2 * NOTE_BYTES);
        assert_eq!(write_records(&params, &mut bytes, end), bytes.len());

        let read_notes = Section::<WireNote>::new(&bytes[1..], 2);
        assert_eq!(read_notes.iter().collect::<Vec<_>>(), notes);
        let read_params = Section::<WireParam>::new(&bytes[end..], 2);
        assert_eq!(read_params.iter().collect::<Vec<_>>(), params);
    }

    #[test]
    fn a_short_section_yields_only_whole_records() {
        let bytes = [0u8; 2 * NOTE_BYTES + 5];
        assert_eq!(Section::<WireNote>::new(&bytes, 9).len(), 2);
        assert_eq!(Section::<WireNote>::new(&bytes, 1).len(), 1);
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
