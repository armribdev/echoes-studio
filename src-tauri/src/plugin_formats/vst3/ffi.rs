//! VST3 raw COM bindings — Windows implementation.
//!
//! Defines the minimal vtable subset needed to enumerate, load, and process
//! audio through VST3 plugins without pulling in the full VST3 SDK.
//!
//! ## Safety contract
//!
//! All COM method calls are unsafe.  They must only be made on the command
//! thread (prepare/lifecycle) or the audio thread (process).  The two are
//! never concurrent thanks to the atomic scheduler swap.
//!
//! The `Vst3Library` struct wraps the HMODULE and calls `FreeLibrary` on drop.
//! It must outlive every `VstWrapperNode` that was created from it — callers
//! must keep an `Arc<Vst3Library>` alive alongside the node.

use std::ffi::c_void;
use std::path::Path;

// ── Primitive types ────────────────────────────────────────────────────────────

/// VST3 result code (0 = OK, 1 = FALSE, negative = error).
pub type TResult = i32;
/// 16-byte interface / class identifier.
pub type Tuid = [u8; 16];

pub const K_RESULT_OK:    TResult = 0;
pub const K_RESULT_FALSE: TResult = 1;

// ── Interface IDs ──────────────────────────────────────────────────────────────
// Derived from the VST3 SDK using FUID::toTUID() big-endian serialisation.

pub const IID_IPLUGIN_FACTORY: Tuid = [
    0x7A,0x4D,0x81,0x1C, 0x52,0x11,0x4A,0x1F,
    0xAE,0xD9,0xD2,0xEE, 0x0D,0xB2,0x5A,0x41,
];
pub const IID_ICOMPONENT: Tuid = [
    0xE8,0x31,0xFF,0x31, 0xF2,0xD5,0x43,0x01,
    0x92,0x8E,0xBB,0xEE, 0x25,0x69,0x78,0x02,
];
pub const IID_IAUDIO_PROCESSOR: Tuid = [
    0x42,0x04,0x3F,0x99, 0xB7,0xDA,0x45,0x3C,
    0xA5,0x69,0xE7,0x9D, 0x9A,0xAE,0xC3,0x3D,
];

// ── Speaker arrangements ───────────────────────────────────────────────────────

/// L | R  (bits 0 and 1)
pub const SPEAKER_STEREO: u64 = 0x0000_0003;
/// Mono center (bit 19 = kSpeakerM)
pub const SPEAKER_MONO:   u64 = 0x0008_0000;

// ── PClassInfo ─────────────────────────────────────────────────────────────────

pub const CATEGORY_AUDIO_MODULE: &[u8] = b"Audio Module Class";
pub const CATEGORY_COMPONENT:    &[u8] = b"Component Controller Class";

/// Minimal class info returned by IPluginFactory::getClassInfo.
#[repr(C)]
pub struct PClassInfo {
    pub cid: [u8; 16],
    pub cardinality: i32,
    pub category: [u8; 32],
    pub name: [u8; 64],
}

impl PClassInfo {
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(self.name.len());
        std::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
    pub fn category_str(&self) -> &str {
        let end = self.category.iter().position(|&b| b == 0).unwrap_or(self.category.len());
        std::str::from_utf8(&self.category[..end]).unwrap_or("")
    }
    pub fn is_audio_module(&self) -> bool {
        self.category.starts_with(CATEGORY_AUDIO_MODULE)
    }
}

// ── FUnknown (vtable index 0–2) ────────────────────────────────────────────────

/// Base COM object — only the vtable pointer at offset 0.
#[repr(C)]
pub struct FUnknown {
    pub vtbl: *const FUnknownVtbl,
}

#[repr(C)]
pub struct FUnknownVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release:  unsafe extern "C" fn(*mut FUnknown) -> u32,
}

// ── IPluginFactory (vtable index 0–6) ─────────────────────────────────────────

#[repr(C)]
pub struct IPluginFactory {
    pub vtbl: *const IPluginFactoryVtbl,
}

#[repr(C)]
pub struct IPluginFactoryVtbl {
    // FUnknown
    pub query_interface:
        unsafe extern "C" fn(*mut IPluginFactory, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IPluginFactory) -> u32,
    pub release:  unsafe extern "C" fn(*mut IPluginFactory) -> u32,
    // IPluginFactory
    pub get_factory_info: unsafe extern "C" fn(*mut IPluginFactory, *mut c_void) -> TResult,
    pub count_classes:    unsafe extern "C" fn(*mut IPluginFactory) -> i32,
    pub get_class_info:
        unsafe extern "C" fn(*mut IPluginFactory, i32, *mut PClassInfo) -> TResult,
    pub create_instance:
        unsafe extern "C" fn(*mut IPluginFactory, *const u8, *const u8, *mut *mut c_void)
            -> TResult,
}

