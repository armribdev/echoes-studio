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

/// VST3 result code (0 = OK, 1 = FALSE, negative = error).
pub type TResult = i32;
/// 16-byte interface / class identifier.
pub type Tuid = [u8; 16];

pub const K_RESULT_OK: TResult = 0;
pub const K_RESULT_FALSE: TResult = 1;

// Derived from the VST3 SDK using FUID::toTUID() big-endian serialisation.

/// Windows VST3 is COM-compatible and matches interface IIDs in GUID byte order
/// (`Data1`/`Data2`/`Data3` little-endian, `Data4` unchanged).  The literals
/// below are the platform-neutral big-endian form; on Windows the first three
/// groups must be byte-swapped or `createInstance`/`queryInterface` return
/// E_NOINTERFACE for every plugin (see docs/active/audits/ffi_lifecycle_validation.md, F1).
const fn interface_iid(be: Tuid) -> Tuid {
    if cfg!(windows) {
        [
            be[3], be[2], be[1], be[0], be[5], be[4], be[7], be[6], be[8], be[9], be[10], be[11],
            be[12], be[13], be[14], be[15],
        ]
    } else {
        be
    }
}

/// Steinberg's FUnknown IID (non-COM-compatible, modern SDK convention).
pub const IID_FUNKNOWN: Tuid = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

pub const IID_ICOMPONENT: Tuid = interface_iid([
    0xE8, 0x31, 0xFF, 0x31, 0xF2, 0xD5, 0x43, 0x01, 0x92, 0x8E, 0xBB, 0xEE, 0x25, 0x69, 0x78, 0x02,
]);
pub const IID_IAUDIO_PROCESSOR: Tuid = interface_iid([
    0x42, 0x04, 0x3F, 0x99, 0xB7, 0xDA, 0x45, 0x3C, 0xA5, 0x69, 0xE7, 0x9D, 0x9A, 0xAE, 0xC3, 0x3D,
]);

/// L | R  (bits 0 and 1)
pub const SPEAKER_STEREO: u64 = 0x0000_0003;

pub const CATEGORY_AUDIO_MODULE: &[u8] = b"Audio Module Class";

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
        let end = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.name.len());
        std::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
    pub fn is_audio_module(&self) -> bool {
        self.category.starts_with(CATEGORY_AUDIO_MODULE)
    }
}

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
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
}

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
    pub release: unsafe extern "C" fn(*mut IPluginFactory) -> u32,
    // IPluginFactory
    pub get_factory_info: unsafe extern "C" fn(*mut IPluginFactory, *mut c_void) -> TResult,
    pub count_classes: unsafe extern "C" fn(*mut IPluginFactory) -> i32,
    pub get_class_info: unsafe extern "C" fn(*mut IPluginFactory, i32, *mut PClassInfo) -> TResult,
    pub create_instance: unsafe extern "C" fn(
        *mut IPluginFactory,
        *const u8,
        *const u8,
        *mut *mut c_void,
    ) -> TResult,
}

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
    pub release: unsafe extern "C" fn(*mut IComponent) -> u32,
    // IPluginBase
    pub initialize: unsafe extern "C" fn(*mut IComponent, *mut FUnknown) -> TResult,
    pub terminate: unsafe extern "C" fn(*mut IComponent) -> TResult,
    // IComponent
    pub get_controller_class_id: unsafe extern "C" fn(*mut IComponent, *mut u8) -> TResult,
    pub set_io_mode: unsafe extern "C" fn(*mut IComponent, i32) -> TResult,
    pub get_bus_count: unsafe extern "C" fn(*mut IComponent, i32, i32) -> i32,
    pub get_bus_info: unsafe extern "C" fn(*mut IComponent, i32, i32, i32, *mut c_void) -> TResult,
    pub get_routing_info:
        unsafe extern "C" fn(*mut IComponent, *mut c_void, *mut c_void) -> TResult,
    pub activate_bus: unsafe extern "C" fn(*mut IComponent, i32, i32, i32, u8) -> TResult,
    pub set_active: unsafe extern "C" fn(*mut IComponent, u8) -> TResult,
    pub set_state: unsafe extern "C" fn(*mut IComponent, *mut c_void) -> TResult,
    pub get_state: unsafe extern "C" fn(*mut IComponent, *mut c_void) -> TResult,
}

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
    pub process_mode: i32,         // 0 = realtime
    pub symbolic_sample_size: i32, // 0 = float32
    pub max_samples_per_block: i32,
    pub sample_rate: f64,
}

/// Per-block processing data (null pointers for unused fields are valid).
#[repr(C)]
pub struct ProcessData {
    pub process_mode: i32,         // 0 = realtime
    pub symbolic_sample_size: i32, // 0 = float32
    pub num_samples: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    pub inputs: *mut AudioBusBuffers,
    pub outputs: *mut AudioBusBuffers,
    pub input_param_changes: *mut c_void,
    pub output_param_changes: *mut c_void,
    pub input_events: *mut c_void,
    pub output_events: *mut c_void,
    pub process_context: *mut c_void,
}

