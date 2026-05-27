//! Plugin format adapters.
//!
//! Each sub-module adapts a specific plugin format into the graph-compatible
//! `Device + AudioNode` interface.  The audio graph runtime never imports
//! these modules; only the `PluginHost` does.
//!
//! ## Status
//!
//! | Format   | Module     | Status                                      |
//! |----------|------------|---------------------------------------------|
//! | VST3     | `vst3`     | Interface defined; COM FFI pending          |
//! | AU       | `au`       | Interface defined; macOS CoreAudio pending  |
//! | WASM     | `internal` | Interface defined; wasmtime pending         |

pub mod au;
pub mod internal;
pub mod vst3;
