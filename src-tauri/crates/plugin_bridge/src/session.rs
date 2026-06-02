//! Spawn + control handshake for the out-of-process bridge (ADR-015, Phase 6d-ii).
//!
//! This is the **off-real-time** lifecycle layer that sits between the OS mapping
//! ([`SharedRegion`](crate::region::SharedRegion)) and the future real-time audio
//! round-trip (6d-iv).  It answers a single question before any audio crosses the
//! process boundary: *is the child process alive, attached to the right region,
//! and speaking the right ABI?*
//!
//! ## Control block
//!
//! A small fixed header lives at the very start of the shared region, ahead of
//! any audio/param/event rings (those are carved in later, from
//! [`CONTROL_BYTES`] onward):
//!
//! ```text
//!   magic:u32 | protocol:u32 | region_bytes:u32 | status:AtomicU32
//! ```
//!
//! `status` is the only mutable field; everything else is stamped once by the
//! owner before spawn.  The owner and the child hand the `status` atomic back and
//! forth through the states below.
//!
//! ## Handshake
//!
//! ```text
//!   engine (owner)                         plugin_host_bridge (peer)
//!   ─ create region                        ─ open region (--region-name)
//!   ─ write magic/proto/bytes, INIT        ─ validate magic + protocol + bytes
//!   ─ spawn child ───────────────────────► ─ on mismatch → ERROR, exit
//!   ─ poll status (bounded) ◄───────────── ─ on ok        → READY
//!   ─ READY ⇒ session live                 ─ poll status for SHUTDOWN
//!   ─ Drop ⇒ SHUTDOWN ──────────────────►  ─ SHUTDOWN ⇒ return / exit 0
//!   ─ reap child (grace, then kill)
//! ```
//!
//! Everything here runs on the command thread (engine side) or in the child
//! process — never the audio thread.  `std::thread::sleep` / `Instant` are fine.

use std::io;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::protocol::{BlockHeader, BLOCK_HEADER_BYTES, BRIDGE_PROTOCOL_VERSION};
use crate::region::SharedRegion;
use crate::shmem::{RingLayout, ShmemRing};

/// Magic stamped in the control block — ASCII `"EsCt"` (Echoes Control), LE.
/// Distinct from the per-block [`BLOCK_MAGIC`](crate::protocol::BLOCK_MAGIC).
pub const CONTROL_MAGIC: u32 = u32::from_le_bytes(*b"EsCt");

/// Owner has stamped the header; child has not yet acknowledged.
pub const STATUS_INIT: u32 = 0;
/// Child opened the region, validated the ABI, and is ready to serve.
pub const STATUS_READY: u32 = 1;
/// Owner is asking the child to wind down and exit.
pub const STATUS_SHUTDOWN: u32 = 2;
/// Child rejected the handshake (magic / protocol / size mismatch).
pub const STATUS_ERROR: u32 = 3;

const OFF_MAGIC: usize = 0;
const OFF_PROTOCOL: usize = 4;
const OFF_REGION_BYTES: usize = 8;
const OFF_STATUS: usize = 12;

/// Bytes reserved for the control block at the head of the region.  Audio / param
/// / event rings (6d-iii) start at this offset.
pub const CONTROL_BYTES: usize = 16;

// ── Region layout ───────────────────────────────────────────────────────────

/// Describes how a device's shared region is carved up, so the engine (owner)
/// and the bridge (peer) agree on ring offsets and sizes (ADR-015, 6d-iii).
///
/// Layout: `[control block][audio-in ring][audio-out ring]`.  Param / event
/// rings are a later micro-step (same pattern); they would extend this struct.
#[derive(Debug, Clone, Copy)]
pub struct BridgeLayout {
    /// Frames per processing block.
    pub block_size: u32,
    /// Audio channel count (per block, interleaved-by-channel).
    pub channel_count: u32,
    /// Slot count for each audio ring (depth of in-flight blocks).
    pub audio_capacity: usize,
}

