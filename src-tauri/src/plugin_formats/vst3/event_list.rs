//! Host-side `IEventList`: the notes handed to a VST3 instrument each block.
//!
//! Allocated once in `prepare`; the audio thread only resets it and pushes into
//! the fixed array, so nothing is allocated or freed per block.

use std::ffi::c_void;

use super::ffi::{
    Event, TResult, Tuid, IID_FUNKNOWN, IID_IEVENT_LIST, K_INVALID_ARGUMENT, K_NO_INTERFACE,
    K_RESULT_FALSE, K_RESULT_OK,
};

/// Events one block can carry.  `MidiEventList` holds 256, and the host may add
/// the releases it kept from a skipped block.
pub const MAX_EVENTS: usize = 512;

#[repr(C)]
pub struct IEventListVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut HostEventList, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut HostEventList) -> u32,
    pub release: unsafe extern "C" fn(*mut HostEventList) -> u32,
    pub get_event_count: unsafe extern "C" fn(*mut HostEventList) -> i32,
    pub get_event: unsafe extern "C" fn(*mut HostEventList, i32, *mut Event) -> TResult,
    pub add_event: unsafe extern "C" fn(*mut HostEventList, *mut Event) -> TResult,
}

/// `#[repr(C)]` keeps the vtable pointer at offset 0 so the plugin can dispatch
/// through it.
#[repr(C)]
pub struct HostEventList {
    vtbl: *const IEventListVtbl,
    count: usize,
    events: [Event; MAX_EVENTS],
}

