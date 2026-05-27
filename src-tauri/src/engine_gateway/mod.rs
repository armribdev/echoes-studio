//! Engine Gateway — receives typed commands from the UI, validates them,
//! appends project events to the event store, updates project state, triggers
//! audio graph recompilation, and emits events back to the frontend.
//!
//! This is the ONLY path through which project-structural mutations are allowed
//! in the new architecture. The legacy individual Tauri commands in main.rs
//! coexist during the incremental migration period.

pub mod api;
pub mod commands;
pub mod events;
pub mod store;
