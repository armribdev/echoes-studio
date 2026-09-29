//! Which in-process plugin was running when Echoes died (ADR-015 amendment).
//!
//! A 4 KiB file, mapped into memory, records the plugin currently executing on
//! each kind of thread.  Its pages belong to the OS, so they survive the
//! process: at the next start, an unclean previous session with a plugin still
//! marked names the suspect, and the UI offers to isolate it.
//!
//! Marking is an atomic store into the mapping — no allocation, no lock, no
//! call into the kernel — so the audio thread does it around every plugin
//! block.  (After the OS writes the page back, the next store may take a soft
//! page fault: no I/O, no lock.)  Without [`open`] (tests, a mapping failure)
//! marking does nothing.

use std::path::Path;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

const MAGIC: u64 = u64::from_le_bytes(*b"EsCrash1");
const WORDS: usize = 8;
const MARKER_BYTES: usize = 4096;

const OFF_MAGIC: usize = 0;
const OFF_SESSION: usize = 1;
const SESSION_RUNNING: u64 = 1;
const SESSION_CLEAN: u64 = 2;

/// The threads that call into plugins.  Each has its own slot so they do not
/// overwrite each other.
#[derive(Debug, Clone, Copy)]
pub enum PluginThread {
    Audio = 2,
    Control = 3,
    Ui = 4,
}

const SLOTS: [usize; 3] = [
    PluginThread::Audio as usize,
    PluginThread::Control as usize,
    PluginThread::Ui as usize,
];

static MARKER: AtomicPtr<AtomicU64> = AtomicPtr::new(std::ptr::null_mut());

/// Plugin crashes Echoes survived this session (sandboxed plugin condemned,
/// in-process crash caught).  The UI watcher polls it to report them.
static SESSION_CRASHES: AtomicU64 = AtomicU64::new(0);

/// Lock-free: called from the audio thread.
pub fn note_crash() {
    SESSION_CRASHES.fetch_add(1, Ordering::Relaxed);
}

pub fn crash_count() -> u64 {
    SESSION_CRASHES.load(Ordering::Relaxed)
}

/// Stable 64-bit tag of a plugin id (FNV-1a), never 0 (0 = no plugin).
pub fn plugin_tag(plugin_id: &str) -> u64 {
    let hash = plugin_id
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3)
        });
    hash.max(1)
}

fn word(index: usize) -> Option<&'static AtomicU64> {
    let base = MARKER.load(Ordering::Acquire);
    // SAFETY: a non-null base is a live mapping of `MARKER_BYTES` that is never unmapped.
    (!base.is_null()).then(|| unsafe { &*base.add(index) })
}

/// Map the marker file, return the tags of the plugins that were running if the
/// previous session died, then start a new session.  Call once, at startup.
pub fn open(path: &Path) -> Vec<u64> {
    let Some(base) = imp::map(path) else {
        eprintln!("[crash-marker] cannot map {}", path.display());
        return Vec::new();
    };
    // SAFETY: `imp::map` returns a page-aligned, never-unmapped mapping of `MARKER_BYTES`.
    let words = unsafe { std::slice::from_raw_parts(base, WORDS) };
    let suspects = suspects(&std::array::from_fn(|index| {
        words[index].load(Ordering::Acquire)
    }));

    SLOTS
        .iter()
        .for_each(|&slot| words[slot].store(0, Ordering::Relaxed));
    words[OFF_MAGIC].store(MAGIC, Ordering::Relaxed);
    words[OFF_SESSION].store(SESSION_RUNNING, Ordering::Release);
    MARKER.store(base, Ordering::Release);
    suspects
}

/// Tags left marked by a session that did not exit cleanly.
fn suspects(words: &[u64; WORDS]) -> Vec<u64> {
    if words[OFF_MAGIC] != MAGIC || words[OFF_SESSION] != SESSION_RUNNING {
        return Vec::new();
    }
    let mut tags: Vec<u64> = SLOTS
        .iter()
        .map(|&slot| words[slot])
        .filter(|&tag| tag != 0)
        .collect();
    tags.sort_unstable();
    tags.dedup();
    tags
}