#[repr(C)]
pub struct IAudioProcessorVtbl {
    // FUnknown
    pub query_interface:
        unsafe extern "C" fn(*mut IAudioProcessor, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
    pub release: unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
    // IAudioProcessor
    pub set_bus_arrangements:
        unsafe extern "C" fn(*mut IAudioProcessor, *mut u64, i32, *mut u64, i32) -> TResult,
    pub get_bus_arrangement:
        unsafe extern "C" fn(*mut IAudioProcessor, i32, i32, *mut u64) -> TResult,
    pub can_process_sample_size: unsafe extern "C" fn(*mut IAudioProcessor, i32) -> TResult,
    pub get_latency_samples: unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
    pub setup_processing:
        unsafe extern "C" fn(*mut IAudioProcessor, *const ProcessSetup) -> TResult,
    pub set_processing: unsafe extern "C" fn(*mut IAudioProcessor, u8) -> TResult,
    pub process: unsafe extern "C" fn(*mut IAudioProcessor, *mut ProcessData) -> TResult,
    pub get_tail_samples: unsafe extern "C" fn(*mut IAudioProcessor) -> u32,
}

// ADR-007.  The host fills these structures each block from the SPSC parameter
// ring and hands them to the plugin via `ProcessData::input_param_changes`.
// The plugin only *reads* them (getParameterCount → getParameterData → per
// queue getParameterId / getPointCount / getPoint); the add* methods exist for
// spec completeness and are what the host uses internally to fill the data.

#[repr(C)]
pub struct IParamValueQueueVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut HostParamValueQueue, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut HostParamValueQueue) -> u32,
    pub release: unsafe extern "C" fn(*mut HostParamValueQueue) -> u32,
    pub get_parameter_id: unsafe extern "C" fn(*mut HostParamValueQueue) -> u32,
    pub get_point_count: unsafe extern "C" fn(*mut HostParamValueQueue) -> i32,
    pub get_point:
        unsafe extern "C" fn(*mut HostParamValueQueue, i32, *mut i32, *mut f64) -> TResult,
    pub add_point: unsafe extern "C" fn(*mut HostParamValueQueue, i32, f64, *mut i32) -> TResult,
}

#[repr(C)]
pub struct IParameterChangesVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut HostParameterChanges, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut HostParameterChanges) -> u32,
    pub release: unsafe extern "C" fn(*mut HostParameterChanges) -> u32,
    pub get_parameter_count: unsafe extern "C" fn(*mut HostParameterChanges) -> i32,
    pub get_parameter_data:
        unsafe extern "C" fn(*mut HostParameterChanges, i32) -> *mut HostParamValueQueue,
    pub add_parameter_data: unsafe extern "C" fn(
        *mut HostParameterChanges,
        *const u32,
        *mut i32,
    ) -> *mut HostParamValueQueue,
}

/// Max distinct parameters carried in one block, and max value points per
/// parameter per block.  `MAX_POINTS_PER_QUEUE` covers one sub-block modulation
/// point per `MODULATION_CONTROL_PERIOD` over a full `MAX_BLOCK_FRAMES` block
/// (ADR-019: ceil(4096 / 32) = 128) plus UI edits.
pub const MAX_PARAM_QUEUES: usize = 128;
pub const MAX_POINTS_PER_QUEUE: usize = 128;

/// One parameter's value points for the current block.  `#[repr(C)]` keeps the
/// `vtbl` at offset 0 so the plugin can dispatch through it; the remaining
/// fields are private host data the vtable methods read.
#[repr(C)]
pub struct HostParamValueQueue {
    pub vtbl: *const IParamValueQueueVtbl,
    pub param_id: u32,
    pub offsets: [i32; MAX_POINTS_PER_QUEUE],
    pub values: [f64; MAX_POINTS_PER_QUEUE],
    pub count: i32,
}

/// The per-block parameter-change list handed to the plugin.
#[repr(C)]
pub struct HostParameterChanges {
    pub vtbl: *const IParameterChangesVtbl,
    pub queues: [HostParamValueQueue; MAX_PARAM_QUEUES],
    pub count: i32,
}

unsafe extern "C" fn pvq_query_interface(
    _this: *mut HostParamValueQueue,
    _iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if !obj.is_null() {
        *obj = std::ptr::null_mut();
    }
    -1 // kNoInterface
}
unsafe extern "C" fn pvq_add_ref(_this: *mut HostParamValueQueue) -> u32 {
    1
}
unsafe extern "C" fn pvq_release(_this: *mut HostParamValueQueue) -> u32 {
    1
}

