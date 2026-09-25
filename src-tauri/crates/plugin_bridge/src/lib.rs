//! Shared transport core for the out-of-process plugin sandbox (ADR-015).
//!
//! The sandbox hosts an untrusted plugin in a separate process
//! (`plugin_host_bridge`).  Both the engine and that process link this crate so
//! they share one definition of the cross-process ABI.
//!
//! - [`shmem`]    — SPSC ring living inside a raw byte region.
//! - [`region`]   — named OS shared memory + the wake event (Windows first).
//! - [`protocol`] — the per-block message layout.
//! - [`control`]  — off-real-time request/response frames over the child's pipes.
//! - [`session`]  — spawn/handshake, the engine's real-time [`session::AudioPort`]
//!   and the bridge's audio loop.
//!
//! The engine drives [`session::AudioPort`] on its audio thread: no allocation,
//! no lock, a bounded wait.  Everything else runs off the audio thread.

pub mod control;
pub mod protocol;
pub mod region;
pub mod session;
pub mod shmem;
