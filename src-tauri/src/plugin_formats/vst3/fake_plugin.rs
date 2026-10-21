//! A hand-made VST3 plugin for tests that must not depend on an installed one.
//!
//! Its component, processor and controller answer only what the host does while
//! it holds and tears down the instance; any other method aborts the test run.
//! The controller delegates `queryInterface` to a [`FakeListener`], which records
//! what the host tells it through `IInfoListener`.

use std::cell::{Cell, RefCell};
use std::ffi::{c_void, CString};
use std::mem::{offset_of, size_of};
use std::ptr::{null_mut, NonNull};
use std::sync::Arc;

use super::channel_context::{CHANNEL_COLOR_KEY, CHANNEL_NAME_KEY, CHANNEL_NAME_LENGTH_KEY};
use super::component_handler::ComponentHandler;
use super::ffi::{
    FUnknown, IAttributeListVtbl, IAudioProcessor, IAudioProcessorVtbl, IComponent, IComponentVtbl,
    IEditController, IEditControllerVtbl, IInfoListenerVtbl, TResult, Tuid, Vst3Controller,
    Vst3Library, IID_FUNKNOWN, IID_IINFO_LISTENER, K_NO_INTERFACE, K_RESULT_OK,
};
use crate::plugin_system::instance::PluginInstance;

/// What a controller read from the list it was given.
#[derive(Debug, PartialEq)]
pub(crate) struct Received {
    pub name: Option<String>,
    pub name_length: Option<i64>,
    pub color: Option<i64>,
}

impl Received {
    unsafe fn read(list: *mut FUnknown) -> Self {
        Self {
            name: read_text(list, CHANNEL_NAME_KEY),
            name_length: read_int(list, CHANNEL_NAME_LENGTH_KEY),
            color: read_int(list, CHANNEL_COLOR_KEY),
        }
    }
}

unsafe fn read_int(list: *mut FUnknown, name: &str) -> Option<i64> {
    let vtbl = &*((*list).vtbl as *const IAttributeListVtbl);
    let key = CString::new(name).unwrap();
    let mut value = 0;
    ((vtbl.get_int)(list, key.as_ptr(), &mut value) == K_RESULT_OK).then_some(value)
}

unsafe fn read_text(list: *mut FUnknown, name: &str) -> Option<String> {
    let vtbl = &*((*list).vtbl as *const IAttributeListVtbl);
    let key = CString::new(name).unwrap();
    let mut buffer = [0u16; 512];
    let size = size_of::<[u16; 512]>() as u32;
    let result = (vtbl.get_string)(list, key.as_ptr(), buffer.as_mut_ptr(), size);
    (result == K_RESULT_OK).then(|| {
        let length = buffer.iter().position(|&unit| unit == 0).unwrap();
        String::from_utf16_lossy(&buffer[..length])
    })
}

/// The `IInfoListener` of a controller, or a controller that has none.
#[repr(C)]
pub(crate) struct FakeListener {
    vtbl: *const IInfoListenerVtbl,
    pub references: Cell<u32>,
    listens: bool,
    pub received: RefCell<Vec<Received>>,
}

impl FakeListener {
    pub fn new(listens: bool) -> Box<Self> {
        Box::new(Self {
            vtbl: &LISTENER_VTBL,
            references: Cell::new(1),
            listens,
            received: RefCell::new(Vec::new()),
        })
    }

    pub fn as_funknown(&mut self) -> *mut FUnknown {
        self as *mut Self as *mut FUnknown
    }
}

unsafe fn listener<'a>(this: *mut FUnknown) -> &'a FakeListener {
    &*(this as *const FakeListener)
}