unsafe extern "C" fn pvq_get_parameter_id(this: *mut HostParamValueQueue) -> u32 {
    (*this).param_id
}
unsafe extern "C" fn pvq_get_point_count(this: *mut HostParamValueQueue) -> i32 {
    (*this).count
}
unsafe extern "C" fn pvq_get_point(
    this: *mut HostParamValueQueue,
    index: i32,
    sample_offset: *mut i32,
    value: *mut f64,
) -> TResult {
    let q = &*this;
    if index < 0 || index >= q.count {
        return K_RESULT_FALSE;
    }
    let i = index as usize;
    if !sample_offset.is_null() {
        *sample_offset = q.offsets[i];
    }
    if !value.is_null() {
        *value = q.values[i];
    }
    K_RESULT_OK
}
unsafe extern "C" fn pvq_add_point(
    this: *mut HostParamValueQueue,
    sample_offset: i32,
    value: f64,
    index: *mut i32,
) -> TResult {
    let q = &mut *this;
    if (q.count as usize) >= MAX_POINTS_PER_QUEUE {
        return K_RESULT_FALSE;
    }
    let i = q.count as usize;
    q.offsets[i] = sample_offset;
    q.values[i] = value;
    q.count += 1;
    if !index.is_null() {
        *index = i as i32;
    }
    K_RESULT_OK
}

unsafe extern "C" fn pc_query_interface(
    _this: *mut HostParameterChanges,
    _iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if !obj.is_null() {
        *obj = std::ptr::null_mut();
    }
    -1
}
unsafe extern "C" fn pc_add_ref(_this: *mut HostParameterChanges) -> u32 {
    1
}
unsafe extern "C" fn pc_release(_this: *mut HostParameterChanges) -> u32 {
    1
}

unsafe extern "C" fn pc_get_parameter_count(this: *mut HostParameterChanges) -> i32 {
    (*this).count
}
unsafe extern "C" fn pc_get_parameter_data(
    this: *mut HostParameterChanges,
    index: i32,
) -> *mut HostParamValueQueue {
    let pc = &mut *this;
    if index < 0 || index >= pc.count {
        return std::ptr::null_mut();
    }
    &mut pc.queues[index as usize] as *mut HostParamValueQueue
}
unsafe extern "C" fn pc_add_parameter_data(
    this: *mut HostParameterChanges,
    id: *const u32,
    index: *mut i32,
) -> *mut HostParamValueQueue {
    let pc = &mut *this;
    let param_id = if id.is_null() { 0 } else { *id };
    // Reuse an existing queue for this id if present.
    for i in 0..pc.count as usize {
        if pc.queues[i].param_id == param_id {
            if !index.is_null() {
                *index = i as i32;
            }
            return &mut pc.queues[i] as *mut HostParamValueQueue;
        }
    }
    if (pc.count as usize) >= MAX_PARAM_QUEUES {
        return std::ptr::null_mut();
    }
    let i = pc.count as usize;
    pc.queues[i].param_id = param_id;
    pc.queues[i].count = 0;
    pc.count += 1;
    if !index.is_null() {
        *index = i as i32;
    }
    &mut pc.queues[i] as *mut HostParamValueQueue
}

static PVQ_VTBL: IParamValueQueueVtbl = IParamValueQueueVtbl {
    query_interface: pvq_query_interface,
    add_ref: pvq_add_ref,
    release: pvq_release,
    get_parameter_id: pvq_get_parameter_id,
    get_point_count: pvq_get_point_count,
    get_point: pvq_get_point,
    add_point: pvq_add_point,
};

static PC_VTBL: IParameterChangesVtbl = IParameterChangesVtbl {
    query_interface: pc_query_interface,
    add_ref: pc_add_ref,
    release: pc_release,
    get_parameter_count: pc_get_parameter_count,
    get_parameter_data: pc_get_parameter_data,
    add_parameter_data: pc_add_parameter_data,
};

impl HostParameterChanges {
    /// Allocate a zeroed, vtable-wired parameter-change list on the heap
    /// (pointer-stable).  Call once at `prepare` — never on the audio thread.
    pub fn boxed() -> Box<Self> {
        // SAFETY: every field is plain data or a raw pointer; zeroing yields a
        // valid empty list.  We then patch in the vtable pointers.
        let mut b: Box<HostParameterChanges> = unsafe { Box::new(std::mem::zeroed()) };
        b.vtbl = &PC_VTBL as *const _;
        b.count = 0;
        for q in b.queues.iter_mut() {
            q.vtbl = &PVQ_VTBL as *const _;
            q.count = 0;
        }
        b
    }

    /// Reset to empty for a new block (no allocation).
    #[inline]
    pub fn reset(&mut self) {
        self.count = 0;
    }

