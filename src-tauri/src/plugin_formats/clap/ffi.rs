//! CLAP C-ABI bindings — engine-side re-export (ADR-015, Phase 6d-iii-b-2).
//!
//! The bindings themselves were extracted **verbatim** into the standalone
//! [`clap_ffi`] crate so the engine and the out-of-process bridge host
//! (`plugin_host_bridge`) link **one** definition of the CLAP ABI.  This module
//! re-exports them, so every existing in-engine path
//! (`crate::plugin_formats::clap::ffi::ClapPlugin`, `…::ClapLibrary`, …) keeps
//! resolving unchanged — the move is invisible to the rest of the engine.
pub use clap_ffi::*;
