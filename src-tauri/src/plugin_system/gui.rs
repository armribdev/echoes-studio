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
//! 3. A `WM_TIMER` (every Windows timer tick, ~15.6 ms) runs the plugin's
//!    `clap.timer-support` timers that are due.  They run only while this
//!    window exists; `on_main_thread` is drained by `CLAP_GUI_DRAIN_MESSAGE`
//!    (A4, below), not by the timer.
//! 4. `WM_CLOSE` (× button, Alt-F4, `post_close`) takes USERDATA back via
//!    `Box::from_raw`, runs the `cleanup` closure (destroys plugin GUI, then
//!    plugin instance, in that order), removes the entry from `open_editors`,
//!    and calls `DestroyWindow`.
//!
//! ## CLAP `request_callback` dispatch (A3)
//!
//! Plugins may call `host->request_callback` from *any* thread.  The callback
//! synchronously dispatching `on_main_thread` would violate CLAP's threading
//! contract and crash on worker-thread calls.  The host instead posts
//! `CLAP_GUI_DRAIN_MESSAGE` to the editor HWND stored in that plugin's own
//! `ClapHostContext`; the WndProc drains via the `tick` closure.  Plugin-initiated
//! `clap.gui` resizes arrive the same way (`CLAP_GUI_RESIZE_MESSAGE`).

#![cfg(windows)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::plugin_formats::clap::ffi::{
    TimerSchedule, CLAP_GUI_DRAIN_MESSAGE, CLAP_GUI_RESIZE_MESSAGE,
};
use crate::plugin_system::crash_marker::PluginThread;
use crate::plugin_system::instance::PluginInstance;

#[allow(non_snake_case, clippy::upper_case_acronyms)] // Win32 ABI type names
mod user32 {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct WNDCLASSEXW {
        pub cbSize: u32,
        pub style: u32,
        pub lpfnWndProc: unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
        pub cbClsExtra: i32,
        pub cbWndExtra: i32,
        pub hInstance: *mut c_void,
        pub hIcon: *mut c_void,
        pub hCursor: *mut c_void,
        pub hbrBackground: *mut c_void,
        pub lpszMenuName: *const u16,
        pub lpszClassName: *const u16,
        pub hIconSm: *mut c_void,
    }

    #[repr(C)]
    pub struct RECT {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    #[repr(C)]
    pub struct POINT {
        pub x: i32,
        pub y: i32,
    }

    #[repr(C)]
    pub struct PAINTSTRUCT {
        pub hdc: *mut c_void,
        pub f_erase: i32,
        pub rc_paint: RECT,
        pub f_restore: i32,
        pub f_inc_update: i32,
        pub rgb_reserved: [u8; 32],
    }

    #[repr(C)]
    pub struct TRACKMOUSEEVENT {
        pub cb_size: u32,
        pub dw_flags: u32,
        pub hwnd_track: *mut c_void,
        pub dw_hover_time: u32,
    }

    /// WM_NCCREATE payload (Windows `CREATESTRUCTW`).  Only the first field
    /// matters for us — `lpCreateParams` carries the `lpParam` we passed to
    /// `CreateWindowExW`, which is our boxed `WindowUserData` raw pointer.
    #[repr(C)]
    pub struct CREATESTRUCTW {
        pub lpCreateParams: *mut c_void,
        pub hInstance: *mut c_void,
        pub hMenu: *mut c_void,
        pub hwndParent: *mut c_void,
        pub cy: i32,
        pub cx: i32,
        pub y: i32,
        pub x: i32,
        pub style: i32,
        pub lpszName: *const u16,
        pub lpszClass: *const u16,
        pub dwExStyle: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn RegisterClassExW(lpWndClass: *const WNDCLASSEXW) -> u16;
        pub fn CreateWindowExW(
            dwExStyle: u32,
            lpClassName: *const u16,
            lpWindowName: *const u16,
            dwStyle: u32,
            X: i32,
            Y: i32,
            nWidth: i32,
            nHeight: i32,
            hWndParent: *mut c_void,
            hMenu: *mut c_void,
            hInstance: *mut c_void,
            lpParam: *mut c_void,
        ) -> *mut c_void;
        pub fn ShowWindow(hWnd: *mut c_void, nCmdShow: i32) -> i32;
        pub fn UpdateWindow(hWnd: *mut c_void) -> i32;
        pub fn DestroyWindow(hWnd: *mut c_void) -> i32;
        pub fn DefWindowProcW(hWnd: *mut c_void, Msg: u32, wParam: usize, lParam: isize) -> isize;
        pub fn GetWindowLongPtrW(hWnd: *mut c_void, nIndex: i32) -> isize;
        pub fn SetWindowLongPtrW(hWnd: *mut c_void, nIndex: i32, dwNewLong: isize) -> isize;
        pub fn PostMessageW(hWnd: *mut c_void, Msg: u32, wParam: usize, lParam: isize) -> i32;
        pub fn GetDpiForWindow(hWnd: *mut c_void) -> u32;
        pub fn GetWindow(hWnd: *mut c_void, uCmd: u32) -> *mut c_void;
        pub fn GetClientRect(hWnd: *mut c_void, lpRect: *mut RECT) -> i32;
        pub fn GetSystemMetrics(nIndex: i32) -> i32;
        pub fn ScreenToClient(hWnd: *mut c_void, lpPoint: *mut POINT) -> i32;
        pub fn BeginPaint(hWnd: *mut c_void, lpPaint: *mut PAINTSTRUCT) -> *mut c_void;
        pub fn EndPaint(hWnd: *mut c_void, lpPaint: *const PAINTSTRUCT) -> i32;
        pub fn SelectClipRgn(hdc: *mut c_void, hrgn: *mut c_void) -> i32;
        pub fn InvalidateRect(hWnd: *mut c_void, lpRect: *const RECT, bErase: i32) -> i32;
        pub fn TrackMouseEvent(lpEventTrack: *mut TRACKMOUSEEVENT) -> i32;
        pub fn SetWindowPos(
            hWnd: *mut c_void,
            hWndInsertAfter: *mut c_void,
            X: i32,
            Y: i32,
            cx: i32,
            cy: i32,
            uFlags: u32,
        ) -> i32;
        pub fn SetTimer(
            hWnd: *mut c_void,
            nIDEvent: usize,
            uElapse: u32,
            lpTimerFunc: *mut c_void,
        ) -> usize;
        pub fn KillTimer(hWnd: *mut c_void, uIDEvent: usize) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetModuleHandleW(lpModuleName: *const u16) -> *mut c_void;
    }