// ── IComponent (vtable index 0–13) ────────────────────────────────────────────
// Extends FUnknown (0–2) + IPluginBase (3–4) + IComponent own methods (5–13).

#[repr(C)]
pub struct IComponent {
    pub vtbl: *const IComponentVtbl,
}

#[repr(C)]
pub struct IComponentVtbl {
    // FUnknown
    pub query_interface:
        unsafe extern "C" fn(*mut IComponent, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IComponent) -> u32,
    pub release:  unsafe extern "C" fn(*mut IComponent) -> u32,
    // IPluginBase
    pub initialize: unsafe extern "C" fn(*mut IComponent, *mut FUnknown) -> TResult,
    pub terminate:  unsafe extern "C" fn(*mut IComponent) -> TResult,
    // IComponent
    pub get_controller_class_id: unsafe extern "C" fn(*mut IComponent, *mut u8) -> TResult,
    pub set_io_mode: unsafe extern "C" fn(*mut IComponent, i32) -> TResult,
    pub get_bus_count: unsafe extern "C" fn(*mut IComponent, i32, i32) -> i32,
    pub get_bus_info:  unsafe extern "C" fn(*mut IComponent, i32, i32, i32, *mut c_void) -> TResult,
    pub get_routing_info: unsafe extern "C" fn(*mut IComponent, *mut c_void, *mut c_void) -> TResult,
    pub activate_bus: unsafe extern "C" fn(*mut IComponent, i32, i32, i32, u8) -> TResult,
    pub set_active:   unsafe extern "C" fn(*mut IComponent, u8) -> TResult,
    pub set_state:    unsafe extern "C" fn(*mut IComponent, *mut c_void) -> TResult,
    pub get_state:    unsafe extern "C" fn(*mut IComponent, *mut c_void) -> TResult,
}

// ── IAudioProcessor (vtable index 0–10) ───────────────────────────────────────
// Extends FUnknown (0–2) only.

#[repr(C)]
pub struct IAudioProcessor {
    pub vtbl: *const IAudioProcessorVtbl,
}

/// Audio bus channel pointer array + silence flags for one bus.
#[repr(C)]
pub struct AudioBusBuffers {
    pub num_channels: i32,
    pub silence_flags: u64,
    pub channel_buffers32: *mut *mut f32,
}

/// Setup passed to `setupProcessing`.
#[repr(C)]
pub struct ProcessSetup {
    pub process_mode:           i32,  // 0 = realtime
    pub symbolic_sample_size:   i32,  // 0 = float32
    pub max_samples_per_block:  i32,
    pub sample_rate:            f64,
}

/// Per-block processing data (null pointers for unused fields are valid).
#[repr(C)]
pub struct ProcessData {
    pub process_mode:         i32,  // 0 = realtime
    pub symbolic_sample_size: i32,  // 0 = float32
    pub num_samples:          i32,
    pub num_inputs:           i32,
    pub num_outputs:          i32,
    pub inputs:               *mut AudioBusBuffers,
    pub outputs:              *mut AudioBusBuffers,
    pub input_param_changes:  *mut c_void,
    pub output_param_changes: *mut c_void,
    pub input_events:         *mut c_void,
    pub output_events:        *mut c_void,
    pub process_context:      *mut c_void,
}

