//! WASM-based internal plugin runtime.
//!
//! Status: **design complete, runtime integration pending**.
//!
//! ## Goals
//!
//! - Sandboxed DSP execution in a WASM module
//! - Memory isolation: plugin cannot corrupt engine state
//! - Portable plugins: one `.wasm` binary runs on all platforms
//! - Future extensibility: community plugin ecosystem
//!
//! ## Implementation plan
//!
//! Use `wasmtime` (Bytecode Alliance) as the WASM runtime:
//!
//! ```toml
//! [dependencies]
//! wasmtime = { version = "20", optional = true }
//! ```
//!
//! ### Host ABI (exposed to WASM module)
//!
//! The WASM plugin must export:
//! - `alloc(size: i32) -> i32`          — allocate shared memory
//! - `dealloc(ptr: i32, size: i32)`     — free shared memory
//! - `prepare(sr: f32, block: i32)`     — initialise
//! - `process(inputs: i32, outputs: i32, frames: i32)` — DSP entry
//! - `reset()`                          — clear state
//!
//! Audio buffers are exchanged through a pre-allocated shared linear memory
//! region negotiated at `prepare` time.  No allocation occurs in `process`.
//!
//! ### Safety
//!
//! WASM modules execute in a sandboxed linear memory region.  They cannot
//! access Rust heap memory outside the shared region.  The host validates
//! all pointer arguments before use.

/// WASM runtime context.
///
/// Holds the `wasmtime::Engine` + `Store` for one plugin instance.
pub struct WasmRuntime;

impl WasmRuntime {
    pub fn new() -> Self { WasmRuntime }

    /// Load a `.wasm` binary and prepare it for audio processing.
    pub fn load(&self, _wasm_bytes: &[u8]) -> Result<(), String> {
        Err("WASM runtime not yet implemented".to_string())
    }
}

impl Default for WasmRuntime {
    fn default() -> Self { Self::new() }
}
