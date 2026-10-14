//! CLAP plugin API — raw C struct definitions (CLAP 1.x spec).
//!
//! CLAP is a pure C ABI (no COM, no vtables).  Every plugin exports a
//! `clap_entry` symbol; from there we reach the factory, create instances,
//! and call `process`.
//!
//! ## Safety contract
//!
//! All function pointers inside `ClapPlugin`, `ClapPluginFactory`, and
//! `ClapHost` are valid as long as the owning `ClapLibrary` is not dropped.
//! Keep an `Arc<ClapLibrary>` alive alongside any wrapper node.

// Verbatim extraction of the engine's hand-written CLAP C-ABI bindings
// (ADR-015 6d-iii-b-2).  These lints are intrinsic to FFI binding code; allowing
// them at the crate root keeps the extraction faithful rather than rewriting the
// proven bindings.
#![allow(
    unused_mut,
    clippy::missing_safety_doc,
    clippy::manual_c_str_literals,
    clippy::transmutes_expressible_as_ptr_casts
)]

use std::ffi::{c_char, c_void};
use std::path::Path;

mod notes;
mod presets;
mod process_sleep;
mod timers;
mod track_info;
pub use notes::*;
pub use presets::*;
pub use process_sleep::*;
pub use timers::*;
pub use track_info::*;

// ── Version ────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClapVersion {
    pub major: u32,
    pub minor: u32,
    pub revision: u32,
}

pub const CLAP_VERSION_1: ClapVersion = ClapVersion {
    major: 1,
    minor: 0,
    revision: 0,
};

// ── Entry point ────────────────────────────────────────────────────────────────

pub const CLAP_PLUGIN_FACTORY_ID: &[u8] = b"clap.plugin-factory\0";

#[repr(C)]
pub struct ClapPluginEntry {
    pub clap_version: ClapVersion,
    pub init: unsafe extern "C" fn(plugin_path: *const c_char) -> bool,
    pub deinit: unsafe extern "C" fn(),
    pub get_factory: unsafe extern "C" fn(factory_id: *const c_char) -> *const c_void,
}

// ── Plugin descriptor ──────────────────────────────────────────────────────────

#[repr(C)]
pub struct ClapPluginDescriptor {
    pub clap_version: ClapVersion,
    pub id: *const c_char,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub url: *const c_char,
    pub manual_url: *const c_char,
    pub support_url: *const c_char,
    pub version: *const c_char,
    pub description: *const c_char,
    pub features: *const *const c_char,
}

impl ClapPluginDescriptor {
    pub fn id_str(&self) -> &str {
        if self.id.is_null() {
            return "";
        }
        unsafe { std::ffi::CStr::from_ptr(self.id) }
            .to_str()
            .unwrap_or("")
    }
    pub fn name_str(&self) -> &str {
        if self.name.is_null() {
            return "";
        }
        unsafe { std::ffi::CStr::from_ptr(self.name) }
            .to_str()
            .unwrap_or("")
    }
    pub fn vendor_str(&self) -> &str {
        if self.vendor.is_null() {
            return "";
        }
        unsafe { std::ffi::CStr::from_ptr(self.vendor) }
            .to_str()
            .unwrap_or("")
    }
}

// ── Factory ────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct ClapPluginFactory {
    pub get_plugin_count: unsafe extern "C" fn(*const ClapPluginFactory) -> u32,
    pub get_plugin_descriptor:
        unsafe extern "C" fn(*const ClapPluginFactory, u32) -> *const ClapPluginDescriptor,
    pub create_plugin: unsafe extern "C" fn(
        *const ClapPluginFactory,
        *const ClapHost,
        *const c_char,
    ) -> *const ClapPlugin,
}

// ── Host struct (provided by Echoes Studio to plugins) ─────────────────────────

#[repr(C)]
pub struct ClapHost {
    pub clap_version: ClapVersion,
    pub host_data: *mut c_void,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub url: *const c_char,
    pub version: *const c_char,
    // ── Callback order is ABI-load-bearing ──────────────────────────────────────
    // CLAP 1.x `clap_host` declares these four function pointers in this exact
    // order: get_extension, request_restart, request_process, request_callback.
    // `#[repr(C)]` lays them out by declaration order, so any other order makes
    // the plugin call the wrong slot — e.g. its first `get_extension(...)` during
    // `init()` lands on `request_restart`, whose garbage return value is then
    // dereferenced as an "extension" pointer → STATUS_ACCESS_VIOLATION (Vital and
    // every CLAP that probes extensions on init).  Do not reorder.
    pub get_extension: unsafe extern "C" fn(*const ClapHost, *const c_char) -> *const c_void,
    pub request_restart: unsafe extern "C" fn(*const ClapHost),
    pub request_process: unsafe extern "C" fn(*const ClapHost),
    pub request_callback: unsafe extern "C" fn(*const ClapHost),
}

pub unsafe extern "C" fn host_request_restart(host: *const ClapHost) {
    if let Some(context) = host_context(host) {
        context.request_restart();
    }
}
pub unsafe extern "C" fn host_request_process(host: *const ClapHost) {
    if let Some(context) = host_context(host) {
        context.request_process();
    }
}
pub unsafe extern "C" fn host_request_callback(host: *const ClapHost) {
    if let Some(context) = host_context(host) {
        context.request_gui_drain();
    }
}

/// Posted to a plugin's editor window: run its `on_main_thread`.
pub const CLAP_GUI_DRAIN_MESSAGE: u32 = 0x8000 + 1;
/// Posted to a plugin's editor window: resize the client area to (wparam, lparam).
pub const CLAP_GUI_RESIZE_MESSAGE: u32 = 0x8000 + 2;
const WM_CLOSE: u32 = 0x0010;

/// Per-plugin host state behind `clap_host.host_data`, owned by its `ClapHost`,
/// so host callbacks reach the editor of the plugin that made them.
pub struct ClapHostContext {
    gui_hwnd: std::sync::atomic::AtomicPtr<c_void>,
    gui_drain_pending: std::sync::atomic::AtomicBool,
    restart_requested: std::sync::atomic::AtomicBool,
    process_requested: std::sync::atomic::AtomicBool,
    tail_changed: std::sync::atomic::AtomicBool,
    track_info: track_info::TrackInfoCell,
    timers: timers::TimerTable,
}

impl ClapHostContext {
    fn new() -> Self {
        Self {
            gui_hwnd: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            gui_drain_pending: std::sync::atomic::AtomicBool::new(false),
            restart_requested: std::sync::atomic::AtomicBool::new(false),
            process_requested: std::sync::atomic::AtomicBool::new(false),
            tail_changed: std::sync::atomic::AtomicBool::new(false),
            track_info: track_info::TrackInfoCell::new(),
            timers: timers::TimerTable::new(),
        }
    }

    /// The timers the plugin registered through `clap.timer-support`.
    pub fn timers(&self) -> &timers::TimerTable {
        &self.timers
    }

    /// Whether the plugin's editor window exists.  Its timers only run then.
    pub fn editor_open(&self) -> bool {
        !self
            .gui_hwnd
            .load(std::sync::atomic::Ordering::Acquire)
            .is_null()
    }

    /// Records the plugin's track for `clap.track-info.get`; returns whether it
    /// differs from the previous one (the caller then tells the plugin).  Runs
    /// on an off-RT thread; readers never wait for it.
    pub fn set_track_info(&self, info: &HostTrackInfo) -> bool {
        self.track_info.store(info)
    }

    fn read_track_info(&self, out: &mut ClapTrackInfo) -> bool {
        self.track_info.read_into(out)
    }

