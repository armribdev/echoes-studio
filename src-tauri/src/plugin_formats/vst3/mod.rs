//! VST3 plugin format support.
//!
//! Status: **interface defined, FFI pending**.
//!
//! The module structure is complete and correct.  `VstWrapperNode` passes
//! audio through and implements all required traits.  Wiring in the actual
//! VST3 COM calls requires adding the `vst3-sys` crate and implementing
//! `ffi.rs` for each target platform.

pub mod ffi;
pub mod host;
pub mod wrapper;

pub use host::Vst3Host;
pub use wrapper::VstWrapperNode;