    /// Append one normalised value point for `param_id` at `sample_offset`.
    /// Finds or creates the parameter's queue.  Silently drops if either the
    /// queue table or the per-queue point list is full.
    #[inline]
    pub fn push(&mut self, param_id: u32, sample_offset: i32, normalised: f64) {
        let this = self as *mut HostParameterChanges;
        unsafe {
            let q = pc_add_parameter_data(this, &param_id, std::ptr::null_mut());
            if !q.is_null() {
                pvq_add_point(q, sample_offset, normalised, std::ptr::null_mut());
            }
        }
    }
}

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
                return Err(format!(
                    "GetPluginFactory not found in {}",
                    dll_path.display()
                ));
            }
        };

        type GetPluginFactoryFn = unsafe extern "C" fn() -> *mut IPluginFactory;
        let get_factory: GetPluginFactoryFn = unsafe { std::mem::transmute(factory_fn) };
        let factory = unsafe { get_factory() };

        if factory.is_null() {
            unsafe { win32::FreeLibrary(handle) };
            return Err("GetPluginFactory returned null".into());
        }

        let lib = std::sync::Arc::new(Vst3Library {
            handle,
            path: dll_path.to_path_buf(),
        });
        Ok((lib, factory))
    }

    /// Re-obtain the plugin factory from the already-mapped DLL.  The module is
    /// kept alive by this `Vst3Library`, so `GetPluginFactory` is safe to call
    /// again; the returned factory carries a ref the caller must release.
    ///
    /// # Safety
    /// Call off the audio thread.  The DLL must still be loaded (guaranteed
    /// while `self` is alive).
    pub unsafe fn factory(&self) -> Option<*mut IPluginFactory> {
        let factory_fn = win32::GetProcAddress(self.handle, b"GetPluginFactory\0".as_ptr())?;
        type GetPluginFactoryFn = unsafe extern "C" fn() -> *mut IPluginFactory;
        let get_factory: GetPluginFactoryFn = std::mem::transmute(factory_fn);
        let factory = get_factory();
        if factory.is_null() {
            None
        } else {
            Some(factory)
        }
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

// Extends FUnknown (0–2) + IPluginBase (3–4) + IEditController own methods (5–17).

/// IEditController — UI/parameter controller interface, main-thread only.
/// Passed to the plugin's `queryInterface` / `createInstance`, so it must carry
/// COM byte order on Windows like `IID_ICOMPONENT` (F1/V4 — the GUI half of the
/// IID byte-order defect; without the swap QI/createInstance return E_NOINTERFACE).
pub const IID_IEDIT_CONTROLLER: Tuid = interface_iid([
    0xDC, 0xD7, 0xBB, 0xE3, 0x77, 0x42, 0x44, 0x8D, 0xA8, 0x74, 0xAA, 0xCC, 0x97, 0x9C, 0x9D, 0x2C,
]);

#[repr(C)]
pub struct IEditController {
    pub vtbl: *const IEditControllerVtbl,
}

#[repr(C)]
pub struct IEditControllerVtbl {
    // FUnknown (0–2)
    pub query_interface:
        unsafe extern "C" fn(*mut IEditController, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IEditController) -> u32,
    pub release: unsafe extern "C" fn(*mut IEditController) -> u32,
    // IPluginBase (3–4)
    pub initialize: unsafe extern "C" fn(*mut IEditController, *mut FUnknown) -> TResult,
    pub terminate: unsafe extern "C" fn(*mut IEditController) -> TResult,
    // IEditController own — methods 5–16 mostly unused; *const c_void as placeholder
    // (5) setComponentState — typed so a separate controller can be synced to the
    // processor's current state (dual-component plugins, audit V4).
    pub set_component_state: unsafe extern "C" fn(*mut IEditController, *mut c_void) -> TResult,
    pub _pad6: *const c_void,  // setState
    pub _pad7: *const c_void,  // getState
    pub _pad8: *const c_void,  // getParameterCount
    pub _pad9: *const c_void,  // getParameterInfo
    pub _pad10: *const c_void, // getParamStringByValue
    pub _pad11: *const c_void, // getParamValueByString
    pub _pad12: *const c_void, // normalizedParamToPlain
    pub _pad13: *const c_void, // plainParamToNormalized
    pub _pad14: *const c_void, // getParamNormalized
    pub _pad15: *const c_void, // setParamNormalized
    // (16) setComponentHandler — typed so we can wire IComponentHandler
    pub set_component_handler: unsafe extern "C" fn(*mut IEditController, *mut FUnknown) -> TResult,
    // (17) createView — the one we call
    pub create_view: unsafe extern "C" fn(*mut IEditController, *const u8) -> *mut IPlugView,
}

/// Plugin-preferred view rectangle (in screen pixels).
#[repr(C)]
pub struct ViewRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
pub struct IPlugView {
    pub vtbl: *const IPlugViewVtbl,
}

#[repr(C)]
pub struct IPlugViewVtbl {
    // FUnknown (0–2)
    pub query_interface:
        unsafe extern "C" fn(*mut IPlugView, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IPlugView) -> u32,
    pub release: unsafe extern "C" fn(*mut IPlugView) -> u32,
    // IPlugView (3–14)
    pub is_platform_type_supported: unsafe extern "C" fn(*mut IPlugView, *const u8) -> TResult,
    pub attached: unsafe extern "C" fn(*mut IPlugView, *mut c_void, *const u8) -> TResult,
    pub removed: unsafe extern "C" fn(*mut IPlugView) -> TResult,
    pub _pad6: *const c_void, // onWheel
    pub _pad7: *const c_void, // onKeyDown
    pub _pad8: *const c_void, // onKeyUp
    pub get_size: unsafe extern "C" fn(*mut IPlugView, *mut ViewRect) -> TResult,
    pub _pad10: *const c_void, // onSize
    pub _pad11: *const c_void, // onFocus
    pub set_frame: unsafe extern "C" fn(*mut IPlugView, *mut FUnknown) -> TResult,
    pub _pad13: *const c_void, // canResize
    pub _pad14: *const c_void, // checkSizeConstraint
}

// Fail-fast: vtable layouts depend on each entry being pointer-sized.  These
// asserts catch accidental width drift on exotic targets.
const _: () = assert!(
    std::mem::size_of::<IPlugViewVtbl>() == 15 * std::mem::size_of::<usize>(),
    "IPlugViewVtbl layout drift",
);
const _: () = assert!(
    std::mem::size_of::<IEditControllerVtbl>() == 18 * std::mem::size_of::<usize>(),
    "IEditControllerVtbl layout drift",
);

/// Optional `IPlugView` companion (Windows only): the host passes the monitor
/// scale, since Win32 view sizes are in physical pixels.
pub const IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT: Tuid = interface_iid([
    0x65, 0xED, 0x96, 0x90, 0x8A, 0xC4, 0x45, 0x25, 0x8A, 0xAD, 0xEF, 0x7A, 0x72, 0xEA, 0x70, 0x3F,
]);

#[repr(C)]
pub struct IPlugViewContentScaleSupport {
    pub vtbl: *const IPlugViewContentScaleSupportVtbl,
}

#[repr(C)]
pub struct IPlugViewContentScaleSupportVtbl {
    pub query_interface: unsafe extern "C" fn(
        *mut IPlugViewContentScaleSupport,
        *const Tuid,
        *mut *mut c_void,
    ) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IPlugViewContentScaleSupport) -> u32,
    pub release: unsafe extern "C" fn(*mut IPlugViewContentScaleSupport) -> u32,
    pub set_content_scale_factor:
        unsafe extern "C" fn(*mut IPlugViewContentScaleSupport, f32) -> TResult,
}

