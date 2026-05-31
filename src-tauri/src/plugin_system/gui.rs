//! Native plugin GUI windows — Win32 container for VST3 IPlugView and CLAP clap.gui.
//!
//! ## Threading model
//!
//! All public functions must be called from the **Win32 main thread** (the
//! thread that runs Tauri's event loop).  Plugin GUI APIs are main-thread-only
//! per spec, and Win32 windows receive messages on the thread that created
//! them.  Use `app.run_on_main_thread(...)` at every call site.
//!
//! ## Window lifecycle
//!
//! 1. `open_*_editor` instantiates the plugin's GUI resources, builds a
//!    `WindowUserData` (with a `cleanup` `FnOnce` that owns every FFI handle),
//!    and calls `CreateWindowExW` passing the boxed userdata via `lpParam`.
//! 2. `WM_NCCREATE` installs the userdata pointer into `GWLP_USERDATA` *before*
//!    any other message can arrive — closing the W1 race where `attached` /
//!    `set_parent` could send messages while USERDATA was still zero.
//! 3. A `WM_TIMER` (16 ms) drains `on_main_thread` for CLAP (A4) — required
//!    for plugins like Vital that queue deferred GUI work after every user
//!    interaction.
//! 4. `WM_CLOSE` (× button, Alt-F4, `post_close`) takes USERDATA back via
//!    `Box::from_raw`, runs the `cleanup` closure (destroys plugin GUI, then
//!    plugin instance, in that order), removes the entry from `open_editors`,
//!    and calls `DestroyWindow`.
//!
//! ## CLAP `request_callback` dispatch (A3)
//!
//! Plugins may call `host->request_callback` from *any* thread.  The callback
//! synchronously dispatching `on_main_thread` would violate CLAP's threading
//! contract and crash on worker-thread calls.  We instead `PostMessageW` a
//! `WM_APP_DRAIN` to the container HWND; the main thread's WndProc drains via
//! the same `tick` closure used by `WM_TIMER`.

#![cfg(windows)]

use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::plugin_system::registry::PluginManifest;

// ── Win32 bindings ────────────────────────────────────────────────────────────

#[allow(non_snake_case)]
mod user32 {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct WNDCLASSEXW {
        pub cbSize:        u32,
        pub style:         u32,
        pub lpfnWndProc:   unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
        pub cbClsExtra:    i32,
        pub cbWndExtra:    i32,
        pub hInstance:     *mut c_void,
        pub hIcon:         *mut c_void,
        pub hCursor:       *mut c_void,
        pub hbrBackground: *mut c_void,
        pub lpszMenuName:  *const u16,
        pub lpszClassName: *const u16,
        pub hIconSm:       *mut c_void,
    }

    #[repr(C)]
    pub struct RECT { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }

    /// WM_NCCREATE payload (Windows `CREATESTRUCTW`).  Only the first field
    /// matters for us — `lpCreateParams` carries the `lpParam` we passed to
    /// `CreateWindowExW`, which is our boxed `WindowUserData` raw pointer.
    #[repr(C)]
    pub struct CREATESTRUCTW {
        pub lpCreateParams: *mut c_void,
        pub hInstance:      *mut c_void,
        pub hMenu:          *mut c_void,
        pub hwndParent:     *mut c_void,
        pub cy:             i32,
        pub cx:             i32,
        pub y:              i32,
        pub x:              i32,
        pub style:          i32,
        pub lpszName:       *const u16,
        pub lpszClass:      *const u16,
        pub dwExStyle:      u32,
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn RegisterClassExW(lpWndClass: *const WNDCLASSEXW) -> u16;
        pub fn CreateWindowExW(
            dwExStyle: u32, lpClassName: *const u16, lpWindowName: *const u16,
            dwStyle: u32, X: i32, Y: i32, nWidth: i32, nHeight: i32,
            hWndParent: *mut c_void, hMenu: *mut c_void,
            hInstance: *mut c_void, lpParam: *mut c_void,
        ) -> *mut c_void;
        pub fn ShowWindow(hWnd: *mut c_void, nCmdShow: i32) -> i32;
        pub fn UpdateWindow(hWnd: *mut c_void) -> i32;
        pub fn DestroyWindow(hWnd: *mut c_void) -> i32;
        pub fn DefWindowProcW(
            hWnd: *mut c_void, Msg: u32, wParam: usize, lParam: isize,
        ) -> isize;
        pub fn GetWindowLongPtrW(hWnd: *mut c_void, nIndex: i32) -> isize;
        pub fn SetWindowLongPtrW(hWnd: *mut c_void, nIndex: i32, dwNewLong: isize) -> isize;
        pub fn PostMessageW(hWnd: *mut c_void, Msg: u32, wParam: usize, lParam: isize) -> i32;
        pub fn AdjustWindowRectEx(
            lpRect: *mut RECT, dwStyle: u32, bMenu: i32, dwExStyle: u32,
        ) -> i32;
        pub fn SetWindowPos(
            hWnd: *mut c_void, hWndInsertAfter: *mut c_void,
            X: i32, Y: i32, cx: i32, cy: i32, uFlags: u32,
        ) -> i32;
        pub fn SetTimer(
            hWnd: *mut c_void, nIDEvent: usize, uElapse: u32,
            lpTimerFunc: *mut c_void,
        ) -> usize;
        pub fn KillTimer(hWnd: *mut c_void, uIDEvent: usize) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetModuleHandleW(lpModuleName: *const u16) -> *mut c_void;
    }
}