unsafe extern "C" fn query_interface(
    this: *mut HostEventList,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() {
        return K_INVALID_ARGUMENT;
    }
    if !iid.is_null() && (*iid == IID_IEVENT_LIST || *iid == IID_FUNKNOWN) {
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = std::ptr::null_mut();
    K_NO_INTERFACE
}

unsafe extern "C" fn add_ref(_this: *mut HostEventList) -> u32 {
    1
}

unsafe extern "C" fn release(_this: *mut HostEventList) -> u32 {
    1
}

unsafe extern "C" fn get_event_count(this: *mut HostEventList) -> i32 {
    (*this).count as i32
}

unsafe extern "C" fn get_event(this: *mut HostEventList, index: i32, event: *mut Event) -> TResult {
    let list = &*this;
    if event.is_null() || index < 0 || index as usize >= list.count {
        return K_RESULT_FALSE;
    }
    *event = list.events[index as usize];
    K_RESULT_OK
}

unsafe extern "C" fn add_event(this: *mut HostEventList, event: *mut Event) -> TResult {
    if event.is_null() || !(*this).push(*event) {
        return K_RESULT_FALSE;
    }
    K_RESULT_OK
}

static EVENT_LIST_VTBL: IEventListVtbl = IEventListVtbl {
    query_interface,
    add_ref,
    release,
    get_event_count,
    get_event,
    add_event,
};

impl HostEventList {
    /// Heap-allocated, so the pointer stays valid while the plugin holds it.
    /// Call once at `prepare` — never on the audio thread.
    pub fn boxed() -> Box<Self> {
        Box::new(HostEventList {
            vtbl: &EVENT_LIST_VTBL,
            count: 0,
            events: [Event::EMPTY; MAX_EVENTS],
        })
    }

    pub fn reset(&mut self) {
        self.count = 0;
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.count
    }

    #[cfg(test)]
    pub fn as_slice(&self) -> &[Event] {
        &self.events[..self.count]
    }

    /// `false` when the list is full: the event is dropped.
    pub fn push(&mut self, event: Event) -> bool {
        if self.count >= MAX_EVENTS {
            return false;
        }
        self.events[self.count] = event;
        self.count += 1;
        true
    }

    pub fn as_ffi_ptr(&mut self) -> *mut c_void {
        self as *mut HostEventList as *mut c_void
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_formats::vst3::ffi::{K_NOTE_OFF_EVENT, K_NOTE_ON_EVENT};

    /// Calls the list the way a plugin does: through its vtable.
    fn vtbl(list: &mut HostEventList) -> &'static IEventListVtbl {
        unsafe { &*list.vtbl }
    }

    #[test]
    fn the_list_answers_the_sdk_codes_to_a_query() {
        let mut list = HostEventList::boxed();
        let this: *mut HostEventList = &mut *list;
        let table = vtbl(&mut list);
        let mut found: *mut c_void = std::ptr::null_mut();
        let unknown = [7u8; 16];

        assert_eq!(
            unsafe { (table.query_interface)(this, &IID_IEVENT_LIST, &mut found) },
            K_RESULT_OK
        );
        assert_eq!(found, this as *mut c_void);
        assert_eq!(
            unsafe { (table.query_interface)(this, &unknown, &mut found) },
            K_NO_INTERFACE
        );
        assert!(found.is_null());
        assert_eq!(
            unsafe { (table.query_interface)(this, &IID_IEVENT_LIST, std::ptr::null_mut()) },
            K_INVALID_ARGUMENT
        );
    }

    #[test]
    fn a_plugin_reads_back_the_events_in_order() {
        let mut list = HostEventList::boxed();
        list.push(Event::note_on(10, 60, 0.5, -1));
        list.push(Event::note_off(200, 60, -1));
        let this: *mut HostEventList = &mut *list;
        let table = vtbl(&mut list);

        assert_eq!(unsafe { (table.get_event_count)(this) }, 2);
        let mut event = Event::EMPTY;
        assert_eq!(
            unsafe { (table.get_event)(this, 0, &mut event) },
            K_RESULT_OK
        );
        assert_eq!((event.kind, event.sample_offset), (K_NOTE_ON_EVENT, 10));
        let note_on = unsafe { event.payload.note_on };
        assert_eq!(
            (note_on.pitch, note_on.velocity, note_on.note_id),
            (60, 0.5, -1)
        );
        assert_eq!(
            unsafe { (table.get_event)(this, 1, &mut event) },
            K_RESULT_OK
        );
        assert_eq!((event.kind, event.sample_offset), (K_NOTE_OFF_EVENT, 200));
    }

    #[test]
    fn an_index_out_of_range_is_refused() {
        let mut list = HostEventList::boxed();
        list.push(Event::note_on(0, 60, 1.0, -1));
        let this: *mut HostEventList = &mut *list;
        let table = vtbl(&mut list);
        let mut event = Event::EMPTY;

        assert_eq!(
            unsafe { (table.get_event)(this, 1, &mut event) },
            K_RESULT_FALSE
        );
        assert_eq!(
            unsafe { (table.get_event)(this, -1, &mut event) },
            K_RESULT_FALSE
        );
        assert_eq!(
            unsafe { (table.get_event)(this, 0, std::ptr::null_mut()) },
            K_RESULT_FALSE
        );
    }

    #[test]
    fn add_event_stops_when_the_list_is_full() {
        let mut list = HostEventList::boxed();
        let this: *mut HostEventList = &mut *list;
        let table = vtbl(&mut list);
        let mut event = Event::note_on(0, 60, 1.0, -1);

        for _ in 0..MAX_EVENTS {
            assert_eq!(unsafe { (table.add_event)(this, &mut event) }, K_RESULT_OK);
        }
        assert_eq!(
            unsafe { (table.add_event)(this, &mut event) },
            K_RESULT_FALSE
        );
        assert_eq!(list.len(), MAX_EVENTS);
    }

    #[test]
    fn reset_empties_the_list_for_the_next_block() {
        let mut list = HostEventList::boxed();
        list.push(Event::note_on(0, 60, 1.0, -1));
        list.reset();
        assert_eq!(list.len(), 0);
        assert!(list.as_slice().is_empty());
    }

    #[test]
    fn it_answers_for_ieventlist_and_funknown_only() {
        let mut list = HostEventList::boxed();
        let this: *mut HostEventList = &mut *list;
        let table = vtbl(&mut list);
        let mut out: *mut c_void = std::ptr::null_mut();

        assert_eq!(
            unsafe { (table.query_interface)(this, &IID_IEVENT_LIST, &mut out) },
            K_RESULT_OK
        );
        assert_eq!(out, this as *mut c_void);
        assert_eq!(
            unsafe { (table.query_interface)(this, &IID_FUNKNOWN, &mut out) },
            K_RESULT_OK
        );
        let other: Tuid = [7; 16];
        assert_ne!(
            unsafe { (table.query_interface)(this, &other, &mut out) },
            K_RESULT_OK
        );
        assert!(out.is_null());
    }
}