// Dual-component plugins (audit V4) expose IComponent and IEditController as two
// distinct COM objects.  They are wired together through IConnectionPoint so the
// processor and controller can exchange IMessage notifications.

pub const IID_ICONNECTION_POINT: Tuid = interface_iid([
    0x70, 0xA4, 0x15, 0x6F, 0x6E, 0x6E, 0x65, 0x63, 0x74, 0x69, 0x6F, 0x6E, 0x50, 0x6F, 0x69, 0x6E,
]);

#[repr(C)]
pub struct IConnectionPoint {
    pub vtbl: *const IConnectionPointVtbl,
}

#[repr(C)]
pub struct IConnectionPointVtbl {
    // FUnknown (0–2)
    pub query_interface:
        unsafe extern "C" fn(*mut IConnectionPoint, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut IConnectionPoint) -> u32,
    pub release: unsafe extern "C" fn(*mut IConnectionPoint) -> u32,
    // IConnectionPoint (3–5)
    pub connect: unsafe extern "C" fn(*mut IConnectionPoint, *mut IConnectionPoint) -> TResult,
    pub disconnect: unsafe extern "C" fn(*mut IConnectionPoint, *mut IConnectionPoint) -> TResult,
    pub notify: unsafe extern "C" fn(*mut IConnectionPoint, *mut c_void) -> TResult,
}

/// A separately-created `IEditController` for a dual-component VST3, plus the
/// two `IConnectionPoint`s wiring it to the processor's `IComponent` (null when
/// the plugin exposes none).  Returned by [`create_separate_controller`] and
/// torn down by [`teardown_separate_controller`].
pub struct SeparateController {
    pub controller: *mut IEditController,
    comp_connection: *mut IConnectionPoint,
    ctrl_connection: *mut IConnectionPoint,
}

