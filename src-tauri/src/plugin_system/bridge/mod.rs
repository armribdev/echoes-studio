//! Out-of-process plugin bridge — shared transport primitives (ADR-015, Phase 6c-i).
//!
//! ## Scope of this module
//!
//! The Phase 6 sandbox (ADR-015) hosts an untrusted plugin in a **separate
//! process** and exchanges audio / parameters / events with it over
//! **shared-memory ring buffers**.  On the engine side those rings are drained
//! and filled on the **audio thread**, so the transport must be real-time safe
//! by construction (no allocation, no lock, fixed-size copies).
//!
//! This module provides that transport **core**, built and unit-tested entirely
//! **off** the audio thread (step 6c-i):
//!
//! - [`shmem`]    — [`shmem::ShmemRing`], a region-agnostic SPSC ring that lives
//!   inside a raw byte region (an OS shared-memory mapping in production, an
//!   aligned heap allocation in tests).
//! - [`protocol`] — the `#[repr(C)]` POD message types that pin the cross-process
//!   ABI carried over those rings.
//!
//! ## What this module is *not* (yet)
//!
//! Nothing here is wired into the engine.  There is no separate process, no
//! `instantiate` branch, and no audio-thread call site.  Those land in later
//! sub-steps:
//!   * 6c-ii — the `plugin_host_bridge` binary crate (and the workspace decision
//!     it requires).
//!   * 6d    — `BridgedPluginInstance` + the real-time wiring (bounded spin +
//!     timeout + silence) that finally crosses the RT boundary.
//!
//! Because no runtime path references these items in 6c-i, the module allows
//! dead code; every item becomes a consumer's dependency in 6d.

#![allow(dead_code)] // 6c-i scaffolding — consumed by BridgedPluginInstance in step 6d.

pub mod protocol;
pub mod shmem;
