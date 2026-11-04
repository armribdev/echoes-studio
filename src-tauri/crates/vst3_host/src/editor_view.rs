//! The editor view of a VST3 plugin and the frame the host gives it (ADR-033, step 7).
//!
//! What is the same wherever the window lives — the Echoes process or the plugin
//! bridge: `createView`, the frame that answers `resizeView` in the same call stack,
//! the content scale, and the order of `attached` / `removed`. The window itself, and
//! how it is resized, belong to the caller: it gives the frame a function that sizes
//! its window. All calls are made on the thread of the window.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};

use vst3_ffi::{
    FUnknown, IEditController, IPlugFrameVtbl, IPlugView, IPlugViewContentScaleSupport, TResult,
    Tuid, ViewRect, IID_FUNKNOWN, IID_IPLUG_FRAME, IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT,
    K_INVALID_ARGUMENT, K_NO_INTERFACE, K_RESULT_FALSE, K_RESULT_OK,
};

use crate::host_context::{static_add_ref, static_release, tuid_eq};

/// Nested `onSize` → `resizeView` → `onSize` rounds accepted before a request is refused.
pub const MAX_RESIZE_DEPTH: u8 = 3;

/// Sizes the caller's window so that the area the view lives in is `width` × `height`
/// physical pixels. Called with the window the frame was attached to.
pub type ResizeWindow = unsafe fn(window: *mut c_void, width: i32, height: i32);

#[repr(C)]
struct PlugFrame {
    vtbl: *const IPlugFrameVtbl,
    window: AtomicPtr<c_void>,
    resize_window: ResizeWindow,
    view: *mut IPlugView,
    /// `onSize` calls in progress: a plugin may call `resizeView` from `onSize`.
    resize_depth: Cell<u8>,
}

unsafe extern "C" fn frame_query_interface(
    this: *mut FUnknown,
    iid: *const Tuid,
    object: *mut *mut c_void,
) -> TResult {
    if object.is_null() {
        return K_INVALID_ARGUMENT;
    }
    if tuid_eq(iid, &IID_IPLUG_FRAME) || tuid_eq(iid, &IID_FUNKNOWN) {
        *object = this as *mut c_void;
        return K_RESULT_OK;
    }
    *object = null_mut();
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
    let window = frame.window.load(Ordering::Acquire);
    let depth = frame.resize_depth.get();
    if window.is_null() || depth >= MAX_RESIZE_DEPTH {
        return K_RESULT_FALSE;
    }

    let mut requested = *rect;
    let vtbl = &*(*view).vtbl;
    let mut reported = requested;
    if (vtbl.get_size)(view, &mut reported) == K_RESULT_OK && reported == requested {
        return K_RESULT_OK;
    }

    (frame.resize_window)(
        window,
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
    query_interface: frame_query_interface,
    add_ref: static_add_ref,
    release: static_release,
    resize_view: frame_resize_view,
};

impl PlugFrame {
    fn boxed(view: *mut IPlugView, resize_window: ResizeWindow) -> Box<Self> {
        Box::new(PlugFrame {
            vtbl: &PLUG_FRAME_VTBL,
            window: AtomicPtr::new(null_mut()),
            resize_window,
            view,
            resize_depth: Cell::new(0),
        })
    }

    fn as_funknown(&self) -> *mut FUnknown {
        self as *const PlugFrame as *mut FUnknown
    }
}

/// A plugin's editor view with the frame it was given. Dropping it takes the frame
/// away, then calls `removed()`, then releases the view: a plugin must not ask for a
/// size while it detaches, and the view goes before the controller that made it.
pub struct EditorView {
    view: *mut IPlugView,
    frame: Box<PlugFrame>,
}

impl EditorView {
    /// `createView("editor")` on the controller, which must be one `HWND` can attach to,
    /// with the frame already set (a view may ask for a size as soon as it exists).
    ///
    /// # Safety
    /// `controller` is a live edit controller, and the caller keeps its calls apart
    /// from `process` the way it does for any other controller call (ADR-010).
    pub unsafe fn create(
        controller: *mut IEditController,
        resize_window: ResizeWindow,
    ) -> Result<Self, String> {
        let view = ((*(*controller).vtbl).create_view)(controller, c"editor".as_ptr().cast());
        if view.is_null() {
            return Err("VST3 GUI: IEditController::createView returned null".into());
        }
        if ((*(*view).vtbl).is_platform_type_supported)(view, c"HWND".as_ptr().cast())
            != K_RESULT_OK
        {
            ((*(*view).vtbl).release)(view);
            return Err("VST3 GUI: plugin view does not support HWND".into());
        }
        let frame = PlugFrame::boxed(view, resize_window);
        let _ = ((*(*view).vtbl).set_frame)(view, frame.as_funknown());
        Ok(EditorView { view, frame })
    }

    /// The size the view asks for before it is shown, in physical pixels.
    ///
    /// # Safety
    /// Window thread.
    pub unsafe fn size(&self) -> Option<(i32, i32)> {
        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if ((*(*self.view).vtbl).get_size)(self.view, &mut rect) != K_RESULT_OK {
            return None;
        }
        Some((rect.right - rect.left, rect.bottom - rect.top))
    }

    /// `attached` to `area`, the child window of `window` the view draws into. From
    /// now on a `resizeView` of the view sizes `window`.
    ///
    /// # Safety
    /// Window thread; both are live windows of this process.
    pub unsafe fn attach(&self, window: *mut c_void, area: *mut c_void) -> Result<(), String> {
        self.frame.window.store(window, Ordering::Release);
        match ((*(*self.view).vtbl).attached)(self.view, area, c"HWND".as_ptr().cast()) {
            K_RESULT_OK => Ok(()),
            code => Err(format!("VST3 GUI: IPlugView::attached failed: {code}")),
        }
    }

    /// Hands the monitor scale to the view, when it takes one, and returns the size
    /// it reports at that scale.
    ///
    /// # Safety
    /// Window thread.
    pub unsafe fn apply_content_scale(&self, scale: f64) -> Option<(i32, i32)> {
        let mut support: *mut c_void = null_mut();
        let found = ((*(*self.view).vtbl).query_interface)(
            self.view,
            &IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT,
            &mut support,
        );
        if found == K_RESULT_OK && !support.is_null() {
            let support = support as *mut IPlugViewContentScaleSupport;
            let vtbl = &*(*support).vtbl;
            let _ = (vtbl.set_content_scale_factor)(support, scale as f32);
            (vtbl.release)(support);
        }
        self.size()
    }
}

impl Drop for EditorView {
    fn drop(&mut self) {
        // SAFETY: the view is ours; the caller drops on the window thread.
        unsafe {
            let vtbl = &*(*self.view).vtbl;
            let _ = (vtbl.set_frame)(self.view, null_mut());
            (vtbl.removed)(self.view);
            (vtbl.release)(self.view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn no_window(_: *mut c_void, _: i32, _: i32) {}

    fn frame_for(view: *mut IPlugView) -> Box<PlugFrame> {
        PlugFrame::boxed(view, no_window)
    }

    #[test]
    fn the_frame_answers_the_sdk_codes() {
        let frame = frame_for(null_mut());
        let this = frame.as_funknown();
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
        let frame = frame_for(view);
        let this = frame.as_funknown();
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
