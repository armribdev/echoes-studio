//! The plugin's editor, shown in a top-level window owned by this process
//! (Windows).  Main thread only: CLAP GUI calls and the window's messages both
//! run there.  One bridge hosts one plugin, so there is at most one editor.
//!
//! The plugin's host callbacks reach the window through its `ClapHostContext`
//! (`request_callback` → drain message, `clap.gui` resize → resize message),
//! exactly as for an in-process editor.

use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::ptr::null_mut;

use clap_ffi::{
    ClapGuiExtension, ClapHostContext, ClapPlugin, ClapWindow, CLAP_EXT_GUI,
    CLAP_GUI_DRAIN_MESSAGE, CLAP_GUI_RESIZE_MESSAGE, CLAP_WINDOW_API_WIN32,
};

use crate::plugin::LoadedPlugin;

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
        pub fn AdjustWindowRectExForDpi(
            rect: *mut RECT,
            style: u32,
            menu: i32,
            ex_style: u32,
            dpi: u32,
        ) -> i32;
        pub fn GetDpiForWindow(hwnd: *mut c_void) -> u32;
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
}

const WM_CLOSE: u32 = 0x0010;
const WM_DPICHANGED: u32 = 0x02E0;
const WS_OVERLAPPED_WINDOW_FIXED: u32 = 0x00C0_0000 | 0x0008_0000 | 0x0002_0000; // caption | sysmenu | minimize
const SW_SHOW: i32 = 5;
const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
const SWP_RESIZE_ONLY: u32 = 0x0002 | 0x0004 | 0x0010;
const SWP_MOVE_AND_RESIZE: u32 = 0x0004 | 0x0010;
const PM_REMOVE: u32 = 0x0001;
const QS_ALLINPUT: u32 = 0x04FF;
const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
const DEFAULT_DPI: u32 = 96;
const CLASS_NAME: &str = "EchoesSandboxedPluginWindow";

#[derive(Clone, Copy)]
struct Editor {
    hwnd: *mut c_void,
    plugin: *const ClapPlugin,
    gui: *const ClapGuiExtension,
    context: *const ClapHostContext,
}

thread_local! {
    // `Cell` of plain pointers: the plugin may re-enter the window procedure
    // from inside any GUI call, so nothing here is ever borrowed across one.
    static EDITOR: Cell<Option<Editor>> = const { Cell::new(None) };
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

/// Open the editor (or bring it to the front) and return its window handle.
pub fn open(plugin: &LoadedPlugin, title: &str) -> Result<u64, String> {
    if let Some(editor) = EDITOR.get() {
        unsafe { user32::SetForegroundWindow(editor.hwnd) };
        return Ok(editor.hwnd as u64);
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
            plugin: clap,
            gui,
            context,
        };
        EDITOR.set(Some(editor));
        (*context).set_gui_hwnd(hwnd);
        fit_to_dpi(&editor, window_dpi(hwnd));

        let window = ClapWindow {
            api,
            specific: hwnd as usize,
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

/// Destroy the editor if it is open.
pub fn close() {
    let Some(editor) = EDITOR.take() else {
        return;
    };
    unsafe {
        (*editor.context).set_gui_hwnd(null_mut());
        let _ = ((*editor.gui).hide)(editor.plugin);
        ((*editor.gui).destroy)(editor.plugin);
        user32::DestroyWindow(editor.hwnd);
    }
}

unsafe fn create_window(title: &str) -> *mut c_void {
    let class = wide(CLASS_NAME);
    let instance = user32::GetModuleHandleW(std::ptr::null());
    let window_class = user32::WNDCLASSEXW {
        cbSize: std::mem::size_of::<user32::WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: window_proc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
        hIconSm: null_mut(),
    };
    // A second registration fails harmlessly: the class already exists.
    user32::RegisterClassExW(&window_class);
    let title = wide(title);
    user32::CreateWindowExW(
        0,
        class.as_ptr(),
        title.as_ptr(),
        WS_OVERLAPPED_WINDOW_FIXED,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        0,
        0,
        null_mut(),
        null_mut(),
        instance,
        null_mut(),
    )
}

fn window_dpi(hwnd: *mut c_void) -> u32 {
    match unsafe { user32::GetDpiForWindow(hwnd) } {
        0 => DEFAULT_DPI,
        dpi => dpi,
    }
}

unsafe fn resize_to_client(hwnd: *mut c_void, width: i32, height: i32) {
    let mut rect = user32::RECT {
        left: 0,
        top: 0,
        right: width.max(200),
        bottom: height.max(100),
    };
    user32::AdjustWindowRectExForDpi(
        &mut rect,
        WS_OVERLAPPED_WINDOW_FIXED,
        0,
        0,
        window_dpi(hwnd),
    );
    user32::SetWindowPos(
        hwnd,
        null_mut(),
        0,
        0,
        rect.right - rect.left,
        rect.bottom - rect.top,
        SWP_RESIZE_ONLY,
    );
}

/// Hands the monitor scale to the plugin, then fits the window to its size.
unsafe fn fit_to_dpi(editor: &Editor, dpi: u32) {
    let _ = ((*editor.gui).set_scale)(editor.plugin, dpi as f64 / DEFAULT_DPI as f64);
    let (mut width, mut height) = (0u32, 0u32);
    if ((*editor.gui).get_size)(editor.plugin, &mut width, &mut height) {
        resize_to_client(editor.hwnd, width as i32, height as i32);
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
        (CLAP_GUI_DRAIN_MESSAGE, Some(editor)) => {
            (*editor.context).clear_gui_drain();
            ((*editor.plugin).on_main_thread)(editor.plugin);
            (*editor.context).rearm_gui_drain();
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