impl BridgeLayout {
    pub fn new(block_size: u32, channel_count: u32, audio_capacity: usize) -> Self {
        BridgeLayout {
            block_size,
            channel_count,
            audio_capacity,
        }
    }

    /// Ring shape for one audio ring: each slot holds a [`BlockHeader`] followed
    /// by `channel_count * block_size` `f32` samples.
    pub fn audio_ring_layout(&self) -> RingLayout {
        let samples = self.channel_count as usize * self.block_size as usize;
        RingLayout::new(self.audio_capacity, BLOCK_HEADER_BYTES + samples * 4)
    }

    fn audio_ring_bytes(&self) -> usize {
        self.audio_ring_layout().bytes_required()
    }

    /// Byte offset of the audio-in ring (engine → bridge) within the region.
    pub fn audio_in_offset(&self) -> usize {
        CONTROL_BYTES
    }

    /// Byte offset of the audio-out ring (bridge → engine) within the region.
    pub fn audio_out_offset(&self) -> usize {
        CONTROL_BYTES + self.audio_ring_bytes()
    }

    /// Total bytes the shared region must provide.
    pub fn region_bytes(&self) -> usize {
        CONTROL_BYTES + 2 * self.audio_ring_bytes()
    }
}

// ── Control-block accessors (operate on a raw region base) ──────────────────────

/// # Safety
/// `base` must point to at least [`CONTROL_BYTES`] writable, 4-byte-aligned bytes.
unsafe fn write_control_init(base: *mut u8, region_bytes: u32) {
    core::ptr::write_unaligned(base.add(OFF_MAGIC) as *mut u32, CONTROL_MAGIC);
    core::ptr::write_unaligned(base.add(OFF_PROTOCOL) as *mut u32, BRIDGE_PROTOCOL_VERSION);
    core::ptr::write_unaligned(base.add(OFF_REGION_BYTES) as *mut u32, region_bytes);
    (*(base.add(OFF_STATUS) as *const AtomicU32)).store(STATUS_INIT, Ordering::Release);
}

/// Returns `(magic, protocol, region_bytes)` — the immutable header fields.
///
/// # Safety
/// Same requirements as [`write_control_init`]; the header must be initialised.
unsafe fn read_control(base: *mut u8) -> (u32, u32, u32) {
    (
        core::ptr::read_unaligned(base.add(OFF_MAGIC) as *const u32),
        core::ptr::read_unaligned(base.add(OFF_PROTOCOL) as *const u32),
        core::ptr::read_unaligned(base.add(OFF_REGION_BYTES) as *const u32),
    )
}

/// # Safety
/// `base` must satisfy the requirements of [`write_control_init`].
unsafe fn status_load(base: *mut u8) -> u32 {
    (*(base.add(OFF_STATUS) as *const AtomicU32)).load(Ordering::Acquire)
}

/// # Safety
/// `base` must satisfy the requirements of [`write_control_init`].
unsafe fn status_store(base: *mut u8, value: u32) {
    (*(base.add(OFF_STATUS) as *const AtomicU32)).store(value, Ordering::Release);
}

// ── Engine side ─────────────────────────────────────────────────────────────

/// A live bridge child process, its shared region, and the engine-side ends of
/// the audio rings (owner side).
///
/// Construction performs the full spawn + handshake; if it returns `Ok`, the
/// child is attached and `READY`.  Dropping the session asks the child to shut
/// down and reaps it.
pub struct BridgeSession {
    region: SharedRegion,
    child: Child,
    layout: BridgeLayout,
    /// Engine → bridge audio ring (owner is the **producer**).
    audio_in: ShmemRing,
    /// Bridge → engine audio ring (owner is the **consumer**).
    audio_out: ShmemRing,
    /// Region name (kept for diagnostics).
    name: String,
}