unsafe extern "C" fn listener_query_interface(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    let fake = listener(this);
    if fake.listens && (*iid == IID_IINFO_LISTENER || *iid == IID_FUNKNOWN) {
        fake.references.set(fake.references.get() + 1);
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = null_mut();
    K_NO_INTERFACE
}

unsafe extern "C" fn listener_add_ref(this: *mut FUnknown) -> u32 {
    let fake = listener(this);
    fake.references.set(fake.references.get() + 1);
    fake.references.get()
}

unsafe extern "C" fn listener_release(this: *mut FUnknown) -> u32 {
    let fake = listener(this);
    fake.references.set(fake.references.get() - 1);
    fake.references.get()
}

unsafe extern "C" fn listener_set_channel_context_infos(
    this: *mut FUnknown,
    list: *mut FUnknown,
) -> TResult {
    listener(this)
        .received
        .borrow_mut()
        .push(Received::read(list));
    K_RESULT_OK
}

static LISTENER_VTBL: IInfoListenerVtbl = IInfoListenerVtbl {
    query_interface: listener_query_interface,
    add_ref: listener_add_ref,
    release: listener_release,
    set_channel_context_infos: listener_set_channel_context_infos,
};

#[repr(C)]
struct FakeController {
    vtbl: *const IEditControllerVtbl,
    listener: *mut FakeListener,
}

unsafe extern "C" fn controller_query_interface(
    this: *mut IEditController,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    let listener = (*(this as *const FakeController)).listener as *mut FUnknown;
    ((*(*listener).vtbl).query_interface)(listener, iid, obj)
}

unsafe extern "C" fn controller_add_ref(_: *mut IEditController) -> u32 {
    1
}

unsafe extern "C" fn controller_release(_: *mut IEditController) -> u32 {
    0
}

unsafe extern "C" fn controller_set_component_handler(
    _: *mut IEditController,
    _: *mut FUnknown,
) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn component_terminate(_: *mut IComponent) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn component_release(_: *mut IComponent) -> u32 {
    0
}

unsafe extern "C" fn processor_release(_: *mut IAudioProcessor) -> u32 {
    0
}

unsafe extern "C" fn unexpected_call() {
    eprintln!("the fake VST3 plugin was called through a method it does not implement");
    std::process::abort();
}

/// A vtable of `T` (all pointers) where every method aborts but the given ones,
/// as `(offset of the field, function address)`.  Lives for the rest of the run.
fn vtbl_of<T>(methods: &[(usize, *const ())]) -> *const T {
    let mut table =
        vec![unexpected_call as *const () as usize; size_of::<T>() / size_of::<usize>()];
    for &(offset, function) in methods {
        table[offset / size_of::<usize>()] = function as usize;
    }
    Box::leak(table.into_boxed_slice()).as_ptr().cast()
}

/// A single-controller VST3 instance whose controller answers `listener`.  Nothing
/// is activated; dropping the instance tears the fakes down as for a real plugin.
pub(crate) fn fake_vst3_instance(listener: &mut FakeListener) -> Arc<PluginInstance> {
    let controller = Box::leak(Box::new(FakeController {
        vtbl: vtbl_of(&[
            (
                offset_of!(IEditControllerVtbl, query_interface),
                controller_query_interface as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, add_ref),
                controller_add_ref as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, release),
                controller_release as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, set_component_handler),
                controller_set_component_handler as *const (),
            ),
        ]),
        listener,
    }));
    let component = Box::leak(Box::new(IComponent {
        vtbl: vtbl_of::<IComponentVtbl>(&[
            (
                offset_of!(IComponentVtbl, terminate),
                component_terminate as *const (),
            ),
            (
                offset_of!(IComponentVtbl, release),
                component_release as *const (),
            ),
        ]),
    }));
    let processor = Box::leak(Box::new(IAudioProcessor {
        vtbl: vtbl_of::<IAudioProcessorVtbl>(&[(
            offset_of!(IAudioProcessorVtbl, release),
            processor_release as *const (),
        )]),
    }));
    let controller = NonNull::from(controller);
    PluginInstance::new_vst3(
        "fake://vst3",
        NonNull::from(component),
        NonNull::from(processor),
        Some(Vst3Controller::Shared(controller.cast())),
        ComponentHandler::boxed(),
        Vst3Library::placeholder(),
    )
}