// ── Constants ─────────────────────────────────────────────────────────────────

const WM_NCCREATE:      u32 = 0x0081;
const WM_CLOSE:         u32 = 0x0010;
const WM_TIMER:         u32 = 0x0113;
/// `WM_APP + 1` — posted by `host->request_callback` from any thread.
const WM_APP_DRAIN:     u32 = 0x8000 + 1;
const GWLP_USERDATA:    i32 = -21;
const WS_CAPTION:       u32 = 0x00C0_0000;
const WS_SYSMENU:       u32 = 0x0008_0000;
const WS_POPUP:         u32 = 0x8000_0000;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const SW_SHOW:          i32 = 5;
const CW_USEDEFAULT:    i32 = 0x8000_0000u32 as i32;
/// SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE
const SWP_RESIZE_ONLY:  u32 = 0x0002 | 0x0004 | 0x0010;
const CLASS_NAME: &str = "EchoesPluginWindow";

const TIMER_ID_DRAIN:     usize = 1;
const TIMER_INTERVAL_MS:  u32   = 16;

/// Sentinel placed at the head of every valid `WindowUserData`.  Verified
/// before each USERDATA dereference (W2: protects against third-party hooks
/// or pathological plugins writing into `GWLP_USERDATA`).
const USERDATA_MAGIC: u64 = 0xEC_E0_5E_50_15_00_F1_00;

// ── Per-window data stored in GWLP_USERDATA ───────────────────────────────────

struct WindowUserData {
    magic:        u64,
    effect_id:    String,
    open_editors: Arc<Mutex<HashMap<String, usize>>>,
    /// Called on each `WM_TIMER` and `WM_APP_DRAIN` — drains pending
    /// main-thread tasks (CLAP `on_main_thread`).  No-op for VST3.
    tick:         Box<dyn Fn()>,
    /// Plugin-specific teardown (destroy GUI, destroy plugin, drop FFI Arcs).
    /// `Option` so it can be `.take()`-n out before invocation.
    cleanup:      Option<Box<dyn FnOnce()>>,
}

// ── WndProc ───────────────────────────────────────────────────────────────────

/// Safely read the userdata for this HWND, validating the magic sentinel.
unsafe fn userdata<'a>(hwnd: *mut c_void) -> Option<&'a mut WindowUserData> {
    let ptr = user32::GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowUserData;
    if ptr.is_null() { return None; }
    let ud = &mut *ptr;
    if ud.magic != USERDATA_MAGIC { return None; }
    Some(ud)
}

unsafe extern "system" fn plugin_wnd_proc(
    hwnd:   *mut c_void,
    msg:    u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    // ── WM_NCCREATE: install USERDATA before any other message ──
    // Without this, `attached` / `set_parent` could send messages (paint,
    // resize, focus, even WM_CLOSE) with USERDATA=0, and `DefWindowProc`
    // would destroy the window under the plugin's feet → UAF.
    if msg == WM_NCCREATE {
        let cs = lparam as *const user32::CREATESTRUCTW;
        if !cs.is_null() {
            let data_ptr = (*cs).lpCreateParams;
            if !data_ptr.is_null() {
                user32::SetWindowLongPtrW(hwnd, GWLP_USERDATA, data_ptr as isize);
            }
        }
        return user32::DefWindowProcW(hwnd, msg, wparam, lparam);
    }

    match msg {
        WM_TIMER | WM_APP_DRAIN => {
            if let Some(ud) = userdata(hwnd) {
                (ud.tick)();
                return 0;
            }
        }
        WM_CLOSE => {
            let ptr = user32::GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if ptr != 0 {
                // Atomically take ownership so a re-entrant WM_CLOSE can't
                // double-free.  Validate magic *after* take.
                user32::SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                user32::KillTimer(hwnd, TIMER_ID_DRAIN);

                let mut data: WindowUserData = *Box::from_raw(ptr as *mut WindowUserData);
                if data.magic == USERDATA_MAGIC {
                    let oe  = Arc::clone(&data.open_editors);
                    let eid = data.effect_id.clone();
                    if let Some(cb) = data.cleanup.take() { cb(); }
                    drop(data);
                    if let Ok(mut map) = oe.lock() {
                        map.remove(&eid);
                    };
                }
            }
            user32::DestroyWindow(hwnd);
            return 0;
        }
        _ => {}
    }

    user32::DefWindowProcW(hwnd, msg, wparam, lparam)
}

// ── Window class registration (once per process) ──────────────────────────────

