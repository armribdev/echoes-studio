//! The host context a VST3 plugin receives in `initialize` (`IHostApplication`).
//!
//! A stateless static with dummy refcounts: the host owns it for the whole
//! process, so plugins may keep the pointer as long as they like.

use std::ffi::c_void;
use std::ptr::null_mut;

use super::ffi::{
    FUnknown, IHostApplicationVtbl, TResult, Tuid, IID_FUNKNOWN, IID_IHOST_APPLICATION,
    K_RESULT_FALSE, K_RESULT_OK,
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