/// Create and wire the `IEditController` for a dual-component plugin: read the
/// controller class id from the component, instantiate it through the factory,
/// initialise it with the host context, connect the two halves, and sync the
/// controller to the processor's current state (`getState` → `setComponentState`).
///
/// # Safety
/// `comp` must be a valid, initialised `IComponent` from `library`.  Call on the
/// command/main thread, never the audio thread.
pub unsafe fn create_separate_controller(
    library: &Vst3Library,
    comp: *mut IComponent,
    host_context: *mut FUnknown,
) -> Result<SeparateController, String> {
    let mut ctrl_cid: Tuid = [0u8; 16];
    let r = ((*(*comp).vtbl).get_controller_class_id)(comp, ctrl_cid.as_mut_ptr());
    if r != K_RESULT_OK {
        return Err(format!("getControllerClassId failed: {r}"));
    }

    let factory = library
        .factory()
        .ok_or_else(|| "GetPluginFactory returned null".to_string())?;

    let mut ctrl_raw: *mut c_void = std::ptr::null_mut();
    let r = ((*(*factory).vtbl).create_instance)(
        factory,
        ctrl_cid.as_ptr(),
        IID_IEDIT_CONTROLLER.as_ptr(),
        &mut ctrl_raw,
    );
    ((*(*factory).vtbl).release)(factory);
    if r != K_RESULT_OK || ctrl_raw.is_null() {
        return Err(format!("createInstance(IEditController) failed: {r}"));
    }
    let ctrl = ctrl_raw as *mut IEditController;

    let r = ((*(*ctrl).vtbl).initialize)(ctrl, host_context);
    if r != K_RESULT_OK && r != K_RESULT_FALSE {
        ((*(*ctrl).vtbl).release)(ctrl as *mut _);
        return Err(format!("IEditController::initialize failed: {r}"));
    }

    sync_controller_state(comp, ctrl);

    let (comp_connection, ctrl_connection) = connect_component_controller(comp, ctrl);

    Ok(SeparateController {
        controller: ctrl,
        comp_connection,
        ctrl_connection,
    })
}

/// Reverse [`create_separate_controller`]: disconnect the two halves, terminate
/// the controller, and release every ref this host added.
///
/// # Safety
/// `sc` must come from [`create_separate_controller`] and not have been torn
/// down already.  Call on the command/main thread.
pub unsafe fn teardown_separate_controller(sc: SeparateController) {
    if !sc.comp_connection.is_null() && !sc.ctrl_connection.is_null() {
        ((*(*sc.comp_connection).vtbl).disconnect)(sc.comp_connection, sc.ctrl_connection);
        ((*(*sc.ctrl_connection).vtbl).disconnect)(sc.ctrl_connection, sc.comp_connection);
    }
    if !sc.comp_connection.is_null() {
        ((*(*sc.comp_connection).vtbl).release)(sc.comp_connection);
    }
    if !sc.ctrl_connection.is_null() {
        ((*(*sc.ctrl_connection).vtbl).release)(sc.ctrl_connection);
    }
    ((*(*sc.controller).vtbl).terminate)(sc.controller);
    ((*(*sc.controller).vtbl).release)(sc.controller as *mut _);
}

/// Copy the processor's current state into the controller so the editor opens
/// with the right values (`IComponent::getState` → `IEditController::setComponentState`).
/// Best-effort: a plugin that reports no state simply leaves the controller at
/// its defaults.
unsafe fn sync_controller_state(comp: *mut IComponent, ctrl: *mut IEditController) {
    let mut stream = HostStream::empty();
    let ib = stream.as_mut() as *mut HostStream as *mut c_void;
    if ((*(*comp).vtbl).get_state)(comp, ib) != K_RESULT_OK {
        return;
    }
    stream.pos = 0;
    ((*(*ctrl).vtbl).set_component_state)(ctrl, ib);
}

/// Wire the processor and controller halves together via `IConnectionPoint`
/// (peer-to-peer).  Returns the two connection points so the caller can
/// disconnect/release them on teardown; either is null when the plugin exposes
/// no connection point on that half.
unsafe fn connect_component_controller(
    comp: *mut IComponent,
    ctrl: *mut IEditController,
) -> (*mut IConnectionPoint, *mut IConnectionPoint) {
    let comp_cp = query_connection_point((*comp).vtbl as *const _, comp as *mut c_void);
    let ctrl_cp = query_connection_point((*ctrl).vtbl as *const _, ctrl as *mut c_void);
    if !comp_cp.is_null() && !ctrl_cp.is_null() {
        ((*(*comp_cp).vtbl).connect)(comp_cp, ctrl_cp);
        ((*(*ctrl_cp).vtbl).connect)(ctrl_cp, comp_cp);
    }
    (comp_cp, ctrl_cp)
}

/// `queryInterface(IConnectionPoint)` on a COM object reached through its
/// FUnknown-compatible vtable head.  Returns null when unsupported.
unsafe fn query_connection_point(
    vtbl: *const FUnknownVtbl,
    object: *mut c_void,
) -> *mut IConnectionPoint {
    let mut out: *mut c_void = std::ptr::null_mut();
    let r = ((*vtbl).query_interface)(
        object as *mut FUnknown,
        IID_ICONNECTION_POINT.as_ptr() as *const Tuid,
        &mut out,
    );
    if r == K_RESULT_OK && !out.is_null() {
        out as *mut IConnectionPoint
    } else {
        std::ptr::null_mut()
    }
}

// Minimal host context passed to IComponent::initialize.  Plugins QI for this
// to fetch the host name and to ask the host to create auxiliary objects
// (IMessage, IAttributeList).  Returning kResultFalse for unsupported requests
// is the documented fall-back; well-behaved plugins must accept it.

pub const IID_IHOST_APPLICATION: Tuid = interface_iid([
    0x58, 0xE5, 0x95, 0xCD, 0x9A, 0x2C, 0x44, 0x49, 0xAD, 0xCA, 0xED, 0xA9, 0x96, 0xDC, 0xEA, 0xA0,
]);