impl BridgeSession {
    /// Create the region sized for `layout`, initialise the control block and the
    /// two audio rings, spawn `bridge_exe`, and wait until the child reports
    /// `READY` (or `ready_timeout` elapses / the child dies / it reports `ERROR`).
    pub fn spawn(
        bridge_exe: &Path,
        region_name: &str,
        layout: &BridgeLayout,
        ready_timeout: Duration,
    ) -> io::Result<Self> {
        let region_bytes = layout.region_bytes();
        let region = SharedRegion::create(region_name, region_bytes)?;
        let base = region.as_mut_ptr();

        // Owner initialises every header (control + both ring headers) before the
        // child can attach.  SAFETY: region is page-aligned and large enough.
        let (audio_in, audio_out) = unsafe {
            write_control_init(base, region_bytes as u32);
            let ring = layout.audio_ring_layout();
            let bytes = ring.bytes_required();
            let audio_in = ShmemRing::init(base.add(layout.audio_in_offset()), bytes, ring);
            let audio_out = ShmemRing::init(base.add(layout.audio_out_offset()), bytes, ring);
            (audio_in, audio_out)
        };

        let mut child = Command::new(bridge_exe)
            .arg("--region-name")
            .arg(region_name)
            .arg("--block-size")
            .arg(layout.block_size.to_string())
            .arg("--channels")
            .arg(layout.channel_count.to_string())
            .arg("--audio-capacity")
            .arg(layout.audio_capacity.to_string())
            .spawn()?;

        let deadline = Instant::now() + ready_timeout;
        loop {
            // SAFETY: region stays mapped for the duration of this call.
            match unsafe { status_load(base) } {
                STATUS_READY => break,
                STATUS_ERROR => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "bridge rejected control handshake",
                    ));
                }
                _ => {}
            }
            if let Some(exit) = child.try_wait()? {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    format!("bridge exited before handshake ({exit})"),
                ));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "bridge did not become ready within timeout",
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        Ok(BridgeSession {
            region,
            child,
            layout: *layout,
            audio_in,
            audio_out,
            name: region_name.to_string(),
        })
    }

    /// Enqueue one input block (`header` + interleaved `samples` bytes) for the
    /// bridge to process.  Returns `false` if the input ring is full.
    ///
    /// **Off-RT helper** (allocates a small scratch buffer): the 6d-iv audio-thread
    /// proxy will instead format the block into a preallocated scratch and use a
    /// non-allocating two-part push.
    pub fn try_send_input(&self, header: &BlockHeader, samples: &[u8]) -> bool {
        let mut buf = Vec::with_capacity(BLOCK_HEADER_BYTES + samples.len());
        buf.extend_from_slice(&header.to_bytes());
        buf.extend_from_slice(samples);
        self.audio_in.push(&buf)
    }

    /// Dequeue one processed output block into `out` (header + samples bytes),
    /// returning its length, or `None` if no block is ready.
    pub fn try_recv_output(&self, out: &mut [u8]) -> Option<usize> {
        self.audio_out.pop(out)
    }

    /// Byte size of one audio ring slot (header + samples) — the size `out` for
    /// [`try_recv_output`](Self::try_recv_output) should provide.
    pub fn audio_slot_size(&self) -> usize {
        self.layout.audio_ring_layout().slot_size
    }

    /// The region's layout.
    pub fn layout(&self) -> &BridgeLayout {
        &self.layout
    }

    /// The region name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for BridgeSession {
    fn drop(&mut self) {
        // Ask the child to wind down, then reap it: grace period, then kill.
        // SAFETY: region is still mapped until after this Drop completes.
        unsafe { status_store(self.region.as_mut_ptr(), STATUS_SHUTDOWN) };

        let deadline = Instant::now() + Duration::from_millis(1000);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
    }
}

// ── Bridge side ─────────────────────────────────────────────────────────────

