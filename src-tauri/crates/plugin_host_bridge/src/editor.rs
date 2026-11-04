//! The plugin's editor, shown in a top-level window owned by this process
//! (Windows).  Main thread only: the plugin's GUI calls and the window's messages
//! both run there.  One bridge hosts one plugin, so there is at most one editor.
//!
//! A CLAP plugin's host callbacks reach the window through its `ClapHostContext`
//! (`request_callback` → drain message, `clap.gui` resize → resize message),
//! exactly as for an in-process editor.  A VST3 view is attached to the same plugin
//! area; its frame resizes the window (`vst3_host::editor_view`), and a timer of the
//! window lets its controller follow the processor (ADR-033, step 7).

use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::ptr::null_mut;
use std::sync::{Arc, Once};

use clap_ffi::{
    ClapGuiExtension, ClapHostContext, ClapPlugin, ClapWindow, CLAP_EXT_GUI,
    CLAP_GUI_DRAIN_MESSAGE, CLAP_GUI_RESIZE_MESSAGE, CLAP_WINDOW_API_WIN32,
};

use vst3_host::editor_view::EditorView;

use crate::clap_adapter::ClapAdapter;
use crate::vst3_adapter::{ParamMirrorWatch, Vst3Adapter};

#[allow(non_snake_case, clippy::upper_case_acronyms)]
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

    #[repr(C)]
    pub struct MSG {
        pub hwnd: *mut c_void,
        pub message: u32,
        pub wParam: usize,
        pub lParam: isize,
        pub time: u32,
        pub pt_x: i32,
        pub pt_y: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn RegisterClassExW(class: *const WNDCLASSEXW) -> u16;
        pub fn CreateWindowExW(
            ex_style: u32,
            class: *const u16,
            title: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: *mut c_void,
            menu: *mut c_void,
            instance: *mut c_void,
            param: *mut c_void,
        ) -> *mut c_void;
        pub fn DestroyWindow(hwnd: *mut c_void) -> i32;
        pub fn DefWindowProcW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
        pub fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
        pub fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
        pub fn SetWindowPos(
            hwnd: *mut c_void,
            after: *mut c_void,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        pub fn GetDpiForWindow(hwnd: *mut c_void) -> u32;
        pub fn GetWindow(hwnd: *mut c_void, cmd: u32) -> *mut c_void;
        pub fn GetClientRect(hwnd: *mut c_void, rect: *mut RECT) -> i32;
        pub fn GetSystemMetrics(index: i32) -> i32;
        pub fn ScreenToClient(hwnd: *mut c_void, point: *mut POINT) -> i32;
        pub fn BeginPaint(hwnd: *mut c_void, paint: *mut PAINTSTRUCT) -> *mut c_void;
        pub fn EndPaint(hwnd: *mut c_void, paint: *const PAINTSTRUCT) -> i32;
        pub fn SelectClipRgn(hdc: *mut c_void, region: *mut c_void) -> i32;
        pub fn InvalidateRect(hwnd: *mut c_void, rect: *const RECT, erase: i32) -> i32;
        pub fn TrackMouseEvent(event: *mut TRACKMOUSEEVENT) -> i32;
        pub fn PostMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
        pub fn SetTimer(hwnd: *mut c_void, id: usize, ms: u32, callback: *mut c_void) -> usize;
        pub fn KillTimer(hwnd: *mut c_void, id: usize) -> i32;
        pub fn SetProcessDpiAwarenessContext(value: isize) -> i32;
        pub fn PeekMessageW(
            msg: *mut MSG,
            hwnd: *mut c_void,
            min: u32,
            max: u32,
            remove: u32,
        ) -> i32;
        pub fn TranslateMessage(msg: *const MSG) -> i32;
        pub fn DispatchMessageW(msg: *const MSG) -> isize;
        pub fn MsgWaitForMultipleObjects(
            count: u32,
            handles: *const c_void,
            wait_all: i32,
            ms: u32,
            mask: u32,
        ) -> u32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }

    #[link(name = "gdi32")]
    extern "system" {
        pub fn CreateSolidBrush(color: u32) -> *mut c_void;
    }

    #[link(name = "dwmapi")]
    extern "system" {
        pub fn DwmSetWindowAttribute(
            hwnd: *mut c_void,
            attribute: u32,
            value: *const c_void,
            size: u32,
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

/// Starts this process's one GDI+ session (idempotent, never torn down —
/// the OS reclaims it on exit like any other DLL state).
fn ensure_gdiplus() {
    static TOKEN: Once = Once::new();
    TOKEN.call_once(|| unsafe {
        let input = gdiplus::StartupInput {
            gdiplus_version: 1,
            debug_event_callback: null_mut(),
            suppress_background_thread: 0,
            suppress_external_codecs: 0,
        };
        let mut token: usize = 0;
        gdiplus::GdiplusStartup(&mut token, &input, null_mut());
    });
}

const WM_CLOSE: u32 = 0x0010;
const WM_NCHITTEST: u32 = 0x0084;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_PAINT: u32 = 0x000F;
const TME_LEAVE: u32 = 0x0000_0002;
const HOVER_DEFAULT: u32 = 0xFFFF_FFFF;
const WM_DPICHANGED: u32 = 0x02E0;
const WM_TIMER: u32 = 0x0113;
const WS_POPUP: u32 = 0x8000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WINDOW_STYLE: u32 = WS_POPUP | WS_CLIPCHILDREN;
const WINDOW_EX_STYLE: u32 = WS_EX_TOOLWINDOW;
const PLUGIN_AREA_STYLE: u32 = WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN;
const GW_CHILD: u32 = 5;
const HTCLIENT: isize = 1;
const HTCAPTION: isize = 2;
/// The window is borderless: this strip above the plugin area (logical px,
/// DPI-scaled) is the only part of it the plugin doesn't cover, and it drags the window.
const DRAG_STRIP_HEIGHT_LOGICAL: i32 = 18;
/// Echoes' `$panel-background-color` (#222229) as a Win32 COLORREF (0x00BBGGRR).
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
/// new position) whenever the window's width or height changes.
const CONTAINER_CLASS_STYLE: u32 = 0x0002 | 0x0001;
const SW_SHOW: i32 = 5;
const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
const SWP_RESIZE_ONLY: u32 = 0x0002 | 0x0004 | 0x0010;
const SWP_MOVE_AND_RESIZE: u32 = 0x0004 | 0x0010;
/// SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE
const SWP_MOVE_ONLY: u32 = 0x0001 | 0x0004 | 0x0010;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const PM_REMOVE: u32 = 0x0001;
const QS_ALLINPUT: u32 = 0x04FF;
const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
const DEFAULT_DPI: u32 = 96;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: u32 = 2;
const MIRROR_TIMER_ID: usize = 1;
/// `USER_TIMER_MINIMUM`: Windows fires the timer on every system tick (~15.6 ms).
const MIRROR_TIMER_INTERVAL_MS: u32 = 10;
const CLASS_NAME: &str = "EchoesSandboxedPluginWindow";
const PLUGIN_AREA_CLASS_NAME: &str = "EchoesSandboxedPluginArea";

#[derive(Clone, Copy)]
struct Editor {
    hwnd: *mut c_void,
    content: Content,
}

#[derive(Clone, Copy)]
enum Content {
    Clap {
        plugin: *const ClapPlugin,
        gui: *const ClapGuiExtension,
        context: *const ClapHostContext,
    },
    Vst3(*mut Vst3Session),
}

/// What an open VST3 editor holds besides its window.  Fields drop in this order: the
/// view first (frame taken away, `removed()`, released), then the parameter watch,
/// then the adapter, whose controller must outlive the view.
struct Vst3Session {
    view: EditorView,
    _watch: ParamMirrorWatch,
    adapter: Arc<Vst3Adapter>,
}

thread_local! {
    // `Cell` of plain pointers: the plugin may re-enter the window procedure
    // from inside any GUI call, so nothing here is ever borrowed across one.
    static EDITOR: Cell<Option<Editor>> = const { Cell::new(None) };
    // One bridge hosts one plugin, so there is at most one window and one hover flag.
    static CLOSE_BUTTON_HOVER: Cell<bool> = const { Cell::new(false) };
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Physical-pixel window scale, as `clap.gui` expects.
pub fn enable_per_monitor_dpi() {
    unsafe { user32::SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// Dispatch pending window messages, then wait up to `timeout_ms` for more.
pub fn pump_messages(timeout_ms: u32) {
    unsafe {
        let mut msg: user32::MSG = std::mem::zeroed();
        while user32::PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            user32::TranslateMessage(&msg);
            user32::DispatchMessageW(&msg);
        }
        user32::MsgWaitForMultipleObjects(0, std::ptr::null(), 0, timeout_ms, QS_ALLINPUT);
    }
}

/// Brings the open editor to the front, if there is one.
fn raise_open_editor() -> Option<u64> {
    let editor = EDITOR.get()?;
    unsafe { user32::SetForegroundWindow(editor.hwnd) };
    Some(editor.hwnd as u64)
}

/// Open the CLAP editor (or bring it to the front) and return its window handle.
pub fn open(plugin: &ClapAdapter, title: &str) -> Result<u64, String> {
    if let Some(hwnd) = raise_open_editor() {
        return Ok(hwnd);
    }
    let gui = plugin
        .extension::<ClapGuiExtension>(CLAP_EXT_GUI)
        .ok_or("the plugin has no editor")?;
    let context = plugin.host().context().ok_or("no host context")? as *const ClapHostContext;
    let clap = plugin.plugin();
    let api = CLAP_WINDOW_API_WIN32.as_ptr() as *const c_char;
    unsafe {
        if !((*gui).is_api_supported)(clap, api, false) {
            return Err("the editor does not support Win32 windows".into());
        }
        if !((*gui).create)(clap, api, false) {
            return Err("clap_gui.create failed".into());
        }
        let hwnd = create_window(title);
        if hwnd.is_null() {
            ((*gui).destroy)(clap);
            return Err("CreateWindowExW failed".into());
        }
        let editor = Editor {
            hwnd,
            content: Content::Clap {
                plugin: clap,
                gui,
                context,
            },
        };
        EDITOR.set(Some(editor));
        (*context).set_gui_hwnd(hwnd);
        fit_to_dpi(&editor, window_dpi(hwnd));
        center_on_screen(hwnd);

        let window = ClapWindow {
            api,
            specific: plugin_area(hwnd) as usize,
        };
        if !((*gui).set_parent)(clap, &window) {
            close();
            return Err("clap_gui.set_parent failed".into());
        }
        let _ = ((*gui).show)(clap);
        ((*clap).on_main_thread)(clap);
        user32::ShowWindow(hwnd, SW_SHOW);
        user32::SetForegroundWindow(hwnd);
        Ok(hwnd as u64)
    }
}

/// Open the VST3 editor (or bring it to the front) and return its window handle.
///
/// The controller first takes the processor's state and then follows it while the
/// window exists; the view is created under the gate of ADR-010 and attached to the
/// plugin area, its frame sizing the window.
pub fn open_vst3(adapter: &Arc<Vst3Adapter>, title: &str) -> Result<u64, String> {
    if let Some(hwnd) = raise_open_editor() {
        return Ok(hwnd);
    }
    let watch = adapter.watch_params()?;
    let view = adapter.open_view(resize_to_client)?;
    unsafe {
        let (width, height) = view
            .size()
            .map_or((800, 600), |(width, height)| (width.max(1), height.max(1)));
        let hwnd = create_window(title);
        if hwnd.is_null() {
            return Err("CreateWindowExW failed".into());
        }
        let session = Box::into_raw(Box::new(Vst3Session {
            view,
            _watch: watch,
            adapter: Arc::clone(adapter),
        }));
        let editor = Editor {
            hwnd,
            content: Content::Vst3(session),
        };
        EDITOR.set(Some(editor));
        resize_to_client(hwnd, width, height);
        if let Err(error) = (*session).view.attach(hwnd, plugin_area(hwnd)) {
            close();
            return Err(error);
        }
        fit_to_dpi(&editor, window_dpi(hwnd));
        center_on_screen(hwnd);
        user32::SetTimer(hwnd, MIRROR_TIMER_ID, MIRROR_TIMER_INTERVAL_MS, null_mut());
        user32::ShowWindow(hwnd, SW_SHOW);
        user32::SetForegroundWindow(hwnd);
        Ok(hwnd as u64)
    }
}

/// Destroy the editor if it is open.
pub fn close() {
    let Some(editor) = EDITOR.take() else {
        return;
    };
    unsafe {
        match editor.content {
            Content::Clap {
                plugin,
                gui,
                context,
            } => {
                (*context).set_gui_hwnd(null_mut());
                let _ = ((*gui).hide)(plugin);
                ((*gui).destroy)(plugin);
            }
            Content::Vst3(session) => {
                user32::KillTimer(editor.hwnd, MIRROR_TIMER_ID);
                drop(Box::from_raw(session));
            }
        }
        user32::DestroyWindow(editor.hwnd);
    }
}

unsafe fn register_class(
    name: &[u16],
    style: u32,
    wnd_proc: unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
    background: *mut c_void,
) {
    let window_class = user32::WNDCLASSEXW {
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
        lpszClassName: name.as_ptr(),
        hIconSm: null_mut(),
    };
    user32::RegisterClassExW(&window_class);
}

/// Borderless top-level window holding the drag strip, with the plugin area
/// as its only child.  Returns null if either window can't be created.
///
/// No owner HWND: see `plugin_system/gui.rs` (`create_container`) — ownership
/// would pin the editor above Echoes and can deadlock a sandboxed OpenEditor.
unsafe fn create_window(title: &str) -> *mut c_void {
    let class = wide(CLASS_NAME);
    let area_class = wide(PLUGIN_AREA_CLASS_NAME);
    let instance = user32::GetModuleHandleW(std::ptr::null());
    static REGISTERED: Once = Once::new();
    REGISTERED.call_once(|| {
        // Also fills the plugin area wherever the plugin hasn't repainted yet (mid-resize).
        let background = user32::CreateSolidBrush(DRAG_STRIP_COLOR);
        register_class(&class, CONTAINER_CLASS_STYLE, window_proc, background);
        register_class(&area_class, 0, user32::DefWindowProcW, background);
    });
    let title = wide(title);
    let hwnd = user32::CreateWindowExW(
        WINDOW_EX_STYLE,
        class.as_ptr(),
        title.as_ptr(),
        WINDOW_STYLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        0,
        0,
        null_mut(),
        null_mut(),
        instance,
        null_mut(),
    );
    if hwnd.is_null() {
        return hwnd;
    }
    let area = user32::CreateWindowExW(
        0,
        area_class.as_ptr(),
        std::ptr::null(),
        PLUGIN_AREA_STYLE,
        0,
        0,
        0,
        0,
        hwnd,
        null_mut(),
        instance,
        null_mut(),
    );
    if area.is_null() {
        user32::DestroyWindow(hwnd);
        return null_mut();
    }
    round_corners(hwnd);
    hwnd
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
fn plugin_area(hwnd: *mut c_void) -> *mut c_void {
    unsafe { user32::GetWindow(hwnd, GW_CHILD) }
}

fn drag_strip_height(hwnd: *mut c_void) -> i32 {
    (DRAG_STRIP_HEIGHT_LOGICAL as f64 * window_dpi(hwnd) as f64 / DEFAULT_DPI as f64).round() as i32
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
    let scale = window_dpi(hwnd) as f64 / DEFAULT_DPI as f64;
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

    let hover = CLOSE_BUTTON_HOVER.get();
    let rect = close_button_rect(hwnd);
    // Square glyph centered in the (wider-than-tall) button rect.
    let cx = (rect.left + rect.right) / 2;
    let cy = (rect.top + rect.bottom) / 2;
    let half = ((rect.bottom - rect.top) as f64 * 0.17).round().max(3.0) as i32;
    let stroke_width = (1.5 * window_dpi(hwnd) as f64 / DEFAULT_DPI as f64).max(1.0) as f32;
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

fn window_dpi(hwnd: *mut c_void) -> u32 {
    match unsafe { user32::GetDpiForWindow(hwnd) } {
        0 => DEFAULT_DPI,
        dpi => dpi,
    }
}

/// Size the plugin area to `width` × `height` with the drag strip on top.
unsafe fn resize_to_client(hwnd: *mut c_void, width: i32, height: i32) {
    let width = width.max(200);
    let height = height.max(100);
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

/// Hands the monitor scale to the plugin, then fits the window to its size.
unsafe fn fit_to_dpi(editor: &Editor, dpi: u32) {
    let scale = dpi as f64 / DEFAULT_DPI as f64;
    let size = match editor.content {
        Content::Clap { plugin, gui, .. } => {
            let _ = ((*gui).set_scale)(plugin, scale);
            let (mut width, mut height) = (0u32, 0u32);
            ((*gui).get_size)(plugin, &mut width, &mut height)
                .then_some((width as i32, height as i32))
        }
        Content::Vst3(session) => (*session).view.apply_content_scale(scale),
    };
    if let Some((width, height)) = size {
        resize_to_client(editor.hwnd, width, height);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: *mut c_void,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let editor = EDITOR.get().filter(|editor| editor.hwnd == hwnd);
    match (msg, editor) {
        (WM_NCHITTEST, _) => match user32::DefWindowProcW(hwnd, msg, wparam, lparam) {
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
        },
        (WM_LBUTTONUP, _) => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            if point_in_rect(&close_button_rect(hwnd), x, y) {
                user32::PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            0
        }
        (WM_MOUSEMOVE, _) => {
            // Re-armed on every move: TrackMouseEvent's leave notification is
            // one-shot, and cheap enough to request unconditionally.
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
            if over != CLOSE_BUTTON_HOVER.get() {
                CLOSE_BUTTON_HOVER.set(over);
                let rect = close_button_rect(hwnd);
                user32::InvalidateRect(hwnd, &rect, 0);
            }
            0
        }
        (WM_MOUSELEAVE, _) => {
            if CLOSE_BUTTON_HOVER.get() {
                CLOSE_BUTTON_HOVER.set(false);
                let rect = close_button_rect(hwnd);
                user32::InvalidateRect(hwnd, &rect, 0);
            }
            0
        }
        (WM_PAINT, _) => {
            draw_close_button(hwnd);
            0
        }
        (
            CLAP_GUI_DRAIN_MESSAGE,
            Some(Editor {
                content: Content::Clap {
                    plugin, context, ..
                },
                ..
            }),
        ) => {
            (*context).clear_gui_drain();
            ((*plugin).on_main_thread)(plugin);
            (*context).rearm_gui_drain();
            0
        }
        (
            WM_TIMER,
            Some(Editor {
                content: Content::Vst3(session),
                ..
            }),
        ) => {
            (*session).adapter.mirror_to_controller();
            0
        }
        (CLAP_GUI_RESIZE_MESSAGE, Some(_)) => {
            resize_to_client(hwnd, wparam as i32, lparam as i32);
            0
        }
        (WM_DPICHANGED, Some(editor)) => {
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
            fit_to_dpi(&editor, (wparam & 0xFFFF) as u32);
            0
        }
        (WM_CLOSE, Some(_)) => {
            close();
            0
        }
        _ => user32::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
