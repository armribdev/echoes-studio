//! Plugin format adapters.
//!
//! Each sub-module adapts a specific plugin format into the graph-compatible
//! `Device + AudioNode` interface.  The audio graph runtime never imports
//! these modules; only the `PluginHost` does.
//!
//! ## Status
//!
//! | Format   | Module     | Status                                              |
//! |----------|------------|-----------------------------------------------------|
//! | VST3     | `vst3`     | Hosted                                              |
//! | CLAP     | `clap`     | Hosted (in-process or sandboxed)                    |
//! | AU       | `au`       | Stub behind the `au` feature; CoreAudio pending     |
//! | WASM     | `internal` | Stub behind the `wasm` feature; wasmtime pending    |

#[cfg(feature = "au")]
pub mod au;
pub mod clap;
pub(crate) mod crash_guard;
#[cfg(feature = "wasm")]
pub mod internal;
pub mod vst3;
