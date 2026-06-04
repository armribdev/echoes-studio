/*
 * Minimal SEH wrapper for plugin process() calls.
 *
 * On Windows, C++ structured exceptions (access violations, stack overflows,
 * illegal instructions) are NOT caught by Rust's catch_unwind.  This shim
 * wraps the FFI call in __try/__except so a misbehaving plugin cannot crash
 * the host process.  The caller receives -1 as a sentinel and can mute the
 * plugin permanently.
 *
 * On non-Windows the function forwards directly; Rust's catch_unwind in the
 * scheduler handles Rust panics on those platforms.
 */

#include <stdint.h>

typedef int32_t (*crash_guard_fn)(void *ctx);

#ifdef _WIN32
#include <windows.h>

int32_t crash_guard_call(crash_guard_fn fn, void *ctx) {
    __try {
        return fn(ctx);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return -1;
    }
}
#else

int32_t crash_guard_call(crash_guard_fn fn, void *ctx) {
    return fn(ctx);
}
#endif
