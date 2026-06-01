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

use std::ffi::{c_char, c_void};
use std::path::Path;

// ── Version ────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClapVersion {
    pub major:    u32,
    pub minor:    u32,
    pub revision: u32,
}

pub const CLAP_VERSION_1: ClapVersion = ClapVersion { major: 1, minor: 0, revision: 0 };

// ── Entry point ────────────────────────────────────────────────────────────────

pub const CLAP_PLUGIN_FACTORY_ID: &[u8] = b"clap.plugin-factory\0";

#[repr(C)]
pub struct ClapPluginEntry {
    pub clap_version: ClapVersion,
    pub init:         unsafe extern "C" fn(plugin_path: *const c_char) -> bool,
    pub deinit:       unsafe extern "C" fn(),
    pub get_factory:  unsafe extern "C" fn(factory_id: *const c_char) -> *const c_void,
}

// ── Plugin descriptor ──────────────────────────────────────────────────────────

#[repr(C)]
pub struct ClapPluginDescriptor {
    pub clap_version: ClapVersion,
    pub id:           *const c_char,
    pub name:         *const c_char,
    pub vendor:       *const c_char,
    pub url:          *const c_char,
    pub manual_url:   *const c_char,
    pub support_url:  *const c_char,
    pub version:      *const c_char,
    pub description:  *const c_char,
    pub features:     *const *const c_char,
}

impl ClapPluginDescriptor {
    pub fn id_str(&self) -> &str {
        if self.id.is_null() { return ""; }
        unsafe { std::ffi::CStr::from_ptr(self.id) }.to_str().unwrap_or("")
    }
    pub fn name_str(&self) -> &str {
        if self.name.is_null() { return ""; }
        unsafe { std::ffi::CStr::from_ptr(self.name) }.to_str().unwrap_or("")
    }
    pub fn vendor_str(&self) -> &str {
        if self.vendor.is_null() { return ""; }
        unsafe { std::ffi::CStr::from_ptr(self.vendor) }.to_str().unwrap_or("")
    }
}

// ── Factory ────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct ClapPluginFactory {
    pub get_plugin_count:      unsafe extern "C" fn(*const ClapPluginFactory) -> u32,
    pub get_plugin_descriptor: unsafe extern "C" fn(
        *const ClapPluginFactory,
        u32,
    ) -> *const ClapPluginDescriptor,
    pub create_plugin:         unsafe extern "C" fn(
        *const ClapPluginFactory,
        *const ClapHost,
        *const c_char,
    ) -> *const ClapPlugin,
}

// ── Host struct (provided by Echoes Studio to plugins) ─────────────────────────

#[repr(C)]
pub struct ClapHost {
    pub clap_version: ClapVersion,
    pub host_data:    *mut c_void,
    pub name:         *const c_char,
    pub vendor:       *const c_char,
    pub url:          *const c_char,
    pub version:      *const c_char,
    pub request_restart:  unsafe extern "C" fn(*const ClapHost),
    pub request_process:  unsafe extern "C" fn(*const ClapHost),
    pub request_callback: unsafe extern "C" fn(*const ClapHost),
    pub get_extension:    unsafe extern "C" fn(*const ClapHost, *const c_char) -> *const c_void,
}

// No-op host callbacks — extensions not yet implemented.
pub unsafe extern "C" fn host_request_restart(_: *const ClapHost)  {}
pub unsafe extern "C" fn host_request_process(_: *const ClapHost)  {}
pub unsafe extern "C" fn host_request_callback(_: *const ClapHost) {}
pub unsafe extern "C" fn host_get_extension(_: *const ClapHost, _: *const c_char) -> *const c_void {
    std::ptr::null()
}

// Constant C strings for the host descriptor.
static HOST_NAME:    &[u8] = b"Echoes Studio\0";
static HOST_VENDOR:  &[u8] = b"Echoes Studio\0";
static HOST_URL:     &[u8] = b"https://github.com/echoes-studio/echoes-studio\0";
static HOST_VERSION: &[u8] = b"0.1.0\0";

/// Build a stable ClapHost struct (lives on the heap, pointer-stable).
pub fn make_host() -> Box<ClapHost> {
    Box::new(ClapHost {
        clap_version:     CLAP_VERSION_1,
        host_data:        std::ptr::null_mut(),
        name:             HOST_NAME.as_ptr() as *const c_char,
        vendor:           HOST_VENDOR.as_ptr() as *const c_char,
        url:              HOST_URL.as_ptr() as *const c_char,
        version:          HOST_VERSION.as_ptr() as *const c_char,
        request_restart:  host_request_restart,
        request_process:  host_request_process,
        request_callback: host_request_callback,
        get_extension:    host_get_extension,
    })
}

