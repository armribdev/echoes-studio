//! Internal WASM plugin runtime.
//!
//! Status: **interface defined, wasmtime integration pending**.

pub mod wasm_runtime;
pub mod wrapper;

pub use wasm_runtime::WasmRuntime;
pub use wrapper::WasmWrapperNode;
