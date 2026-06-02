//! Out-of-process plugin bridge — engine-side re-export (ADR-015, Phase 6c-ii).
//!
//! The shared-memory transport core (the SPSC ring and the wire protocol) was
//! extracted into the standalone [`plugin_bridge`] crate so the bridge host
//! binary (`plugin_host_bridge`) and the engine link **one** definition of the
//! cross-process ABI.  This module re-exports it so existing in-engine paths
//! (`crate::plugin_system::bridge::shmem::ShmemRing`, `…::protocol::…`) keep
//! resolving when step 6d wires `BridgedPluginInstance` against them.
//!
//! Nothing in the engine consumes these yet — the re-export is dead until 6d.
#![allow(unused_imports)]

pub use plugin_bridge::{protocol, region, session, shmem};