    #[link(name = "gdi32")]
    extern "system" {
        pub fn CreateSolidBrush(color: u32) -> *mut c_void;
    }

    #[link(name = "dwmapi")]
    extern "system" {
        pub fn DwmSetWindowAttribute(
            hWnd: *mut c_void,
            dwAttribute: u32,
            pvAttribute: *const c_void,
            cbAttribute: u32,
        ) -> i32;
    }
}

/// Minimal bindings for the GDI+ flat C API — only what the close glyph needs.
/// GDI lines have no anti-aliasing; GDI+ is the standard way to get it on
/// Windows without a graphics library dependency (gdiplus.dll ships with the OS).
#[allow(non_snake_case)]
mod gdiplus {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct StartupInput {
        pub gdiplus_version: u32,
        pub debug_event_callback: *mut c_void,
        pub suppress_background_thread: i32,
        pub suppress_external_codecs: i32,
    }

    #[link(name = "gdiplus")]
    extern "system" {
        pub fn GdiplusStartup(
            token: *mut usize,
            input: *const StartupInput,
            output: *mut c_void,
        ) -> i32;
        pub fn GdipCreateFromHDC(hdc: *mut c_void, graphics: *mut *mut c_void) -> i32;
        pub fn GdipDeleteGraphics(graphics: *mut c_void) -> i32;
        pub fn GdipSetSmoothingMode(graphics: *mut c_void, mode: i32) -> i32;
        pub fn GdipCreatePen1(argb: u32, width: f32, unit: i32, pen: *mut *mut c_void) -> i32;
        pub fn GdipSetPenStartCap(pen: *mut c_void, cap: i32) -> i32;
        pub fn GdipSetPenEndCap(pen: *mut c_void, cap: i32) -> i32;
        pub fn GdipDeletePen(pen: *mut c_void) -> i32;
        pub fn GdipDrawLineI(
            graphics: *mut c_void,
            pen: *mut c_void,
            x1: i32,
            y1: i32,
            x2: i32,
            y2: i32,
        ) -> i32;
    }
}

const GDIP_SMOOTHING_ANTIALIAS: i32 = 4;
const GDIP_UNIT_PIXEL: i32 = 2;
const GDIP_LINE_CAP_ROUND: i32 = 2;

/// Starts the process's one GDI+ session (idempotent, never torn down —
/// the OS reclaims it on exit like any other DLL state).
fn ensure_gdiplus() {
    static TOKEN: OnceLock<usize> = OnceLock::new();
    TOKEN.get_or_init(|| unsafe {
        let input = gdiplus::StartupInput {
            gdiplus_version: 1,
            debug_event_callback: null_mut(),
            suppress_background_thread: 0,
            suppress_external_codecs: 0,
        };
        let mut token: usize = 0;
        gdiplus::GdiplusStartup(&mut token, &input, null_mut());
        token
    });
}

const WM_NCCREATE: u32 = 0x0081;
const WM_NCHITTEST: u32 = 0x0084;
const WM_CLOSE: u32 = 0x0010;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_PAINT: u32 = 0x000F;
const TME_LEAVE: u32 = 0x0000_0002;
const HOVER_DEFAULT: u32 = 0xFFFF_FFFF;
const WM_TIMER: u32 = 0x0113;
const WM_DPICHANGED: u32 = 0x02E0;
const USER_DEFAULT_SCREEN_DPI: u32 = 96;
const GWLP_USERDATA: i32 = -21;
const WS_POPUP: u32 = 0x8000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const CONTAINER_STYLE: u32 = WS_POPUP | WS_CLIPCHILDREN;
const CONTAINER_EX_STYLE: u32 = WS_EX_TOOLWINDOW;
const PLUGIN_AREA_STYLE: u32 = WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN;
const GW_CHILD: u32 = 5;
const HTCLIENT: isize = 1;
const HTCAPTION: isize = 2;
/// The container is borderless: this strip above the plugin area (logical px,
/// DPI-scaled) is the only part of it the plugin doesn't cover, and it drags the window.
const DRAG_STRIP_HEIGHT_LOGICAL: i32 = 18;
/// `$panel-background-color` (#222229) as a Win32 COLORREF (0x00BBGGRR).
const DRAG_STRIP_COLOR: u32 = 0x0029_2222;
/// Close button, right-aligned in the drag strip (logical px, DPI-scaled).
const CLOSE_BUTTON_WIDTH_LOGICAL: i32 = 32;
/// Keeps the button clear of the window's rounded top-right corner.
const CLOSE_BUTTON_MARGIN_LOGICAL: i32 = 2;
/// Muted gray glyph (ARGB), opaque, drawn with GDI+ for anti-aliasing.
const CLOSE_GLYPH_ARGB: u32 = 0xFF8C_8C8C;
/// Brighter on hover.
const CLOSE_GLYPH_HOVER_ARGB: u32 = 0xFFC8_C8C8;
/// CS_HREDRAW | CS_VREDRAW — repaints the whole strip (and the close button's
/// new position) whenever the container's width or height changes.
const CONTAINER_CLASS_STYLE: u32 = 0x0002 | 0x0001 | 0x0020;
const SW_SHOW: i32 = 5;
const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
/// SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE
const SWP_RESIZE_ONLY: u32 = 0x0002 | 0x0004 | 0x0010;
/// SWP_NOZORDER | SWP_NOACTIVATE
const SWP_MOVE_AND_RESIZE: u32 = 0x0004 | 0x0010;
/// SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE
const SWP_MOVE_ONLY: u32 = 0x0001 | 0x0004 | 0x0010;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: u32 = 2;
const CLASS_NAME: &str = "EchoesPluginWindow";
const PLUGIN_AREA_CLASS_NAME: &str = "EchoesPluginArea";

const TIMER_ID_PUMP: usize = 1;
/// `USER_TIMER_MINIMUM`: Windows fires the timer on every system tick (~15.6 ms).
/// A larger value would skip a tick, e.g. 16 ms would fire every other one.
const TIMER_PUMP_INTERVAL_MS: u32 = 10;

/// Sentinel placed at the head of every valid `WindowUserData`.  Verified
/// before each USERDATA dereference (W2: protects against third-party hooks
/// or pathological plugins writing into `GWLP_USERDATA`).
const USERDATA_MAGIC: u64 = 0xEC_E0_5E_50_15_00_F1_00;

struct WindowUserData {
    magic: u64,
    effect_id: String,
    open_editors: Arc<Mutex<HashMap<String, usize>>>,
    /// Called on `CLAP_GUI_DRAIN_MESSAGE` — drains pending main-thread tasks
    /// (CLAP `on_main_thread`).  No-op for VST3.
    tick: Box<dyn Fn()>,
    /// Called on each `WM_TIMER`: runs the plugin's due `clap.timer-support`
    /// timers.  No-op for VST3.
    on_timer: Box<dyn Fn()>,
    /// Called on `WM_DPICHANGED` with the window's new DPI: hands the new
    /// scale to the plugin and fits the window to its resulting size.
    on_dpi_changed: Box<dyn Fn(*mut c_void, u32)>,
    /// Whether the cursor is currently over the close button (drives its hover color).
    hover: std::cell::Cell<bool>,
    /// Plugin-specific teardown (destroy GUI, destroy plugin, drop FFI Arcs).
    /// `Option` so it can be `.take()`-n out before invocation.
    cleanup: Option<Box<dyn FnOnce()>>,
}

/// Safely read the userdata for this HWND, validating the magic sentinel.
unsafe fn userdata<'a>(hwnd: *mut c_void) -> Option<&'a mut WindowUserData> {
    let ptr = user32::GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowUserData;
    if ptr.is_null() {
        return None;
    }
    let ud = &mut *ptr;
    if ud.magic != USERDATA_MAGIC {
        return None;
    }
    Some(ud)
}

unsafe extern "system" fn plugin_wnd_proc(
    hwnd: *mut c_void,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    // WM_NCCREATE: install USERDATA before any other message.
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
        WM_NCHITTEST => {
            return match user32::DefWindowProcW(hwnd, msg, wparam, lparam) {
                HTCLIENT => {
                    let mut pt = user32::POINT {
                        x: (lparam & 0xFFFF) as i16 as i32,
                        y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
                    };
                    user32::ScreenToClient(hwnd, &mut pt);
                    if point_in_rect(&close_button_rect(hwnd), pt.x, pt.y) {
                        HTCLIENT
                    } else {
                        HTCAPTION
                    }
                }
                hit => hit,
            };
        }
        WM_LBUTTONUP => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            if point_in_rect(&close_button_rect(hwnd), x, y) {
                user32::PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            return 0;
        }
        WM_MOUSEMOVE => {
            if let Some(ud) = userdata(hwnd) {
                // Re-armed on every move: TrackMouseEvent's leave notification
                // is one-shot, and cheap enough to request unconditionally.
                let mut track = user32::TRACKMOUSEEVENT {
                    cb_size: std::mem::size_of::<user32::TRACKMOUSEEVENT>() as u32,
                    dw_flags: TME_LEAVE,
                    hwnd_track: hwnd,
                    dw_hover_time: HOVER_DEFAULT,
                };
                user32::TrackMouseEvent(&mut track);
                let x = (lparam & 0xFFFF) as i16 as i32;
                let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
                let over = point_in_rect(&close_button_rect(hwnd), x, y);
                if over != ud.hover.get() {
                    ud.hover.set(over);
                    let rect = close_button_rect(hwnd);
                    user32::InvalidateRect(hwnd, &rect, 0);
                }
            }
            return 0;
        }
        WM_MOUSELEAVE => {
            if let Some(ud) = userdata(hwnd) {
                if ud.hover.get() {
                    ud.hover.set(false);
                    let rect = close_button_rect(hwnd);
                    user32::InvalidateRect(hwnd, &rect, 0);
                }
            }
            return 0;
        }
        WM_PAINT => {
            draw_close_button(hwnd);
            return 0;
        }
        CLAP_GUI_DRAIN_MESSAGE => {
            if let Some(ud) = userdata(hwnd) {
                (ud.tick)();
                return 0;
            }
        }
        CLAP_GUI_RESIZE_MESSAGE if userdata(hwnd).is_some() => {
            resize_to_client(hwnd, wparam as i32, lparam as i32);
            return 0;
        }
        WM_DPICHANGED => {
            if let Some(ud) = userdata(hwnd) {
                // Take Windows' suggested placement on the new monitor, then
                // let the plugin pick its exact size at the new scale.
                let suggested = &*(lparam as *const user32::RECT);
                user32::SetWindowPos(
                    hwnd,
                    null_mut(),
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_MOVE_AND_RESIZE,
                );
                (ud.on_dpi_changed)(hwnd, (wparam & 0xFFFF) as u32);
                return 0;
            }
        }
        WM_TIMER => {
            if let Some(ud) = userdata(hwnd) {
                (ud.on_timer)();
            }
            return 0;
        }
        WM_CLOSE => {
            let ptr = user32::GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if ptr != 0 {
                // Atomically take ownership so a re-entrant WM_CLOSE can't
                // double-free.  Validate magic *after* take.
                user32::SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                user32::KillTimer(hwnd, TIMER_ID_PUMP);

                let mut data: WindowUserData = *Box::from_raw(ptr as *mut WindowUserData);
                if data.magic == USERDATA_MAGIC {
                    let oe = Arc::clone(&data.open_editors);
                    let eid = data.effect_id.clone();
                    if let Some(cb) = data.cleanup.take() {
                        cb();
                    }
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

unsafe fn register_class(
    name: &str,
    style: u32,
    wnd_proc: unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
    background: *mut c_void,
) {
    let class_wide = wto16(name);
    let wc = user32::WNDCLASSEXW {
        cbSize: std::mem::size_of::<user32::WNDCLASSEXW>() as u32,
        style,
        lpfnWndProc: wnd_proc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: user32::GetModuleHandleW(std::ptr::null()),
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: background,
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_wide.as_ptr(),
        hIconSm: null_mut(),
    };
    user32::RegisterClassExW(&wc);
}

fn ensure_class_registered() {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| unsafe {
        // Also fills the plugin area wherever the plugin hasn't repainted yet (mid-resize).
        let background = user32::CreateSolidBrush(DRAG_STRIP_COLOR);
        register_class(
            CLASS_NAME,
            CONTAINER_CLASS_STYLE,
            plugin_wnd_proc,
            background,
        );
        register_class(
            PLUGIN_AREA_CLASS_NAME,
            0,
            user32::DefWindowProcW,
            background,
        );
    });
}

/// Windows 11 only rounds captioned windows by default; earlier versions ignore this.
unsafe fn round_corners(hwnd: *mut c_void) {
    let preference = DWMWCP_ROUND;
    user32::DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &preference as *const u32 as *const c_void,
        std::mem::size_of::<u32>() as u32,
    );
}

/// The child window the plugin's GUI is embedded into, below the drag strip.
fn plugin_area(container: *mut c_void) -> *mut c_void {
    unsafe { user32::GetWindow(container, GW_CHILD) }
}

fn drag_strip_height(hwnd: *mut c_void) -> i32 {
    (DRAG_STRIP_HEIGHT_LOGICAL as f64 * scale_for_dpi(window_dpi(hwnd))).round() as i32
}

/// Client-coordinate rect of the close button, right-aligned in the drag strip.
fn close_button_rect(hwnd: *mut c_void) -> user32::RECT {
    let mut client = user32::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe { user32::GetClientRect(hwnd, &mut client) };
    let scale = scale_for_dpi(window_dpi(hwnd));
    let width = (CLOSE_BUTTON_WIDTH_LOGICAL as f64 * scale).round() as i32;
    let margin = (CLOSE_BUTTON_MARGIN_LOGICAL as f64 * scale).round() as i32;
    user32::RECT {
        left: client.right - margin - width,
        top: 0,
        right: client.right - margin,
        bottom: drag_strip_height(hwnd),
    }
}

fn point_in_rect(rect: &user32::RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

/// Centers the window on the primary monitor, at its current size.
unsafe fn center_on_screen(hwnd: *mut c_void) {
    let mut client = user32::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    user32::GetClientRect(hwnd, &mut client);
    let screen_w = user32::GetSystemMetrics(SM_CXSCREEN);
    let screen_h = user32::GetSystemMetrics(SM_CYSCREEN);
    let x = ((screen_w - (client.right - client.left)) / 2).max(0);
    let y = ((screen_h - (client.bottom - client.top)) / 2).max(0);
    user32::SetWindowPos(hwnd, null_mut(), x, y, 0, 0, SWP_MOVE_ONLY);
}

/// Draws the × glyph; the strip background itself is erased by the class brush.
unsafe fn draw_close_button(hwnd: *mut c_void) {
    ensure_gdiplus();
    let mut ps: user32::PAINTSTRUCT = std::mem::zeroed();
    let hdc = user32::BeginPaint(hwnd, &mut ps);
    // BeginPaint clips drawing to ps.rcPaint, which can be a partial dirty
    // rect (e.g. a small OS-driven repaint outside our own resize handling);
    // the glyph must draw in full regardless of what triggered this WM_PAINT.
    user32::SelectClipRgn(hdc, null_mut());

    let hover = userdata(hwnd).is_some_and(|ud| ud.hover.get());
    let rect = close_button_rect(hwnd);
    // Square glyph centered in the (wider-than-tall) button rect.
    let cx = (rect.left + rect.right) / 2;
    let cy = (rect.top + rect.bottom) / 2;
    let half = ((rect.bottom - rect.top) as f64 * 0.17).round().max(3.0) as i32;
    let stroke_width = (1.5 * scale_for_dpi(window_dpi(hwnd))).max(1.0) as f32;
    let color = if hover {
        CLOSE_GLYPH_HOVER_ARGB
    } else {
        CLOSE_GLYPH_ARGB
    };

    let mut graphics: *mut c_void = null_mut();
    gdiplus::GdipCreateFromHDC(hdc, &mut graphics);
    gdiplus::GdipSetSmoothingMode(graphics, GDIP_SMOOTHING_ANTIALIAS);
    let mut pen: *mut c_void = null_mut();
    gdiplus::GdipCreatePen1(color, stroke_width, GDIP_UNIT_PIXEL, &mut pen);
    gdiplus::GdipSetPenStartCap(pen, GDIP_LINE_CAP_ROUND);
    gdiplus::GdipSetPenEndCap(pen, GDIP_LINE_CAP_ROUND);
    gdiplus::GdipDrawLineI(graphics, pen, cx - half, cy - half, cx + half, cy + half);
    gdiplus::GdipDrawLineI(graphics, pen, cx + half, cy - half, cx - half, cy + half);
    gdiplus::GdipDeletePen(pen);
    gdiplus::GdipDeleteGraphics(graphics);

    user32::EndPaint(hwnd, &ps);
}

fn wto16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Create the Win32 container with `userdata` installed via `WM_NCCREATE`.
///
/// On success the window owns `userdata` (USERDATA = raw pointer); the caller
/// MUST NOT free it.  On failure the caller is given back the raw pointer to
/// drop manually.
unsafe fn create_container(
    title: &str,
    client_w: i32,
    client_h: i32,
    userdata: *mut WindowUserData,
) -> Result<*mut c_void, String> {
    ensure_class_registered();

    let title_w = wto16(title);
    let class_w = wto16(CLASS_NAME);

    // Created at zero size: the drag strip height depends on the DPI of the
    // monitor the window lands on, only known once the HWND exists.
    // No owner HWND: owned popups stay above the owner (bad DAW UX), and a
    // cross-process owner (sandbox bridge → Echoes) can deadlock the UI thread
    // waiting on OpenEditor while CreateWindowEx waits on the owner's queue.
    let hwnd = user32::CreateWindowExW(
        CONTAINER_EX_STYLE,
        class_w.as_ptr(),
        title_w.as_ptr(),
        CONTAINER_STYLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        0,
        0,
        null_mut(),
        null_mut(),
        user32::GetModuleHandleW(std::ptr::null()),
        userdata as *mut c_void,
    );

    if hwnd.is_null() {
        return Err("Win32 CreateWindowExW failed for plugin container".into());
    }

    let area_class_w = wto16(PLUGIN_AREA_CLASS_NAME);
    let area = user32::CreateWindowExW(
        0,
        area_class_w.as_ptr(),
        std::ptr::null(),
        PLUGIN_AREA_STYLE,
        0,
        0,
        0,
        0,
        hwnd,
        null_mut(),
        user32::GetModuleHandleW(std::ptr::null()),
        null_mut(),
    );
    if area.is_null() {
        // Detach the userdata first so DestroyWindow can't free what the caller reclaims.
        user32::SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        user32::DestroyWindow(hwnd);
        return Err("Win32 CreateWindowExW failed for plugin area".into());
    }

    round_corners(hwnd);
    resize_to_client(hwnd, client_w, client_h);
    Ok(hwnd)
}

fn window_dpi(hwnd: *mut c_void) -> u32 {
    match unsafe { user32::GetDpiForWindow(hwnd) } {
        0 => USER_DEFAULT_SCREEN_DPI,
        dpi => dpi,
    }
}

fn scale_for_dpi(dpi: u32) -> f64 {
    dpi as f64 / USER_DEFAULT_SCREEN_DPI as f64
}

/// Resize the window so the plugin area matches the given dimensions
/// (physical pixels), with the drag strip on top.  Called from the host-side
/// resize handlers (CLAP `host_gui.request_resize`, VST3 `IPlugFrame::resizeView`).
unsafe fn resize_to_client(hwnd: *mut c_void, client_w: i32, client_h: i32) {
    let width = client_w.max(200);
    let height = client_h.max(100);
    let strip = drag_strip_height(hwnd);
    user32::SetWindowPos(
        hwnd,
        null_mut(),
        0,
        0,
        width,
        height + strip,
        SWP_RESIZE_ONLY,
    );
    user32::SetWindowPos(
        plugin_area(hwnd),
        null_mut(),
        0,
        strip,
        width,
        height,
        SWP_MOVE_AND_RESIZE,
    );
}

/// Show the window and install the timer that pumps the plugin's timers.
unsafe fn show_and_start_timer(hwnd: *mut c_void) {
    user32::ShowWindow(hwnd, SW_SHOW);
    user32::UpdateWindow(hwnd);
    user32::SetTimer(hwnd, TIMER_ID_PUMP, TIMER_PUMP_INTERVAL_MS, null_mut());
}

use crate::plugin_formats::clap::ffi::{ClapGuiExtension, ClapPlugin};

/// Hands the monitor scale to the plugin (a plugin that reads the DPI itself
/// ignores it), then fits the window to the size it reports at that scale.
unsafe fn fit_clap_gui_to_dpi(
    gui: *const ClapGuiExtension,
    plugin: *const ClapPlugin,
    hwnd: *mut c_void,
    dpi: u32,
) {
    let _ = ((*gui).set_scale)(plugin, scale_for_dpi(dpi));
    let (mut width, mut height) = (0u32, 0u32);
    if ((*gui).get_size)(plugin, &mut width, &mut height) {
        resize_to_client(hwnd, width as i32, height as i32);
    }
}

use crate::plugin_formats::vst3::ffi::{
    FUnknown, IPlugFrameVtbl, IPlugView, IPlugViewContentScaleSupport, TResult, Tuid, ViewRect,
    IID_FUNKNOWN, IID_IPLUG_FRAME, IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT, K_INVALID_ARGUMENT,
    K_NO_INTERFACE, K_RESULT_FALSE, K_RESULT_OK,
};
use crate::plugin_formats::vst3::host_context::{static_add_ref, static_release, tuid_eq};

// Per-editor instance so `resizeView` can route to the right container HWND.

#[repr(C)]
struct PlugFrame {
    vtbl: *const IPlugFrameVtbl,
    container_hwnd: AtomicPtr<c_void>,
    view: *mut IPlugView,
    /// `onSize` calls in progress: a plugin may call `resizeView` from `onSize`.
    resize_depth: Cell<u8>,
}

/// Nested `onSize` → `resizeView` → `onSize` rounds accepted before a request is refused.
const MAX_RESIZE_DEPTH: u8 = 3;

unsafe extern "C" fn frame_qi(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() {
        return K_INVALID_ARGUMENT;
    }
    if tuid_eq(iid, &IID_IPLUG_FRAME) || tuid_eq(iid, &IID_FUNKNOWN) {
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = null_mut();
    K_NO_INTERFACE
}

/// `IPlugFrame::resizeView`: resizes the window, then tells the view with `onSize`
/// in the same call stack, as `iplugview.h` requires.
unsafe extern "C" fn frame_resize_view(
    this: *mut FUnknown,
    view: *mut IPlugView,
    rect: *mut ViewRect,
) -> TResult {
    if this.is_null() || rect.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let frame = &*(this as *const PlugFrame);
    if view.is_null() || view != frame.view {
        return K_INVALID_ARGUMENT;
    }
    let hwnd = frame.container_hwnd.load(Ordering::Acquire);
    let depth = frame.resize_depth.get();
    if hwnd.is_null() || depth >= MAX_RESIZE_DEPTH {
        return K_RESULT_FALSE;
    }

    let mut requested = *rect;
    let vtbl = &*(*view).vtbl;
    let mut reported = requested;
    if (vtbl.get_size)(view, &mut reported) == K_RESULT_OK && reported == requested {
        return K_RESULT_OK;
    }

    resize_to_client(
        hwnd,
        (requested.right - requested.left).max(1),
        (requested.bottom - requested.top).max(1),
    );

    // A view that already follows the new size on its own needs no `onSize`.
    let mut after = requested;
    let follows = (vtbl.get_size)(view, &mut after) == K_RESULT_OK && after == requested;
    if !follows {
        frame.resize_depth.set(depth + 1);
        let _ = (vtbl.on_size)(view, &mut requested);
        frame.resize_depth.set(depth);
    }
    K_RESULT_OK
}

static PLUG_FRAME_VTBL: IPlugFrameVtbl = IPlugFrameVtbl {
    query_interface: frame_qi,
    add_ref: static_add_ref,
    release: static_release,
    resize_view: frame_resize_view,
};

fn make_plug_frame(view: *mut IPlugView) -> Box<PlugFrame> {
    Box::new(PlugFrame {
        vtbl: &PLUG_FRAME_VTBL,
        container_hwnd: AtomicPtr::new(null_mut()),
        view,
        resize_depth: Cell::new(0),
    })
}

/// Hands the monitor scale to the view (if it supports it), then fits the
/// window to the size the view reports at that scale.
unsafe fn fit_vst3_view_to_dpi(view: *mut IPlugView, hwnd: *mut c_void, dpi: u32) {
    let mut scale_support: *mut c_void = null_mut();
    let found = ((*(*view).vtbl).query_interface)(
        view,
        &IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT,
        &mut scale_support,
    );
    if found == K_RESULT_OK && !scale_support.is_null() {
        let scale_support = scale_support as *mut IPlugViewContentScaleSupport;
        let vtbl = &*(*scale_support).vtbl;
        let _ = (vtbl.set_content_scale_factor)(scale_support, scale_for_dpi(dpi) as f32);
        (vtbl.release)(scale_support);
    }

    let mut rect = ViewRect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if ((*(*view).vtbl).get_size)(view, &mut rect) == K_RESULT_OK {
        resize_to_client(hwnd, rect.right - rect.left, rect.bottom - rect.top);
    }
}

/// Open a VST3 native editor window (main thread only).
///
/// # Safety
///
/// Must be called on the Win32 main thread.  The caller passes a clone of the
/// shared `Arc<PluginInstance>` (held by `PluginHost`); the editor captures
/// this clone in its cleanup closure, so the plugin handle stays live until
/// the editor window closes — even if the audio side retires its own clone in
/// the meantime (e.g. through a recompile).
///
/// The view comes from the instance's own edit controller, created with the
/// instance and released by `PluginInstance::Drop` — one COM object per device
/// (A1 of the plugin-GUI audit).
pub unsafe fn open_vst3_editor(
    instance: Arc<PluginInstance>,
    name: &str,
    effect_id: &str,
    open_editors: Arc<Mutex<HashMap<String, usize>>>,
) -> Result<usize, String> {
    let _running = instance.mark(PluginThread::Ui);
    let ctrl = instance
        .vst3_controller()
        .ok_or_else(|| "VST3 GUI: the plugin has no edit controller".to_string())?
        .as_ptr();
    // Before the view exists, so it opens on current values; dropped with the window.
    let mirror_watch = instance.watch_vst3_params();

    let view = ((*(*ctrl).vtbl).create_view)(ctrl, c"editor".as_ptr().cast());
    if view.is_null() {
        return Err("VST3 GUI: IEditController::createView returned null".into());
    }

    if ((*(*view).vtbl).is_platform_type_supported)(view, c"HWND".as_ptr().cast()) != K_RESULT_OK {
        ((*(*view).vtbl).release)(view as *mut _);
        return Err("VST3 GUI: plugin view does not support HWND".into());
    }

    let plug_frame = make_plug_frame(view); // before attached(); owned by cleanup
    let plug_frame_ptr = &*plug_frame as *const PlugFrame;
    let frame_funknown = plug_frame_ptr as *mut FUnknown;
    let _ = ((*(*view).vtbl).set_frame)(view, frame_funknown);

    let mut rect = ViewRect {
        left: 0,
        top: 0,
        right: 800,
        bottom: 600,
    };
    let _ = ((*(*view).vtbl).get_size)(view, &mut rect);
    let probe_w = (rect.right - rect.left).max(1);
    let probe_h = (rect.bottom - rect.top).max(1);

    // Build cleanup + tick closures.  The shared `Arc<PluginInstance>` is
    // captured here so the plugin stays live until the editor closes.  The
    // closure does NOT call `terminate` on IComponent and does NOT release
    // its primary ref, nor the controller — those are owned by
    // `PluginInstance::Drop`.  It only tears down the view and the frame.
    let frame_owned = plug_frame;
    let instance_owned = instance.clone();
    let cleanup: Box<dyn FnOnce()> = Box::new(move || {
        let _frame = frame_owned;
        let _instance = instance_owned; // keep plugin alive through teardown
        let _running = _instance.mark(PluginThread::Ui);
        unsafe {
            // Frame first, as the SDK host does: a plugin must not ask for a size while detaching.
            let _ = ((*(*view).vtbl).set_frame)(view, null_mut());
            ((*(*view).vtbl).removed)(view);
            ((*(*view).vtbl).release)(view as *mut _);
        }
        drop(mirror_watch);
        // _instance Arc drops last — if this was the last clone, the host
        // already released its own clone earlier, and PluginInstance::Drop
        // now runs terminate + release on the IComponent.
    });
    let tick: Box<dyn Fn()> = Box::new(|| {});
    let instance_for_timer = Arc::clone(&instance);
    let on_timer: Box<dyn Fn()> = Box::new(move || instance_for_timer.mirror_to_controller());
    let instance_for_dpi = Arc::clone(&instance);
    let on_dpi_changed: Box<dyn Fn(*mut c_void, u32)> = Box::new(move |hwnd, dpi| {
        let _running = instance_for_dpi.mark(PluginThread::Ui);
        unsafe { fit_vst3_view_to_dpi(view, hwnd, dpi) }
    });

    let userdata = Box::new(WindowUserData {
        magic: USERDATA_MAGIC,
        effect_id: effect_id.to_string(),
        open_editors: Arc::clone(&open_editors),
        tick,
        on_timer,
        on_dpi_changed,
        hover: std::cell::Cell::new(false),
        cleanup: Some(cleanup),
    });
    let userdata_raw = Box::into_raw(userdata);

    let hwnd = match create_container(name, probe_w, probe_h, userdata_raw) {
        Ok(h) => h,
        Err(e) => {
            // Recover ownership and run cleanup ourselves
            let mut d = *Box::from_raw(userdata_raw);
            if let Some(cb) = d.cleanup.take() {
                cb();
            }
            return Err(e);
        }
    };

    (*plug_frame_ptr)
        .container_hwnd
        .store(hwnd, Ordering::Release);

    let attach_r = ((*(*view).vtbl).attached)(view, plugin_area(hwnd), c"HWND".as_ptr().cast());
    if attach_r != K_RESULT_OK {
        user32::PostMessageW(hwnd, WM_CLOSE, 0, 0);
        return Err(format!("VST3 GUI: IPlugView::attached failed: {attach_r}"));
    }

    fit_vst3_view_to_dpi(view, hwnd, window_dpi(hwnd));
    center_on_screen(hwnd);

    show_and_start_timer(hwnd);

    if let Ok(mut map) = open_editors.lock() {
        map.insert(effect_id.to_string(), hwnd as usize);
    }

    Ok(hwnd as usize)
}

/// Open a CLAP native editor window (main thread only).
///
/// # Safety
///
/// Must be called on the Win32 main thread.  The editor reuses the audio-side
/// ClapPlugin pointer through `instance.clap_plugin()` (A1 of the plugin-GUI
/// audit).  `plugin.init()` was already called by `ClapWrapperNode::prepare`
/// during graph compile and MUST NOT be called again (CLAP spec).  The window
/// is registered in the instance's `ClapHostContext`, so that plugin's
/// `request_callback` and `clap.gui` resize requests reach this window (CL3).
pub unsafe fn open_clap_editor(
    instance: Arc<PluginInstance>,
    name: &str,
    effect_id: &str,
    open_editors: Arc<Mutex<HashMap<String, usize>>>,
) -> Result<usize, String> {
    use crate::plugin_formats::clap::ffi::{ClapWindow, CLAP_EXT_GUI, CLAP_WINDOW_API_WIN32};

    let _running = instance.mark(PluginThread::Ui);
    let plugin = instance
        .clap_plugin()
        .ok_or_else(|| "CLAP GUI: shared instance has no CLAP plugin pointer".to_string())?;

    if instance.clap_host_context().is_none() {
        return Err("CLAP GUI: shared instance has no host context".into());
    }

    // Nulled at the start of cleanup so a racing `tick` stops calling the plugin.
    let live_plugin = Arc::new(AtomicPtr::new(plugin as *mut ClapPlugin));

    let gui_raw = ((*plugin).get_extension)(plugin, CLAP_EXT_GUI.as_ptr() as *const c_char);
    if gui_raw.is_null() {
        return Err("CLAP GUI: plugin does not expose clap.gui extension".into());
    }
    let gui = gui_raw as *const ClapGuiExtension;

    if !((*gui).is_api_supported)(
        plugin,
        CLAP_WINDOW_API_WIN32.as_ptr() as *const c_char,
        false,
    ) {
        return Err("CLAP GUI: plugin clap.gui does not support Win32".into());
    }

    if !((*gui).create)(
        plugin,
        CLAP_WINDOW_API_WIN32.as_ptr() as *const c_char,
        false,
    ) {
        return Err("CLAP GUI: clap_gui.create failed".into());
    }

    let mut pw: u32 = 800;
    let mut ph: u32 = 600;
    let _ = ((*gui).get_size)(plugin, &mut pw, &mut ph);

    // Cleanup order:
    //   1. live_plugin = null, context hwnd = null → no more ticks or posts to this window
    //   2. gui.hide / gui.destroy  → release GUI resources only
    //                                 (plugin instance survives — owned by Arc)
    //   3. instance Arc clones drop (here and in `tick`) → if last, PluginInstance::Drop
    //      runs plugin.destroy (CLAP spec teardown).
    let live_plugin_for_cleanup = Arc::clone(&live_plugin);
    let instance_for_cleanup = Arc::clone(&instance);
    let cleanup: Box<dyn FnOnce()> = Box::new(move || {
        let _running = instance_for_cleanup.mark(PluginThread::Ui);
        live_plugin_for_cleanup.store(null_mut(), Ordering::Release);
        if let Some(context) = instance_for_cleanup.clap_host_context() {
            context.set_gui_hwnd(null_mut());
        }
        unsafe {
            let _ = ((*gui).hide)(plugin);
            ((*gui).destroy)(plugin);
        }
    });

    // `RefCell`: a plugin that runs a nested message loop inside `on_timer`
    // re-enters this closure, and that call must not touch the schedule.
    let schedule = RefCell::new(TimerSchedule::default());
    let instance_for_timers = Arc::clone(&instance);
    let live_plugin_for_timers = Arc::clone(&live_plugin);
    let on_timer: Box<dyn Fn()> = Box::new(move || {
        if live_plugin_for_timers.load(Ordering::Acquire).is_null() {
            return;
        }
        if let Ok(mut schedule) = schedule.try_borrow_mut() {
            instance_for_timers.run_due_timers(&mut schedule, Instant::now());
        }
    });

    let instance_for_tick = Arc::clone(&instance);
    let tick: Box<dyn Fn()> = Box::new(move || {
        let p = live_plugin.load(Ordering::Acquire);
        if p.is_null() {
            return;
        }
        let _running = instance_for_tick.mark(PluginThread::Ui);
        let context = instance_for_tick.clap_host_context();
        if let Some(context) = context {
            context.clear_gui_drain();
        }
        unsafe {
            ((*p).on_main_thread)(p);
        }
        if let Some(context) = context {
            context.rearm_gui_drain();
        }
    });

    let instance_for_dpi = Arc::clone(&instance);
    let on_dpi_changed: Box<dyn Fn(*mut c_void, u32)> = Box::new(move |hwnd, dpi| {
        let _running = instance_for_dpi.mark(PluginThread::Ui);
        unsafe { fit_clap_gui_to_dpi(gui, plugin, hwnd, dpi) }
    });

    let userdata = Box::new(WindowUserData {
        magic: USERDATA_MAGIC,
        effect_id: effect_id.to_string(),
        open_editors: Arc::clone(&open_editors),
        tick,
        on_timer,
        on_dpi_changed,
        hover: std::cell::Cell::new(false),
        cleanup: Some(cleanup),
    });
    let userdata_raw = Box::into_raw(userdata);

    let hwnd = match create_container(name, pw.max(100) as i32, ph.max(100) as i32, userdata_raw) {
        Ok(h) => h,
        Err(e) => {
            let mut d = *Box::from_raw(userdata_raw);
            if let Some(cb) = d.cleanup.take() {
                cb();
            }
            return Err(e);
        }
    };

    // From this point on, ANY error path triggers cleanup via WM_CLOSE.
    if let Some(context) = instance.clap_host_context() {
        context.set_gui_hwnd(hwnd);
    }

    fit_clap_gui_to_dpi(gui, plugin, hwnd, window_dpi(hwnd));
    center_on_screen(hwnd);

    let clap_win = ClapWindow {
        api: CLAP_WINDOW_API_WIN32.as_ptr() as *const c_char,
        specific: plugin_area(hwnd) as usize,
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

/// Post `WM_CLOSE` to the plugin window from any thread.
/// The WndProc handles cleanup on the main thread.
pub fn post_close(hwnd: usize) {
    if hwnd != 0 {
        unsafe {
            user32::PostMessageW(hwnd as *mut c_void, WM_CLOSE, 0, 0);
        }
    }
}

#[cfg(test)]
mod plug_frame_tests {
    use super::*;

    #[test]
    fn the_frame_answers_the_sdk_codes() {
        let frame = make_plug_frame(null_mut());
        let this = &*frame as *const PlugFrame as *mut FUnknown;
        let table = unsafe { &*frame.vtbl };
        let mut found: *mut c_void = null_mut();
        let unknown = [7u8; 16];

        unsafe {
            assert_eq!(
                (table.query_interface)(this, &IID_IPLUG_FRAME, &mut found),
                K_RESULT_OK
            );
            assert_eq!(
                (table.query_interface)(this, &unknown, &mut found),
                K_NO_INTERFACE
            );
            assert!(found.is_null());
            assert_eq!(
                (table.query_interface)(this, &IID_IPLUG_FRAME, null_mut()),
                K_INVALID_ARGUMENT
            );
        }
    }

    #[test]
    fn a_resize_without_a_window_or_a_size_is_refused_with_the_right_code() {
        let view = 0x10 as *mut IPlugView;
        let frame = make_plug_frame(view);
        let this = &*frame as *const PlugFrame as *mut FUnknown;
        let table = unsafe { &*frame.vtbl };
        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 50,
        };

        unsafe {
            assert_eq!(
                (table.resize_view)(this, view, null_mut()),
                K_INVALID_ARGUMENT
            );
            assert_eq!(
                (table.resize_view)(this, null_mut(), &mut rect),
                K_INVALID_ARGUMENT,
                "not the view of this frame"
            );
            assert_eq!(
                (table.resize_view)(this, view, &mut rect),
                K_RESULT_FALSE,
                "no window is attached to this frame"
            );
        }
    }
}

#[cfg(test)]
mod vst3_resize_tests {
    use super::test_support::pump_thread_messages;
    use super::*;
    use crate::plugin_formats::vst3::fake_plugin::{
        fake_vst3_instance_with_view, FakeListener, FakeView, ViewCall,
    };
    use std::time::Duration;

    /// Opens the editor of a fake plugin whose view is `view`, runs `while_open` on
    /// the window, then closes it and lets the window procedure tear it down.
    fn open_and_close(view: &mut FakeView, while_open: impl FnOnce(*mut c_void)) {
        let mut listener = FakeListener::new(false);
        let instance = fake_vst3_instance_with_view(&mut listener, view);
        let hwnd = unsafe {
            open_vst3_editor(
                instance,
                "Fake",
                "fake",
                Arc::new(Mutex::new(HashMap::new())),
            )
        }
        .expect("the editor opens");
        while_open(hwnd as *mut c_void);
        post_close(hwnd);
        pump_thread_messages(Duration::from_millis(100));
    }

    fn plugin_area_size(hwnd: *mut c_void) -> (i32, i32) {
        let mut rect = user32::RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe { user32::GetClientRect(plugin_area(hwnd), &mut rect) };
        (rect.right - rect.left, rect.bottom - rect.top)
    }

    fn on_size(width: i32, height: i32) -> ViewCall {
        ViewCall::OnSize { width, height }
    }

    #[test]
    fn a_size_asked_while_attaching_resizes_the_window_and_reaches_on_size() {
        let mut view = FakeView::new(400, 300).asking_when_attached(640, 480);

        open_and_close(&mut view, |hwnd| {
            assert_eq!(plugin_area_size(hwnd), (640, 480));
        });

        assert_eq!(
            view.calls(),
            [
                ViewCall::SetFrame { given: true },
                ViewCall::Attached,
                on_size(640, 480),
                ViewCall::ResizeAnswered(K_RESULT_OK),
                ViewCall::SetFrame { given: false },
                ViewCall::Removed,
            ],
            "onSize comes inside resizeView; the frame is dropped before removed()"
        );
        assert_eq!(view.size(), (640, 480));
    }

    #[test]
    fn asking_for_the_size_it_already_has_does_not_reach_on_size() {
        let mut view = FakeView::new(400, 300).asking_when_attached(400, 300);

        open_and_close(&mut view, |hwnd| {
            assert_eq!(plugin_area_size(hwnd), (400, 300));
        });

        assert!(!view
            .calls()
            .iter()
            .any(|call| matches!(call, ViewCall::OnSize { .. })));
        assert!(view
            .calls()
            .contains(&ViewCall::ResizeAnswered(K_RESULT_OK)));
    }

    #[test]
    fn a_view_that_asks_again_from_on_size_is_stopped_after_a_few_rounds() {
        let mut view = FakeView::new(400, 300)
            .asking_when_attached(500, 400)
            .asking_again_from_on_size();

        open_and_close(&mut view, |hwnd| {
            assert_eq!(
                plugin_area_size(hwnd),
                (520, 420),
                "the 530 x 430 ask is refused"
            );
        });

        let calls = view.calls();
        let on_size_calls: Vec<_> = calls
            .iter()
            .filter(|call| matches!(call, ViewCall::OnSize { .. }))
            .collect();
        assert_eq!(on_size_calls.len(), usize::from(MAX_RESIZE_DEPTH));
        assert_eq!(
            calls
                .iter()
                .filter(|call| **call == ViewCall::ResizeAnswered(K_RESULT_FALSE))
                .count(),
            1
        );
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::ffi::c_void;
    use std::time::{Duration, Instant};

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Msg {
        hwnd: *mut c_void,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: Point,
    }

    const PM_REMOVE: u32 = 1;

    #[link(name = "user32")]
    extern "system" {
        fn PeekMessageW(msg: *mut Msg, hwnd: *mut c_void, min: u32, max: u32, remove: u32) -> i32;
        fn TranslateMessage(msg: *const Msg) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> isize;
    }

    /// Runs the calling thread's message loop for `duration`, as Tauri's loop
    /// does for the editor windows the thread created.
    pub(crate) fn pump_thread_messages(duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            // SAFETY: plain Win32 message dispatch on this thread's own queue.
            unsafe {
                let mut msg: Msg = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