fn ensure_class_registered() {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| unsafe {
        let class_wide: Vec<u16> = CLASS_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let wc = user32::WNDCLASSEXW {
            cbSize:        std::mem::size_of::<user32::WNDCLASSEXW>() as u32,
            style:         0x0020, // CS_OWNDC
            lpfnWndProc:   plugin_wnd_proc,
            cbClsExtra:    0,
            cbWndExtra:    0,
            hInstance:     user32::GetModuleHandleW(std::ptr::null()),
            hIcon:         null_mut(),
            hCursor:       null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName:  std::ptr::null(),
            lpszClassName: class_wide.as_ptr(),
            hIconSm:       null_mut(),
        };
        user32::RegisterClassExW(&wc);
    });
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn wto16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Create the Win32 container with `userdata` installed via `WM_NCCREATE`.
///
/// On success the window owns `userdata` (USERDATA = raw pointer); the caller
/// MUST NOT free it.  On failure the caller is given back the raw pointer to
/// drop manually.
unsafe fn create_container(
    title:    &str,
    client_w: i32,
    client_h: i32,
    userdata: *mut WindowUserData,
) -> Result<*mut c_void, String>
{
    ensure_class_registered();

    let style    = WS_POPUP | WS_CAPTION | WS_SYSMENU;
    let ex_style = WS_EX_TOOLWINDOW;

    // Expand from desired client size to window size (title bar + borders)
    let mut wr = user32::RECT {
        left: 0, top: 0, right: client_w.max(200), bottom: client_h.max(100),
    };
    user32::AdjustWindowRectEx(&mut wr, style, 0, ex_style);
    let ww = wr.right  - wr.left;
    let wh = wr.bottom - wr.top;

    let title_w = wto16(title);
    let class_w = wto16(CLASS_NAME);

    let hwnd = user32::CreateWindowExW(
        ex_style,
        class_w.as_ptr(), title_w.as_ptr(),
        style,
        CW_USEDEFAULT, CW_USEDEFAULT, ww, wh,
        null_mut(), null_mut(),
        user32::GetModuleHandleW(std::ptr::null()),
        userdata as *mut c_void,
    );

    if hwnd.is_null() {
        Err("Win32 CreateWindowExW failed for plugin container".into())
    } else {
        Ok(hwnd)
    }
}

/// Resize the window so the *client* area matches the given dimensions.
/// Called from the host-side resize handlers (CLAP `host_gui.request_resize`,
/// VST3 `IPlugFrame::resizeView`).
unsafe fn resize_to_client(hwnd: *mut c_void, client_w: i32, client_h: i32) {
    let style    = WS_POPUP | WS_CAPTION | WS_SYSMENU;
    let ex_style = WS_EX_TOOLWINDOW;
    let mut wr = user32::RECT {
        left: 0, top: 0,
        right:  client_w.max(200),
        bottom: client_h.max(100),
    };
    user32::AdjustWindowRectEx(&mut wr, style, 0, ex_style);
    user32::SetWindowPos(
        hwnd, null_mut(), 0, 0,
        wr.right - wr.left, wr.bottom - wr.top,
        SWP_RESIZE_ONLY,
    );
}

/// Show the window and install the 16 ms drain timer.
unsafe fn show_and_start_timer(hwnd: *mut c_void) {
    user32::ShowWindow(hwnd, SW_SHOW);
    user32::UpdateWindow(hwnd);
    user32::SetTimer(hwnd, TIMER_ID_DRAIN, TIMER_INTERVAL_MS, null_mut());
}

// ── CLAP host extensions and shared state ─────────────────────────────────────

use crate::plugin_formats::clap::ffi::{
    CLAP_EXT_HOST_GUI, CLAP_EXT_LOG, ClapHost, ClapHostGui, ClapHostLog,
    ClapLogSeverity, ClapPlugin, CLAP_LOG_DEBUG, CLAP_LOG_ERROR, CLAP_LOG_FATAL,
    CLAP_LOG_HOST_MISBEHAVING, CLAP_LOG_INFO, CLAP_LOG_PLUGIN_MISBEHAVING,
    CLAP_LOG_WARNING,
};

/// State shared between the CLAP host vtable callbacks (called from any thread)
/// and the main-thread WndProc.  Lives behind an `Arc` so multiple paths can
/// hold it without lifetime acrobatics.
struct ClapHostShared {
    /// Plugin pointer, written once after `create_plugin` succeeds and reset
    /// to null at the start of cleanup so any in-flight `tick` no-ops safely.
    plugin:    AtomicPtr<ClapPlugin>,
    /// Container HWND, written after `CreateWindowExW`; `request_callback`
    /// posts to it.
    host_hwnd: AtomicPtr<c_void>,
}

unsafe extern "C" fn clap_host_request_callback(host: *const ClapHost) {
    if host.is_null() { return; }
    let data = (*host).host_data;
    if data.is_null() { return; }
    let shared = &*(data as *const ClapHostShared);
    let hwnd = shared.host_hwnd.load(Ordering::Acquire);
    if hwnd.is_null() { return; }
    // PostMessage is thread-safe and queues into the target thread's queue —
    // the only sound way to honour `on_main_thread`'s [main-thread] contract
    // when `request_callback` may be invoked from a plugin worker thread.
    user32::PostMessageW(hwnd, WM_APP_DRAIN, 0, 0);
}

