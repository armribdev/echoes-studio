//! VST3 COM bindings — engine-side re-export (ADR-033, step 1).
//!
//! The bindings live in the standalone [`vst3_ffi`] crate so the engine and the
//! out-of-process bridge host link one definition of the VST3 ABI. This module
//! re-exports them, so every in-engine path (`crate::plugin_formats::vst3::ffi::…`)
//! keeps resolving unchanged.
pub use vst3_ffi::*;