/// Bridge-side process loop (ADR-015, 6d-iii): open the region described by
/// `layout`, validate the control block, attach the audio rings, signal `READY`,
/// then process input blocks into output blocks until the owner sets `SHUTDOWN`.
///
/// **The "processor" here is an identity pass-through stand-in**: it copies each
/// input block to the output ring unchanged.  6d-iii-b replaces the copy with the
/// real loaded VST3/CLAP plugin's `process()`.  No plugin is loaded yet.
///
/// Returns once `SHUTDOWN` is observed, or an error if the handshake fails.
pub fn serve(region_name: &str, layout: &BridgeLayout) -> io::Result<()> {
    let region_bytes = layout.region_bytes();
    let region = SharedRegion::open(region_name, region_bytes)?;
    let base = region.as_mut_ptr();

    // SAFETY: the owner created and initialised the region before spawning us.
    let (magic, protocol, declared_bytes) = unsafe { read_control(base) };
    if magic != CONTROL_MAGIC
        || protocol != BRIDGE_PROTOCOL_VERSION
        || declared_bytes as usize != region_bytes
    {
        unsafe { status_store(base, STATUS_ERROR) };
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "control mismatch: magic={magic:#x} proto={protocol} bytes={declared_bytes} \
                 (expected magic={CONTROL_MAGIC:#x} proto={BRIDGE_PROTOCOL_VERSION} \
                 bytes={region_bytes})"
            ),
        ));
    }

    // Attach the audio rings the owner initialised.  Owner is the producer of
    // audio-in (we consume) and the consumer of audio-out (we produce).
    // SAFETY: offsets/sizes come from the shared layout; headers are valid.
    let ring = layout.audio_ring_layout();
    let bytes = ring.bytes_required();
    let (audio_in, audio_out) = unsafe {
        (
            ShmemRing::attach(base.add(layout.audio_in_offset()), bytes),
            ShmemRing::attach(base.add(layout.audio_out_offset()), bytes),
        )
    };

    unsafe { status_store(base, STATUS_READY) };

    // Preallocated scratch — no allocation inside the loop.
    let mut scratch = vec![0u8; audio_in.slot_size()];

    loop {
        if unsafe { status_load(base) } == STATUS_SHUTDOWN {
            return Ok(());
        }
        match audio_in.pop(&mut scratch) {
            Some(n) => {
                // Torn-region guard: a valid block starts with a `BlockHeader`
                // whose magic matches.  Drop malformed blocks rather than echo
                // garbage.  (The real plugin's process() replaces this copy.)
                if BlockHeader::from_bytes(&scratch[..n])
                    .is_some_and(|h| h.magic == crate::protocol::BLOCK_MAGIC)
                {
                    audio_out.push(&scratch[..n]);
                }
            }
            None => std::thread::sleep(Duration::from_micros(200)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shmem::OwnedRegion;

    // The control-block layout is region-agnostic, so it can be exercised on a
    // plain heap region on every platform — no OS mapping or child process.
    #[test]
    fn control_block_roundtrips_on_heap_region() {
        let region = OwnedRegion::new(CONTROL_BYTES);
        let base = region.as_mut_ptr();

        unsafe { write_control_init(base, CONTROL_BYTES as u32) };
        let (magic, proto, bytes) = unsafe { read_control(base) };
        assert_eq!(magic, CONTROL_MAGIC);
        assert_eq!(proto, BRIDGE_PROTOCOL_VERSION);
        assert_eq!(bytes, CONTROL_BYTES as u32);
        assert_eq!(unsafe { status_load(base) }, STATUS_INIT);
    }

    #[test]
    fn status_transitions_are_observable() {
        let region = OwnedRegion::new(CONTROL_BYTES);
        let base = region.as_mut_ptr();
        unsafe { write_control_init(base, CONTROL_BYTES as u32) };

        unsafe { status_store(base, STATUS_READY) };
        assert_eq!(unsafe { status_load(base) }, STATUS_READY);
        unsafe { status_store(base, STATUS_SHUTDOWN) };
        assert_eq!(unsafe { status_load(base) }, STATUS_SHUTDOWN);
    }

    #[test]
    fn control_magic_is_distinct_from_block_magic() {
        assert_ne!(CONTROL_MAGIC, crate::protocol::BLOCK_MAGIC);
        assert_eq!(&CONTROL_MAGIC.to_le_bytes(), b"EsCt");
    }
}
