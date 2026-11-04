//! The host's `IMessage`: what a plugin asks `IHostApplication::createInstance` for
//! to send a message between its component and its controller.
//!
//! The two halves of a dual-component plugin are connected directly
//! (`IConnectionPoint::connect`), so the host only creates messages; it never
//! relays them.  Counted and heap allocated like [`HostAttributeList`](super::attribute_list).

use std::ffi::{c_char, c_void, CStr};
use std::ptr::null_mut;
use std::sync::Mutex;

use super::attribute_list::OwnedAttributeList;
#[cfg(test)]
use super::com_object::LifeProbe;
use super::com_object::ReferenceCount;
use super::host_context::tuid_eq;
use vst3_ffi::{
    FUnknown, IMessageVtbl, TResult, Tuid, IID_FUNKNOWN, IID_IMESSAGE, K_INVALID_ARGUMENT,
    K_NO_INTERFACE, K_RESULT_OK,
};

/// A longer identifier is cut: `setMessageID` cannot report an error.
const MAX_ID_BYTES: usize = 256;

/// The COM object handed to the plugin.  `vtbl` MUST stay at offset 0.
#[repr(C)]
pub struct HostMessage {
    vtbl: *const IMessageVtbl,
    references: ReferenceCount,
    /// NUL-terminated; `getMessageID` hands out a pointer into it, valid until
    /// the next `setMessageID`.
    id: Mutex<Vec<u8>>,
    attributes: OwnedAttributeList,
    #[cfg(test)]
    probe: LifeProbe,
}

impl HostMessage {
    /// A message with an empty identifier and attribute list; the caller owns the
    /// one reference, to release through COM.
    pub fn new_raw() -> *mut c_void {
        Self::allocate(OwnedAttributeList::new())
    }

    fn allocate(attributes: OwnedAttributeList) -> *mut c_void {
        let message = Self {
            vtbl: &MESSAGE_VTBL,
            references: ReferenceCount::new(),
            id: Mutex::new(vec![0]),
            attributes,
            #[cfg(test)]
            probe: LifeProbe::default(),
        };
        Box::into_raw(Box::new(message)) as *mut c_void
    }
}

unsafe fn message<'a>(this: *mut FUnknown) -> &'a HostMessage {
    &*(this as *const HostMessage)
}

