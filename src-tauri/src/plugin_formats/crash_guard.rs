//! Thin Rust binding over the `crash_guard.c` SEH shim.
//!
//! On Windows, `guarded()` routes the FFI call through a C `__try/__except`
//! block.  Any SEH exception (access violation, stack overflow, illegal
//! instruction) is caught and converted to the `CAUGHT` sentinel (-1) instead
//! of aborting the process.
//!
//! On non-Windows, the C file provides a direct-call stub — zero overhead.
//!
//! # RT contract
//! No heap allocation.  The caller passes a raw pointer to a stack-allocated
//! context struct; the trampoline unpacks it and performs the real FFI call.

use std::ffi::c_void;

extern "C" {
    /// Calls `fn(ctx)` inside a Windows SEH `__try/__except`.
    /// Returns `fn`'s return value, or `CAUGHT` if an exception was raised.
    pub fn crash_guard_call(
        f: unsafe extern "C" fn(*mut c_void) -> i32,
        ctx: *mut c_void,
    ) -> i32;
}

/// Returned by the C shim when an SEH exception is caught.
pub const CAUGHT: i32 = -1;

/// Call `f(ctx)` through the SEH guard.
///
/// # Safety
/// `ctx` must remain valid for the lifetime of the call.
#[inline]
pub unsafe fn guarded(f: unsafe extern "C" fn(*mut c_void) -> i32, ctx: *mut c_void) -> i32 {
    crash_guard_call(f, ctx)
}
