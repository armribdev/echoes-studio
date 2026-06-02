//! Cross-process wire protocol for the plugin bridge (ADR-015, Phase 6c-i).
//!
//! These `#[repr(C)]` POD types pin the ABI of the messages the
//! [`ShmemRing`](super::shmem::ShmemRing) carries between the engine and the
//! `plugin_host_bridge` process.  They are **declared now** so the layout is
//! frozen before the rings are wired (6d); no behaviour is attached yet.
//!
//! ABI rules:
//!   * `#[repr(C)]`, fixed-width integer / `f32` fields only — no `usize`, no
//!     pointers, no enums-with-payload.  Both processes are the same
//!     architecture (the bridge is spawned by the engine), but explicit widths
//!     keep the layout self-documenting and forward-portable.
//!   * Any change to a struct below is a protocol break → bump
//!     [`BRIDGE_PROTOCOL_VERSION`] and gate on it at handshake.

/// Wire-format version.  The engine writes it into [`BlockHeader::version`];
/// the bridge rejects a mismatch at handshake (6d).
pub const BRIDGE_PROTOCOL_VERSION: u32 = 1;

/// Magic stamped at the start of every audio block so a desynced reader can
/// detect a torn / misaligned region instead of processing garbage.
/// ASCII `"EsBr"` (Echoes Bridge), little-endian.
pub const BLOCK_MAGIC: u32 = u32::from_le_bytes(*b"EsBr");

/// Logical role of a ring within one bridged device.  Stored as a `u32` tag in
/// the handshake; each role gets its own [`ShmemRing`](super::shmem::ShmemRing).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRole {
    /// Engine → bridge: input audio blocks to process.
    AudioIn = 0,
    /// Bridge → engine: processed output audio blocks.
    AudioOut = 1,
    /// Engine → bridge: parameter changes (drained before `process`).
    Params = 2,
    /// Engine → bridge: MIDI / note events for this block.
    Events = 3,
}

/// Header prefixed to one audio block transferred over an `AudioIn` / `AudioOut`
/// ring.  Followed by `channel_count * block_size` interleaved-by-channel `f32`
/// samples in the same message payload.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockHeader {
    /// [`BLOCK_MAGIC`] — torn-region guard.
    pub magic: u32,
    /// [`BRIDGE_PROTOCOL_VERSION`] at the time the block was produced.
    pub version: u32,
    /// Frames in this block (e.g. 256, 512).
    pub block_size: u32,
    /// Number of audio channels following the header.
    pub channel_count: u32,
    /// Engine sample rate, Hz.
    pub sample_rate: u32,
    /// Reserved for alignment / future flags; must be 0.
    pub _reserved: u32,
    /// Monotonic block counter, for drop / reorder detection across the bridge.
    pub sequence: u64,
}

/// One parameter change carried over a `Params` ring.  ABI-stable mirror of the
/// engine's in-process `ParamChange` (`plugin_system::param_queue`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamMessage {
    /// Format-native parameter id (VST3 `ParamID`, CLAP `clap_id`).
    pub native_id: u32,
    /// Intra-block frame offset (0 at block-rate, BP-7).
    pub sample_offset: u32,
    /// Normalised value in `[0, 1]`.
    pub normalised: f32,
    /// Reserved for alignment / future flags; must be 0.
    pub _reserved: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, size_of};

    // ABI is frozen: these sizes/alignments are part of the protocol.  A change
    // here is intentional only alongside a `BRIDGE_PROTOCOL_VERSION` bump.
    #[test]
    fn block_header_abi_is_stable() {
        assert_eq!(size_of::<BlockHeader>(), 32);
        assert_eq!(align_of::<BlockHeader>(), 8); // u64 sequence drives alignment
    }

    #[test]
    fn param_message_abi_is_stable() {
        assert_eq!(size_of::<ParamMessage>(), 16);
        assert_eq!(align_of::<ParamMessage>(), 4);
    }

    #[test]
    fn magic_is_readable_ascii() {
        assert_eq!(&BLOCK_MAGIC.to_le_bytes(), b"EsBr");
    }
}
