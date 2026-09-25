//! Named shared-memory region for the plugin bridge (ADR-015, Phase 6d-i).
//!
//! [`ShmemRing`](crate::shmem::ShmemRing) is region-agnostic: it operates on a
//! raw `*mut u8` + length.  In tests that region is a heap
//! [`OwnedRegion`](crate::shmem::OwnedRegion); in production it must be memory
//! **mapped into two processes** so the engine and the `plugin_host_bridge`
//! child see each other's writes.  `SharedRegion` is that mapping.
//!
//! ## Lifecycle
//!
//! - The **owner** (engine, in 6d-ii) calls [`SharedRegion::create`] with a
//!   unique name and the byte length the rings need
//!   ([`RingLayout::bytes_required`](crate::shmem::RingLayout::bytes_required)
//!   summed over the device's channels).
//! - The **peer** (bridge child) calls [`SharedRegion::open`] with the same name
//!   and length once the owner has passed it on the command line.
//! - Each side then builds its `ShmemRing` views (`init` on the owner, `attach`
//!   on the peer) over `as_mut_ptr()`.
//!
//! ## Real-time note
//!
//! Mapping is a one-time **setup** operation (off the audio thread).  Once
//! mapped, the audio thread only ever touches the region through the lock-free
//! `ShmemRing` — never through this type.  `SharedRegion` performs no allocation
//! or syscall after construction except `Drop`.
//!
//! ## Platform support
//!
//! Windows is implemented first (the engine's primary target; see migration plan
//! §6.8).  Other targets get a stub that returns [`io::ErrorKind::Unsupported`]
//! so the crate still compiles cross-platform; their real mapping (POSIX
//! `shm_open` + `mmap`) lands alongside cross-platform sandbox work.

use std::io;

/// A named shared-memory region mapped into the current process.
///
/// The mapped base address is allocation-granularity aligned by the OS, so it
/// always satisfies `ShmemRing`'s 4-byte alignment requirement.  Dropping the
/// region unmaps the view and closes the mapping handle.
pub struct SharedRegion {
    inner: imp::Mapping,
    len: usize,
}

// SAFETY: `SharedRegion` owns its mapping exclusively and performs no interior
// mutation; `&SharedRegion` only hands out the base pointer and length.  What is
// done through that pointer is governed by the ring and control-block protocols.
unsafe impl Send for SharedRegion {}
unsafe impl Sync for SharedRegion {}

impl SharedRegion {
    /// Create and zero-initialise a new named region of `len` bytes (owner side).
    ///
    /// `name` lives in the session object namespace and must be unique for the
    /// lifetime of the mapping; reusing a live name opens the existing mapping
    /// instead of creating a fresh one.
    pub fn create(name: &str, len: usize) -> io::Result<Self> {
        assert!(len > 0, "SharedRegion::create: zero-length region");
        Ok(SharedRegion {
            inner: imp::Mapping::create(name, len)?,
            len,
        })
    }

    /// Open an existing named region of `len` bytes (peer side).
    pub fn open(name: &str, len: usize) -> io::Result<Self> {
        assert!(len > 0, "SharedRegion::open: zero-length region");
        Ok(SharedRegion {
            inner: imp::Mapping::open(name, len)?,
            len,
        })
    }

    /// Base pointer of the mapped view (stable for the region's lifetime).
    #[inline]
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.inner.base()
    }

    /// Mapped length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the region is empty (always false — `create`/`open` reject 0).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// A named auto-reset event: the engine signals it after queuing a block so the
/// bridge's audio thread wakes at once instead of polling.
///
/// `signal` is a single non-blocking kernel call (no lock is held by another
/// thread that could make it wait), which is why it is allowed on the audio
/// thread (ADR-015 amendment 2026-09-25).
pub struct SharedEvent {
    inner: imp::Event,
}

// SAFETY: the OS event handle may be used from any thread.
unsafe impl Send for SharedEvent {}
unsafe impl Sync for SharedEvent {}

impl SharedEvent {
    /// Create a new named event (owner side).
    pub fn create(name: &str) -> io::Result<Self> {
        Ok(SharedEvent {
            inner: imp::Event::create(name)?,
        })
    }

    /// Open an event created by the owner (peer side).
    pub fn open(name: &str) -> io::Result<Self> {
        Ok(SharedEvent {
            inner: imp::Event::open(name)?,
        })
    }

    /// Wake one waiter.  Non-blocking.
    #[inline]
    pub fn signal(&self) {
        self.inner.signal();
    }

    /// Block until signalled or `timeout_ms` elapses; `true` when signalled.
    pub fn wait(&self, timeout_ms: u32) -> bool {
        self.inner.wait(timeout_ms)
    }
}

