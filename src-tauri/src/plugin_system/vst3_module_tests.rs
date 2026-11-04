//! How the host loads a VST3 module, with the real test DLL (ADR-033, step 2):
//! the procedure of `base/doc.h` ("How the host will load a plug-in") — `InitDll`
//! after `LoadLibrary`, `ExitDll` before `FreeLibrary` — and which COM apartment the
//! plugin finds on the thread that loads it.

use std::ffi::{c_void, CStr};
use std::path::Path;
use std::ptr::null;

use test_vst3_plugin::{DIAGNOSTIC_RANGE, EFFECT_CLASS_ID, INIT_APARTMENT};
use windows_sys::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::plugin_formats::vst3::ffi::{
    instantiate_processor, IEditController, IPluginFactory, Vst3Library, IID_IEDIT_CONTROLLER,
};
use crate::plugin_formats::vst3::test_dll::copy_of_test_vst3_dll;

#[link(name = "kernel32")]
extern "system" {
    fn FreeLibrary(module: *mut c_void) -> i32;
}

/// An extra reference to a module: it stays mapped after the host unloads it, so a
/// test can still read the counters the plugin kept.
struct HeldModule(*mut c_void);

impl HeldModule {
    fn new(path: &Path) -> Self {
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
        assert!(!handle.is_null(), "LoadLibraryW failed");
        Self(handle)
    }

    fn export<T>(&self, name: &CStr) -> unsafe extern "C" fn() -> T {
        let address = unsafe { GetProcAddress(self.0, name.as_ptr().cast()) }
            .unwrap_or_else(|| panic!("{name:?} is not exported"));
        // SAFETY: the test plugin exports these as `extern "C" fn() -> T`.
        unsafe { std::mem::transmute(address) }
    }

    fn calls(&self) -> (u32, u32) {
        let init = self.export::<u32>(c"test_vst3_init_calls");
        let exit = self.export::<u32>(c"test_vst3_exit_calls");
        unsafe { (init(), exit()) }
    }
}

impl Drop for HeldModule {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.0) };
    }
}

#[test]
fn the_loader_runs_init_dll_after_loading_and_exit_dll_before_unloading() {
    let copy = copy_of_test_vst3_dll("loader");
    let held = HeldModule::new(&copy);
    assert_eq!(held.calls(), (0, 0));

    let (library, factory) = Vst3Library::load(&copy).expect("the test plugin must load");
    assert_eq!(held.calls(), (1, 0), "InitDll runs once, ExitDll not yet");

    unsafe { ((*(*factory).vtbl).release)(factory as *mut _) };
    drop(library);
    assert_eq!(
        held.calls(),
        (1, 1),
        "ExitDll runs when the library is dropped"
    );
}

#[test]
fn two_users_of_one_module_share_one_init_dll_and_one_exit_dll() {
    let copy = copy_of_test_vst3_dll("shared");
    let held = HeldModule::new(&copy);
    let release = |factory: *mut IPluginFactory| unsafe {
        ((*(*factory).vtbl).release)(factory as *mut _);
    };

    let (first, first_factory) = Vst3Library::load(&copy).expect("the test plugin must load");
    let (second, second_factory) = Vst3Library::load(&copy).expect("the test plugin must load");
    release(first_factory);
    release(second_factory);
    assert_eq!(
        held.calls(),
        (1, 0),
        "the second user does not initialise the module again"
    );

    drop(first);
    assert_eq!(
        held.calls(),
        (1, 0),
        "the module stays initialised while a user is left"
    );
    drop(second);
    assert_eq!(held.calls(), (1, 1));

    let (third, third_factory) = Vst3Library::load(&copy).expect("the test plugin must load");
    release(third_factory);
    assert_eq!(
        held.calls(),
        (2, 1),
        "a load after the last user starts the module again"
    );
    drop(third);
    assert_eq!(held.calls(), (2, 2));
}

