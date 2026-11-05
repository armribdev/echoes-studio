//! A VST3 module whose `GetPluginFactory` never returns.

use std::ffi::c_void;
use std::time::Duration;

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
    test_crashing_plugin::note_load("hang_in_factory");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
