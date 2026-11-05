//! Helpers shared by the crashing plugin examples.
//!
//! Each example is a DLL that dies or hangs the way a broken plugin does while the host scans it.
//! Before dying it appends a line to the file named by `ECHOES_TEST_CRASH_LOG`, so a test
//! can count how many times a scan loaded it.

pub const CRASH_LOG_VARIABLE: &str = "ECHOES_TEST_CRASH_LOG";

pub fn note_load(what: &str) {
    use std::io::Write;
    let Some(path) = std::env::var_os(CRASH_LOG_VARIABLE) else {
        return;
    };
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(log, "{what}");
    }
}

pub fn access_violation() -> ! {
    // SAFETY: writing through a null pointer is the crash this crate exists to cause.
    unsafe { std::ptr::null_mut::<u32>().write_volatile(1) };
    unreachable!()
}