#[test]
fn threads_loading_and_dropping_one_module_keep_init_dll_and_exit_dll_paired() {
    const THREADS: usize = 8;
    const LOADS_PER_THREAD: usize = 50;
    let copy = copy_of_test_vst3_dll("threads");
    let held = HeldModule::new(&copy);

    let workers: Vec<_> = (0..THREADS)
        .map(|_| {
            let copy = copy.clone();
            std::thread::spawn(move || {
                for _ in 0..LOADS_PER_THREAD {
                    let (library, factory) = Vst3Library::load(&copy).expect("must load");
                    let (component, processor) = unsafe {
                        instantiate_processor(factory, &EFFECT_CLASS_ID, std::ptr::null_mut())
                    }
                    .expect("must instantiate");
                    unsafe {
                        ((*(*processor).vtbl).release)(processor);
                        ((*(*component).vtbl).release)(component);
                        ((*(*factory).vtbl).release)(factory as *mut _);
                    }
                    drop(library);
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("a loading thread panicked");
    }

    let (init, exit) = held.calls();
    assert_eq!(
        init, exit,
        "every InitDll has its ExitDll once nobody is left"
    );
    assert!(init >= 1);
}

/// `(apartment InitDll found, apartment initialize found)` when the module is loaded
/// from a thread that first calls `CoInitializeEx(mode)`, or from a bare thread.
/// 0 = COM not initialised on that thread, else 1 + `APTTYPE` (1 single-threaded
/// apartment, 2 multi-threaded, 4 main single-threaded).
fn apartments_seen_by_a_plugin_loaded_from(mode: Option<i32>, tag: &'static str) -> (i32, i32) {
    std::thread::spawn(move || {
        let copy = copy_of_test_vst3_dll(tag);
        if let Some(mode) = mode {
            assert!(unsafe { CoInitializeEx(null(), mode as u32) } >= 0);
        }
        let held = HeldModule::new(&copy);
        let (library, factory) = Vst3Library::load(&copy).expect("the test plugin must load");
        let (component, processor) =
            unsafe { instantiate_processor(factory, &EFFECT_CLASS_ID, std::ptr::null_mut()) }
                .expect("the effect must instantiate");

        let mut controller: *mut c_void = std::ptr::null_mut();
        unsafe {
            ((*(*component).vtbl).query_interface)(
                component,
                &IID_IEDIT_CONTROLLER,
                &mut controller,
            )
        };
        let controller = controller as *mut IEditController;
        let seen_by_initialize =
            unsafe { ((*(*controller).vtbl).get_param_normalized)(controller, INIT_APARTMENT) }
                * DIAGNOSTIC_RANGE;
        let seen_by_init_dll = unsafe { held.export::<i32>(c"test_vst3_init_apartment")() };

        unsafe {
            ((*(*controller).vtbl).release)(controller);
            ((*(*processor).vtbl).release)(processor);
            ((*(*component).vtbl).release)(component);
            ((*(*factory).vtbl).release)(factory as *mut _);
        }
        drop(library);
        if mode.is_some() {
            unsafe { CoUninitialize() };
        }
        (seen_by_init_dll, seen_by_initialize as i32)
    })
    .join()
    .expect("the loading thread must not panic")
}

#[test]
fn the_plugin_finds_the_com_apartment_of_the_thread_that_loads_it() {
    assert_eq!(
        apartments_seen_by_a_plugin_loaded_from(None, "com_none"),
        (0, 0)
    );
    // The first single-threaded apartment of a process is its "main" one (APTTYPE 3).
    let single_threaded =
        apartments_seen_by_a_plugin_loaded_from(Some(COINIT_APARTMENTTHREADED), "com_sta");
    assert!(
        [(1, 1), (4, 4)].contains(&single_threaded),
        "single-threaded apartment: {single_threaded:?}"
    );
    assert_eq!(
        apartments_seen_by_a_plugin_loaded_from(Some(COINIT_MULTITHREADED), "com_mta"),
        (2, 2)
    );
}