unsafe extern "C" fn message_query_interface(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() {
        return K_INVALID_ARGUMENT;
    }
    if tuid_eq(iid, &IID_IMESSAGE) || tuid_eq(iid, &IID_FUNKNOWN) {
        message(this).references.add();
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = null_mut();
    K_NO_INTERFACE
}

unsafe extern "C" fn message_add_ref(this: *mut FUnknown) -> u32 {
    message(this).references.add()
}

unsafe extern "C" fn message_release(this: *mut FUnknown) -> u32 {
    let remaining = message(this).references.release();
    if remaining == 0 {
        drop(Box::from_raw(this as *mut HostMessage));
    }
    remaining
}

unsafe extern "C" fn message_get_message_id(this: *mut FUnknown) -> *const c_char {
    let id = message(this).id.lock().unwrap_or_else(|e| e.into_inner());
    id.as_ptr() as *const c_char
}

unsafe extern "C" fn message_set_message_id(this: *mut FUnknown, id: *const c_char) {
    let mut bytes = if id.is_null() {
        Vec::new()
    } else {
        CStr::from_ptr(id).to_bytes().to_vec()
    };
    bytes.truncate(MAX_ID_BYTES);
    bytes.push(0);
    *message(this).id.lock().unwrap_or_else(|e| e.into_inner()) = bytes;
}

/// The list belongs to the message: no reference is added for the caller.
unsafe extern "C" fn message_get_attributes(this: *mut FUnknown) -> *mut FUnknown {
    message(this).attributes.as_funknown()
}

static MESSAGE_VTBL: IMessageVtbl = IMessageVtbl {
    query_interface: message_query_interface,
    add_ref: message_add_ref,
    release: message_release,
    get_message_id: message_get_message_id,
    set_message_id: message_set_message_id,
    get_attributes: message_get_attributes,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use vst3_ffi::{IAttributeListVtbl, IID_IATTRIBUTE_LIST};

    fn vtbl(message: *mut FUnknown) -> &'static IMessageVtbl {
        unsafe { &*(*(message as *mut HostMessage)).vtbl }
    }

    fn list_vtbl(list: *mut FUnknown) -> &'static IAttributeListVtbl {
        unsafe { &*(*(list as *const *const IAttributeListVtbl)) }
    }

    fn id_of(message: *mut FUnknown) -> String {
        let id = unsafe { (vtbl(message).get_message_id)(message) };
        unsafe { CStr::from_ptr(id) }.to_str().unwrap().to_owned()
    }

    fn watched_message(
        message_freed: &Arc<AtomicBool>,
        list_freed: &Arc<AtomicBool>,
    ) -> *mut FUnknown {
        let raw = HostMessage::allocate(OwnedAttributeList::watching(list_freed));
        unsafe { (*(raw as *mut HostMessage)).probe = LifeProbe::watching(message_freed) };
        raw as *mut FUnknown
    }

    #[test]
    fn a_new_message_has_an_empty_id_and_its_own_attributes() {
        let message = HostMessage::new_raw() as *mut FUnknown;
        assert_eq!(id_of(message), "");

        let attributes = unsafe { (vtbl(message).get_attributes)(message) };
        let key = CString::new("n").unwrap();
        unsafe {
            assert_eq!(
                (list_vtbl(attributes).set_int)(attributes, key.as_ptr(), 9),
                0
            );
            let again = (vtbl(message).get_attributes)(message);
            assert_eq!(again, attributes, "the same list every time");
            let mut value = 0;
            assert_eq!(
                (list_vtbl(again).get_int)(again, key.as_ptr(), &mut value),
                0
            );
            assert_eq!(value, 9);
            assert_eq!((vtbl(message).release)(message), 0);
        }
    }

    #[test]
    fn the_id_can_be_set_replaced_cleared_and_is_cut_when_huge() {
        let message = HostMessage::new_raw() as *mut FUnknown;
        let v = vtbl(message);
        unsafe {
            (v.set_message_id)(message, CString::new("TextMessage").unwrap().as_ptr());
            assert_eq!(id_of(message), "TextMessage");
            (v.set_message_id)(message, CString::new("Other").unwrap().as_ptr());
            assert_eq!(id_of(message), "Other");
            (v.set_message_id)(message, std::ptr::null());
            assert_eq!(id_of(message), "");
            let huge = CString::new("x".repeat(MAX_ID_BYTES + 50)).unwrap();
            (v.set_message_id)(message, huge.as_ptr());
            assert_eq!(id_of(message).len(), MAX_ID_BYTES);
            (v.release)(message);
        }
    }

    #[test]
    fn releasing_the_message_frees_its_list() {
        let message_freed = Arc::new(AtomicBool::new(false));
        let list_freed = Arc::new(AtomicBool::new(false));
        let message = watched_message(&message_freed, &list_freed);
        unsafe {
            assert_eq!((vtbl(message).add_ref)(message), 2);
            assert_eq!((vtbl(message).release)(message), 1);
            assert!(!message_freed.load(Ordering::Acquire));
            assert_eq!((vtbl(message).release)(message), 0);
        }
        assert!(message_freed.load(Ordering::Acquire));
        assert!(list_freed.load(Ordering::Acquire));
    }

    #[test]
    fn a_list_the_plugin_kept_outlives_its_message() {
        let message_freed = Arc::new(AtomicBool::new(false));
        let list_freed = Arc::new(AtomicBool::new(false));
        let message = watched_message(&message_freed, &list_freed);
        unsafe {
            let attributes = (vtbl(message).get_attributes)(message);
            assert_eq!((list_vtbl(attributes).add_ref)(attributes), 2);

            assert_eq!((vtbl(message).release)(message), 0);
            assert!(message_freed.load(Ordering::Acquire));
            assert!(!list_freed.load(Ordering::Acquire));

            assert_eq!((list_vtbl(attributes).release)(attributes), 0);
        }
        assert!(list_freed.load(Ordering::Acquire));
    }

    #[test]
    fn query_interface_adds_a_reference_and_refuses_other_interfaces() {
        let message_freed = Arc::new(AtomicBool::new(false));
        let list_freed = Arc::new(AtomicBool::new(false));
        let message = watched_message(&message_freed, &list_freed);
        let v = vtbl(message);
        let mut found: *mut c_void = null_mut();
        let mut other: *mut c_void = 1 as *mut c_void;
        unsafe {
            assert_eq!(
                (v.query_interface)(message, &IID_IMESSAGE, &mut found),
                K_RESULT_OK
            );
            assert_eq!(found, message as *mut c_void);
            assert_eq!(
                (v.query_interface)(message, &IID_IATTRIBUTE_LIST, &mut other),
                K_NO_INTERFACE
            );
            assert!(other.is_null());
            assert_eq!(
                (v.query_interface)(message, &IID_IMESSAGE, null_mut()),
                K_INVALID_ARGUMENT
            );

            assert_eq!((v.release)(message), 1);
            assert!(!message_freed.load(Ordering::Acquire));
            assert_eq!((v.release)(found as *mut FUnknown), 0);
        }
        assert!(message_freed.load(Ordering::Acquire));
    }
}
