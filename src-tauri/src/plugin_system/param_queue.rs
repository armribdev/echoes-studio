//! Lock-free SPSC parameter queue — engine-side re-export (ADR-033, step 1).
//!
//! The queue (ADR-007 / ADR-008) lives in the standalone [`plugin_param_queue`]
//! crate so the engine and the bridge host share one implementation. This module
//! re-exports it, so `crate::plugin_system::param_queue::…` keeps resolving.
pub use plugin_param_queue::*;