unsafe extern "C" fn clap_host_request_restart(_host: *const ClapHost) {
    // No-op: restart requires deactivate/activate on the audio thread, which
    // is out of scope for the GUI-only path.
}

unsafe extern "C" fn clap_host_request_process(_host: *const ClapHost) {
    // No-op: GUI-only instance is never activated for processing.
}

// ── clap.log host extension ──

unsafe extern "C" fn clap_host_log_log(
    _host: *const ClapHost,
    severity: ClapLogSeverity,
    msg: *const c_char,
) {
    if msg.is_null() { return; }
    let s = std::ffi::CStr::from_ptr(msg).to_string_lossy();
    let label = match severity {
        CLAP_LOG_DEBUG              => "DBG",
        CLAP_LOG_INFO               => "INF",
        CLAP_LOG_WARNING            => "WRN",
        CLAP_LOG_ERROR              => "ERR",
        CLAP_LOG_FATAL              => "FTL",
        CLAP_LOG_HOST_MISBEHAVING   => "HOST!",
        CLAP_LOG_PLUGIN_MISBEHAVING => "PLUG!",
        _ => "???",
    };
    eprintln!("[clap.{label}] {s}");
}

static CLAP_LOG_VTBL: ClapHostLog = ClapHostLog { log: clap_host_log_log };

// ── clap.gui host extension ──

unsafe extern "C" fn clap_host_gui_resize_hints_changed(_host: *const ClapHost) {}

unsafe extern "C" fn clap_host_gui_request_resize(
    host: *const ClapHost, w: u32, h: u32,
) -> bool {
    if host.is_null() { return false; }
    let data = (*host).host_data;
    if data.is_null() { return false; }
    let shared = &*(data as *const ClapHostShared);
    let hwnd = shared.host_hwnd.load(Ordering::Acquire);
    if hwnd.is_null() { return false; }
    resize_to_client(hwnd, w.max(1) as i32, h.max(1) as i32);
    true
}

unsafe extern "C" fn clap_host_gui_request_show(_host: *const ClapHost) -> bool { true }
unsafe extern "C" fn clap_host_gui_request_hide(_host: *const ClapHost) -> bool { true }

unsafe extern "C" fn clap_host_gui_closed(host: *const ClapHost, _was_destroyed: bool) {
    if host.is_null() { return; }
    let data = (*host).host_data;
    if data.is_null() { return; }
    let shared = &*(data as *const ClapHostShared);
    let hwnd = shared.host_hwnd.load(Ordering::Acquire);
    if !hwnd.is_null() {
        user32::PostMessageW(hwnd, WM_CLOSE, 0, 0);
    }
}

static CLAP_HOST_GUI_VTBL: ClapHostGui = ClapHostGui {
    resize_hints_changed: clap_host_gui_resize_hints_changed,
    request_resize:       clap_host_gui_request_resize,
    request_show:         clap_host_gui_request_show,
    request_hide:         clap_host_gui_request_hide,
    closed:               clap_host_gui_closed,
};

// ── host_get_extension router ──

unsafe extern "C" fn gui_host_get_extension(
    _host: *const ClapHost,
    id:    *const c_char,
) -> *const c_void {
    if id.is_null() { return null(); }
    let id_bytes = std::ffi::CStr::from_ptr(id).to_bytes_with_nul();
    if id_bytes == CLAP_EXT_LOG {
        return &CLAP_LOG_VTBL      as *const _ as *const c_void;
    }
    if id_bytes == CLAP_EXT_HOST_GUI {
        return &CLAP_HOST_GUI_VTBL as *const _ as *const c_void;
    }
    null()
}

// ── VST3 host-side COM objects ────────────────────────────────────────────────

use crate::plugin_formats::vst3::ffi::{
    FUnknown, IComponentHandlerVtbl, IHostApplicationVtbl, IPlugFrameVtbl,
    IPlugView, IID_FUNKNOWN, IID_ICOMPONENT_HANDLER, IID_IHOST_APPLICATION,
    IID_IPLUG_FRAME, K_RESULT_FALSE, K_RESULT_OK, TResult, Tuid, ViewRect,
};

fn tuid_eq(a: *const Tuid, b: &Tuid) -> bool {
    if a.is_null() { return false; }
    unsafe { (*a) == *b }
}

unsafe extern "C" fn vst3_refcount_addref(_: *mut FUnknown) -> u32 { 1 }
unsafe extern "C" fn vst3_refcount_release(_: *mut FUnknown) -> u32 { 1 }

// ── IHostApplication ──

