//! The host context a VST3 plugin receives in `initialize` (`IHostApplication`).
//!
//! A stateless static with dummy refcounts: the host owns it for the whole
//! process, so plugins may keep the pointer as long as they like.  The objects it
//! creates for a plugin (`IMessage`, `IAttributeList`) are not static: they are
//! counted and freed by their last `release`.

use std::ffi::c_void;
use std::ptr::null_mut;

use super::attribute_list::OwnedAttributeList;
use super::ffi::{
    FUnknown, IHostApplicationVtbl, TResult, Tuid, IID_FUNKNOWN, IID_IATTRIBUTE_LIST,
    IID_IHOST_APPLICATION, IID_IMESSAGE, K_INVALID_ARGUMENT, K_RESULT_FALSE, K_RESULT_OK,
};
use super::message::HostMessage;

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

/// Like the SDK's host: an object is made only when `cid` and `iid` both name the
/// same interface (`allocateMessage` passes `IMessage::iid` twice).  The caller
/// owns the one reference of the new object.
unsafe extern "C" fn host_app_create_instance(
    _this: *mut FUnknown,
    cid: *const Tuid,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() {
        return K_INVALID_ARGUMENT;
    }
    *obj = null_mut();
    if tuid_eq(cid, &IID_IMESSAGE) && tuid_eq(iid, &IID_IMESSAGE) {
        *obj = HostMessage::new_raw();
        return K_RESULT_OK;
    }
    if tuid_eq(cid, &IID_IATTRIBUTE_LIST) && tuid_eq(iid, &IID_IATTRIBUTE_LIST) {
        *obj = OwnedAttributeList::new().into_raw();
        return K_RESULT_OK;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_formats::vst3::ffi::{IAttributeListVtbl, IMessageVtbl, IID_IBSTREAM};

    fn create(cid: &Tuid, iid: &Tuid) -> (TResult, *mut c_void) {
        let host = host_application();
        let mut obj: *mut c_void = 1 as *mut c_void;
        let result = unsafe {
            let vtbl = &*(*(host as *const HostApplication)).vtbl;
            (vtbl.create_instance)(host, cid, iid, &mut obj)
        };
        (result, obj)
    }

    #[test]
    fn a_message_is_created_when_the_plugin_asks_for_one() {
        let (result, obj) = create(&IID_IMESSAGE, &IID_IMESSAGE);
        assert_eq!(result, K_RESULT_OK);
        let message = obj as *mut FUnknown;
        let vtbl = unsafe { &*(*(message as *const *const IMessageVtbl)) };
        unsafe {
            assert_eq!((vtbl.add_ref)(message), 2);
            assert_eq!((vtbl.release)(message), 1);
            assert_eq!((vtbl.release)(message), 0);
        }
    }

    #[test]
    fn an_attribute_list_is_created_when_the_plugin_asks_for_one() {
        let (result, obj) = create(&IID_IATTRIBUTE_LIST, &IID_IATTRIBUTE_LIST);
        assert_eq!(result, K_RESULT_OK);
        let list = obj as *mut FUnknown;
        let vtbl = unsafe { &*(*(list as *const *const IAttributeListVtbl)) };
        unsafe {
            assert_eq!((vtbl.add_ref)(list), 2);
            assert_eq!((vtbl.release)(list), 1);
            assert_eq!((vtbl.release)(list), 0);
        }
    }

    #[test]
    fn every_other_request_is_refused_with_a_null_object() {
        for (cid, iid) in [
            (&IID_IBSTREAM, &IID_IBSTREAM),
            (&IID_IMESSAGE, &IID_IATTRIBUTE_LIST),
            (&IID_IATTRIBUTE_LIST, &IID_IMESSAGE),
            (&IID_IMESSAGE, &IID_IBSTREAM),
        ] {
            let (result, obj) = create(cid, iid);
            assert_eq!(result, K_RESULT_FALSE);
            assert!(obj.is_null());
        }
    }

    #[test]
    fn a_null_class_or_interface_id_is_refused() {
        let host = host_application();
        let vtbl = unsafe { &*(*(host as *const HostApplication)).vtbl };
        let mut obj: *mut c_void = 1 as *mut c_void;
        let result =
            unsafe { (vtbl.create_instance)(host, std::ptr::null(), &IID_IMESSAGE, &mut obj) };
        assert_eq!(result, K_RESULT_FALSE);
        assert!(obj.is_null());
    }

    #[test]
    fn a_missing_output_pointer_is_an_invalid_argument() {
        let host = host_application();
        let vtbl = unsafe { &*(*(host as *const HostApplication)).vtbl };
        let result = unsafe {
            (vtbl.create_instance)(host, &IID_IMESSAGE, &IID_IMESSAGE, std::ptr::null_mut())
        };
        assert_eq!(result, K_INVALID_ARGUMENT);
    }
}