/// Echoes is exiting normally: nothing to suspect at the next start.
pub fn mark_clean_exit() {
    if let Some(session) = word(OFF_SESSION) {
        session.store(SESSION_CLEAN, Ordering::Release);
    }
}

/// Marks `tag` as running on `thread` until the guard drops.  Tag 0 (a
/// sandboxed plugin) marks nothing.
pub fn enter(thread: PluginThread, tag: u64) -> MarkerGuard {
    let previous = match word(thread as usize) {
        Some(slot) if tag != 0 => slot.swap(tag, Ordering::Relaxed),
        _ => 0,
    };
    MarkerGuard {
        thread,
        tag,
        previous,
    }
}

pub struct MarkerGuard {
    thread: PluginThread,
    tag: u64,
    /// Mark of the enclosing call on this thread, put back on drop.
    previous: u64,
}

impl Drop for MarkerGuard {
    fn drop(&mut self) {
        // Leave the slot alone if a concurrent call replaced our mark.
        if let Some(slot) = word(self.thread as usize).filter(|_| self.tag != 0) {
            let _ = slot.compare_exchange(
                self.tag,
                self.previous,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::atomic::AtomicU64;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, OPEN_ALWAYS,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, FILE_MAP_ALL_ACCESS, PAGE_READWRITE,
    };

    use super::MARKER_BYTES;

    /// Map `path` (created if missing) for the life of the process.
    pub fn map(path: &Path) -> Option<*mut AtomicU64> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let file = CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ,
                std::ptr::null(),
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            );
            if file == INVALID_HANDLE_VALUE {
                return None;
            }
            let mapping = CreateFileMappingW(
                file,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                MARKER_BYTES as u32,
                std::ptr::null(),
            );
            CloseHandle(file);
            if mapping.is_null() {
                return None;
            }
            let view = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, MARKER_BYTES);
            CloseHandle(mapping);
            (!view.Value.is_null()).then_some(view.Value as *mut AtomicU64)
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;
    use std::sync::atomic::AtomicU64;

    pub fn map(_: &Path) -> Option<*mut AtomicU64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_stable_and_never_zero() {
        assert_eq!(plugin_tag("clap:com.vital"), plugin_tag("clap:com.vital"));
        assert_ne!(plugin_tag("clap:com.vital"), plugin_tag("clap:com.surge"));
        assert_ne!(plugin_tag(""), 0);
    }

    fn session(state: u64, audio_tag: u64) -> [u64; WORDS] {
        let mut words = [0; WORDS];
        words[OFF_MAGIC] = MAGIC;
        words[OFF_SESSION] = state;
        words[PluginThread::Audio as usize] = audio_tag;
        words
    }

    #[test]
    fn only_a_session_that_did_not_exit_accuses_the_running_plugin() {
        let vital = plugin_tag("clap:vital");
        assert_eq!(suspects(&session(SESSION_RUNNING, vital)), vec![vital]);
        assert!(
            suspects(&session(SESSION_CLEAN, vital)).is_empty(),
            "a clean exit accuses nobody"
        );
        assert!(
            suspects(&session(SESSION_RUNNING, 0)).is_empty(),
            "no plugin was running"
        );
        assert!(suspects(&[0; WORDS]).is_empty(), "a fresh file");
    }

    #[cfg(windows)]
    #[test]
    fn marks_reach_the_file_as_the_next_process_would_read_it() {
        let path = std::env::temp_dir().join(format!("echoes_crash_marker_{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let base = imp::map(&path).unwrap();
        let words = unsafe { std::slice::from_raw_parts(base, WORDS) };
        words[OFF_MAGIC].store(MAGIC, Ordering::Release);
        words[OFF_SESSION].store(SESSION_RUNNING, Ordering::Release);
        words[PluginThread::Audio as usize].store(plugin_tag("clap:vital"), Ordering::Release);

        let bytes = std::fs::read(&path).unwrap();
        let on_disk: [u64; WORDS] = std::array::from_fn(|i| {
            u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap())
        });
        assert_eq!(suspects(&on_disk), vec![plugin_tag("clap:vital")]);
    }
}
