//! Flush-to-Zero / Denormals-Are-Zero control for the audio thread.
//!
//! Sub-normal IEEE-754 floats (|x| < ~1.18e-38) trigger a microcode slow path
//! on x86: arithmetic that touches denormals can run 10×–100× slower than
//! normal-range math. In feedback DSP — reverb tails, delay feedback, RC
//! envelopes — long silences let the state value decay smoothly through that
//! sub-normal range, which turns an idle channel into an XRun-causing CPU
//! spike.
//!
//! The portable mitigation is to flip two bits in the FPU control register of
//! the audio thread so the hardware silently rounds sub-normals to zero on both
//! read (DAZ) and write (FTZ). These flags are **thread-local**, so we must
//! call this from inside the audio callback itself.
//!
//! ## Per-architecture notes
//!
//! - **x86 / x86_64**: SSE is mandatory on x86_64. We set bits 15 (FTZ) and 6
//!   (DAZ) of MXCSR via `_MM_SET_*_MODE`.
//! - **aarch64**: We set bit 24 (FZ) of FPCR via `mrs`/`msr`. FZ16 (bit 19) is
//!   left alone — we don't process f16.
//! - **Other targets**: no-op. The DC-bias trick inside individual feedback
//!   nodes (e.g. reverb comb filter) remains the fallback.

/// Enable FTZ/DAZ on the current thread. Idempotent; safe to call from every
/// audio callback invocation, but designed to be called once.
///
/// MXCSR bits flipped: bit 15 (FTZ — outputs flushed to zero), bit 6 (DAZ —
/// inputs treated as zero). Both predate this codebase's minimum-CPU target by
/// 20 years (Pentium 4 / Athlon 64). `_mm_setcsr` is deprecated in std as of
/// 1.75 but remains the supported way to set these flags without dropping into
/// hand-written `stmxcsr`/`ldmxcsr`; the deprecation only flags that callers
/// should not casually mutate other MXCSR bits.
#[inline]
pub fn enable_flush_to_zero() {
    #[cfg(target_arch = "x86_64")]
    {
        #[allow(deprecated)]
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        const FTZ_DAZ: u32 = (1 << 15) | (1 << 6);
        #[allow(deprecated)]
        unsafe {
            _mm_setcsr(_mm_getcsr() | FTZ_DAZ);
        }
    }

    #[cfg(all(target_arch = "x86", target_feature = "sse"))]
    {
        #[allow(deprecated)]
        use std::arch::x86::{_mm_getcsr, _mm_setcsr};
        const FTZ_DAZ: u32 = (1 << 15) | (1 << 6);
        #[allow(deprecated)]
        unsafe {
            _mm_setcsr(_mm_getcsr() | FTZ_DAZ);
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            let mut fpcr: u64;
            std::arch::asm!("mrs {}, fpcr", out(reg) fpcr, options(nomem, nostack));
            fpcr |= 1 << 24; // FZ
            std::arch::asm!("msr fpcr, {}", in(reg) fpcr, options(nomem, nostack));
        }
    }
}

/// Tiny DC bias added inside feedback paths so the recursive state cannot
/// converge into the sub-normal range on architectures where FTZ/DAZ is not
/// available or not yet activated. The value is well below audible threshold
/// (≈ −500 dBFS) but large enough to dominate denormals (which start at
/// ≈ 1.18e-38).
pub const ANTI_DENORMAL: f32 = 1.0e-25;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::assertions_on_constants)] // regression guard on the literal value itself
    fn anti_denormal_bias_is_inaudible_but_above_the_denormal_floor() {
        // Must dominate the largest sub-normal (~1.18e-38) while staying far
        // below anything a listener could perceive.
        assert!(ANTI_DENORMAL > 1.0e-38);
        assert!(ANTI_DENORMAL < 1.0e-10);
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn enable_flush_to_zero_sets_ftz_and_daz_bits() {
        #[allow(deprecated)]
        use std::arch::x86_64::_mm_getcsr;

        enable_flush_to_zero();

        #[allow(deprecated)]
        let mxcsr = unsafe { _mm_getcsr() };
        const FTZ: u32 = 1 << 15;
        const DAZ: u32 = 1 << 6;
        assert_eq!(mxcsr & FTZ, FTZ, "FTZ bit not set");
        assert_eq!(mxcsr & DAZ, DAZ, "DAZ bit not set");
    }

    #[test]
    fn enable_flush_to_zero_is_idempotent() {
        // Calling it repeatedly (as every audio callback does) must not panic
        // or toggle any bit back off.
        enable_flush_to_zero();
        enable_flush_to_zero();
        enable_flush_to_zero();
    }
}
