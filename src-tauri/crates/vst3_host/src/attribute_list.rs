//! The host's `IAttributeList`: a small bounded dictionary that a plugin and the
//! host exchange values through (the attributes of an `IMessage`, the channel
//! context of `IInfoListener`).
//!
//! Counted and heap allocated, so a plugin may keep a list past the call that gave
//! it over; the last `release` frees it.  The limits below are host policy, not the
//! SDK's: a full list answers `kOutOfMemory`, an oversized value `kInvalidArgument`.
//! A list lives on non-real-time threads (the spec calls these methods from the UI
//! thread); the host never touches one from the audio thread.

use std::ffi::{c_char, c_void, CStr};
use std::mem::size_of;
use std::ops::Deref;
use std::ptr::{null_mut, NonNull};
use std::sync::Mutex;

#[cfg(test)]
use super::com_object::LifeProbe;
use super::com_object::ReferenceCount;
use super::host_context::tuid_eq;
use vst3_ffi::{
    FUnknown, IAttributeListVtbl, TResult, Tuid, IID_FUNKNOWN, IID_IATTRIBUTE_LIST,
    K_INVALID_ARGUMENT, K_NO_INTERFACE, K_OUT_OF_MEMORY, K_RESULT_FALSE, K_RESULT_OK,
};

const MAX_ENTRIES: usize = 32;
const MAX_KEY_BYTES: usize = 64;
const MAX_TEXT_UNITS: usize = 1024;
const MAX_BINARY_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Text(Vec<u16>),
    Binary(Vec<u8>),
}

struct Entry {
    key: Vec<u8>,
    value: Value,
}

/// The COM object handed to the plugin.  `vtbl` MUST stay at offset 0.
#[repr(C)]
pub struct HostAttributeList {
    vtbl: *const IAttributeListVtbl,
    references: ReferenceCount,
    entries: Mutex<Vec<Entry>>,
    #[cfg(test)]
    probe: LifeProbe,
}

impl HostAttributeList {
    fn new() -> Self {
        Self {
            vtbl: &ATTRIBUTE_LIST_VTBL,
            references: ReferenceCount::new(),
            entries: Mutex::new(Vec::new()),
            #[cfg(test)]
            probe: LifeProbe::default(),
        }
    }

    fn set_value(&self, key: &[u8], value: Value) -> TResult {
        if key.is_empty() || key.len() > MAX_KEY_BYTES {
            return K_INVALID_ARGUMENT;
        }
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.iter_mut().find(|entry| entry.key == key) {
            entry.value = value;
            return K_RESULT_OK;
        }
        if entries.len() >= MAX_ENTRIES {
            return K_OUT_OF_MEMORY;
        }
        entries.push(Entry {
            key: key.to_vec(),
            value,
        });
        K_RESULT_OK
    }

    fn find<R>(&self, key: &[u8], pick: impl FnOnce(&Value) -> Option<R>) -> Option<R> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .iter()
            .find(|entry| entry.key == key)
            .and_then(|entry| pick(&entry.value))
    }

    pub fn set_int(&self, key: &str, value: i64) -> TResult {
        self.set_value(key.as_bytes(), Value::Int(value))
    }

    /// Stored as UTF-16 without a terminator.
    pub fn set_text(&self, key: &str, text: &str) -> TResult {
        let units = text.encode_utf16().collect();
        self.set_value(key.as_bytes(), Value::Text(units))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn value(&self, key: &str) -> Option<Value> {
        self.find(key.as_bytes(), |value| Some(value.clone()))
    }
}

/// One reference to a [`HostAttributeList`], released on drop.
pub struct OwnedAttributeList(NonNull<HostAttributeList>);

impl Default for OwnedAttributeList {
    fn default() -> Self {
        Self::new()
    }
}

impl OwnedAttributeList {
    pub fn new() -> Self {
        Self::own(HostAttributeList::new())
    }

    #[cfg(test)]
    pub fn watching(freed: &std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        let mut list = HostAttributeList::new();
        list.probe = LifeProbe::watching(freed);
        Self::own(list)
    }

    fn own(list: HostAttributeList) -> Self {
        Self(NonNull::from(Box::leak(Box::new(list))))
    }

    pub fn as_funknown(&self) -> *mut FUnknown {
        self.0.as_ptr() as *mut FUnknown
    }

