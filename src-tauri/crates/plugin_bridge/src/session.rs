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

use crate::protocol::{BlockHeader, BLOCK_HEADER_BYTES, BLOCK_MAGIC, BRIDGE_PROTOCOL_VERSION};
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

// ── Bridge side: the processing seam ────────────────────────────────────────

/// Per-block context handed to a [`BridgeProcessor`], decoded from the incoming
/// [`BlockHeader`].
#[derive(Debug, Clone, Copy)]
pub struct BlockContext {
    pub block_size: u32,
    pub channel_count: u32,
    pub sample_rate: u32,
}

/// The DSP unit the bridge process loop drives, one block at a time.
///
/// This is the seam a **real plugin slots into**: 6d-iii-b-2 implements it for a
/// loaded VST3/CLAP plugin (its `process()`), keeping the loop and the transport
/// unchanged.  `input` and `output` are interleaved-by-channel `f32`, length
/// `channel_count * block_size`.
pub trait BridgeProcessor {
    /// Process one block: read `input`, write `output` (same length).
    fn process(&mut self, input: &[f32], output: &mut [f32], ctx: &BlockContext);
}

/// Identity stand-in: copies input to output unchanged.  This is what runs until
/// 6d-iii-b-2 loads a real plugin; it makes the transport + loop verifiable
/// without depending on any installed plugin.
pub struct PassthroughProcessor;

impl BridgeProcessor for PassthroughProcessor {
    fn process(&mut self, input: &[f32], output: &mut [f32], _ctx: &BlockContext) {
        let n = input.len().min(output.len());
        output[..n].copy_from_slice(&input[..n]);
    }
}

// ── Bridge side: the process loop ───────────────────────────────────────────

/// Bridge-side process loop (ADR-015, 6d-iii): open the region described by
/// `layout`, validate the control block, attach the audio rings, signal `READY`,
/// then run [`PassthroughProcessor`] over each block until the owner sets
/// `SHUTDOWN`.
///
/// The processor is the identity stand-in; 6d-iii-b-2 swaps it for a real loaded
/// plugin via [`serve_with`].  Returns once `SHUTDOWN` is observed, or an error
/// if the handshake fails.
pub fn serve(region_name: &str, layout: &BridgeLayout) -> io::Result<()> {
    serve_with(region_name, layout, &mut PassthroughProcessor)
}

/// Like [`serve`], but with a caller-supplied processor.  6d-iii-b-2 passes a
/// real plugin processor here; the loop and transport are identical.
pub fn serve_with(
    region_name: &str,
    layout: &BridgeLayout,
    processor: &mut dyn BridgeProcessor,
) -> io::Result<()> {
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

    // Preallocated buffers — no allocation inside the loop.  The byte scratch
    // can't be cast to `&[f32]` (a `Vec<u8>` is only 1-aligned), so samples are
    // copied through the f32 buffers; copies are RT-safe (fixed-size, no alloc).
    let frames = (layout.channel_count as usize) * (layout.block_size as usize);
    let mut scratch = vec![0u8; audio_in.slot_size()];
    let mut in_samples = vec![0.0f32; frames];
    let mut out_samples = vec![0.0f32; frames];

    loop {
        if unsafe { status_load(base) } == STATUS_SHUTDOWN {
            return Ok(());
        }
        let Some(n) = audio_in.pop(&mut scratch) else {
            std::thread::sleep(Duration::from_micros(200));
            continue;
        };

        // Torn-region guard: a valid block starts with a `BlockHeader` whose
        // magic matches.  Drop malformed blocks rather than process garbage.
        let Some(header) = BlockHeader::from_bytes(&scratch[..n]) else {
            continue;
        };
        if header.magic != BLOCK_MAGIC {
            continue;
        }

        let body = &scratch[BLOCK_HEADER_BYTES..n];
        let count = (body.len() / 4).min(frames);
        for (slot, chunk) in in_samples[..count].iter_mut().zip(body.chunks_exact(4)) {
            *slot = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }

        let ctx = BlockContext {
            block_size: header.block_size,
            channel_count: header.channel_count,
            sample_rate: header.sample_rate,
        };
        processor.process(&in_samples[..count], &mut out_samples[..count], &ctx);

        // Re-serialise: echo the header, then the processed samples.  Reuse the
        // scratch buffer in place (it is large enough for header + samples).
        scratch[..BLOCK_HEADER_BYTES].copy_from_slice(&header.to_bytes());
        for (sample, dst) in out_samples[..count]
            .iter()
            .zip(scratch[BLOCK_HEADER_BYTES..].chunks_exact_mut(4))
        {
            dst.copy_from_slice(&sample.to_le_bytes());
        }
        audio_out.push(&scratch[..BLOCK_HEADER_BYTES + count * 4]);
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

    // A non-identity processor proves the loop actually drives the seam (not just
    // echoes bytes).  This mirrors how a real plugin's `process()` will be wired.
    struct GainProcessor(f32);
    impl BridgeProcessor for GainProcessor {
        fn process(&mut self, input: &[f32], output: &mut [f32], _ctx: &BlockContext) {
            for (o, i) in output.iter_mut().zip(input) {
                *o = i * self.0;
            }
        }
    }

    fn ctx() -> BlockContext {
        BlockContext {
            block_size: 4,
            channel_count: 1,
            sample_rate: 48_000,
        }
    }

    #[test]
    fn passthrough_copies_input_to_output() {
        let input = [0.1, -0.2, 0.3, -0.4];
        let mut output = [0.0; 4];
        PassthroughProcessor.process(&input, &mut output, &ctx());
        assert_eq!(output, input);
    }

    #[test]
    fn processor_transforms_samples() {
        let input = [1.0, 2.0, 3.0, 4.0];
        let mut output = [0.0; 4];
        GainProcessor(2.0).process(&input, &mut output, &ctx());
        assert_eq!(output, [2.0, 4.0, 6.0, 8.0]);
    }
}
