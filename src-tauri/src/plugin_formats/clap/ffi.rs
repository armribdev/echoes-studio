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
    pub path: std::path::PathBuf,
}

#[cfg(windows)]
unsafe impl Send for ClapLibrary {}
#[cfg(windows)]
unsafe impl Sync for ClapLibrary {}

#[cfg(windows)]
impl ClapLibrary {
    pub fn load(dll_path: &Path) -> Result<(std::sync::Arc<Self>, *const ClapPluginEntry), String> {
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
        let lib = std::sync::Arc::new(ClapLibrary { handle, path: dll_path.to_path_buf() });
        Ok((lib, entry))
    }
}

#[cfg(windows)]
impl Drop for ClapLibrary {
    fn drop(&mut self) {
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
