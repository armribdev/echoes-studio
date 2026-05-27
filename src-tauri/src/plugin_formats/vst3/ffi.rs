//! VST3 FFI declarations.
//!
//! This module will contain the raw C ABI bindings for the VST3 SDK.
//!
//! ## Implementation strategy
//!
//! Use the `vst3-sys` or `vst3` crate once the feature flag is enabled, or
//! hand-write the minimal subset of COM interfaces needed:
//!
//! - `IPluginBase` (initialize / terminate)
//! - `IComponent`  (bus arrangement, state I/O)
//! - `IAudioProcessor` (setup processing, process)
//! - `IEditController` (parameter info, normalised value)
//! - `IPluginFactory` (from the .vst3 binary)
//!
//! ## Platform notes
//!
//! - **Windows**: VST3 binaries are `.vst3` directories containing a `.dll`.
//!   Use `LoadLibraryW` + `GetProcAddress("GetPluginFactory")`.
//! - **macOS**: Bundles at `~/Library/Audio/Plug-Ins/VST3/*.vst3`.
//!   Use `dlopen` + `dlsym`.
//! - **Linux**: `~/.vst3/*.vst3` bundles, similar `dlopen` path.
//!
//! ## Safety contract
//!
//! All VST3 COM calls are inherently unsafe.  They must never be made from
//! the audio thread except through `IAudioProcessor::process`, which must
//! only be called while the scheduler holds the exclusive graph lock.

// Future: #[cfg(feature = "vst3")]
// extern crate vst3_sys;

/// Placeholder: VST3 FFI not yet implemented.
///
/// When implementing, add `vst3-sys` to `[dependencies]` in `Cargo.toml`
/// and replace this module with the COM interface bindings.
pub struct Vst3Ffi;

impl Vst3Ffi {
    pub const UNIMPLEMENTED: &'static str = "VST3 FFI — pending platform implementation";
}