// ── Plugin instance ────────────────────────────────────────────────────────────

#[repr(C)]
pub struct ClapPlugin {
    pub desc:        *const ClapPluginDescriptor,
    pub plugin_data: *mut c_void,
    pub init:     unsafe extern "C" fn(*const ClapPlugin) -> bool,
    pub destroy:  unsafe extern "C" fn(*const ClapPlugin),
    pub activate: unsafe extern "C" fn(
        *const ClapPlugin, f64, u32, u32,
    ) -> bool,
    pub deactivate:       unsafe extern "C" fn(*const ClapPlugin),
    pub start_processing: unsafe extern "C" fn(*const ClapPlugin) -> bool,
    pub stop_processing:  unsafe extern "C" fn(*const ClapPlugin),
    pub reset:            unsafe extern "C" fn(*const ClapPlugin),
    pub process: unsafe extern "C" fn(
        *const ClapPlugin, *const ClapProcess,
    ) -> ClapProcessStatus,
    pub get_extension: unsafe extern "C" fn(
        *const ClapPlugin, *const c_char,
    ) -> *const c_void,
    pub on_main_thread: unsafe extern "C" fn(*const ClapPlugin),
}

// ── Audio buffer ───────────────────────────────────────────────────────────────

#[repr(C)]
pub struct ClapAudioBuffer {
    pub data32:         *mut *mut f32,
    pub data64:         *mut *mut f64,
    pub channel_count:  u32,
    pub latency:        u32,
    pub constant_mask:  u64,
}

// ── Events ─────────────────────────────────────────────────────────────────────

pub const CLAP_EVENT_NOTE_ON:  u16 = 0;
pub const CLAP_EVENT_NOTE_OFF: u16 = 1;
/// `CLAP_EVENT_PARAM_VALUE` — host → plugin parameter change (ADR-007).
pub const CLAP_EVENT_PARAM_VALUE: u16 = 5;
pub const CLAP_CORE_EVENT_SPACE: u16 = 0;

#[repr(C)]
pub struct ClapEventHeader {
    pub size:     u32,
    pub time:     u32,   // frame offset within current block
    pub space_id: u16,   // 0 = CLAP core
    pub r#type:   u16,
    pub flags:    u32,
}

#[repr(C)]
pub struct ClapEventNote {
    pub header:     ClapEventHeader,
    pub note_id:    i32,    // -1 if unused
    pub port_index: i16,
    pub channel:    i16,
    pub key:        i16,    // 0..127
    pub _pad:       i16,
    pub velocity:   f64,    // 0..1
}

// Fixed-capacity event list (no allocation in process).
pub const MAX_EVENTS: usize = 256;

#[repr(C)]
pub struct ClapInputEvents {
    pub ctx:  *mut c_void,
    pub size: unsafe extern "C" fn(*const ClapInputEvents) -> u32,
    pub get:  unsafe extern "C" fn(*const ClapInputEvents, u32) -> *const ClapEventHeader,
}

#[repr(C)]
pub struct ClapOutputEvents {
    pub ctx:     *mut c_void,
    pub try_push: unsafe extern "C" fn(*const ClapOutputEvents, *const ClapEventHeader) -> bool,
}

/// Fixed-size MIDI event list for passing notes into CLAP instruments.
pub struct FixedEventList {
    pub events: [ClapEventNote; MAX_EVENTS],
    pub count:  usize,
    pub input_events_vtable: ClapInputEvents,
}

impl FixedEventList {
    pub fn new() -> Box<Self> {
        let mut list = Box::new(FixedEventList {
            events: unsafe { std::mem::zeroed() },
            count: 0,
            input_events_vtable: ClapInputEvents {
                ctx:  std::ptr::null_mut(),
                size: fel_size,
                get:  fel_get,
            },
        });
        // Point ctx back to self so the vtable callbacks can reach the data.
        list.input_events_vtable.ctx = list.as_mut() as *mut FixedEventList as *mut c_void;
        list
    }

    pub fn clear(&mut self) { self.count = 0; }

