//! A module that aborts the process in `DllMain`, before any entry point is called.
//!
//! An access violation there would not do: the Windows loader catches it and the load
//! merely fails. An abort (a failed assertion in a static initialiser, say) cannot be
//! caught.

use std::ffi::c_void;

const DLL_PROCESS_ATTACH: u32 = 1;

#[no_mangle]
pub extern "system" fn DllMain(_module: *mut c_void, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        test_crashing_plugin::note_load("crash_on_load");
        std::process::abort();
    }
    1
}
