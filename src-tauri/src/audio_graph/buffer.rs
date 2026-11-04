//! Planar audio buffer — engine-side re-export (ADR-033, step 3).
//!
//! The buffer lives in the standalone [`audio_buffer`] crate so the engine and the
//! plugin bridge host (VST3 buses) share one type. This module re-exports it, so
//! `crate::audio_graph::buffer::AudioBuffer` keeps resolving.
pub use audio_buffer::*;