    /// Gives the reference to the caller, who releases it through COM.
    pub fn into_raw(self) -> *mut c_void {
        let raw = self.0.as_ptr() as *mut c_void;
        std::mem::forget(self);
        raw
    }
}

impl Deref for OwnedAttributeList {
    type Target = HostAttributeList;

    fn deref(&self) -> &HostAttributeList {
        // SAFETY: this owner holds a reference, so the list is alive.
        unsafe { self.0.as_ref() }
    }
}

impl Drop for OwnedAttributeList {
    fn drop(&mut self) {
        // SAFETY: releases the one reference this owner holds.
        unsafe { list_release(self.as_funknown()) };
    }
}

unsafe fn list<'a>(this: *mut FUnknown) -> &'a HostAttributeList {
    &*(this as *const HostAttributeList)
}

unsafe fn key_bytes<'a>(id: *const c_char) -> Option<&'a [u8]> {
    (!id.is_null()).then(|| CStr::from_ptr(id).to_bytes())
}

unsafe extern "C" fn list_query_interface(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() {
        return K_INVALID_ARGUMENT;
    }
    if tuid_eq(iid, &IID_IATTRIBUTE_LIST) || tuid_eq(iid, &IID_FUNKNOWN) {
        list(this).references.add();
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = null_mut();
    K_NO_INTERFACE
}

unsafe extern "C" fn list_add_ref(this: *mut FUnknown) -> u32 {
    list(this).references.add()
}

unsafe extern "C" fn list_release(this: *mut FUnknown) -> u32 {
    let remaining = list(this).references.release();
    if remaining == 0 {
        drop(Box::from_raw(this as *mut HostAttributeList));
    }
    remaining
}

unsafe extern "C" fn list_set_int(this: *mut FUnknown, id: *const c_char, value: i64) -> TResult {
    match key_bytes(id) {
        Some(key) => list(this).set_value(key, Value::Int(value)),
        None => K_INVALID_ARGUMENT,
    }
}

unsafe extern "C" fn list_get_int(
    this: *mut FUnknown,
    id: *const c_char,
    value: *mut i64,
) -> TResult {
    let Some(key) = key_bytes(id).filter(|_| !value.is_null()) else {
        return K_INVALID_ARGUMENT;
    };
    let found = list(this).find(key, |stored| match stored {
        Value::Int(int) => Some(*int),
        _ => None,
    });
    match found {
        Some(int) => {
            *value = int;
            K_RESULT_OK
        }
        None => K_RESULT_FALSE,
    }
}

unsafe extern "C" fn list_set_float(this: *mut FUnknown, id: *const c_char, value: f64) -> TResult {
    match key_bytes(id) {
        Some(key) => list(this).set_value(key, Value::Float(value)),
        None => K_INVALID_ARGUMENT,
    }
}

unsafe extern "C" fn list_get_float(
    this: *mut FUnknown,
    id: *const c_char,
    value: *mut f64,
) -> TResult {
    let Some(key) = key_bytes(id).filter(|_| !value.is_null()) else {
        return K_INVALID_ARGUMENT;
    };
    let found = list(this).find(key, |stored| match stored {
        Value::Float(float) => Some(*float),
        _ => None,
    });
    match found {
        Some(float) => {
            *value = float;
            K_RESULT_OK
        }
        None => K_RESULT_FALSE,
    }
}

unsafe extern "C" fn list_set_string(
    this: *mut FUnknown,
    id: *const c_char,
    string: *const u16,
) -> TResult {
    let Some(key) = key_bytes(id).filter(|_| !string.is_null()) else {
        return K_INVALID_ARGUMENT;
    };
    // Never read past the limit looking for a terminator that is not there.
    let mut length = 0;
    while length <= MAX_TEXT_UNITS && *string.add(length) != 0 {
        length += 1;
    }
    if length > MAX_TEXT_UNITS {
        return K_INVALID_ARGUMENT;
    }
    let text = std::slice::from_raw_parts(string, length).to_vec();
    list(this).set_value(key, Value::Text(text))
}

/// Copies as much of `text` as fits in `capacity` units, always terminated.
unsafe fn write_text(text: &[u16], buffer: *mut u16, capacity: usize) -> TResult {
    if capacity == 0 {
        return K_RESULT_FALSE;
    }
    let copied = text.len().min(capacity - 1);
    std::ptr::copy_nonoverlapping(text.as_ptr(), buffer, copied);
    *buffer.add(copied) = 0;
    K_RESULT_OK
}

unsafe extern "C" fn list_get_string(
    this: *mut FUnknown,
    id: *const c_char,
    string: *mut u16,
    size_in_bytes: u32,
) -> TResult {
    let Some(key) = key_bytes(id).filter(|_| !string.is_null()) else {
        return K_INVALID_ARGUMENT;
    };
    let capacity = size_in_bytes as usize / size_of::<u16>();
    let written = list(this).find(key, |stored| match stored {
        Value::Text(text) => Some(write_text(text, string, capacity)),
        _ => None,
    });
    written.unwrap_or(K_RESULT_FALSE)
}

unsafe extern "C" fn list_set_binary(
    this: *mut FUnknown,
    id: *const c_char,
    data: *const c_void,
    size_in_bytes: u32,
) -> TResult {
    let Some(key) = key_bytes(id) else {
        return K_INVALID_ARGUMENT;
    };
    let size = size_in_bytes as usize;
    if size > MAX_BINARY_BYTES || (data.is_null() && size > 0) {
        return K_INVALID_ARGUMENT;
    }
    let bytes = if size == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(data as *const u8, size).to_vec()
    };
    list(this).set_value(key, Value::Binary(bytes))
}