#[repr(C)]
pub struct IHostApplicationVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
    /// Fill `name` with at most 128 UTF-16 code units (Steinberg `String128`).
    pub get_name: unsafe extern "C" fn(*mut FUnknown, *mut u16) -> TResult,
    pub create_instance:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *const Tuid, *mut *mut c_void) -> TResult,
}

// Plugins call this to notify the host of parameter edits (begin/perform/end)
// and to request a component restart.  Many plugins assume the handler is
// non-null and crash on the first knob touch without one.

pub const IID_ICOMPONENT_HANDLER: Tuid = interface_iid([
    0x93, 0xA0, 0xBE, 0xA3, 0x94, 0xB8, 0xB4, 0x85, 0xBC, 0xBE, 0x57, 0xDB, 0x18, 0xA5, 0x2D, 0xB7,
]);

#[repr(C)]
pub struct IComponentHandlerVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub begin_edit: unsafe extern "C" fn(*mut FUnknown, u32) -> TResult,
    pub perform_edit: unsafe extern "C" fn(*mut FUnknown, u32, f64) -> TResult,
    pub end_edit: unsafe extern "C" fn(*mut FUnknown, u32) -> TResult,
    pub restart_component: unsafe extern "C" fn(*mut FUnknown, i32) -> TResult,
}

// Set on the IPlugView so the plugin can request a resize of its container.

pub const IID_IPLUG_FRAME: Tuid = interface_iid([
    0x36, 0x7F, 0xAF, 0x01, 0xAF, 0xA9, 0x46, 0x93, 0x8D, 0x4D, 0xA2, 0xA0, 0xED, 0x05, 0x70, 0xE6,
]);

#[repr(C)]
pub struct IPlugFrameVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut FUnknown, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub release: unsafe extern "C" fn(*mut FUnknown) -> u32,
    pub resize_view: unsafe extern "C" fn(*mut FUnknown, *mut IPlugView, *mut ViewRect) -> TResult,
}

/// Enumerate all audio-module classes in a loaded factory.
///
/// Returns `(class_id, plugin_name)` pairs.  The class_id is passed back to
/// `IPluginFactory::createInstance` to create the audio component.
pub unsafe fn enumerate_classes(factory: *mut IPluginFactory) -> Vec<([u8; 16], String)> {
    let count = ((*(*factory).vtbl).count_classes)(factory);
    let mut out = Vec::with_capacity(count.max(0) as usize);

    for i in 0..count {
        let mut info: PClassInfo = std::mem::zeroed();
        let r = ((*(*factory).vtbl).get_class_info)(factory, i, &mut info);
        if r != K_RESULT_OK {
            continue;
        }
        if !info.is_audio_module() {
            continue;
        }
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

// Phase 4b (ADR-010).  `IComponent::getState` asks the host for an `IBStream`
// to *write* the plugin's processor state into; `setState` asks for one to
// *read* it back from.  The VST3 SDK ships a concrete `MemoryStream`, but we do
// not link the SDK — so the host provides its own `Vec<u8>`-backed stream
// exposing the four IBStream methods (read / write / seek / tell) on top of
// FUnknown.
//
// Lifetime: a `HostStream` is built on the command thread for the duration of a
// single get/set_state call and dropped immediately after.  The plugin only
// touches it synchronously inside that call.  Refcount methods return a dummy
// count (the host owns the box outright), matching the pattern used by
// `HostParameterChanges` and the GUI-side COM stubs.

/// Steinberg `IBStream` IID.  The plugin queries our host stream against it, so
/// it carries COM byte order on Windows like the other interface IIDs.
pub const IID_IBSTREAM: Tuid = interface_iid([
    0xC3, 0xBF, 0x6E, 0xA2, 0x30, 0x99, 0x47, 0x52, 0x9B, 0x6B, 0xF9, 0x90, 0x1E, 0xE3, 0x3E, 0x9B,
]);

/// `IBStream::IStreamSeekMode`
const K_IB_SEEK_SET: i32 = 0;
const K_IB_SEEK_CUR: i32 = 1;
const K_IB_SEEK_END: i32 = 2;

#[repr(C)]
pub struct IBStreamVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut HostStream, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut HostStream) -> u32,
    pub release: unsafe extern "C" fn(*mut HostStream) -> u32,
    pub read: unsafe extern "C" fn(*mut HostStream, *mut c_void, i32, *mut i32) -> TResult,
    pub write: unsafe extern "C" fn(*mut HostStream, *mut c_void, i32, *mut i32) -> TResult,
    pub seek: unsafe extern "C" fn(*mut HostStream, i64, i32, *mut i64) -> TResult,
    pub tell: unsafe extern "C" fn(*mut HostStream, *mut i64) -> TResult,
}