    /// Lock-free: the plugin asks to be processed even if the host put it to sleep.
    pub fn request_process(&self) {
        self.process_requested
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Whether the plugin asked to be processed since the last call, and clears it.
    pub fn take_process_request(&self) -> bool {
        self.process_requested
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    pub fn note_tail_changed(&self) {
        self.tail_changed
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Whether the plugin reported a new tail since the last call, and clears it.
    pub fn take_tail_changed(&self) -> bool {
        self.tail_changed
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    /// Lock-free: `request_restart` is callable from any thread, the audio
    /// thread included.  The app polls [`take_plugin_restart_requested`] to
    /// recompile, and the recompile reactivates this plugin.
    pub fn request_restart(&self) {
        self.restart_requested
            .store(true, std::sync::atomic::Ordering::Release);
        mark_plugin_restart_requested();
    }

    /// Whether the plugin asked for a restart since the last call, and clears it.
    pub fn take_restart_request(&self) -> bool {
        self.restart_requested
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    /// Editor window of this plugin. Null when its editor is closed.
    pub fn set_gui_hwnd(&self, hwnd: *mut c_void) {
        self.gui_hwnd
            .store(hwnd, std::sync::atomic::Ordering::Release);
        if hwnd.is_null() {
            self.gui_drain_pending
                .store(false, std::sync::atomic::Ordering::Release);
        }
    }

    /// Called by the editor before it runs `on_main_thread`, so a request that
    /// arrives during the drain is posted again.
    pub fn clear_gui_drain(&self) {
        self.gui_drain_pending
            .store(false, std::sync::atomic::Ordering::Release);
    }

    /// If `request_callback` fired during `on_main_thread`, the coalesce skipped
    /// the post. Queue one more drain.
    pub fn rearm_gui_drain(&self) {
        if self
            .gui_drain_pending
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.post_to_gui(CLAP_GUI_DRAIN_MESSAGE, 0, 0);
        }
    }

    fn request_gui_drain(&self) {
        if self
            .gui_hwnd
            .load(std::sync::atomic::Ordering::Acquire)
            .is_null()
        {
            return;
        }
        // One posted drain at a time. A synth GUI can request a callback every
        // block; posting each one floods the UI thread and crackles the audio.
        if self
            .gui_drain_pending
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        self.post_to_gui(CLAP_GUI_DRAIN_MESSAGE, 0, 0);
    }

    /// Non-blocking from any thread. False when no editor is open.
    fn post_to_gui(&self, message: u32, wparam: usize, lparam: isize) -> bool {
        let hwnd = self.gui_hwnd.load(std::sync::atomic::Ordering::Acquire);
        if hwnd.is_null() {
            return false;
        }
        #[cfg(windows)]
        unsafe {
            #[link(name = "user32")]
            unsafe extern "system" {
                fn PostMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
            }
            PostMessageW(hwnd, message, wparam, lparam) != 0
        }
        #[cfg(not(windows))]
        {
            let _ = (message, wparam, lparam);
            false
        }
    }
}

impl ClapHost {
    pub fn context(&self) -> Option<&ClapHostContext> {
        unsafe { (self.host_data as *const ClapHostContext).as_ref() }
    }
}

// `host_data` is only ever null or the context allocated by `make_host`.
impl Drop for ClapHost {
    fn drop(&mut self) {
        if !self.host_data.is_null() {
            drop(unsafe { Box::from_raw(self.host_data as *mut ClapHostContext) });
        }
    }
}

unsafe fn host_context<'a>(host: *const ClapHost) -> Option<&'a ClapHostContext> {
    if host.is_null() {
        return None;
    }
    ((*host).host_data as *const ClapHostContext).as_ref()
}

pub unsafe extern "C" fn host_get_extension(
    _: *const ClapHost,
    id: *const c_char,
) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    let id = std::ffi::CStr::from_ptr(id).to_bytes_with_nul();
    if id == CLAP_EXT_STATE {
        return &HOST_STATE as *const ClapHostState as *const c_void;
    }
    if id == CLAP_EXT_THREAD_CHECK {
        return &HOST_THREAD_CHECK as *const ClapHostThreadCheck as *const c_void;
    }
    if id == CLAP_EXT_HOST_GUI {
        return &HOST_GUI as *const ClapHostGui as *const c_void;
    }
    if id == CLAP_EXT_LOG {
        return &HOST_LOG as *const ClapHostLog as *const c_void;
    }
    if id == CLAP_EXT_LATENCY {
        return &HOST_LATENCY as *const ClapHostLatency as *const c_void;
    }
    if id == CLAP_EXT_TAIL {
        return &HOST_TAIL as *const ClapHostTail as *const c_void;
    }
    if id == CLAP_EXT_PARAMS {
        return &HOST_PARAMS as *const ClapHostParams as *const c_void;
    }
    if id == CLAP_EXT_NOTE_PORTS {
        return &notes::HOST_NOTE_PORTS as *const ClapHostNotePorts as *const c_void;
    }
    if id == CLAP_EXT_NOTE_NAME {
        return &notes::HOST_NOTE_NAME as *const ClapHostNoteName as *const c_void;
    }
    if id == CLAP_EXT_TRACK_INFO || id == CLAP_EXT_TRACK_INFO_COMPAT {
        return &track_info::HOST_TRACK_INFO as *const ClapHostTrackInfo as *const c_void;
    }
    if id == CLAP_EXT_TIMER_SUPPORT {
        return &timers::HOST_TIMER_SUPPORT as *const ClapHostTimerSupport as *const c_void;
    }
    if id == CLAP_EXT_PRESET_LOAD || id == CLAP_EXT_PRESET_LOAD_COMPAT {
        return &presets::HOST_PRESET_LOAD as *const ClapHostPresetLoad as *const c_void;
    }
    std::ptr::null()
}

pub const CLAP_PARAM_RESCAN_VALUES: u32 = 1 << 0;
pub const CLAP_PARAM_RESCAN_TEXT: u32 = 1 << 1;
pub const CLAP_PARAM_RESCAN_INFO: u32 = 1 << 2;
pub const CLAP_PARAM_RESCAN_ALL: u32 = 1 << 3;

/// Host-side `clap.params`.
#[repr(C)]
pub struct ClapHostParams {
    pub rescan: unsafe extern "C" fn(*const ClapHost, u32),
    pub clear: unsafe extern "C" fn(*const ClapHost, u32, u32),
    pub request_flush: unsafe extern "C" fn(*const ClapHost),
}

static HOST_PARAMS: ClapHostParams = ClapHostParams {
    rescan: host_params_rescan,
    clear: host_params_clear,
    request_flush: host_params_request_flush,
};

unsafe extern "C" fn host_params_rescan(_: *const ClapHost, flags: u32) {
    mark_plugin_params_rescan(flags);
}

// Dropping the automation or modulation of a parameter is a project change; a
// plugin callback does not get to make it.
unsafe extern "C" fn host_params_clear(_: *const ClapHost, _: u32, _: u32) {}

// A plugin that is processing gets its flush through the next `process`; a
// sleeping one is woken for it. One that is not active has no parameter work
// the host could schedule.
unsafe extern "C" fn host_params_request_flush(host: *const ClapHost) {
    if let Some(context) = host_context(host) {
        context.request_process();
    }
}

/// `CLAP_PARAM_RESCAN_*` flags reported by any hosted plugin since the last take.
static PLUGIN_PARAMS_RESCAN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Lock-free: callable from any thread.
pub fn mark_plugin_params_rescan(flags: u32) {
    PLUGIN_PARAMS_RESCAN.fetch_or(flags, std::sync::atomic::Ordering::AcqRel);
}

/// The rescan flags reported since the last call, and clears them.
pub fn take_plugin_params_rescan() -> u32 {
    PLUGIN_PARAMS_RESCAN.swap(0, std::sync::atomic::Ordering::AcqRel)
}

/// Host-side `clap.tail`: the audio thread rereads the tail after `changed`.
#[repr(C)]
pub struct ClapHostTail {
    pub changed: unsafe extern "C" fn(*const ClapHost),
}

static HOST_TAIL: ClapHostTail = ClapHostTail {
    changed: host_tail_changed,
};

unsafe extern "C" fn host_tail_changed(host: *const ClapHost) {
    if let Some(context) = host_context(host) {
        context.note_tail_changed();
    }
}

/// Host-side `clap.latency`.  `changed` is only allowed during `activate`, and
/// the host reads the latency after every activation, so there is nothing to do.
#[repr(C)]
pub struct ClapHostLatency {
    pub changed: unsafe extern "C" fn(*const ClapHost),
}

static HOST_LATENCY: ClapHostLatency = ClapHostLatency {
    changed: host_latency_changed,
};

unsafe extern "C" fn host_latency_changed(_: *const ClapHost) {}

/// Set when any hosted plugin requests a restart (e.g. its latency changed).
static PLUGIN_RESTART_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Lock-free: callable from the audio thread.
pub fn mark_plugin_restart_requested() {
    PLUGIN_RESTART_REQUESTED.store(true, std::sync::atomic::Ordering::Release);
}

/// Returns whether a plugin requested a restart since the last call, and clears it.
pub fn take_plugin_restart_requested() -> bool {
    PLUGIN_RESTART_REQUESTED.swap(false, std::sync::atomic::Ordering::AcqRel)
}

static HOST_GUI: ClapHostGui = ClapHostGui {
    resize_hints_changed: host_gui_resize_hints_changed,
    request_resize: host_gui_request_resize,
    request_show: host_gui_request_show,
    request_hide: host_gui_request_hide,
    closed: host_gui_closed,
};

unsafe extern "C" fn host_gui_resize_hints_changed(_: *const ClapHost) {}

// Posted rather than resized in place: `request_resize` is callable from any
// thread, and the window belongs to the Win32 UI thread.
unsafe extern "C" fn host_gui_request_resize(
    host: *const ClapHost,
    width: u32,
    height: u32,
) -> bool {
    host_context(host).is_some_and(|context| {
        context.post_to_gui(CLAP_GUI_RESIZE_MESSAGE, width as usize, height as isize)
    })
}

// The editor is embedded and always visible while open; the host cannot show or hide it on demand.
unsafe extern "C" fn host_gui_request_show(_: *const ClapHost) -> bool {
    false
}

unsafe extern "C" fn host_gui_request_hide(_: *const ClapHost) -> bool {
    false
}

unsafe extern "C" fn host_gui_closed(host: *const ClapHost, _was_destroyed: bool) {
    if let Some(context) = host_context(host) {
        context.post_to_gui(WM_CLOSE, 0, 0);
    }
}

static HOST_LOG: ClapHostLog = ClapHostLog { log: host_log };

unsafe extern "C" fn host_log(
    _: *const ClapHost,
    severity: ClapLogSeverity,
    message: *const c_char,
) {
    // Printing allocates and does I/O, which the audio callback must never do.
    if message.is_null() || host_is_audio_thread(std::ptr::null()) {
        return;
    }
    let label = match severity {
        CLAP_LOG_DEBUG => "DBG",
        CLAP_LOG_INFO => "INF",
        CLAP_LOG_WARNING => "WRN",
        CLAP_LOG_ERROR => "ERR",
        CLAP_LOG_FATAL => "FTL",
        CLAP_LOG_HOST_MISBEHAVING => "HOST!",
        CLAP_LOG_PLUGIN_MISBEHAVING => "PLUG!",
        _ => "???",
    };
    let text = std::ffi::CStr::from_ptr(message).to_string_lossy();
    eprintln!("[clap.{label}] {text}");
}

/// `clap.thread-check` — lets a plugin stay off the message lock inside
/// `process`. The audio thread is a role, not a fixed thread (CLAP: the thread
/// that processes, which may change, never two at once): a thread holds it
/// only inside an [`audio_thread_scope`]. Every other thread counts as main.
pub const CLAP_EXT_THREAD_CHECK: &[u8] = b"clap.thread-check\0";

#[repr(C)]
pub struct ClapHostThreadCheck {
    pub is_main_thread: unsafe extern "C" fn(*const ClapHost) -> bool,
    pub is_audio_thread: unsafe extern "C" fn(*const ClapHost) -> bool,
}

static HOST_THREAD_CHECK: ClapHostThreadCheck = ClapHostThreadCheck {
    is_main_thread: host_is_main_thread,
    is_audio_thread: host_is_audio_thread,
};

thread_local! {
    // Const-initialised and without `Drop`: reading or setting it never
    // allocates, so the audio callback may use it.
    static IN_AUDIO_ROLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Makes the current thread the plugin's audio thread until the guard drops.
/// Wrap every `start_processing` / `process` / `stop_processing` call in one.
/// Off the audio callback (teardown, reactivation), the caller must guarantee
/// no `process` runs concurrently.
pub fn audio_thread_scope() -> AudioThreadScope {
    AudioThreadScope {
        previous: IN_AUDIO_ROLE.with(|role| role.replace(true)),
    }
}

pub struct AudioThreadScope {
    previous: bool,
}

impl Drop for AudioThreadScope {
    fn drop(&mut self) {
        IN_AUDIO_ROLE.with(|role| role.set(self.previous));
    }
}

unsafe extern "C" fn host_is_main_thread(_: *const ClapHost) -> bool {
    // Plugin lifecycle and `clap.params` run on the command thread; the editor
    // runs on the Win32 thread. Both are main-thread work.
    !host_is_audio_thread(std::ptr::null())
}

unsafe extern "C" fn host_is_audio_thread(_: *const ClapHost) -> bool {
    IN_AUDIO_ROLE.with(std::cell::Cell::get)
}

/// `clap_host_state` — lets a plugin report that its state changed.
#[repr(C)]
pub struct ClapHostState {
    pub mark_dirty: unsafe extern "C" fn(*const ClapHost),
}

static HOST_STATE: ClapHostState = ClapHostState {
    mark_dirty: host_state_mark_dirty,
};

/// Set when any hosted plugin reports an edit; the app polls it to mark the project unsaved.
static PLUGIN_STATE_EDITED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

unsafe extern "C" fn host_state_mark_dirty(_: *const ClapHost) {
    mark_plugin_state_edited();
}

/// Lock-free: callable from the audio thread.
pub fn mark_plugin_state_edited() {
    PLUGIN_STATE_EDITED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Returns whether a plugin reported an edit since the last call, and clears it.
pub fn take_plugin_state_edited() -> bool {
    PLUGIN_STATE_EDITED.swap(false, std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod host_abi_tests {
    use super::*;
    use std::mem::offset_of;

    /// Guards the CLAP 1.x `clap_host` callback order. A wrong order routes the
    /// plugin's `get_extension(...)` call (made during `init()`) to a different
    /// slot, returning a garbage pointer that the plugin dereferences →
    /// STATUS_ACCESS_VIOLATION on instantiation. Must stay get_extension <
    /// request_restart < request_process < request_callback.
    #[test]
    fn clap_host_callback_order_matches_abi() {
        assert!(
            offset_of!(ClapHost, get_extension) < offset_of!(ClapHost, request_restart),
            "get_extension must precede request_restart (CLAP ABI)"
        );
        assert!(
            offset_of!(ClapHost, request_restart) < offset_of!(ClapHost, request_process),
            "request_restart must precede request_process (CLAP ABI)"
        );
        assert!(
            offset_of!(ClapHost, request_process) < offset_of!(ClapHost, request_callback),
            "request_process must precede request_callback (CLAP ABI)"
        );
    }

    #[test]
    fn the_audio_role_lasts_only_for_its_scope_and_its_thread() {
        let host = make_host();
        unsafe {
            assert!(!host_is_audio_thread(&*host));
            {
                let _audio = audio_thread_scope();
                {
                    let _nested = audio_thread_scope();
                }
                assert!(
                    host_is_audio_thread(&*host),
                    "a nested scope keeps the role"
                );
                let other = std::thread::spawn(|| host_is_audio_thread(std::ptr::null()));
                assert!(
                    !other.join().unwrap(),
                    "another thread is not the audio thread"
                );
            }
            assert!(
                !host_is_audio_thread(&*host),
                "the role ends with the scope"
            );
        }
    }

    #[test]
    fn thread_check_audio_thread_is_not_main() {
        let host = make_host();
        let _audio = audio_thread_scope();
        unsafe {
            assert!(host_is_audio_thread(&*host));
            assert!(!host_is_main_thread(&*host));
            let ext = host_get_extension(&*host, CLAP_EXT_THREAD_CHECK.as_ptr() as *const c_char);
            assert!(!ext.is_null());
        }
    }

    #[test]
    fn a_restart_request_reaches_the_plugin_context_and_the_app() {
        let host = make_host();
        let context = host.context().unwrap();
        unsafe { host_request_restart(&*host) };
        assert!(take_plugin_restart_requested());
        assert!(!take_plugin_restart_requested());
        assert!(context.take_restart_request());
        assert!(!context.take_restart_request());

        // A rescan of all note ports is a restart request too; a rename is not.
        // Same test: the flag is process-wide.
        unsafe {
            let ports = host_get_extension(&*host, CLAP_EXT_NOTE_PORTS.as_ptr() as *const c_char)
                as *const ClapHostNotePorts;
            ((*ports).rescan)(&*host, CLAP_NOTE_PORTS_RESCAN_NAMES);
            assert!(!context.take_restart_request());
            ((*ports).rescan)(&*host, CLAP_NOTE_PORTS_RESCAN_ALL);
        }
        assert!(context.take_restart_request());
        assert!(take_plugin_restart_requested());
    }

    #[test]
    fn process_and_tail_requests_reach_the_plugin_context_once() {
        let host = make_host();
        let context = host.context().unwrap();
        unsafe {
            host_request_process(&*host);
            let tail = host_get_extension(&*host, CLAP_EXT_TAIL.as_ptr() as *const c_char)
                as *const ClapHostTail;
            ((*tail).changed)(&*host);
        }
        assert!(context.take_process_request());
        assert!(!context.take_process_request());
        assert!(context.take_tail_changed());
        assert!(!context.take_tail_changed());
    }

    #[test]
    fn host_params_collects_rescans_and_wakes_the_plugin_for_a_flush() {
        let host = make_host();
        let context = host.context().unwrap();
        take_plugin_params_rescan();
        unsafe {
            let params = host_get_extension(&*host, CLAP_EXT_PARAMS.as_ptr() as *const c_char)
                as *const ClapHostParams;
            assert!(!params.is_null());
            ((*params).rescan)(&*host, CLAP_PARAM_RESCAN_TEXT);
            ((*params).rescan)(&*host, CLAP_PARAM_RESCAN_INFO);
            ((*params).clear)(&*host, 7, 1);
            assert!(!context.take_process_request());
            ((*params).request_flush)(&*host);
        }
        assert_eq!(
            take_plugin_params_rescan(),
            CLAP_PARAM_RESCAN_TEXT | CLAP_PARAM_RESCAN_INFO
        );
        assert_eq!(take_plugin_params_rescan(), 0);
        assert!(context.take_process_request());
    }

    #[test]
    fn host_exposes_gui_log_and_latency_extensions() {
        let host = make_host();
        unsafe {
            for id in [
                CLAP_EXT_HOST_GUI,
                CLAP_EXT_LOG,
                CLAP_EXT_LATENCY,
                CLAP_EXT_TAIL,
            ] {
                let ext = host_get_extension(&*host, id.as_ptr() as *const c_char);
                assert!(!ext.is_null());
            }
        }
    }

    #[test]
    fn each_host_has_its_own_context() {
        let first = make_host();
        let second = make_host();
        let first_context = first.context().unwrap() as *const ClapHostContext;
        let second_context = second.context().unwrap() as *const ClapHostContext;
        assert_ne!(first_context, second_context);
    }

    #[test]
    fn resize_request_is_refused_without_an_open_editor() {
        let host = make_host();
        unsafe {
            assert!(!host_gui_request_resize(&*host, 640, 480));
            assert!(!host_gui_request_resize(std::ptr::null(), 640, 480));
        }
    }
}

// Constant C strings for the host descriptor.
static HOST_NAME: &[u8] = b"Echoes Studio\0";
static HOST_VENDOR: &[u8] = b"Echoes Studio\0";
static HOST_URL: &[u8] = b"https://github.com/echoes-studio/echoes-studio\0";
static HOST_VERSION: &[u8] = b"0.1.0\0";

/// Build a stable ClapHost struct (lives on the heap, pointer-stable).
pub fn make_host() -> Box<ClapHost> {
    Box::new(ClapHost {
        clap_version: CLAP_VERSION_1,
        host_data: Box::into_raw(Box::new(ClapHostContext::new())) as *mut c_void,
        name: HOST_NAME.as_ptr() as *const c_char,
        vendor: HOST_VENDOR.as_ptr() as *const c_char,
        url: HOST_URL.as_ptr() as *const c_char,
        version: HOST_VERSION.as_ptr() as *const c_char,
        // Order matches the `ClapHost` field order (= CLAP ABI). See note there.
        get_extension: host_get_extension,
        request_restart: host_request_restart,
        request_process: host_request_process,
        request_callback: host_request_callback,
    })
}

// ── Plugin instance ────────────────────────────────────────────────────────────

#[repr(C)]
pub struct ClapPlugin {
    pub desc: *const ClapPluginDescriptor,
    pub plugin_data: *mut c_void,
    pub init: unsafe extern "C" fn(*const ClapPlugin) -> bool,
    pub destroy: unsafe extern "C" fn(*const ClapPlugin),
    pub activate: unsafe extern "C" fn(*const ClapPlugin, f64, u32, u32) -> bool,
    pub deactivate: unsafe extern "C" fn(*const ClapPlugin),
    pub start_processing: unsafe extern "C" fn(*const ClapPlugin) -> bool,
    pub stop_processing: unsafe extern "C" fn(*const ClapPlugin),
    pub reset: unsafe extern "C" fn(*const ClapPlugin),
    pub process: unsafe extern "C" fn(*const ClapPlugin, *const ClapProcess) -> ClapProcessStatus,
    pub get_extension: unsafe extern "C" fn(*const ClapPlugin, *const c_char) -> *const c_void,
    pub on_main_thread: unsafe extern "C" fn(*const ClapPlugin),
}

// ── Audio buffer ───────────────────────────────────────────────────────────────

#[repr(C)]
pub struct ClapAudioBuffer {
    pub data32: *mut *mut f32,
    pub data64: *mut *mut f64,
    pub channel_count: u32,
    pub latency: u32,
    pub constant_mask: u64,
}

// ── Events ─────────────────────────────────────────────────────────────────────

pub const CLAP_EVENT_NOTE_ON: u16 = 0;
pub const CLAP_EVENT_NOTE_OFF: u16 = 1;
/// `CLAP_EVENT_PARAM_VALUE` — host → plugin parameter change (ADR-007).
pub const CLAP_EVENT_PARAM_VALUE: u16 = 5;
/// `CLAP_EVENT_PARAM_MOD` — non-destructive offset added by the plugin on top of
/// the parameter's current value.  Same layout as `clap_event_param_value`, with
/// the modulation amount (plain units) in `value`.
pub const CLAP_EVENT_PARAM_MOD: u16 = 6;
/// `CLAP_EVENT_PARAM_GESTURE_BEGIN` / `_END` — plugin → host: the user starts /
/// stops dragging a control in the plugin window (`clap_event_param_gesture`).
pub const CLAP_EVENT_PARAM_GESTURE_BEGIN: u16 = 7;
pub const CLAP_EVENT_PARAM_GESTURE_END: u16 = 8;
/// `CLAP_EVENT_MIDI` — a raw 3-byte MIDI 1.0 message (`clap_event_midi`).
pub const CLAP_EVENT_MIDI: u16 = 10;
pub const CLAP_CORE_EVENT_SPACE: u16 = 0;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClapEventParamGesture {
    pub header: ClapEventHeader,
    pub param_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClapEventHeader {
    pub size: u32,
    pub time: u32,     // frame offset within current block
    pub space_id: u16, // 0 = CLAP core
    pub r#type: u16,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClapEventNote {
    pub header: ClapEventHeader,
    pub note_id: i32, // -1 if unused
    pub port_index: i16,
    pub channel: i16,
    pub key: i16, // 0..127
    pub _pad: i16,
    pub velocity: f64, // 0..1
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClapEventMidi {
    pub header: ClapEventHeader,
    pub port_index: u16,
    pub data: [u8; 3],
    /// Spells out the C tail padding so `write_bytes` never copies uninitialised bytes.
    pub _pad: [u8; 3],
}

// Fixed-capacity event list (no allocation in process).
pub const MAX_EVENTS: usize = 256;

/// Capacity of the param-value event list.  Each modulated param can carry one
/// sub-block point per `MODULATION_CONTROL_PERIOD` (32) over a full
/// `MAX_BLOCK_FRAMES` (4096) block — up to 128 points (ADR-019).  Sized so
/// several modulated params at the offline block size never drop points.
pub const MAX_PARAM_EVENTS: usize = 1024;

#[repr(C)]
pub struct ClapInputEvents {
    pub ctx: *mut c_void,
    pub size: unsafe extern "C" fn(*const ClapInputEvents) -> u32,
    pub get: unsafe extern "C" fn(*const ClapInputEvents, u32) -> *const ClapEventHeader,
}

#[repr(C)]
pub struct ClapOutputEvents {
    pub ctx: *mut c_void,
    pub try_push: unsafe extern "C" fn(*const ClapOutputEvents, *const ClapEventHeader) -> bool,
}

/// Fixed-size MIDI event list for passing notes into CLAP instruments.
pub struct FixedEventList {
    pub events: [ClapEventNote; MAX_EVENTS],
    pub count: usize,
    pub input_events_vtable: ClapInputEvents,
}

impl FixedEventList {
    pub fn new() -> Box<Self> {
        let mut list = Box::new(FixedEventList {
            events: unsafe { std::mem::zeroed() },
            count: 0,
            input_events_vtable: ClapInputEvents {
                ctx: std::ptr::null_mut(),
                size: fel_size,
                get: fel_get,
            },
        });
        // Point ctx back to self so the vtable callbacks can reach the data.
        list.input_events_vtable.ctx = list.as_mut() as *mut FixedEventList as *mut c_void;
        list
    }

    pub fn clear(&mut self) {
        self.count = 0;
    }

    pub fn push_note_on(&mut self, time: u32, channel: i16, key: i16, velocity: f64) {
        if self.count >= MAX_EVENTS {
            return;
        }
        self.events[self.count] = ClapEventNote {
            header: ClapEventHeader {
                size: std::mem::size_of::<ClapEventNote>() as u32,
                time,
                space_id: CLAP_CORE_EVENT_SPACE,
                r#type: CLAP_EVENT_NOTE_ON,
                flags: 0,
            },
            note_id: -1,
            port_index: 0,
            channel,
            key,
            _pad: 0,
            velocity,
        };
        self.count += 1;
    }

    pub fn push_note_off(&mut self, time: u32, channel: i16, key: i16) {
        if self.count >= MAX_EVENTS {
            return;
        }
        self.events[self.count] = ClapEventNote {
            header: ClapEventHeader {
                size: std::mem::size_of::<ClapEventNote>() as u32,
                time,
                space_id: CLAP_CORE_EVENT_SPACE,
                r#type: CLAP_EVENT_NOTE_OFF,
                flags: 0,
            },
            note_id: -1,
            port_index: 0,
            channel,
            key,
            _pad: 0,
            velocity: 0.0,
        };
        self.count += 1;
    }
}

unsafe extern "C" fn fel_size(list: *const ClapInputEvents) -> u32 {
    let fel = (*list).ctx as *const FixedEventList;
    (*fel).count as u32
}

unsafe extern "C" fn fel_get(list: *const ClapInputEvents, index: u32) -> *const ClapEventHeader {
    let fel = (*list).ctx as *const FixedEventList;
    let fel = &*fel;
    if index as usize >= fel.count {
        return std::ptr::null();
    }
    &fel.events[index as usize].header as *const ClapEventHeader
}

// ── Parameter-value events (host → plugin, ADR-007) ────────────────────────────

/// `clap_event_param_value` — carries one parameter change.  The value is in
/// the parameter's **plain** range (CLAP, unlike VST3, does not normalise), so
/// the wrapper denormalises before pushing.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClapEventParamValue {
    pub header: ClapEventHeader,
    pub param_id: u32, // clap_id
    pub cookie: *mut c_void,
    pub note_id: i32,
    pub port_index: i16,
    pub channel: i16,
    pub key: i16,
    pub _pad: i16,
    pub value: f64, // plain value
}

/// Fixed-capacity list of parameter-value events with a `clap_input_events`
/// vtable.  No allocation in `process`; reset and refilled in place each block.
pub struct FixedParamEventList {
    pub events: [ClapEventParamValue; MAX_PARAM_EVENTS],
    pub count: usize,
    pub input_events_vtable: ClapInputEvents,
}

impl FixedParamEventList {
    pub fn new() -> Box<Self> {
        let mut list = Box::new(FixedParamEventList {
            events: unsafe { std::mem::zeroed() },
            count: 0,
            input_events_vtable: ClapInputEvents {
                ctx: std::ptr::null_mut(),
                size: fpel_size,
                get: fpel_get,
            },
        });
        list.input_events_vtable.ctx = list.as_mut() as *mut FixedParamEventList as *mut c_void;
        list
    }

    #[inline]
    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// Append a parameter change (plain value).  Silently drops if full.
    pub fn push_param_value(&mut self, time: u32, param_id: u32, value: f64) {
        if self.count >= MAX_PARAM_EVENTS {
            return;
        }
        self.events[self.count] = ClapEventParamValue {
            header: ClapEventHeader {
                size: std::mem::size_of::<ClapEventParamValue>() as u32,
                time,
                space_id: CLAP_CORE_EVENT_SPACE,
                r#type: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id,
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            _pad: 0,
            value,
        };
        self.count += 1;
    }
}

unsafe extern "C" fn fpel_size(list: *const ClapInputEvents) -> u32 {
    let l = (*list).ctx as *const FixedParamEventList;
    (*l).count as u32
}

unsafe extern "C" fn fpel_get(list: *const ClapInputEvents, index: u32) -> *const ClapEventHeader {
    let l = (*list).ctx as *const FixedParamEventList;
    let l = &*l;
    if index as usize >= l.count {
        return std::ptr::null();
    }
    &l.events[index as usize].header as *const ClapEventHeader
}

// ── Mixed, time-sorted input events ────────────────────────────────────────────

/// Room for a full block of notes plus a full block of parameter events.
pub const MAX_MIXED_EVENTS: usize = MAX_EVENTS + MAX_PARAM_EVENTS;

#[repr(C)]
#[derive(Clone, Copy)]
union ClapEventSlot {
    header: ClapEventHeader,
    note: ClapEventNote,
    param: ClapEventParamValue,
    midi: ClapEventMidi,
}

/// Fixed-capacity `clap_input_events` list mixing notes, parameter values and
/// parameter modulation.  CLAP requires input events sorted by time: call
/// [`ClapEventBuffer::sort`] after pushing.  No allocation after `new`.
pub struct ClapEventBuffer {
    slots: [ClapEventSlot; MAX_MIXED_EVENTS],
    /// Slot indices in delivery order, set by `sort`.
    order: [u32; MAX_MIXED_EVENTS],
    count: usize,
    pub input_events_vtable: ClapInputEvents,
}

impl ClapEventBuffer {
    pub fn new() -> Box<Self> {
        let mut buffer = Box::new(ClapEventBuffer {
            slots: unsafe { std::mem::zeroed() },
            order: [0; MAX_MIXED_EVENTS],
            count: 0,
            input_events_vtable: ClapInputEvents {
                ctx: std::ptr::null_mut(),
                size: ceb_size,
                get: ceb_get,
            },
        });
        buffer.input_events_vtable.ctx = buffer.as_mut() as *mut ClapEventBuffer as *mut c_void;
        buffer
    }

    pub fn clear(&mut self) {
        self.count = 0;
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Drops the event when full.
    fn push(&mut self, slot: ClapEventSlot) {
        if self.count < MAX_MIXED_EVENTS {
            self.slots[self.count] = slot;
            self.order[self.count] = self.count as u32;
            self.count += 1;
        }
    }

    pub fn push_note_on(&mut self, time: u32, channel: i16, key: i16, velocity: f64) {
        self.push(ClapEventSlot {
            note: note_event(CLAP_EVENT_NOTE_ON, time, channel, key, velocity),
        });
    }

    pub fn push_note_off(&mut self, time: u32, channel: i16, key: i16) {
        self.push(ClapEventSlot {
            note: note_event(CLAP_EVENT_NOTE_OFF, time, channel, key, 0.0),
        });
    }

    /// Pitch bend, control change or aftertouch, on the main note port.
    pub fn push_midi(&mut self, time: u32, data: [u8; 3]) {
        self.push(ClapEventSlot {
            midi: ClapEventMidi {
                header: ClapEventHeader {
                    size: std::mem::size_of::<ClapEventMidi>() as u32,
                    time,
                    space_id: CLAP_CORE_EVENT_SPACE,
                    r#type: CLAP_EVENT_MIDI,
                    flags: 0,
                },
                port_index: 0,
                data,
                _pad: [0; 3],
            },
        });
    }

    /// Sets the parameter to `value` (plain units).
    pub fn push_param_value(&mut self, time: u32, param_id: u32, value: f64) {
        self.push(ClapEventSlot {
            param: param_event(CLAP_EVENT_PARAM_VALUE, time, param_id, value),
        });
    }

    /// Sets the parameter's modulation offset to `amount` (plain units).  The
    /// offset persists until the next modulation event for that parameter.
    pub fn push_param_mod(&mut self, time: u32, param_id: u32, amount: f64) {
        self.push(ClapEventSlot {
            param: param_event(CLAP_EVENT_PARAM_MOD, time, param_id, amount),
        });
    }

    /// Orders events by time; events at the same time keep their push order.
    pub fn sort(&mut self) {
        let slots = &self.slots;
        self.order[..self.count]
            .sort_unstable_by_key(|&slot| (unsafe { slots[slot as usize].header.time }, slot));
    }

    /// Event at delivery position `index` (after `sort`).
    pub fn header(&self, index: usize) -> Option<&ClapEventHeader> {
        let slot = *self.order[..self.count].get(index)?;
        Some(unsafe { &self.slots[slot as usize].header })
    }

    /// Appends the events, in delivery order, as raw CLAP bytes to `out`
    /// starting at `offset`.  Returns the new end, or `None` when `out` is too
    /// small (nothing past `offset` is then meaningful).
    pub fn write_bytes(&self, out: &mut [u8], offset: usize) -> Option<usize> {
        let mut end = offset;
        for index in 0..self.count {
            end = append_event(out, end, self.header(index)?)?;
        }
        Some(end)
    }

    /// Appends one note, MIDI or parameter event given as raw CLAP bytes
    /// (possibly unaligned).  Other event types and malformed bytes are ignored.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        let Some(header) = read_pod::<ClapEventHeader>(bytes) else {
            return;
        };
        if header.space_id != CLAP_CORE_EVENT_SPACE {
            return;
        }
        match header.r#type {
            CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF => {
                if let Some(note) = read_sized::<ClapEventNote>(bytes, &header) {
                    self.push(ClapEventSlot { note });
                }
            }
            CLAP_EVENT_MIDI => {
                if let Some(midi) = read_sized::<ClapEventMidi>(bytes, &header) {
                    self.push(ClapEventSlot { midi });
                }
            }
            CLAP_EVENT_PARAM_VALUE | CLAP_EVENT_PARAM_MOD => {
                if let Some(mut param) = read_sized::<ClapEventParamValue>(bytes, &header) {
                    param.cookie = std::ptr::null_mut();
                    self.push(ClapEventSlot { param });
                }
            }
            _ => {}
        }
    }
}

/// The raw bytes of one event (`header.size` bytes starting at `header`).
///
/// # Safety
/// `header` must head a complete event of `header.size` bytes, as CLAP
/// guarantees for events handed over by a plugin or built by this crate.
pub unsafe fn event_bytes(header: &ClapEventHeader) -> &[u8] {
    std::slice::from_raw_parts(
        header as *const ClapEventHeader as *const u8,
        header.size as usize,
    )
}

/// Appends `header`'s event to `out` at `offset`, returning the new end.
pub fn append_event(out: &mut [u8], offset: usize, header: &ClapEventHeader) -> Option<usize> {
    // SAFETY: every header reachable here heads a complete event.
    let bytes = unsafe { event_bytes(header) };
    let end = offset.checked_add(bytes.len())?;
    out.get_mut(offset..end)?.copy_from_slice(bytes);
    Some(end)
}

/// Splits a raw event stream into its events (each slice starts with a header).
pub fn split_events(mut bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        let header = read_pod::<ClapEventHeader>(bytes)?;
        let size = header.size as usize;
        if size < std::mem::size_of::<ClapEventHeader>() || size > bytes.len() {
            return None;
        }
        let (event, rest) = bytes.split_at(size);
        bytes = rest;
        Some(event)
    })
}

/// Reads a POD event struct from possibly unaligned bytes.
pub fn read_pod<T>(bytes: &[u8]) -> Option<T> {
    (bytes.len() >= std::mem::size_of::<T>())
        .then(|| unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const T) })
}

fn read_sized<T>(bytes: &[u8], header: &ClapEventHeader) -> Option<T> {
    (header.size as usize == std::mem::size_of::<T>())
        .then(|| read_pod(bytes))
        .flatten()
}

fn note_event(r#type: u16, time: u32, channel: i16, key: i16, velocity: f64) -> ClapEventNote {
    ClapEventNote {
        header: ClapEventHeader {
            size: std::mem::size_of::<ClapEventNote>() as u32,
            time,
            space_id: CLAP_CORE_EVENT_SPACE,
            r#type,
            flags: 0,
        },
        note_id: -1,
        port_index: 0,
        channel,
        key,
        _pad: 0,
        velocity,
    }
}

fn param_event(r#type: u16, time: u32, param_id: u32, value: f64) -> ClapEventParamValue {
    ClapEventParamValue {
        header: ClapEventHeader {
            size: std::mem::size_of::<ClapEventParamValue>() as u32,
            time,
            space_id: CLAP_CORE_EVENT_SPACE,
            r#type,
            flags: 0,
        },
        param_id,
        cookie: std::ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        _pad: 0,
        value,
    }
}

unsafe extern "C" fn ceb_size(list: *const ClapInputEvents) -> u32 {
    let buffer = (*list).ctx as *const ClapEventBuffer;
    (*buffer).count as u32
}

unsafe extern "C" fn ceb_get(list: *const ClapInputEvents, index: u32) -> *const ClapEventHeader {
    let buffer = &*((*list).ctx as *const ClapEventBuffer);
    buffer
        .header(index as usize)
        .map_or(std::ptr::null(), |header| header as *const ClapEventHeader)
}

// ── clap.params extension (parameter enumeration) ──────────────────────────────

/// Extension ID for `clap.params` (CLAP 1.x).
pub const CLAP_EXT_PARAMS: &[u8] = b"clap.params\0";

pub const CLAP_NAME_SIZE: usize = 256;

pub const CLAP_PARAM_IS_HIDDEN: u32 = 1 << 2;
pub const CLAP_PARAM_IS_READONLY: u32 = 1 << 3;
pub const CLAP_PARAM_IS_AUTOMATABLE: u32 = 1 << 5;
pub const CLAP_PARAM_IS_MODULATABLE: u32 = 1 << 10;
pub const CLAP_PATH_SIZE: usize = 1024;

/// `clap_param_info` — describes one parameter (subset; trailing text fields
/// kept at spec size so the struct layout matches the plugin's writes).
#[repr(C)]
pub struct ClapParamInfo {
    pub id: u32,
    pub flags: u32,
    pub cookie: *mut c_void,
    pub name: [u8; CLAP_NAME_SIZE],
    pub module: [u8; CLAP_PATH_SIZE],
    pub min_value: f64,
    pub max_value: f64,
    pub default_value: f64,
}

/// `clap_plugin_params` plugin-side vtable (method order matches CLAP 1.x).
/// We only call `count` / `get_info` (main thread, after activate) to learn
/// each parameter's plain range for denormalisation.
#[repr(C)]
pub struct ClapPluginParams {
    pub count: unsafe extern "C" fn(*const ClapPlugin) -> u32,
    pub get_info: unsafe extern "C" fn(*const ClapPlugin, u32, *mut ClapParamInfo) -> bool,
    pub get_value: unsafe extern "C" fn(*const ClapPlugin, u32, *mut f64) -> bool,
    pub value_to_text: unsafe extern "C" fn(*const ClapPlugin, u32, f64, *mut c_char, u32) -> bool,
    pub text_to_value:
        unsafe extern "C" fn(*const ClapPlugin, u32, *const c_char, *mut f64) -> bool,
    pub flush:
        unsafe extern "C" fn(*const ClapPlugin, *const ClapInputEvents, *const ClapOutputEvents),
}

const PARAM_TEXT_CAPACITY: usize = 256;

/// The plugin's own text for a plain parameter value (`-6.0 dB`), or `None`
/// when it has no `clap.params` or cannot render that value.  `[main-thread]`.
///
/// # Safety
/// `plugin` must be a live, initialised CLAP plugin, and no other thread may
/// call into it meanwhile (the instance FFI gate).
pub unsafe fn read_param_text(
    plugin: *const ClapPlugin,
    param_id: u32,
    value: f64,
) -> Option<String> {
    let extension = ((*plugin).get_extension)(plugin, CLAP_EXT_PARAMS.as_ptr() as *const c_char)
        as *const ClapPluginParams;
    let params = extension.as_ref()?;
    let mut buffer = [0 as c_char; PARAM_TEXT_CAPACITY];
    if !(params.value_to_text)(
        plugin,
        param_id,
        value,
        buffer.as_mut_ptr(),
        buffer.len() as u32,
    ) {
        return None;
    }
    buffer[PARAM_TEXT_CAPACITY - 1] = 0;
    let text = std::ffi::CStr::from_ptr(buffer.as_ptr()).to_string_lossy();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

// ── Transport event ────────────────────────────────────────────────────────────

pub const CLAP_EVENT_TRANSPORT: u16 = 9;

/// Fixed-point scale of `clap_beattime` and `clap_sectime`.
pub const CLAP_TIME_FACTOR: f64 = (1u64 << 31) as f64;

pub const CLAP_TRANSPORT_HAS_TEMPO: u32 = 1 << 0;
pub const CLAP_TRANSPORT_HAS_BEATS_TIMELINE: u32 = 1 << 1;
pub const CLAP_TRANSPORT_HAS_SECONDS_TIMELINE: u32 = 1 << 2;
pub const CLAP_TRANSPORT_HAS_TIME_SIGNATURE: u32 = 1 << 3;
pub const CLAP_TRANSPORT_IS_PLAYING: u32 = 1 << 4;

/// `clap_event_transport` — the host's musical position at the start of the block.
#[repr(C)]
pub struct ClapEventTransport {
    pub header: ClapEventHeader,
    pub flags: u32,
    pub song_pos_beats: i64,
    pub song_pos_seconds: i64,
    pub tempo: f64,
    pub tempo_inc: f64,
    pub loop_start_beats: i64,
    pub loop_end_beats: i64,
    pub loop_start_seconds: i64,
    pub loop_end_seconds: i64,
    pub bar_start: i64,
    pub bar_number: i32,
    pub tsig_num: u16,
    pub tsig_denom: u16,
}

// ── Process context ────────────────────────────────────────────────────────────

#[repr(C)]
pub struct ClapProcess {
    pub steady_time: i64,
    pub frames_count: u32,
    pub transport: *const ClapEventTransport,
    pub audio_inputs: *const ClapAudioBuffer,
    pub audio_outputs: *mut ClapAudioBuffer,
    pub audio_inputs_count: u32,
    pub audio_outputs_count: u32,
    pub in_events: *const ClapInputEvents,
    pub out_events: *const ClapOutputEvents,
}

pub type ClapProcessStatus = i32;
pub const CLAP_PROCESS_ERROR: ClapProcessStatus = 0;
pub const CLAP_PROCESS_CONTINUE: ClapProcessStatus = 1;
pub const CLAP_PROCESS_CONTINUE_IF_NOT_QUIET: ClapProcessStatus = 2;
pub const CLAP_PROCESS_TAIL: ClapProcessStatus = 3;
pub const CLAP_PROCESS_SLEEP: ClapProcessStatus = 4;

// ── GUI extension ─────────────────────────────────────────────────────────────

/// Extension ID for the clap.gui extension.
pub const CLAP_EXT_GUI: &[u8] = b"clap.gui\0";

/// Win32 API string for `ClapWindow`.
pub const CLAP_WINDOW_API_WIN32: &[u8] = b"win32\0";

/// Window handle passed to `set_parent` / `set_transient`.
/// The `specific` field carries the platform-specific handle (HWND on Windows).
#[repr(C)]
pub struct ClapWindow {
    pub api: *const c_char,
    pub specific: usize, // HWND as usize (Win32), NSView* (Cocoa), Window (X11)
}

// ── clap.latency extension (plugin → host PDC reporting) ──────────────────────

/// Extension ID for `clap.latency` (CLAP 1.x).
pub const CLAP_EXT_LATENCY: &[u8] = b"clap.latency\0";

/// `clap.latency` plugin-side vtable.
///
/// Plugin calls `get` on the main thread when activated; we read it once in
/// `prepare()` and cache the result.  CLAP spec: `get` is main-thread only,
/// not callable from `process`.  Some plugins return the value lazily — the
/// host must call `get` strictly *after* `activate()` to receive a valid
/// number.
#[repr(C)]
pub struct ClapPluginLatency {
    pub get: unsafe extern "C" fn(*const ClapPlugin) -> u32,
}

// ── clap.tail extension (how long the plugin keeps sounding after its input) ───

pub const CLAP_EXT_TAIL: &[u8] = b"clap.tail\0";

/// `clap.tail` plugin-side vtable.  `get` is callable from the main and the
/// audio thread; the host rereads it after `clap_host_tail.changed`.
#[repr(C)]
pub struct ClapPluginTail {
    pub get: unsafe extern "C" fn(*const ClapPlugin) -> u32,
}

/// The plugin's tail in samples, or `None` when it does not provide `clap.tail`.
pub unsafe fn read_tail(plugin: *const ClapPlugin) -> Option<u32> {
    let extension = ((*plugin).get_extension)(plugin, CLAP_EXT_TAIL.as_ptr() as *const c_char)
        as *const ClapPluginTail;
    extension.as_ref().map(|tail| (tail.get)(plugin))
}

// ── clap.audio-ports extension (plugin bus layout — Phase 5d, ADR-014) ────────
//
// Queried once, off the audio thread: at scan time (`plugin_formats/clap/host.rs`,
// a throwaway create → init → query → destroy cycle, SEH-guarded) to populate
// `PluginManifest.num_sidechain_inputs`. CLAP spec: `count`/`get` are
// `[main-thread]`.

/// Extension ID for `clap.audio-ports` (CLAP 1.x).
pub const CLAP_EXT_AUDIO_PORTS: &[u8] = b"clap.audio-ports\0";

/// `clap_audio_port_info_t::flags` — set on the plugin's single primary bus per
/// direction. A port without this flag is auxiliary (sidechain / aux send).
pub const CLAP_AUDIO_PORT_IS_MAIN: u32 = 1 << 0;

/// `clap_audio_port_info_t`. `name` and `port_type` are fixed host-facing
/// display data we don't currently read; `channel_count` and `flags` drive
/// the aux/main classification.
#[repr(C)]
pub struct ClapAudioPortInfo {
    pub id: u32,
    pub name: [u8; CLAP_NAME_SIZE],
    pub flags: u32,
    pub channel_count: u32,
    pub port_type: *const c_char,
    pub in_place_pair: u32,
}

/// `clap_plugin_audio_ports_t` — plugin-side vtable returned by
/// `get_extension(CLAP_EXT_AUDIO_PORTS)`.
#[repr(C)]
pub struct ClapPluginAudioPorts {
    pub count: unsafe extern "C" fn(*const ClapPlugin, bool) -> u32,
    pub get: unsafe extern "C" fn(*const ClapPlugin, u32, bool, *mut ClapAudioPortInfo) -> bool,
}

// ── clap.state extension (plugin state save/load — Phase 4c, ADR-010) ─────────
//
// The CLAP analogue of VST3's `IComponent::get/setState` (sub-phase 4b).  Unlike
// VST3 (host-implemented `IBStream` over FUnknown), CLAP streams are pure C: a
// `ctx` back-pointer plus a single read/write fn pointer — the same shape the
// event lists in this file already use (`FixedEventList`).  The host supplies a
// `Vec<u8>`-backed `ClapStateStream`; the plugin writes into it (`save`) or
// reads back from it (`load`) synchronously inside the call.
//
// Threading: `save`/`load` are `[main-thread]` in the CLAP spec.  We call them
// on the command thread (project save / open) while the transport is stopped —
// the same operating invariant documented for VST3 in `plugin_system::instance`.

/// Extension ID for `clap.state` (CLAP 1.x).
pub const CLAP_EXT_STATE: &[u8] = b"clap.state\0";

/// `clap_istream` — host-provided input byte stream the plugin reads its state
/// from during `clap_plugin_state.load`.
#[repr(C)]
pub struct ClapIStream {
    pub ctx: *mut c_void,
    /// Returns the number of bytes read; `0` = EOF, `-1` = read error.
    pub read: unsafe extern "C" fn(*const ClapIStream, *mut c_void, u64) -> i64,
}

/// `clap_ostream` — host-provided output byte stream the plugin writes its
/// state into during `clap_plugin_state.save`.
#[repr(C)]
pub struct ClapOStream {
    pub ctx: *mut c_void,
    /// Returns the number of bytes written; `-1` = write error.
    pub write: unsafe extern "C" fn(*const ClapOStream, *const c_void, u64) -> i64,
}

/// `clap_plugin_state` plugin-side vtable.  Both calls are `[main-thread]`.
#[repr(C)]
pub struct ClapPluginState {
    pub save: unsafe extern "C" fn(*const ClapPlugin, *const ClapOStream) -> bool,
    pub load: unsafe extern "C" fn(*const ClapPlugin, *const ClapIStream) -> bool,
}

/// Host-side byte buffer backing both a `clap_istream` and a `clap_ostream`.
///
/// Boxed so its address is stable: the `ctx` field of each embedded stream
/// vtable points back at this struct, exactly like `FixedEventList`.  Lives for
/// the duration of a single save/load call and is dropped immediately after.
pub struct ClapStateStream {
    data: Vec<u8>,
    pos: usize,
    istream: ClapIStream,
    ostream: ClapOStream,
}

unsafe extern "C" fn clap_istream_read(
    stream: *const ClapIStream,
    buffer: *mut c_void,
    size: u64,
) -> i64 {
    if stream.is_null() {
        return -1;
    }
    let s = (*stream).ctx as *mut ClapStateStream;
    if s.is_null() || (buffer.is_null() && size != 0) {
        return -1;
    }
    let s = &mut *s;
    let want = size as usize;
    let avail = s.data.len().saturating_sub(s.pos);
    let n = want.min(avail);
    if n > 0 {
        std::ptr::copy_nonoverlapping(s.data.as_ptr().add(s.pos), buffer as *mut u8, n);
        s.pos += n;
    }
    n as i64 // 0 here naturally signals EOF
}

unsafe extern "C" fn clap_ostream_write(
    stream: *const ClapOStream,
    buffer: *const c_void,
    size: u64,
) -> i64 {
    if stream.is_null() {
        return -1;
    }
    let s = (*stream).ctx as *mut ClapStateStream;
    if s.is_null() || (buffer.is_null() && size != 0) {
        return -1;
    }
    let s = &mut *s;
    let n = size as usize;
    if n > 0 {
        // Overwrite/extend at the current position.
        if s.pos + n > s.data.len() {
            s.data.resize(s.pos + n, 0);
        }
        std::ptr::copy_nonoverlapping(buffer as *const u8, s.data.as_mut_ptr().add(s.pos), n);
        s.pos += n;
    }
    n as i64
}

impl ClapStateStream {
    fn build(data: Vec<u8>) -> Box<Self> {
        let mut s = Box::new(ClapStateStream {
            data,
            pos: 0,
            istream: ClapIStream {
                ctx: std::ptr::null_mut(),
                read: clap_istream_read,
            },
            ostream: ClapOStream {
                ctx: std::ptr::null_mut(),
                write: clap_ostream_write,
            },
        });
        // Point both vtable ctx fields back at the (now heap-stable) struct.
        let self_ptr = s.as_mut() as *mut ClapStateStream as *mut c_void;
        s.istream.ctx = self_ptr;
        s.ostream.ctx = self_ptr;
        s
    }
    fn empty() -> Box<Self> {
        Self::build(Vec::new())
    }
    fn with_data(data: Vec<u8>) -> Box<Self> {
        Self::build(data)
    }
}

/// Serialise a CLAP plugin's state into a byte buffer via the `clap.state`
/// extension.  Returns `None` if the plugin exposes no `clap.state`, reports
/// failure, or writes no bytes.
///
/// # Safety
/// `plugin` must be a valid, initialised `ClapPlugin`.  Call off the audio
/// thread (see the threading caveat in `plugin_system::instance`).
pub unsafe fn read_plugin_state(plugin: *const ClapPlugin) -> Option<Vec<u8>> {
    if plugin.is_null() {
        return None;
    }
    let ext = ((*plugin).get_extension)(plugin, CLAP_EXT_STATE.as_ptr() as *const c_char)
        as *const ClapPluginState;
    if ext.is_null() {
        return None;
    }

    let mut stream = ClapStateStream::empty();
    let os = &stream.ostream as *const ClapOStream;
    let ok = ((*ext).save)(plugin, os);
    if !ok {
        return None;
    }

    let data = std::mem::take(&mut stream.data);
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

/// Restore a previously-saved blob into a CLAP plugin via `clap.state.load`.
/// Returns `true` on success, `false` if unsupported or the plugin rejected it.
///
/// # Safety
/// As [`read_plugin_state`].
pub unsafe fn write_plugin_state(plugin: *const ClapPlugin, bytes: &[u8]) -> bool {
    if plugin.is_null() {
        return false;
    }
    let ext = ((*plugin).get_extension)(plugin, CLAP_EXT_STATE.as_ptr() as *const c_char)
        as *const ClapPluginState;
    if ext.is_null() {
        return false;
    }

    let mut stream = ClapStateStream::with_data(bytes.to_vec());
    let is = &stream.istream as *const ClapIStream;
    ((*ext).load)(plugin, is)
}

// ── Host-side extensions (host_get_extension can return these) ────────────────

pub const CLAP_EXT_LOG: &[u8] = b"clap.log\0";
pub const CLAP_EXT_HOST_GUI: &[u8] = b"clap.gui\0";

pub type ClapLogSeverity = i32;
pub const CLAP_LOG_DEBUG: ClapLogSeverity = 0;
pub const CLAP_LOG_INFO: ClapLogSeverity = 1;
pub const CLAP_LOG_WARNING: ClapLogSeverity = 2;
pub const CLAP_LOG_ERROR: ClapLogSeverity = 3;
pub const CLAP_LOG_FATAL: ClapLogSeverity = 4;
pub const CLAP_LOG_HOST_MISBEHAVING: ClapLogSeverity = 5;
pub const CLAP_LOG_PLUGIN_MISBEHAVING: ClapLogSeverity = 6;

#[repr(C)]
pub struct ClapHostLog {
    pub log: unsafe extern "C" fn(*const ClapHost, ClapLogSeverity, *const c_char),
}

/// Host-side `clap.gui` extension — lets the plugin request resize/show/hide
/// and notify the host that its GUI was closed by the user.
#[repr(C)]
pub struct ClapHostGui {
    pub resize_hints_changed: unsafe extern "C" fn(*const ClapHost),
    pub request_resize: unsafe extern "C" fn(*const ClapHost, u32, u32) -> bool,
    pub request_show: unsafe extern "C" fn(*const ClapHost) -> bool,
    pub request_hide: unsafe extern "C" fn(*const ClapHost) -> bool,
    pub closed: unsafe extern "C" fn(*const ClapHost, bool),
}

/// `clap.gui` extension vtable (method order matches CLAP 1.x spec).
#[repr(C)]
pub struct ClapGuiExtension {
    pub is_api_supported: unsafe extern "C" fn(*const ClapPlugin, *const c_char, bool) -> bool,
    pub get_preferred_api:
        unsafe extern "C" fn(*const ClapPlugin, *mut *const c_char, *mut bool) -> bool,
    pub create: unsafe extern "C" fn(*const ClapPlugin, *const c_char, bool) -> bool,
    pub destroy: unsafe extern "C" fn(*const ClapPlugin),
    pub set_scale: unsafe extern "C" fn(*const ClapPlugin, f64) -> bool,
    pub get_size: unsafe extern "C" fn(*const ClapPlugin, *mut u32, *mut u32) -> bool,
    pub can_resize: unsafe extern "C" fn(*const ClapPlugin) -> bool,
    pub get_resize_hints: unsafe extern "C" fn(*const ClapPlugin, *mut c_void) -> bool,
    pub adjust_size: unsafe extern "C" fn(*const ClapPlugin, *mut u32, *mut u32) -> bool,
    pub set_size: unsafe extern "C" fn(*const ClapPlugin, u32, u32) -> bool,
    pub set_parent: unsafe extern "C" fn(*const ClapPlugin, *const ClapWindow) -> bool,
    pub set_transient: unsafe extern "C" fn(*const ClapPlugin, *const ClapWindow) -> bool,
    pub suggest_title: unsafe extern "C" fn(*const ClapPlugin, *const c_char),
    pub show: unsafe extern "C" fn(*const ClapPlugin) -> bool,
    pub hide: unsafe extern "C" fn(*const ClapPlugin) -> bool,
}

// ── Loaded library ─────────────────────────────────────────────────────────────

#[cfg(windows)]
mod win32 {
    #[link(name = "kernel32")]
    extern "system" {
        pub fn LoadLibraryW(lpLibFileName: *const u16) -> *mut std::ffi::c_void;
        pub fn FreeLibrary(hLibModule: *mut std::ffi::c_void) -> i32;
        pub fn GetProcAddress(
            hModule: *mut std::ffi::c_void,
            lpProcName: *const u8,
        ) -> Option<unsafe extern "system" fn() -> isize>;
    }
}

#[cfg(windows)]
pub struct ClapLibrary {
    handle: *mut c_void,
    entry: *const ClapPluginEntry,
    init_called: std::sync::atomic::AtomicBool,
    pub path: std::path::PathBuf,
}

#[cfg(windows)]
unsafe impl Send for ClapLibrary {}
#[cfg(windows)]
unsafe impl Sync for ClapLibrary {}

// Process-wide strong cache.  CLAP spec: clap_entry.init() must be called
// exactly once before any other use of the DLL.  Re-calling it (even after a
// matching deinit) is undefined and crashes plugins that keep state in
// DllMain/static initialisers (Vital, Surge, many JUCE-based CLAPs).
//
// To honour "exactly once per process per DLL" without coordination at every
// call site, we cache loaded libraries strongly.  Once a `.clap` is loaded it
// stays for the process lifetime; deinit() only ever runs at process exit.
// This mirrors the Bitwig / clap-host reference behaviour.
#[cfg(windows)]
static CLAP_LIBRARIES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, std::sync::Arc<ClapLibrary>>>,
> = std::sync::OnceLock::new();

#[cfg(windows)]
impl ClapLibrary {
    /// Load a CLAP DLL, calling `clap_entry.init` exactly once per process per
    /// canonical path.  Subsequent `load()` calls for the same path return the
    /// cached `Arc` without re-running `init()`.
    pub fn load(dll_path: &Path) -> Result<(std::sync::Arc<Self>, *const ClapPluginEntry), String> {
        use std::sync::atomic::AtomicBool;

        let cache =
            CLAP_LIBRARIES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
        let canonical = dll_path
            .canonicalize()
            .unwrap_or_else(|_| dll_path.to_path_buf());

        // Serialize the whole load: cache lookup + LoadLibrary + init.  Held
        // for the duration of plugin init (can be seconds), but avoids the
        // double-init race that would crash strict plugins.
        let mut map = cache.lock().map_err(|e| e.to_string())?;
        if let Some(lib) = map.get(&canonical) {
            return Ok((std::sync::Arc::clone(lib), lib.entry));
        }

        let wide: Vec<u16> = dll_path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe { win32::LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() {
            return Err(format!("LoadLibraryW failed for {}", dll_path.display()));
        }

        let sym = unsafe { win32::GetProcAddress(handle, b"clap_entry\0".as_ptr()) };
        let sym = match sym {
            Some(s) => s,
            None => {
                unsafe { win32::FreeLibrary(handle) };
                return Err(format!("clap_entry not found in {}", dll_path.display()));
            }
        };

        let entry: *const ClapPluginEntry = unsafe { std::mem::transmute(sym) };

        let path_cstr: Vec<u8> = dll_path
            .to_string_lossy()
            .bytes()
            .chain(std::iter::once(0))
            .collect();
        let init_ok = unsafe { ((*entry).init)(path_cstr.as_ptr() as *const c_char) };
        if !init_ok {
            unsafe { win32::FreeLibrary(handle) };
            return Err(format!(
                "clap_entry.init returned false for {}",
                dll_path.display()
            ));
        }

        let lib = std::sync::Arc::new(ClapLibrary {
            handle,
            entry,
            init_called: AtomicBool::new(true),
            path: canonical.clone(),
        });
        map.insert(canonical, std::sync::Arc::clone(&lib));
        Ok((lib, entry))
    }
}

#[cfg(windows)]
impl Drop for ClapLibrary {
    fn drop(&mut self) {
        // Only fires at process exit because the cache holds strong refs.
        // Still pair init/deinit correctly so future cache eviction would work.
        if self.init_called.load(std::sync::atomic::Ordering::Acquire) {
            unsafe { ((*self.entry).deinit)() };
        }
        unsafe { win32::FreeLibrary(self.handle) };
    }
}

#[cfg(not(windows))]
pub struct ClapLibrary {
    pub path: std::path::PathBuf,
}
#[cfg(not(windows))]
impl ClapLibrary {
    pub fn load(_: &Path) -> Result<(std::sync::Arc<Self>, *const ClapPluginEntry), String> {
        Err("CLAP only supported on Windows currently".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercises the host-side `ClapStateStream` the way a plugin's
    // `clap.state.save`/`load` would: write bytes through the ostream vtable,
    // then read them straight back through the istream vtable.  No real plugin
    // required — validates the stream plumbing read/write/EOF semantics.
    #[test]
    fn clap_state_stream_write_then_read_roundtrips() {
        let payload: Vec<u8> = (0u8..200).collect();

        // ── save: plugin writes into the ostream (two chunks) ──
        let mut out = ClapStateStream::empty();
        let os = &out.ostream as *const ClapOStream;
        unsafe {
            let w1 = (out.ostream.write)(os, payload.as_ptr() as *const c_void, 128);
            let w2 = (out.ostream.write)(
                os,
                payload[128..].as_ptr() as *const c_void,
                (payload.len() - 128) as u64,
            );
            assert_eq!(w1, 128);
            assert_eq!(w2, (payload.len() - 128) as i64);
        }
        let saved = std::mem::take(&mut out.data);
        assert_eq!(saved, payload);

        // ── load: plugin reads back from the istream ──
        let inp = ClapStateStream::with_data(saved.clone());
        let is = &inp.istream as *const ClapIStream;
        let mut buf = vec![0u8; saved.len()];
        unsafe {
            let r = (inp.istream.read)(is, buf.as_mut_ptr() as *mut c_void, saved.len() as u64);
            assert_eq!(r, saved.len() as i64);
            // Past the end → 0 bytes (EOF), not an error.
            let eof = (inp.istream.read)(is, buf.as_mut_ptr() as *mut c_void, 16);
            assert_eq!(eof, 0);
        }
        assert_eq!(buf, payload);
    }

    // A zero-length save must surface as `None` from `read_plugin_state`'s
    // caller contract; here we assert the stream itself stays empty so the
    // helper's `is_empty()` check fires.
    #[test]
    fn clap_state_stream_empty_save_is_empty() {
        let out = ClapStateStream::empty();
        assert!(out.data.is_empty());
    }
}

#[cfg(test)]
mod event_buffer_tests {
    use super::*;

    fn delivered(buffer: &ClapEventBuffer) -> Vec<(u32, u16)> {
        let list = &buffer.input_events_vtable as *const ClapInputEvents;
        unsafe {
            (0..(buffer.input_events_vtable.size)(list))
                .map(|i| {
                    let header = &*(buffer.input_events_vtable.get)(list, i);
                    (header.time, header.r#type)
                })
                .collect()
        }
    }

    #[test]
    fn events_are_delivered_sorted_by_time_keeping_push_order() {
        let mut buffer = ClapEventBuffer::new();
        buffer.push_param_mod(64, 1, 0.5);
        buffer.push_note_off(32, 0, 60);
        buffer.push_note_on(32, 0, 62, 1.0);
        buffer.push_param_value(0, 1, 0.25);
        buffer.sort();

        assert_eq!(
            delivered(&buffer),
            vec![
                (0, CLAP_EVENT_PARAM_VALUE),
                (32, CLAP_EVENT_NOTE_OFF),
                (32, CLAP_EVENT_NOTE_ON),
                (64, CLAP_EVENT_PARAM_MOD),
            ]
        );
    }

    #[test]
    fn param_mod_carries_its_amount() {
        let mut buffer = ClapEventBuffer::new();
        buffer.push_param_mod(0, 9, -3.5);
        buffer.sort();
        let header = buffer.header(0).unwrap();
        let event = unsafe { &*(header as *const ClapEventHeader as *const ClapEventParamValue) };
        assert_eq!(
            header.size as usize,
            std::mem::size_of::<ClapEventParamValue>()
        );
        assert_eq!((event.param_id, event.value), (9, -3.5));
    }

    #[test]
    fn events_survive_a_raw_byte_roundtrip() {
        let mut buffer = ClapEventBuffer::new();
        buffer.push_param_mod(64, 1, 0.5);
        buffer.push_note_on(32, 0, 62, 1.0);
        buffer.push_midi(16, [0xE0, 0x00, 0x60]);
        buffer.push_param_value(0, 1, 0.25);
        buffer.sort();

        let mut bytes = vec![0u8; 4096];
        let end = buffer.write_bytes(&mut bytes, 3).unwrap();
        let mut copy = ClapEventBuffer::new();
        for event in split_events(&bytes[3..end]) {
            copy.push_bytes(event);
        }
        copy.sort();
        assert_eq!(delivered(&copy), delivered(&buffer));
    }

    #[test]
    fn midi_event_matches_the_c_layout_and_keeps_its_bytes() {
        assert_eq!(std::mem::size_of::<ClapEventMidi>(), 24);
        let mut buffer = ClapEventBuffer::new();
        buffer.push_midi(7, [0xB0, 64, 127]);
        buffer.sort();
        let header = buffer.header(0).unwrap();
        let event = unsafe { &*(header as *const ClapEventHeader as *const ClapEventMidi) };
        assert_eq!((header.r#type, header.time), (CLAP_EVENT_MIDI, 7));
        assert_eq!(event.data, [0xB0, 64, 127]);
    }

    #[test]
    fn write_bytes_refuses_a_too_small_buffer() {
        let mut buffer = ClapEventBuffer::new();
        buffer.push_param_value(0, 1, 0.25);
        let mut bytes = [0u8; 8];
        assert_eq!(buffer.write_bytes(&mut bytes, 0), None);
    }

    #[test]
    fn split_events_stops_at_a_truncated_event() {
        let mut buffer = ClapEventBuffer::new();
        buffer.push_param_value(0, 1, 0.25);
        let mut bytes = vec![0u8; 256];
        let end = buffer.write_bytes(&mut bytes, 0).unwrap();
        assert_eq!(split_events(&bytes[..end - 1]).count(), 0);
        assert_eq!(split_events(&bytes[..end]).count(), 1);
    }

    #[test]
    fn clear_empties_and_full_buffer_drops_extra_events() {
        let mut buffer = ClapEventBuffer::new();
        for i in 0..(MAX_MIXED_EVENTS + 5) {
            buffer.push_param_value(i as u32, 0, 0.0);
        }
        assert_eq!(buffer.len(), MAX_MIXED_EVENTS);
        buffer.clear();
        assert!(buffer.is_empty());
        assert!(buffer.header(0).is_none());
    }
}
