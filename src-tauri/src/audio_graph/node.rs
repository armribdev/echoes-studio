//! Core DSP node abstraction.
//!
//! Every element in the audio graph — clips, gains, plugins, buses — implements
//! the [`AudioNode`] trait.  The graph runtime knows nothing about plugin formats
//! or project structure; it only calls `prepare` and `process`.
//!
//! ## Real-time contract for `process`
//!
//! An implementation **must never**:
//! - Allocate heap memory
//! - Acquire a `Mutex` or block on any synchronisation primitive
//! - Perform filesystem or network I/O
//! - Call `async`/`await`
//! - Access `AppState` or any UI-side data
//!
//! `prepare` is called on a non-real-time thread and **may** allocate.

use crate::audio_graph::buffer::AudioBuffer;

// ── Identifiers ────────────────────────────────────────────────────────────────

/// Stable, opaque identifier for a node in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

impl NodeId {
    pub const INVALID: NodeId = NodeId(u64::MAX);
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Node({})", self.0)
    }
}

/// Identifies a specific port on a specific node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortId {
    pub node: NodeId,
    /// 0-based port index.
    pub port: u32,
}

// ── Context types ──────────────────────────────────────────────────────────────

/// Passed to [`AudioNode::prepare`] when the session initialises or when the
/// audio device configuration changes.
///
/// Calling `prepare` is always safe to allocate in.
#[derive(Debug, Clone, Copy)]
pub struct PrepareContext {
    pub sample_rate: f32,
    pub max_block_size: usize,
    /// Channel count that all buffers in this session will use.
    pub num_channels: usize,
}

/// Transport state delivered to every node's `process` call.
///
/// Copied per-block: no locking required on the audio thread.
#[derive(Debug, Clone, Copy)]
pub struct TransportInfo {
    pub is_playing: bool,
    /// Absolute position in samples since the start of the project (never wraps).
    pub position_samples: u64,
    /// Same position converted to seconds (convenience).
    pub position_seconds: f64,
    pub bpm: f32,
    pub sample_rate: f32,
    /// Number of frames in the current block (≤ `max_block_size`).
    pub block_size: usize,
}

impl TransportInfo {
    pub fn stopped(sample_rate: f32) -> Self {
        TransportInfo {
            is_playing: false,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            sample_rate,
            block_size: 512,
        }
    }
}

// ── AudioNode trait ────────────────────────────────────────────────────────────

/// The fundamental DSP processing unit.
///
/// Implementors cover everything from raw audio clips to VST3 wrappers.
/// The graph runtime treats them all identically.
///
/// ## Port model
///
/// Each node declares a fixed number of input and output **ports** via
/// `num_input_ports` / `num_output_ports`.  Each port carries one
/// `AudioBuffer` per block.  Port indices are 0-based.
///
/// The `process` call receives:
/// - `inputs`:  one `&AudioBuffer` per input port  (may be a silence buffer if unconnected)
/// - `outputs`: one `&mut AudioBuffer` per output port (pre-zeroed by the scheduler)
pub trait AudioNode: Send + 'static {
    // ── Identity ───────────────────────────────────────────────────────────────

    fn node_id(&self) -> NodeId;

    /// Human-readable label for debugging / UI display.
    fn label(&self) -> &str {
        "AudioNode"
    }

    // ── Port declaration ───────────────────────────────────────────────────────

    fn num_input_ports(&self) -> usize;
    fn num_output_ports(&self) -> usize;

    // ── Lifecycle ──────────────────────────────────────────────────────────────

    /// Called once before streaming begins and whenever device parameters change.
    ///
    /// Allowed to allocate.  Must be idempotent (may be called multiple times).
    fn prepare(&mut self, ctx: PrepareContext);

    /// Process one block of audio.
    ///
    /// `inputs.len()  == self.num_input_ports()`
    /// `outputs.len() == self.num_output_ports()`
    ///
    /// Outputs are pre-zeroed by the scheduler before this call.
    ///
    /// **Must never allocate or block.**
    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    );

    /// Reset all stateful DSP (filters, envelopes, delay lines) to initial state.
    ///
    /// Called on transport stop or when the node is deactivated.
    fn reset(&mut self);

    // ── Modulation hooks ───────────────────────────────────────────────────────

    /// Reset per-block modulation accumulators to zero.
    ///
    /// Called by the scheduler before the first `apply_modulation` call for
    /// each block.  Default: no-op (nodes without modulated parameters ignore
    /// this call).
    fn reset_modulation(&mut self) {}

    /// Accumulate one modulation contribution onto a named parameter.
    ///
    /// `delta` is already scaled by the route's depth (i.e. `signal * depth`).
    /// Multiple routes to the same parameter are summed across multiple calls.
    /// The node applies `base_value + total_delta` during `process`.
    ///
    /// Default: no-op.
    fn apply_modulation(&mut self, _param: &str, _delta: f32) {}

    // ── Optional metadata ──────────────────────────────────────────────────────

    /// Intrinsic processing latency in samples (default 0).
    ///
    /// Used by the compiler for latency compensation.
    fn latency_samples(&self) -> u32 {
        0
    }

    /// True if the node is a source (no audio inputs, e.g. a track player).
    fn is_source(&self) -> bool {
        self.num_input_ports() == 0
    }

    /// True if the node is a sink (no audio outputs, e.g. the master output).
    fn is_sink(&self) -> bool {
        self.num_output_ports() == 0
    }
}