/// Host-side `IBStream` backed by an in-memory byte buffer.  `vtbl` MUST stay
/// at offset 0 so the plugin can dispatch through it.
#[repr(C)]
pub struct HostStream {
    vtbl: *const IBStreamVtbl,
    data: Vec<u8>,
    pos: usize,
}

unsafe extern "C" fn stream_query_interface(
    this: *mut HostStream,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() {
        return -1;
    }
    if !iid.is_null() && (*iid == IID_IBSTREAM || *iid == IID_FUNKNOWN) {
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = std::ptr::null_mut();
    -1 // kNoInterface
}
unsafe extern "C" fn stream_add_ref(_this: *mut HostStream) -> u32 {
    1
}
unsafe extern "C" fn stream_release(_this: *mut HostStream) -> u32 {
    1
}

unsafe extern "C" fn stream_read(
    this: *mut HostStream,
    buffer: *mut c_void,
    num_bytes: i32,
    num_read: *mut i32,
) -> TResult {
    let s = &mut *this;
    let want = num_bytes.max(0) as usize;
    let avail = s.data.len().saturating_sub(s.pos);
    let n = want.min(avail);
    if n > 0 && !buffer.is_null() {
        std::ptr::copy_nonoverlapping(s.data.as_ptr().add(s.pos), buffer as *mut u8, n);
        s.pos += n;
    }
    if !num_read.is_null() {
        *num_read = n as i32;
    }
    K_RESULT_OK
}

unsafe extern "C" fn stream_write(
    this: *mut HostStream,
    buffer: *mut c_void,
    num_bytes: i32,
    num_written: *mut i32,
) -> TResult {
    let s = &mut *this;
    let n = num_bytes.max(0) as usize;
    if n > 0 && !buffer.is_null() {
        // Overwrite/extend at the current position (MemoryStream semantics).
        if s.pos + n > s.data.len() {
            s.data.resize(s.pos + n, 0);
        }
        std::ptr::copy_nonoverlapping(buffer as *const u8, s.data.as_mut_ptr().add(s.pos), n);
        s.pos += n;
    }
    if !num_written.is_null() {
        *num_written = n as i32;
    }
    K_RESULT_OK
}

unsafe extern "C" fn stream_seek(
    this: *mut HostStream,
    pos: i64,
    mode: i32,
    result: *mut i64,
) -> TResult {
    let s = &mut *this;
    let base = match mode {
        K_IB_SEEK_SET => 0i64,
        K_IB_SEEK_CUR => s.pos as i64,
        K_IB_SEEK_END => s.data.len() as i64,
        _ => return K_RESULT_FALSE,
    };
    // Allow seeking past end (a following write() will zero-fill the gap).
    s.pos = (base + pos).max(0) as usize;
    if !result.is_null() {
        *result = s.pos as i64;
    }
    K_RESULT_OK
}

unsafe extern "C" fn stream_tell(this: *mut HostStream, pos: *mut i64) -> TResult {
    if pos.is_null() {
        return -1;
    }
    *pos = (*this).pos as i64;
    K_RESULT_OK
}

static IBSTREAM_VTBL: IBStreamVtbl = IBStreamVtbl {
    query_interface: stream_query_interface,
    add_ref: stream_add_ref,
    release: stream_release,
    read: stream_read,
    write: stream_write,
    seek: stream_seek,
    tell: stream_tell,
};

impl HostStream {
    fn empty() -> Box<Self> {
        Box::new(HostStream {
            vtbl: &IBSTREAM_VTBL,
            data: Vec::new(),
            pos: 0,
        })
    }
    fn with_data(data: Vec<u8>) -> Box<Self> {
        Box::new(HostStream {
            vtbl: &IBSTREAM_VTBL,
            data,
            pos: 0,
        })
    }
}

/// Read the processor state of a VST3 component into a byte buffer via a
/// host-provided `IBStream` (`IComponent::getState`).  Returns `None` if the
/// plugin reports failure or writes no bytes.
///
/// # Safety
/// `comp` must be a valid, initialised `IComponent`.  Call off the audio thread
/// (see the threading caveat in `plugin_system::instance`).
pub unsafe fn read_component_state(comp: *mut IComponent) -> Option<Vec<u8>> {
    let mut stream = HostStream::empty();
    let ib = stream.as_mut() as *mut HostStream as *mut c_void;
    let r = ((*(*comp).vtbl).get_state)(comp, ib);
    if r != K_RESULT_OK {
        return None;
    }
    let data = std::mem::take(&mut stream.data);
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

/// Restore a previously-saved blob into a VST3 component
/// (`IComponent::setState`).  Returns `true` on `kResultOk`.
///
/// # Safety
/// As [`read_component_state`].
pub unsafe fn write_component_state(comp: *mut IComponent, bytes: &[u8]) -> bool {
    let mut stream = HostStream::with_data(bytes.to_vec());
    let ib = stream.as_mut() as *mut HostStream as *mut c_void;
    ((*(*comp).vtbl).set_state)(comp, ib) == K_RESULT_OK
}