unsafe extern "C" fn host_app_qi(
    this: *mut FUnknown, iid: *const Tuid, obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() { return -1; }
    if tuid_eq(iid, &IID_IHOST_APPLICATION) || tuid_eq(iid, &IID_FUNKNOWN) {
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = null_mut();
    -1
}

unsafe extern "C" fn host_app_get_name(_this: *mut FUnknown, name: *mut u16) -> TResult {
    if name.is_null() { return -1; }
    let host_name = "Echoes Studio";
    let bytes: Vec<u16> = host_name.encode_utf16().take(127).collect();
    for (i, &b) in bytes.iter().enumerate() { *name.add(i) = b; }
    *name.add(bytes.len()) = 0;
    K_RESULT_OK
}

unsafe extern "C" fn host_app_create_instance(
    _this: *mut FUnknown,
    _cid:  *const Tuid,
    _iid:  *const Tuid,
    obj:   *mut *mut c_void,
) -> TResult {
    // We don't expose IMessage / IAttributeList yet; plugins fall back.
    if !obj.is_null() { *obj = null_mut(); }
    K_RESULT_FALSE
}

static HOST_APP_VTBL: IHostApplicationVtbl = IHostApplicationVtbl {
    query_interface: host_app_qi,
    add_ref:         vst3_refcount_addref,
    release:         vst3_refcount_release,
    get_name:        host_app_get_name,
    create_instance: host_app_create_instance,
};

#[repr(C)]
struct HostApplication { vtbl: *const IHostApplicationVtbl }

// SAFETY: `vtbl` is a `'static` pointer to an immutable static; all callbacks
// are stateless and re-entrant-safe.
unsafe impl Sync for HostApplication {}

static HOST_APP: HostApplication = HostApplication { vtbl: &HOST_APP_VTBL };

fn host_app_as_funknown() -> *mut FUnknown {
    &HOST_APP as *const HostApplication as *mut FUnknown
}

// ── IComponentHandler ──

unsafe extern "C" fn handler_qi(
    this: *mut FUnknown, iid: *const Tuid, obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() { return -1; }
    if tuid_eq(iid, &IID_ICOMPONENT_HANDLER) || tuid_eq(iid, &IID_FUNKNOWN) {
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = null_mut();
    -1
}

unsafe extern "C" fn handler_begin_edit(_: *mut FUnknown, _: u32) -> TResult { K_RESULT_OK }
unsafe extern "C" fn handler_perform_edit(_: *mut FUnknown, _: u32, _: f64) -> TResult { K_RESULT_OK }
unsafe extern "C" fn handler_end_edit(_: *mut FUnknown, _: u32) -> TResult { K_RESULT_OK }
unsafe extern "C" fn handler_restart_component(_: *mut FUnknown, _: i32) -> TResult { K_RESULT_OK }

static HANDLER_VTBL: IComponentHandlerVtbl = IComponentHandlerVtbl {
    query_interface:   handler_qi,
    add_ref:           vst3_refcount_addref,
    release:           vst3_refcount_release,
    begin_edit:        handler_begin_edit,
    perform_edit:      handler_perform_edit,
    end_edit:          handler_end_edit,
    restart_component: handler_restart_component,
};

#[repr(C)]
struct ComponentHandler { vtbl: *const IComponentHandlerVtbl }

// SAFETY: see HostApplication above — stateless static, vtbl is `'static`.
unsafe impl Sync for ComponentHandler {}

static COMPONENT_HANDLER: ComponentHandler = ComponentHandler { vtbl: &HANDLER_VTBL };

fn component_handler_as_funknown() -> *mut FUnknown {
    &COMPONENT_HANDLER as *const ComponentHandler as *mut FUnknown
}

// ── IPlugFrame ──
// Per-editor instance so `resizeView` can route to the right container HWND.

#[repr(C)]
struct PlugFrame {
    vtbl:           *const IPlugFrameVtbl,
    container_hwnd: AtomicPtr<c_void>,
}

unsafe extern "C" fn frame_qi(
    this: *mut FUnknown, iid: *const Tuid, obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() { return -1; }
    if tuid_eq(iid, &IID_IPLUG_FRAME) || tuid_eq(iid, &IID_FUNKNOWN) {
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = null_mut();
    -1
}

unsafe extern "C" fn frame_resize_view(
    this: *mut FUnknown, _view: *mut IPlugView, rect: *mut ViewRect,
) -> TResult {
    if this.is_null() || rect.is_null() { return -1; }
    let frame = this as *const PlugFrame;
    let hwnd  = (*frame).container_hwnd.load(Ordering::Acquire);
    if hwnd.is_null() { return -1; }
    let r  = &*rect;
    let cw = (r.right  - r.left).max(1);
    let ch = (r.bottom - r.top ).max(1);
    resize_to_client(hwnd, cw, ch);
    K_RESULT_OK
}

static PLUG_FRAME_VTBL: IPlugFrameVtbl = IPlugFrameVtbl {
    query_interface: frame_qi,
    add_ref:         vst3_refcount_addref,
    release:         vst3_refcount_release,
    resize_view:     frame_resize_view,
};

fn make_plug_frame() -> Box<PlugFrame> {
    Box::new(PlugFrame {
        vtbl: &PLUG_FRAME_VTBL,
        container_hwnd: AtomicPtr::new(null_mut()),
    })
}

// ── VST3 editor ───────────────────────────────────────────────────────────────

/// Open a VST3 native editor window (main thread only).
///
/// # Safety
///
/// Must be called on the Win32 main thread.  All COM pointers obtained here
/// stay live until the WndProc invokes the cleanup closure on `WM_CLOSE`.
pub unsafe fn open_vst3_editor(
    manifest:     &PluginManifest,
    name:         &str,
    effect_id:    &str,
    open_editors: Arc<Mutex<HashMap<String, usize>>>,
) -> Result<usize, String> {
    use crate::plugin_formats::vst3::ffi::{
        IComponent, IEditController, Vst3Library,
        IID_ICOMPONENT, IID_IEDIT_CONTROLLER,
    };
    use crate::plugin_formats::vst3::host::decode_plugin_id as vst3_decode;

    let dll = manifest.path.as_ref()
        .ok_or_else(|| "VST3 GUI: no DLL path in manifest".to_string())?;
    let cid = vst3_decode(&manifest.id)
        .ok_or_else(|| "VST3 GUI: invalid plugin_id format".to_string())?;

    let (lib, factory) = Vst3Library::load(dll)?;

    // Create IComponent (GUI-only instance — never activated for audio)
    let mut comp_raw: *mut c_void = null_mut();
    let r = ((*(*factory).vtbl).create_instance)(
        factory, cid.as_ptr(), IID_ICOMPONENT.as_ptr(), &mut comp_raw,
    );
    ((*(*factory).vtbl).release)(factory);
    if r != K_RESULT_OK || comp_raw.is_null() {
        return Err(format!("VST3 GUI: createInstance(IComponent) failed: {r}"));
    }
    let comp = comp_raw as *mut IComponent;

    // V1: check `initialize`.  Pass our minimal IHostApplication so plugins
    // that QI for it succeed; plugins that don't need it still see a non-null
    // context, which a number of them assume.
    let init_r = ((*(*comp).vtbl).initialize)(comp, host_app_as_funknown());
    if init_r != K_RESULT_OK && init_r != K_RESULT_FALSE {
        ((*(*comp).vtbl).release)(comp as *mut _);
        return Err(format!("VST3 GUI: IComponent::initialize failed: {init_r}"));
    }

    // Query IEditController via QI (single-component model only — dual-
    // component plugins return a separate CID via getControllerClassId,
    // unsupported here).
    let mut ctrl_raw: *mut c_void = null_mut();
    let r = ((*(*comp).vtbl).query_interface)(
        comp,
        IID_IEDIT_CONTROLLER.as_ptr() as *const Tuid,
        &mut ctrl_raw,
    );
    if r != K_RESULT_OK || ctrl_raw.is_null() {
        ((*(*comp).vtbl).terminate)(comp);
        ((*(*comp).vtbl).release)(comp as *mut _);
        return Err("VST3 GUI: IEditController not available via QI (single-component only)".into());
    }
    let ctrl = ctrl_raw as *mut IEditController;
    // ctrl and comp are the same COM object; QI added +1 refcount (total = 2).

    // V2: install component handler so the plugin can begin/perform/end edits.
    // Many plugins assume a non-null handler and crash on the first knob touch.
    let _ = ((*(*ctrl).vtbl).set_component_handler)(ctrl, component_handler_as_funknown());

    // Create view — DO NOT call ctrl.initialize (comp.initialize already did it
    // for the shared object).
    let view = ((*(*ctrl).vtbl).create_view)(ctrl, b"editor\0".as_ptr());
    if view.is_null() {
        ((*(*ctrl).vtbl).terminate)(ctrl);
        ((*(*ctrl).vtbl).release)(ctrl as *mut _);
        ((*(*comp).vtbl).release)(comp as *mut _);
        return Err("VST3 GUI: IEditController::createView returned null".into());
    }

    if ((*(*view).vtbl).is_platform_type_supported)(view, b"HWND\0".as_ptr()) != K_RESULT_OK {
        ((*(*view).vtbl).release)(view as *mut _);
        ((*(*ctrl).vtbl).terminate)(ctrl);
        ((*(*ctrl).vtbl).release)(ctrl as *mut _);
        ((*(*comp).vtbl).release)(comp as *mut _);
        return Err("VST3 GUI: plugin view does not support HWND".into());
    }

    // V3: allocate IPlugFrame and set it on the view *before* attached().
    // The Box must outlive the view; ownership is transferred to cleanup.
    // We also keep a raw *const PlugFrame to wire `container_hwnd` after
    // CreateWindowExW returns — safe because Box address is stable.
    let plug_frame      = make_plug_frame();
    let plug_frame_ptr  = &*plug_frame as *const PlugFrame;
    let frame_funknown  = plug_frame_ptr as *mut FUnknown;
    let _ = ((*(*view).vtbl).set_frame)(view, frame_funknown);

    // Probe preferred size (may return zero before attach; that's OK).
    let mut rect = ViewRect { left: 0, top: 0, right: 800, bottom: 600 };
    let _ = ((*(*view).vtbl).get_size)(view, &mut rect);
    let probe_w = (rect.right  - rect.left).max(1);
    let probe_h = (rect.bottom - rect.top ).max(1);

    // Build cleanup + tick closures (own all FFI handles)
    let lib_owned = lib;
    let frame_owned = plug_frame;
    let cleanup: Box<dyn FnOnce()> = Box::new(move || {
        let _frame = frame_owned;
        let _lib   = lib_owned;
        unsafe {
            ((*(*view).vtbl).removed)(view);
            // Drop our frame ref before terminating ctrl — set_frame(null).
            let _ = ((*(*view).vtbl).set_frame)(view, null_mut());
            ((*(*view).vtbl).release)(view as *mut _);
            ((*(*ctrl).vtbl).terminate)(ctrl);
            ((*(*ctrl).vtbl).release)(ctrl as *mut _);
            ((*(*comp).vtbl).release)(comp as *mut _);
        }
    });
    let tick: Box<dyn Fn()> = Box::new(|| {});

    let userdata = Box::new(WindowUserData {
        magic: USERDATA_MAGIC,
        effect_id: effect_id.to_string(),
        open_editors: Arc::clone(&open_editors),
        tick,
        cleanup: Some(cleanup),
    });
    let userdata_raw = Box::into_raw(userdata);

    // Create container — USERDATA installed via WM_NCCREATE
    let hwnd = match create_container(name, probe_w, probe_h, userdata_raw) {
        Ok(h) => h,
        Err(e) => {
            // Recover ownership and run cleanup ourselves
            let mut d = *Box::from_raw(userdata_raw);
            if let Some(cb) = d.cleanup.take() { cb(); }
            return Err(e);
        }
    };

    // Wire plug_frame -> this HWND so resizeView routes correctly.
    // plug_frame_ptr is valid: the Box is captured by `cleanup` which is
    // owned by the WindowUserData now installed in USERDATA.
    (*plug_frame_ptr).container_hwnd.store(hwnd, Ordering::Release);

    // Attach
    let attach_r = ((*(*view).vtbl).attached)(view, hwnd, b"HWND\0".as_ptr());
    if attach_r != K_RESULT_OK {
        // Cleanup runs through WM_CLOSE — we just trigger it.
        user32::PostMessageW(hwnd, WM_CLOSE, 0, 0);
        return Err(format!("VST3 GUI: IPlugView::attached failed: {attach_r}"));
    }

    // Re-query size after attach (plugin may know real size only post-attach)
    let mut rect2 = ViewRect { left: 0, top: 0, right: probe_w, bottom: probe_h };
    if ((*(*view).vtbl).get_size)(view, &mut rect2) == K_RESULT_OK {
        let cw = (rect2.right  - rect2.left).max(100);
        let ch = (rect2.bottom - rect2.top ).max(100);
        resize_to_client(hwnd, cw, ch);
    }

    show_and_start_timer(hwnd);

    if let Ok(mut map) = open_editors.lock() {
        map.insert(effect_id.to_string(), hwnd as usize);
    }

    Ok(hwnd as usize)
}

// ── CLAP editor ───────────────────────────────────────────────────────────────

/// Open a CLAP native editor window (main thread only).
///
/// # Safety
///
/// Must be called on the Win32 main thread.  `clap_entry.init` was called
/// once by `ClapLibrary::load`, which caches loaded DLLs process-wide.
pub unsafe fn open_clap_editor(
    manifest:     &PluginManifest,
    name:         &str,
    effect_id:    &str,
    open_editors: Arc<Mutex<HashMap<String, usize>>>,
) -> Result<usize, String> {
    use crate::plugin_formats::clap::ffi::{
        ClapGuiExtension, ClapLibrary, ClapPluginFactory, ClapWindow,
        CLAP_EXT_GUI, CLAP_PLUGIN_FACTORY_ID, CLAP_WINDOW_API_WIN32, make_host,
    };
    use crate::plugin_formats::clap::host::decode_plugin_id as clap_decode;

    let dll = manifest.path.as_ref()
        .ok_or_else(|| "CLAP GUI: no DLL path in manifest".to_string())?;
    let clap_id = clap_decode(&manifest.id)
        .ok_or_else(|| "CLAP GUI: invalid plugin_id format".to_string())?;

    // C1 fix: load() handles init() exactly once per DLL per process.
    let (lib, entry) = ClapLibrary::load(dll)?;

    let factory_raw = ((*entry).get_factory)(
        CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const c_char,
    );
    if factory_raw.is_null() {
        return Err("CLAP GUI: get_factory returned null".into());
    }
    let factory = factory_raw as *const ClapPluginFactory;

    // CL1 fix: shared state with AtomicPtr — no torn reads from worker threads
    let shared = Arc::new(ClapHostShared {
        plugin:    AtomicPtr::new(null_mut()),
        host_hwnd: AtomicPtr::new(null_mut()),
    });

    // Build host (heap-stable: Box address is preserved on move).  Wire
    // host_data to the inside of the shared Arc; the Arc is captured by
    // `tick` and `cleanup` so it outlives the plugin.
    let mut host = make_host();
    host.host_data        = Arc::as_ptr(&shared) as *mut c_void;
    host.request_callback = clap_host_request_callback;
    host.request_restart  = clap_host_request_restart;
    host.request_process  = clap_host_request_process;
    host.get_extension    = gui_host_get_extension;

    // Keep clap_id bytes alive until plugin.destroy (CL4: cheap robustness
    // against plugins that store the id pointer past create_plugin's return).
    let clap_id_c: Vec<u8> = clap_id.bytes().chain(std::iter::once(0)).collect();
    let plugin = ((*factory).create_plugin)(
        factory, host.as_ref(), clap_id_c.as_ptr() as *const c_char,
    );
    if plugin.is_null() {
        return Err(format!("CLAP GUI: create_plugin null for '{clap_id}'"));
    }

    shared.plugin.store(plugin as *mut ClapPlugin, Ordering::Release);

    if !((*plugin).init)(plugin) {
        ((*plugin).destroy)(plugin);
        return Err("CLAP GUI: plugin.init failed".into());
    }

    let gui_raw = ((*plugin).get_extension)(
        plugin, CLAP_EXT_GUI.as_ptr() as *const c_char,
    );
    if gui_raw.is_null() {
        ((*plugin).destroy)(plugin);
        return Err("CLAP GUI: plugin does not expose clap.gui extension".into());
    }
    let gui = gui_raw as *const ClapGuiExtension;

    if !((*gui).is_api_supported)(plugin, CLAP_WINDOW_API_WIN32.as_ptr() as *const c_char, false) {
        ((*plugin).destroy)(plugin);
        return Err("CLAP GUI: plugin clap.gui does not support Win32".into());
    }

    if !((*gui).create)(plugin, CLAP_WINDOW_API_WIN32.as_ptr() as *const c_char, false) {
        ((*plugin).destroy)(plugin);
        return Err("CLAP GUI: clap_gui.create failed".into());
    }

    let mut pw: u32 = 800;
    let mut ph: u32 = 600;
    let _ = ((*gui).get_size)(plugin, &mut pw, &mut ph);

    // CL2 fix: build cleanup *first*, in correct drop order
    //   1. shared.plugin = null    → tick no-ops if it races
    //   2. gui.hide / destroy      → release plugin GUI resources
    //   3. plugin.destroy          → no more callbacks possible
    //   4. shared / host / lib / clap_id_c drop                  (captures)
    let lib_owned = lib;
    let host_owned = host;
    let shared_for_cleanup = Arc::clone(&shared);
    let clap_id_owned = clap_id_c;
    let cleanup: Box<dyn FnOnce()> = Box::new(move || {
        let _lib  = lib_owned;
        let _host = host_owned;
        let _clap_id = clap_id_owned;
        // Disable callbacks before touching the plugin
        shared_for_cleanup.plugin.store(null_mut(), Ordering::Release);
        unsafe {
            let _ = ((*gui).hide)(plugin);
            ((*gui).destroy)(plugin);
            ((*plugin).destroy)(plugin);
        }
        drop(shared_for_cleanup);
    });

    // tick: drain on_main_thread.  Reads plugin via AtomicPtr — sees null
    // once cleanup has begun, so it can't race with destroy.
    let shared_for_tick = Arc::clone(&shared);
    let tick: Box<dyn Fn()> = Box::new(move || {
        let p = shared_for_tick.plugin.load(Ordering::Acquire);
        if !p.is_null() {
            unsafe { ((*p).on_main_thread)(p); }
        }
    });

    let userdata = Box::new(WindowUserData {
        magic: USERDATA_MAGIC,
        effect_id: effect_id.to_string(),
        open_editors: Arc::clone(&open_editors),
        tick,
        cleanup: Some(cleanup),
    });
    let userdata_raw = Box::into_raw(userdata);

    let hwnd = match create_container(name, pw.max(100) as i32, ph.max(100) as i32, userdata_raw) {
        Ok(h) => h,
        Err(e) => {
            let mut d = *Box::from_raw(userdata_raw);
            if let Some(cb) = d.cleanup.take() { cb(); }
            return Err(e);
        }
    };

    // From this point on, ANY error path triggers cleanup via WM_CLOSE.
    shared.host_hwnd.store(hwnd, Ordering::Release);

    let clap_win = ClapWindow {
        api:      CLAP_WINDOW_API_WIN32.as_ptr() as *const c_char,
        specific: hwnd as usize,
    };
    if !((*gui).set_parent)(plugin, &clap_win) {
        user32::PostMessageW(hwnd, WM_CLOSE, 0, 0);
        return Err("CLAP GUI: clap_gui.set_parent failed".into());
    }

    let _ = ((*gui).show)(plugin);

    // A4: drain any deferred work the plugin queued during create/set_parent/
    // show.  Ongoing drainage is handled by WM_TIMER + WM_APP_DRAIN.
    ((*plugin).on_main_thread)(plugin);

    show_and_start_timer(hwnd);

    if let Ok(mut map) = open_editors.lock() {
        map.insert(effect_id.to_string(), hwnd as usize);
    }

    Ok(hwnd as usize)
}

// ── Programmatic close ────────────────────────────────────────────────────────

/// Post `WM_CLOSE` to the plugin window from any thread.
/// The WndProc handles cleanup on the main thread.
pub fn post_close(hwnd: usize) {
    if hwnd != 0 {
        unsafe {
            user32::PostMessageW(hwnd as *mut c_void, WM_CLOSE, 0, 0);
        }
    }
}
