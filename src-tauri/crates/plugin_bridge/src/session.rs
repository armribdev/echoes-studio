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

use crate::protocol::BRIDGE_PROTOCOL_VERSION;
use crate::region::SharedRegion;

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

/// A live bridge child process and its shared region (owner side).
///
/// Construction performs the full spawn + handshake; if it returns `Ok`, the
/// child is attached and `READY`.  Dropping the session asks the child to shut
/// down and reaps it.
pub struct BridgeSession {
    region: SharedRegion,
    child: Child,
    /// Region name (kept for diagnostics).
    name: String,
}

impl BridgeSession {
    /// Create the region, spawn `bridge_exe`, and wait until the child reports
    /// `READY` (or `ready_timeout` elapses / the child dies / it reports `ERROR`).
    ///
    /// `region_bytes` must be at least [`CONTROL_BYTES`]; later steps size it to
    /// fit the audio/param/event rings as well.
    pub fn spawn(
        bridge_exe: &Path,
        region_name: &str,
        region_bytes: usize,
        ready_timeout: Duration,
    ) -> io::Result<Self> {
        assert!(
            region_bytes >= CONTROL_BYTES,
            "BridgeSession::spawn: region smaller than control block"
        );

        let region = SharedRegion::create(region_name, region_bytes)?;
        // SAFETY: the region is at least CONTROL_BYTES and page-aligned.
        unsafe { write_control_init(region.as_mut_ptr(), region_bytes as u32) };

        let mut child = Command::new(bridge_exe)
            .arg("--region-name")
            .arg(region_name)
            .arg("--region-bytes")
            .arg(region_bytes.to_string())
            .spawn()?;

        let deadline = Instant::now() + ready_timeout;
        loop {
            // SAFETY: region stays mapped for the duration of this call.
            match unsafe { status_load(region.as_mut_ptr()) } {
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
            // Child died before becoming ready?
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
            name: region_name.to_string(),
        })
    }

    /// The shared region (for ring setup in later steps).
    pub fn region(&self) -> &SharedRegion {
        &self.region
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

/// Bridge-side handshake: open the named region, validate the control block,
/// signal `READY`, then block until the owner sets `SHUTDOWN`.
///
/// In 6d-ii this performs **only** the handshake — no plugin is loaded and no
/// audio is processed (those are 6d-iii / 6d-iv).  Returns once `SHUTDOWN` is
/// observed, or an error if the handshake fails.
pub fn serve(region_name: &str, region_bytes: usize) -> io::Result<()> {
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

    // 6d-iii will attach the audio/param/event rings here before signalling ready.
    unsafe { status_store(base, STATUS_READY) };

    loop {
        if unsafe { status_load(base) } == STATUS_SHUTDOWN {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(2));
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