// ── Windows implementation ──────────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use std::io;

    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
        MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
        SYNCHRONIZATION_SYNCHRONIZE,
    };

    /// Owns a file-mapping handle and one mapped view over it.
    pub struct Mapping {
        handle: HANDLE,
        base: *mut u8,
    }

    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn size_parts(len: usize) -> (u32, u32) {
        let len = len as u64;
        ((len >> 32) as u32, (len & 0xFFFF_FFFF) as u32)
    }

    /// Map a view over `handle` or fail, closing `handle` on error.
    unsafe fn map_view(handle: HANDLE, len: usize) -> io::Result<Mapping> {
        let addr: MEMORY_MAPPED_VIEW_ADDRESS =
            MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, len);
        if addr.Value.is_null() {
            let err = io::Error::last_os_error();
            CloseHandle(handle);
            return Err(err);
        }
        Ok(Mapping {
            handle,
            base: addr.Value as *mut u8,
        })
    }

    impl Mapping {
        pub fn create(name: &str, len: usize) -> io::Result<Self> {
            let name_w = wide(name);
            let (hi, lo) = size_parts(len);
            // INVALID_HANDLE_VALUE → backed by the system paging file (no real
            // file).  Null security attrs → default ACL, not inheritable by name.
            let handle = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null(),
                    PAGE_READWRITE,
                    hi,
                    lo,
                    name_w.as_ptr(),
                )
            };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            unsafe { map_view(handle, len) }
        }

        pub fn open(name: &str, len: usize) -> io::Result<Self> {
            let name_w = wide(name);
            let handle = unsafe {
                OpenFileMappingW(
                    FILE_MAP_ALL_ACCESS,
                    0, /* bInheritHandle */
                    name_w.as_ptr(),
                )
            };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            unsafe { map_view(handle, len) }
        }

        #[inline]
        pub fn base(&self) -> *mut u8 {
            self.base
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.base as *mut _,
                });
                CloseHandle(self.handle);
            }
        }
    }

    pub struct Event {
        handle: HANDLE,
    }

    impl Event {
        pub fn create(name: &str) -> io::Result<Self> {
            let name_w = wide(name);
            // Auto-reset, initially clear.
            let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, name_w.as_ptr()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Event { handle })
        }

        pub fn open(name: &str) -> io::Result<Self> {
            let name_w = wide(name);
            let handle = unsafe {
                OpenEventW(
                    EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
                    0,
                    name_w.as_ptr(),
                )
            };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Event { handle })
        }

        #[inline]
        pub fn signal(&self) {
            unsafe { SetEvent(self.handle) };
        }

        pub fn wait(&self, timeout_ms: u32) -> bool {
            unsafe { WaitForSingleObject(self.handle, timeout_ms) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for Event {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

// ── Non-Windows stub ────────────────────────────────────────────────────────

#[cfg(not(windows))]
mod imp {
    use std::io;

    pub struct Mapping {
        _never: (),
    }

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "SharedRegion is only implemented on Windows (ADR-015 6d-i); \
             POSIX shm_open/mmap support lands with cross-platform sandbox work",
        )
    }

    impl Mapping {
        pub fn create(_name: &str, _len: usize) -> io::Result<Self> {
            Err(unsupported())
        }
        pub fn open(_name: &str, _len: usize) -> io::Result<Self> {
            Err(unsupported())
        }
        pub fn base(&self) -> *mut u8 {
            std::ptr::null_mut()
        }
    }

    pub struct Event {
        _never: (),
    }

    impl Event {
        pub fn create(_name: &str) -> io::Result<Self> {
            Err(unsupported())
        }
        pub fn open(_name: &str) -> io::Result<Self> {
            Err(unsupported())
        }
        pub fn signal(&self) {}
        pub fn wait(&self, _timeout_ms: u32) -> bool {
            false
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::shmem::{RingLayout, ShmemRing};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Unique-per-call mapping name so parallel tests never collide.
    fn unique_name(tag: &str) -> String {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("echoes_bridge_test_{}_{}_{}", std::process::id(), tag, n)
    }

    #[test]
    fn create_then_open_share_memory() {
        let layout = RingLayout::new(16, 16);
        let bytes = layout.bytes_required();
        let name = unique_name("share");

        // Owner side: create + init the ring.
        let owner = SharedRegion::create(&name, bytes).expect("create");
        assert_eq!(owner.len(), bytes);
        let producer = unsafe { ShmemRing::init(owner.as_mut_ptr(), owner.len(), layout) };

        // Peer side: open the SAME name into an independent mapping + attach.
        let peer = SharedRegion::open(&name, bytes).expect("open");
        let consumer = unsafe { ShmemRing::attach(peer.as_mut_ptr(), peer.len()) };

        // Two independent mappings of one region see each other's writes — the
        // exact mechanism a real cross-process bridge relies on.
        assert!(producer.push(b"cross-proc"));
        let mut out = [0u8; 16];
        assert_eq!(consumer.pop(&mut out), Some(10));
        assert_eq!(&out[..10], b"cross-proc");
    }

    #[test]
    fn open_missing_name_errors() {
        let name = unique_name("missing");
        // `.err().unwrap()` extracts the error without requiring `SharedRegion`
        // (the Ok variant) to be `Debug`, which a raw-pointer mapping is not.
        let err = SharedRegion::open(&name, 64)
            .err()
            .expect("open of missing name must fail");
        // The mapping does not exist → OS reports not-found, surfaced as an error
        // rather than a panic or a null deref.
        assert!(err.raw_os_error().is_some());
    }

    #[test]
    fn event_wakes_a_waiter_on_another_handle() {
        let name = unique_name("event");
        let owner = SharedEvent::create(&name).expect("create");
        let peer = SharedEvent::open(&name).expect("open");

        assert!(!peer.wait(0), "a fresh event is not signalled");
        owner.signal();
        assert!(peer.wait(1000));
        assert!(!peer.wait(0), "auto-reset: one signal wakes one wait");
    }

    #[test]
    fn mapped_base_is_ring_aligned() {
        let name = unique_name("align");
        let region = SharedRegion::create(&name, 4096).expect("create");
        assert_eq!(region.as_mut_ptr() as usize % 4, 0);
    }
}
