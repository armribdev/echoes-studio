//! Host-side COM objects a VST3 plugin receives: the host context passed to
//! `initialize` (`IHostApplication`) and the controller's `IComponentHandler`.
//!
//! Both are stateless statics with dummy refcounts: the host owns them for the
//! whole process, so plugins may keep the pointers as long as they like.

use std::ffi::c_void;
use std::ptr::null_mut;

use super::ffi::{
    FUnknown, IComponentHandlerVtbl, IHostApplicationVtbl, TResult, Tuid, IID_FUNKNOWN,
    IID_ICOMPONENT_HANDLER, IID_IHOST_APPLICATION, K_RESULT_FALSE, K_RESULT_OK,
};

pub(crate) fn tuid_eq(a: *const Tuid, b: &Tuid) -> bool {
    if a.is_null() {
        return false;
    }
    unsafe { (*a) == *b }
}

pub(crate) unsafe extern "C" fn static_add_ref(_: *mut FUnknown) -> u32 {
    1
}
pub(crate) unsafe extern "C" fn static_release(_: *mut FUnknown) -> u32 {
    1
}

unsafe extern "C" fn host_app_qi(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() {
        return -1;
    }
    if tuid_eq(iid, &IID_IHOST_APPLICATION) || tuid_eq(iid, &IID_FUNKNOWN) {
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = null_mut();
    -1
}

unsafe extern "C" fn host_app_get_name(_this: *mut FUnknown, name: *mut u16) -> TResult {
    if name.is_null() {
        return -1;
    }
    let host_name = "Echoes Studio";
    let units: Vec<u16> = host_name.encode_utf16().take(127).collect();
    for (i, &unit) in units.iter().enumerate() {
        *name.add(i) = unit;
    }
    *name.add(units.len()) = 0;
    K_RESULT_OK
}

unsafe extern "C" fn host_app_create_instance(
    _this: *mut FUnknown,
    _cid: *const Tuid,
    _iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    // No IMessage / IAttributeList yet; plugins fall back.
    if !obj.is_null() {
        *obj = null_mut();
    }
    K_RESULT_FALSE
}

static HOST_APP_VTBL: IHostApplicationVtbl = IHostApplicationVtbl {
    query_interface: host_app_qi,
    add_ref: static_add_ref,
    release: static_release,
    get_name: host_app_get_name,
    create_instance: host_app_create_instance,
};

#[repr(C)]
struct HostApplication {
    vtbl: *const IHostApplicationVtbl,
}

// SAFETY: `vtbl` points to an immutable static; the callbacks are stateless.
unsafe impl Sync for HostApplication {}

static HOST_APP: HostApplication = HostApplication {
    vtbl: &HOST_APP_VTBL,
};

/// The host context every component and separate controller is initialised with.
pub fn host_application() -> *mut FUnknown {
    &HOST_APP as *const HostApplication as *mut FUnknown
}

unsafe extern "C" fn handler_qi(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() {
        return -1;
    }
    if tuid_eq(iid, &IID_ICOMPONENT_HANDLER) || tuid_eq(iid, &IID_FUNKNOWN) {
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = null_mut();
    -1
}

unsafe extern "C" fn handler_begin_edit(_: *mut FUnknown, _: u32) -> TResult {
    K_RESULT_OK
}
unsafe extern "C" fn handler_perform_edit(_: *mut FUnknown, _: u32, _: f64) -> TResult {
    K_RESULT_OK
}
unsafe extern "C" fn handler_end_edit(_: *mut FUnknown, _: u32) -> TResult {
    K_RESULT_OK
}
unsafe extern "C" fn handler_restart_component(_: *mut FUnknown, _: i32) -> TResult {
    K_RESULT_OK
}

static HANDLER_VTBL: IComponentHandlerVtbl = IComponentHandlerVtbl {
    query_interface: handler_qi,
    add_ref: static_add_ref,
    release: static_release,
    begin_edit: handler_begin_edit,
    perform_edit: handler_perform_edit,
    end_edit: handler_end_edit,
    restart_component: handler_restart_component,
};

#[repr(C)]
struct ComponentHandler {
    vtbl: *const IComponentHandlerVtbl,
}

// SAFETY: see `HostApplication`.
unsafe impl Sync for ComponentHandler {}

static COMPONENT_HANDLER: ComponentHandler = ComponentHandler {
    vtbl: &HANDLER_VTBL,
};

/// Many plugins crash on the first knob touch without a non-null handler.
/// Edits and restart requests are acknowledged but not acted on yet.
pub fn component_handler() -> *mut FUnknown {
    &COMPONENT_HANDLER as *const ComponentHandler as *mut FUnknown
}