#[repr(C)]
pub struct IAudioProcessorVtbl {
    // FUnknown
    pub query_interface:
        unsafe extern "C" fn(*mut IAudioProcessor, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
    pub release:  unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
    // IAudioProcessor
    pub set_bus_arrangements:
        unsafe extern "C" fn(*mut IAudioProcessor, *mut u64, i32, *mut u64, i32) -> TResult,
    pub get_bus_arrangement:
        unsafe extern "C" fn(*mut IAudioProcessor, i32, i32, *mut u64) -> TResult,
    pub can_process_sample_size:
        unsafe extern "C" fn(*mut IAudioProcessor, i32) -> TResult,
    pub get_latency_samples:
        unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
    pub setup_processing:
        unsafe extern "C" fn(*mut IAudioProcessor, *const ProcessSetup) -> TResult,
    pub set_processing:
        unsafe extern "C" fn(*mut IAudioProcessor, u8) -> TResult,
    pub process:
        unsafe extern "C" fn(*mut IAudioProcessor, *mut ProcessData) -> TResult,
    pub get_tail_samples:
        unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
}

// ── Loaded library ─────────────────────────────────────────────────────────────

/// Thin kernel32 wrapper — avoids dependency on a specific windows-sys HMODULE type.
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

/// Owns a loaded .vst3 DLL.  `FreeLibrary` is called on drop.
#[cfg(windows)]
pub struct Vst3Library {
    handle: *mut c_void,
    pub path: std::path::PathBuf,
}

// SAFETY: HMODULE is process-global and safe to send across threads.
#[cfg(windows)]
unsafe impl Send for Vst3Library {}
#[cfg(windows)]
unsafe impl Sync for Vst3Library {}

#[cfg(windows)]
impl Vst3Library {
    pub fn load(dll_path: &Path) -> Result<(std::sync::Arc<Self>, *mut IPluginFactory), String> {
        let wide: Vec<u16> = dll_path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe { win32::LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() {
            return Err(format!("LoadLibraryW failed for {}", dll_path.display()));
        }

        let factory_fn = unsafe { win32::GetProcAddress(handle, b"GetPluginFactory\0".as_ptr()) };
        let factory_fn = match factory_fn {
            Some(f) => f,
            None => {
                unsafe { win32::FreeLibrary(handle) };
                return Err(format!("GetPluginFactory not found in {}", dll_path.display()));
            }
        };

        type GetPluginFactoryFn = unsafe extern "C" fn() -> *mut IPluginFactory;
        let get_factory: GetPluginFactoryFn = unsafe { std::mem::transmute(factory_fn) };
        let factory = unsafe { get_factory() };

        if factory.is_null() {
            unsafe { win32::FreeLibrary(handle) };
            return Err("GetPluginFactory returned null".into());
        }

        let lib = std::sync::Arc::new(Vst3Library { handle, path: dll_path.to_path_buf() });
        Ok((lib, factory))
    }
}

#[cfg(windows)]
impl Drop for Vst3Library {
    fn drop(&mut self) {
        unsafe { win32::FreeLibrary(self.handle) };
    }
}

// Stub for non-Windows builds so the rest of the crate still compiles.
#[cfg(not(windows))]
pub struct Vst3Library {
    pub path: std::path::PathBuf,
}

#[cfg(not(windows))]
impl Vst3Library {
    pub fn load(_dll_path: &Path) -> Result<(std::sync::Arc<Self>, *mut IPluginFactory), String> {
        Err("VST3 only supported on Windows (and macOS/Linux in a future release)".into())
    }
}

// ── Plugin enumeration helper ─────────────────────────────────────────────────

/// Enumerate all audio-module classes in a loaded factory.
///
/// Returns `(class_id, plugin_name)` pairs.  The class_id is passed back to
/// `IPluginFactory::createInstance` to create the audio component.
pub unsafe fn enumerate_classes(
    factory: *mut IPluginFactory,
) -> Vec<([u8; 16], String)> {
    let count = ((*(*factory).vtbl).count_classes)(factory);
    let mut out = Vec::with_capacity(count.max(0) as usize);

    for i in 0..count {
        let mut info: PClassInfo = std::mem::zeroed();
        let r = ((*(*factory).vtbl).get_class_info)(factory, i, &mut info);
        if r != K_RESULT_OK { continue; }
        if !info.is_audio_module() { continue; }
        out.push((info.cid, info.name_str().to_string()));
    }
    out
}

/// Create `IComponent` + `IAudioProcessor` for a given class ID.
///
/// Returns both pointers (which may point to the same COM object) on success.
pub unsafe fn instantiate_processor(
    factory: *mut IPluginFactory,
    cid: &[u8; 16],
) -> Result<(*mut IComponent, *mut IAudioProcessor), String> {
    // Create IComponent
    let mut component: *mut c_void = std::ptr::null_mut();
    let r = ((*(*factory).vtbl).create_instance)(
        factory,
        cid.as_ptr(),
        IID_ICOMPONENT.as_ptr(),
        &mut component,
    );
    if r != K_RESULT_OK || component.is_null() {
        return Err(format!("createInstance(IComponent) failed: {r}"));
    }
    let comp = component as *mut IComponent;

    // Initialize with a null host context (acceptable for audio-only hosts)
    let r = ((*(*comp).vtbl).initialize)(comp, std::ptr::null_mut());
    if r != K_RESULT_OK && r != K_RESULT_FALSE {
        ((*(*comp).vtbl).release)(comp);
        return Err(format!("IComponent::initialize failed: {r}"));
    }

    // Query IAudioProcessor — queryInterface is the first vtable entry,
    // shared layout with FUnknown; cast comp through *mut c_void to avoid
    // the C++ vtable cast that would normally happen in a C++ class hierarchy.
    let mut processor: *mut c_void = std::ptr::null_mut();
    let r = ((*(*comp).vtbl).query_interface)(
        comp,
        IID_IAUDIO_PROCESSOR.as_ptr() as *const Tuid,
        &mut processor,
    );
    if r != K_RESULT_OK || processor.is_null() {
        ((*(*comp).vtbl).terminate)(comp);
        ((*(*comp).vtbl).release)(comp);
        return Err("queryInterface(IAudioProcessor) failed".into());
    }

    Ok((comp, processor as *mut IAudioProcessor))
}
