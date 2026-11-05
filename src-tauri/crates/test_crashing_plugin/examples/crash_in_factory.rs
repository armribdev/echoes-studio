//! A VST3 module that loads but crashes in `GetPluginFactory`.

use std::ffi::c_void;

#[no_mangle]
pub extern "system" fn InitDll() -> bool {
    true
}

#[no_mangle]
pub extern "system" fn ExitDll() -> bool {
    true
}

#[no_mangle]
pub extern "system" fn GetPluginFactory() -> *mut c_void {
    test_crashing_plugin::note_load("crash_in_factory");
    test_crashing_plugin::access_violation()
}
