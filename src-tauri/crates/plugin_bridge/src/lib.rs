//! Shared transport core for the out-of-process plugin sandbox (ADR-015).
//!
//! ## Why this is a separate crate
//!
//! The Phase 6 sandbox hosts an untrusted plugin in a **separate process**
//! (`plugin_host_bridge`) and exchanges audio / parameters / events with it over
//! **shared-memory ring buffers**.  Both sides — the engine (`echoes-studio`) and
//! the bridge host binary — must agree on the *exact* ring layout and message
//! ABI.  A binary cannot reuse another binary's private modules, so the only way
//! to keep **one** source of truth for that ABI is a library crate both depend
//! on.  This is it.
//!
//! On the engine side those rings are drained and filled on the **audio
//! thread**, so the transport is real-time safe by construction: no allocation,
//! no lock, fixed-size copies (see [`shmem`]).
//!
//! ## Contents
//!
//! - [`shmem`]    — [`shmem::ShmemRing`], a region-agnostic SPSC ring living
//!   inside a raw byte region (OS shared memory in production, an aligned heap
//!   backing in tests).
//! - [`protocol`] — the `#[repr(C)]` POD message types that pin the cross-process
//!   ABI carried over those rings.
//!
//! ## Status (ADR-015 step 6c-ii)
//!
//! Extracted from the engine's `plugin_system::bridge` module unchanged; the
//! engine now re-exports this crate under that path.  Nothing crosses the
//! real-time / process boundary yet — that is step 6d.

pub mod protocol;
pub mod shmem;