unsafe extern "C" fn list_get_binary(
    this: *mut FUnknown,
    id: *const c_char,
    data: *mut *const c_void,
    size_in_bytes: *mut u32,
) -> TResult {
    let Some(key) = key_bytes(id).filter(|_| !data.is_null() && !size_in_bytes.is_null()) else {
        return K_INVALID_ARGUMENT;
    };
    let found = list(this).find(key, |stored| match stored {
        Value::Binary(bytes) => Some((bytes.as_ptr() as *const c_void, bytes.len() as u32)),
        _ => None,
    });
    match found {
        Some((pointer, size)) => {
            *data = pointer;
            *size_in_bytes = size;
            K_RESULT_OK
        }
        None => {
            *data = std::ptr::null();
            *size_in_bytes = 0;
            K_RESULT_FALSE
        }
    }
}

static ATTRIBUTE_LIST_VTBL: IAttributeListVtbl = IAttributeListVtbl {
    query_interface: list_query_interface,
    add_ref: list_add_ref,
    release: list_release,
    set_int: list_set_int,
    get_int: list_get_int,
    set_float: list_set_float,
    get_float: list_get_float,
    set_string: list_set_string,
    get_string: list_get_string,
    set_binary: list_set_binary,
    get_binary: list_get_binary,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::mem::size_of_val;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use vst3_ffi::IID_IMESSAGE;

    fn key(text: &str) -> CString {
        CString::new(text).unwrap()
    }

    fn vtbl(list: &OwnedAttributeList) -> &'static IAttributeListVtbl {
        unsafe { &*(*list.0.as_ptr()).vtbl }
    }

    fn utf16(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn set_int(list: &OwnedAttributeList, name: &str, value: i64) -> TResult {
        unsafe { (vtbl(list).set_int)(list.as_funknown(), key(name).as_ptr(), value) }
    }

    fn get_int(list: &OwnedAttributeList, name: &str) -> (TResult, i64) {
        let mut value = -1;
        let result =
            unsafe { (vtbl(list).get_int)(list.as_funknown(), key(name).as_ptr(), &mut value) };
        (result, value)
    }

    #[test]
    fn each_type_reads_back_what_was_written() {
        let list = OwnedAttributeList::new();
        let this = list.as_funknown();
        let v = vtbl(&list);
        let text = utf16("Piste é 🎹");
        let bytes = [1u8, 2, 3, 250];

        unsafe {
            assert_eq!(set_int(&list, "int", -42), K_RESULT_OK);
            assert_eq!(
                (v.set_float)(this, key("float").as_ptr(), 0.25),
                K_RESULT_OK
            );
            assert_eq!(
                (v.set_string)(this, key("text").as_ptr(), text.as_ptr()),
                K_RESULT_OK
            );
            assert_eq!(
                (v.set_binary)(
                    this,
                    key("bytes").as_ptr(),
                    bytes.as_ptr() as *const c_void,
                    bytes.len() as u32
                ),
                K_RESULT_OK
            );

            assert_eq!(get_int(&list, "int"), (K_RESULT_OK, -42));
            let mut float = 0.0;
            assert_eq!(
                (v.get_float)(this, key("float").as_ptr(), &mut float),
                K_RESULT_OK
            );
            assert_eq!(float, 0.25);
            let mut buffer = [0xFFFFu16; 32];
            assert_eq!(
                (v.get_string)(
                    this,
                    key("text").as_ptr(),
                    buffer.as_mut_ptr(),
                    size_of_val(&buffer) as u32
                ),
                K_RESULT_OK
            );
            assert_eq!(&buffer[..text.len()], &text[..]);
            let mut data: *const c_void = std::ptr::null();
            let mut size = 0;
            assert_eq!(
                (v.get_binary)(this, key("bytes").as_ptr(), &mut data, &mut size),
                K_RESULT_OK
            );
            assert_eq!(
                std::slice::from_raw_parts(data as *const u8, size as usize),
                &bytes
            );
        }
    }

    #[test]
    fn a_missing_key_or_another_type_answers_false() {
        let list = OwnedAttributeList::new();
        let this = list.as_funknown();
        let v = vtbl(&list);
        set_int(&list, "int", 7);

        let mut float = 1.5;
        let mut data: *const c_void = 1 as *const c_void;
        let mut size = 9;
        let mut buffer = [0u16; 4];
        unsafe {
            assert_eq!(get_int(&list, "absent").0, K_RESULT_FALSE);
            assert_eq!(
                (v.get_float)(this, key("int").as_ptr(), &mut float),
                K_RESULT_FALSE
            );
            assert_eq!(float, 1.5);
            assert_eq!(
                (v.get_string)(this, key("int").as_ptr(), buffer.as_mut_ptr(), 8),
                K_RESULT_FALSE
            );
            assert_eq!(
                (v.get_binary)(this, key("absent").as_ptr(), &mut data, &mut size),
                K_RESULT_FALSE
            );
        }
        assert_eq!((data, size), (std::ptr::null(), 0));
    }

    #[test]
    fn a_full_list_refuses_a_new_key_but_still_replaces() {
        let list = OwnedAttributeList::new();
        for index in 0..MAX_ENTRIES {
            assert_eq!(set_int(&list, &format!("key{index}"), 1), K_RESULT_OK);
        }
        assert_eq!(set_int(&list, "one too many", 1), K_OUT_OF_MEMORY);
        assert_eq!(get_int(&list, "one too many").0, K_RESULT_FALSE);

        assert_eq!(set_int(&list, "key0", 2), K_RESULT_OK);
        assert_eq!(get_int(&list, "key0"), (K_RESULT_OK, 2));
    }

    #[test]
    fn a_key_can_change_type() {
        let list = OwnedAttributeList::new();
        set_int(&list, "key", 3);
        unsafe {
            (vtbl(&list).set_float)(list.as_funknown(), key("key").as_ptr(), 0.5);
        }
        assert_eq!(list.value("key"), Some(Value::Float(0.5)));
        assert_eq!(get_int(&list, "key").0, K_RESULT_FALSE);
    }

    #[test]
    fn bad_arguments_are_refused() {
        let list = OwnedAttributeList::new();
        let this = list.as_funknown();
        let v = vtbl(&list);
        let long_key = "k".repeat(MAX_KEY_BYTES + 1);
        let long_text: Vec<u16> = std::iter::repeat(65u16)
            .take(MAX_TEXT_UNITS + 1)
            .chain(Some(0))
            .collect();
        let mut value = 0;
        unsafe {
            assert_eq!((v.set_int)(this, std::ptr::null(), 1), K_INVALID_ARGUMENT);
            assert_eq!((v.set_int)(this, key("").as_ptr(), 1), K_INVALID_ARGUMENT);
            assert_eq!(set_int(&list, &long_key, 1), K_INVALID_ARGUMENT);
            assert_eq!(set_int(&list, &"k".repeat(MAX_KEY_BYTES), 1), K_RESULT_OK);
            assert_eq!(
                (v.get_int)(this, key("a").as_ptr(), std::ptr::null_mut()),
                K_INVALID_ARGUMENT
            );
            assert_eq!(
                (v.get_int)(this, std::ptr::null(), &mut value),
                K_INVALID_ARGUMENT
            );
            assert_eq!(
                (v.set_string)(this, key("t").as_ptr(), std::ptr::null()),
                K_INVALID_ARGUMENT
            );
            assert_eq!(
                (v.set_string)(this, key("t").as_ptr(), long_text.as_ptr()),
                K_INVALID_ARGUMENT
            );
            assert_eq!(
                (v.set_binary)(this, key("b").as_ptr(), std::ptr::null(), 4),
                K_INVALID_ARGUMENT
            );
            let big = vec![0u8; MAX_BINARY_BYTES + 1];
            assert_eq!(
                (v.set_binary)(
                    this,
                    key("b").as_ptr(),
                    big.as_ptr() as *const c_void,
                    big.len() as u32
                ),
                K_INVALID_ARGUMENT
            );
        }
        assert_eq!(list.value("t"), None);
        assert_eq!(list.value("b"), None);
    }

    #[test]
    fn the_longest_string_is_kept_without_its_terminator() {
        let list = OwnedAttributeList::new();
        let text: Vec<u16> = std::iter::repeat(66u16)
            .take(MAX_TEXT_UNITS)
            .chain(Some(0))
            .collect();
        unsafe {
            assert_eq!(
                (vtbl(&list).set_string)(list.as_funknown(), key("t").as_ptr(), text.as_ptr()),
                K_RESULT_OK
            );
        }
        assert_eq!(list.value("t"), Some(Value::Text(vec![66; MAX_TEXT_UNITS])));
    }

    #[test]
    fn a_string_too_long_for_the_buffer_is_cut_and_terminated() {
        let list = OwnedAttributeList::new();
        let this = list.as_funknown();
        let v = vtbl(&list);
        let text = utf16("abcdef");
        let mut buffer = [0x7777u16; 6];
        unsafe {
            (v.set_string)(this, key("t").as_ptr(), text.as_ptr());
            assert_eq!(
                (v.get_string)(this, key("t").as_ptr(), buffer.as_mut_ptr(), 8),
                K_RESULT_OK
            );
            assert_eq!(buffer[..4], [97, 98, 99, 0]);
            assert_eq!(buffer[4..], [0x7777, 0x7777], "nothing is written past it");
            assert_eq!(
                (v.get_string)(this, key("t").as_ptr(), buffer.as_mut_ptr(), 1),
                K_RESULT_FALSE
            );
        }
    }

    #[test]
    fn an_empty_binary_is_kept() {
        let list = OwnedAttributeList::new();
        let this = list.as_funknown();
        let v = vtbl(&list);
        let mut data: *const c_void = std::ptr::null();
        let mut size = 5;
        unsafe {
            assert_eq!(
                (v.set_binary)(this, key("b").as_ptr(), std::ptr::null(), 0),
                K_RESULT_OK
            );
            assert_eq!(
                (v.get_binary)(this, key("b").as_ptr(), &mut data, &mut size),
                K_RESULT_OK
            );
        }
        assert_eq!(size, 0);
    }

    #[test]
    fn query_interface_adds_a_reference_and_refuses_other_interfaces() {
        let freed = Arc::new(AtomicBool::new(false));
        let list = OwnedAttributeList::watching(&freed);
        let this = list.as_funknown();
        let v = vtbl(&list);
        let mut found: *mut c_void = null_mut();
        let mut other: *mut c_void = 1 as *mut c_void;
        unsafe {
            assert_eq!(
                (v.query_interface)(this, &IID_IATTRIBUTE_LIST, &mut found),
                K_RESULT_OK
            );
            assert_eq!(found, this as *mut c_void);
            assert_eq!(
                (v.query_interface)(this, &IID_IMESSAGE, &mut other),
                K_NO_INTERFACE
            );
            assert!(other.is_null());
            assert_eq!(
                (v.query_interface)(this, &IID_IATTRIBUTE_LIST, null_mut()),
                K_INVALID_ARGUMENT
            );

            drop(list);
            assert!(
                !freed.load(Ordering::Acquire),
                "the queried reference keeps it"
            );
            assert_eq!((v.release)(found as *mut FUnknown), 0);
        }
        assert!(freed.load(Ordering::Acquire));
    }

    #[test]
    fn the_last_release_frees_the_list() {
        let freed = Arc::new(AtomicBool::new(false));
        let raw = OwnedAttributeList::watching(&freed).into_raw() as *mut FUnknown;
        let v = unsafe { &*(*(raw as *mut HostAttributeList)).vtbl };
        unsafe {
            assert_eq!((v.add_ref)(raw), 2);
            assert_eq!((v.release)(raw), 1);
            assert!(!freed.load(Ordering::Acquire));
            assert_eq!((v.release)(raw), 0);
        }
        assert!(freed.load(Ordering::Acquire));
    }
}