    pub fn push_note_on(&mut self, time: u32, channel: i16, key: i16, velocity: f64) {
        if self.count >= MAX_EVENTS { return; }
        self.events[self.count] = ClapEventNote {
            header: ClapEventHeader {
                size:     std::mem::size_of::<ClapEventNote>() as u32,
                time,
                space_id: CLAP_CORE_EVENT_SPACE,
                r#type:   CLAP_EVENT_NOTE_ON,
                flags:    0,
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
        if self.count >= MAX_EVENTS { return; }
        self.events[self.count] = ClapEventNote {
            header: ClapEventHeader {
                size:     std::mem::size_of::<ClapEventNote>() as u32,
                time,
                space_id: CLAP_CORE_EVENT_SPACE,
                r#type:   CLAP_EVENT_NOTE_OFF,
                flags:    0,
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
pub struct ClapEventParamValue {
    pub header:     ClapEventHeader,
    pub param_id:   u32,    // clap_id
    pub cookie:     *mut c_void,
    pub note_id:    i32,
    pub port_index: i16,
    pub channel:    i16,
    pub key:        i16,
    pub _pad:       i16,
    pub value:      f64,    // plain value
}

/// Fixed-capacity list of parameter-value events with a `clap_input_events`
/// vtable.  No allocation in `process`; reset and refilled in place each block.
pub struct FixedParamEventList {
    pub events: [ClapEventParamValue; MAX_EVENTS],
    pub count:  usize,
    pub input_events_vtable: ClapInputEvents,
}

impl FixedParamEventList {
    pub fn new() -> Box<Self> {
        let mut list = Box::new(FixedParamEventList {
            events: unsafe { std::mem::zeroed() },
            count: 0,
            input_events_vtable: ClapInputEvents {
                ctx:  std::ptr::null_mut(),
                size: fpel_size,
                get:  fpel_get,
            },
        });
        list.input_events_vtable.ctx = list.as_mut() as *mut FixedParamEventList as *mut c_void;
        list
    }

    #[inline]
    pub fn clear(&mut self) { self.count = 0; }

    /// Append a parameter change (plain value).  Silently drops if full.
    pub fn push_param_value(&mut self, time: u32, param_id: u32, value: f64) {
        if self.count >= MAX_EVENTS { return; }
        self.events[self.count] = ClapEventParamValue {
            header: ClapEventHeader {
                size:     std::mem::size_of::<ClapEventParamValue>() as u32,
                time,
                space_id: CLAP_CORE_EVENT_SPACE,
                r#type:   CLAP_EVENT_PARAM_VALUE,
                flags:    0,
            },
            param_id,
            cookie:     std::ptr::null_mut(),
            note_id:    -1,
            port_index: -1,
            channel:    -1,
            key:        -1,
            _pad:       0,
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

// ── clap.params extension (parameter enumeration) ──────────────────────────────

/// Extension ID for `clap.params` (CLAP 1.x).
pub const CLAP_EXT_PARAMS: &[u8] = b"clap.params\0";

pub const CLAP_NAME_SIZE: usize = 256;
pub const CLAP_PATH_SIZE: usize = 1024;

/// `clap_param_info` — describes one parameter (subset; trailing text fields
/// kept at spec size so the struct layout matches the plugin's writes).
#[repr(C)]
pub struct ClapParamInfo {
    pub id:            u32,
    pub flags:         u32,
    pub cookie:        *mut c_void,
    pub name:          [u8; CLAP_NAME_SIZE],
    pub module:        [u8; CLAP_PATH_SIZE],
    pub min_value:     f64,
    pub max_value:     f64,
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
    pub value_to_text:
        unsafe extern "C" fn(*const ClapPlugin, u32, f64, *mut c_char, u32) -> bool,
    pub text_to_value:
        unsafe extern "C" fn(*const ClapPlugin, u32, *const c_char, *mut f64) -> bool,
    pub flush: unsafe extern "C" fn(*const ClapPlugin, *const ClapInputEvents, *const ClapOutputEvents),
}

// ── Process context ────────────────────────────────────────────────────────────

#[repr(C)]
pub struct ClapProcess {
    pub steady_time:        i64,
    pub frames_count:       u32,
    pub transport:          *const c_void,
    pub audio_inputs:       *const ClapAudioBuffer,
    pub audio_outputs:      *mut ClapAudioBuffer,
    pub audio_inputs_count: u32,
    pub audio_outputs_count: u32,
    pub in_events:          *const ClapInputEvents,
    pub out_events:         *const ClapOutputEvents,
}

pub type ClapProcessStatus = i32;
pub const CLAP_PROCESS_ERROR:    ClapProcessStatus = 0;
pub const CLAP_PROCESS_CONTINUE: ClapProcessStatus = 1;

// ── GUI extension ─────────────────────────────────────────────────────────────

/// Extension ID for the clap.gui extension.
pub const CLAP_EXT_GUI: &[u8] = b"clap.gui\0";

/// Win32 API string for `ClapWindow`.
pub const CLAP_WINDOW_API_WIN32: &[u8] = b"win32\0";

/// Window handle passed to `set_parent` / `set_transient`.
/// The `specific` field carries the platform-specific handle (HWND on Windows).
#[repr(C)]
pub struct ClapWindow {
    pub api:      *const c_char,
    pub specific: usize,  // HWND as usize (Win32), NSView* (Cocoa), Window (X11)
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

// ── Host-side extensions (host_get_extension can return these) ────────────────

pub const CLAP_EXT_LOG:      &[u8] = b"clap.log\0";
pub const CLAP_EXT_HOST_GUI: &[u8] = b"clap.gui\0";

pub type ClapLogSeverity = i32;
pub const CLAP_LOG_DEBUG:               ClapLogSeverity = 0;
pub const CLAP_LOG_INFO:                ClapLogSeverity = 1;
pub const CLAP_LOG_WARNING:             ClapLogSeverity = 2;
pub const CLAP_LOG_ERROR:               ClapLogSeverity = 3;
pub const CLAP_LOG_FATAL:               ClapLogSeverity = 4;
pub const CLAP_LOG_HOST_MISBEHAVING:    ClapLogSeverity = 5;
pub const CLAP_LOG_PLUGIN_MISBEHAVING:  ClapLogSeverity = 6;

#[repr(C)]
pub struct ClapHostLog {
    pub log: unsafe extern "C" fn(*const ClapHost, ClapLogSeverity, *const c_char),
}

/// Host-side `clap.gui` extension — lets the plugin request resize/show/hide
/// and notify the host that its GUI was closed by the user.
#[repr(C)]
pub struct ClapHostGui {
    pub resize_hints_changed: unsafe extern "C" fn(*const ClapHost),
    pub request_resize:       unsafe extern "C" fn(*const ClapHost, u32, u32) -> bool,
    pub request_show:         unsafe extern "C" fn(*const ClapHost) -> bool,
    pub request_hide:         unsafe extern "C" fn(*const ClapHost) -> bool,
    pub closed:               unsafe extern "C" fn(*const ClapHost, bool),
}

/// `clap.gui` extension vtable (method order matches CLAP 1.x spec).
#[repr(C)]
pub struct ClapGuiExtension {
    pub is_api_supported:
        unsafe extern "C" fn(*const ClapPlugin, *const c_char, bool) -> bool,
    pub get_preferred_api:
        unsafe extern "C" fn(*const ClapPlugin, *mut *const c_char, *mut bool) -> bool,
    pub create:
        unsafe extern "C" fn(*const ClapPlugin, *const c_char, bool) -> bool,
    pub destroy:
        unsafe extern "C" fn(*const ClapPlugin),
    pub set_scale:
        unsafe extern "C" fn(*const ClapPlugin, f64) -> bool,
    pub get_size:
        unsafe extern "C" fn(*const ClapPlugin, *mut u32, *mut u32) -> bool,
    pub can_resize:
        unsafe extern "C" fn(*const ClapPlugin) -> bool,
    pub get_resize_hints:
        unsafe extern "C" fn(*const ClapPlugin, *mut c_void) -> bool,
    pub adjust_size:
        unsafe extern "C" fn(*const ClapPlugin, *mut u32, *mut u32) -> bool,
    pub set_size:
        unsafe extern "C" fn(*const ClapPlugin, u32, u32) -> bool,
    pub set_parent:
        unsafe extern "C" fn(*const ClapPlugin, *const ClapWindow) -> bool,
    pub set_transient:
        unsafe extern "C" fn(*const ClapPlugin, *const ClapWindow) -> bool,
    pub suggest_title:
        unsafe extern "C" fn(*const ClapPlugin, *const c_char),
    pub show:
        unsafe extern "C" fn(*const ClapPlugin) -> bool,
    pub hide:
        unsafe extern "C" fn(*const ClapPlugin) -> bool,
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
    handle:      *mut c_void,
    entry:       *const ClapPluginEntry,
    init_called: std::sync::atomic::AtomicBool,
    pub path:    std::path::PathBuf,
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

        let cache = CLAP_LIBRARIES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
        let canonical = dll_path.canonicalize().unwrap_or_else(|_| dll_path.to_path_buf());

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

        let path_cstr: Vec<u8> = dll_path.to_string_lossy()
            .bytes().chain(std::iter::once(0)).collect();
        let init_ok = unsafe { ((*entry).init)(path_cstr.as_ptr() as *const c_char) };
        if !init_ok {
            unsafe { win32::FreeLibrary(handle) };
            return Err(format!("clap_entry.init returned false for {}", dll_path.display()));
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
pub struct ClapLibrary { pub path: std::path::PathBuf }
#[cfg(not(windows))]
impl ClapLibrary {
    pub fn load(_: &Path) -> Result<(std::sync::Arc<Self>, *const ClapPluginEntry), String> {
        Err("CLAP only supported on Windows currently".into())
    }
}
