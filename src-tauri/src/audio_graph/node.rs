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

use std::sync::{atomic::{AtomicU32, Ordering}, Arc};

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::live_params::EnvelopeCells;

/// Sub-block modulation control period in samples (ADR-019).  Modulation is
/// sampled at absolute sample positions that are multiples of this period, so
/// the modulated render is deterministic for a given block size.
pub const MODULATION_CONTROL_PERIOD: usize = 32;

/// Upper bound on interior control points per parameter per block:
/// `ceil(MAX_BLOCK_FRAMES / MODULATION_CONTROL_PERIOD)` with `MAX_BLOCK_FRAMES = 4096`.
pub const MAX_CONTROL_POINTS: usize = 128;

/// Pre-allocated slot for one modulated plugin parameter.
///
/// Allocated at graph compile time; iterated by plugin wrappers on the audio thread.
/// `delta` is accumulated each block via `apply_modulation` and reset by
/// `reset_modulation`.  `shadow` holds the command-thread baseline normalised value [0..1].
pub struct PluginModEntry {
    pub native_id: u32,
    /// Accumulated modulation delta for this block.  Written and read exclusively
    /// on the audio thread — no synchronisation needed.
    pub delta: f32,
    /// Baseline normalised value tracked by the command thread.  Shared with
    /// `LiveParams::plugin_param_shadows`; updated when a parameter change is pushed.
    pub shadow: Arc<AtomicU32>,
    /// Interior sub-block modulation points for this block (ADR-019):
    /// `(sample_offset, accumulated value)`.  Written by `apply_modulation_at`,
    /// reset by `reset_modulation`.  Offset 0 rides on `delta` instead.
    pub points: [(u32, f32); MAX_CONTROL_POINTS],
    pub point_count: usize,
}

impl PluginModEntry {
    #[inline]
    pub fn shadow_value(&self) -> f32 {
        f32::from_bits(self.shadow.load(Ordering::Relaxed))
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortId {
    pub node: NodeId,
    pub port: u32,
}

/// Bipolar / unipolar polarity hint for modulation ports.
///
/// Reserved for the future `PortKind::Modulation` variant.  Phase 1 only
/// declares the type; the compiler keeps treating every edge as `Audio`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    Bipolar,
    Unipolar,
}

/// Typed category of a node port.
///
/// **Phase 1 status**: declared as a public enum to fix the contract early
/// (target BP-2), but the graph compiler does **not** read it yet.  Every
/// existing `Edge` is implicitly `Audio`; ADR-008 (Phase 5) introduces the
/// `kind` field on `Edge` and the validation pass.
///
/// Keeping the variant set stable now means later phases extend
/// behaviour without re-introducing exhaustiveness churn across the codebase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortKind {
    Audio { channels: u8 },
    Midi,
    Modulation { polarity: Polarity },
    Control,
}

impl PortKind {
    /// Default for legacy connections that pre-date `PortKind` typing.
    pub const fn default_audio_stereo() -> Self {
        Self::Audio { channels: 2 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PrepareContext {
    pub sample_rate: f32,
    pub max_block_size: usize,
    pub num_channels: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct TransportInfo {
    pub is_playing: bool,
    pub position_samples: u64,
    pub position_seconds: f64,
    pub bpm: f32,
    pub sample_rate: f32,
    pub block_size: usize,
}

impl TransportInfo {
    const DEFAULT_BPM: f32 = 120.0;
    const DEFAULT_BLOCK_SIZE: usize = 512;

    pub fn stopped(sample_rate: f32) -> Self {
        Self {
            is_playing: false,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: Self::DEFAULT_BPM,
            sample_rate,
            block_size: Self::DEFAULT_BLOCK_SIZE,
        }
    }
}

/// DSP unit in the graph (clips, effects, plugins).  `process` must be RT-safe.
pub trait AudioNode: Send + 'static {
    fn node_id(&self) -> NodeId;

    fn label(&self) -> &str {
        "AudioNode"
    }

    fn num_input_ports(&self) -> usize;
    fn num_output_ports(&self) -> usize;

    /// May allocate; idempotent.
    fn prepare(&mut self, ctx: PrepareContext);

    /// Must never allocate or block.  Outputs are pre-zeroed.
    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    );

    fn reset(&mut self);

    fn reset_modulation(&mut self) {}

    /// Resolved once at scheduler build (avoids per-block string compare, audit I12).
    fn param_id(&self, _name: &str) -> Option<u32> {
        None
    }

    /// `delta` is `signal * depth`; summed per parameter before `process`.
    fn apply_modulation(&mut self, _param_id: u32, _delta: f32) {}

    /// Sub-block modulation point at `sample_offset` within the current block
    /// (ADR-019).  Default ignores it — only nodes that deliver sample-accurate
    /// parameter events (plugin wrappers) override this.  Offset 0 is delivered
    /// via `apply_modulation`, so the default scalar path stays bit-identical.
    fn apply_modulation_at(&mut self, _param_id: u32, _sample_offset: u32, _value: f32) {}

    /// Called once at compile time by the `GraphCompiler` to inject pre-allocated
    /// modulation entries for external plugin parameters.  Default no-op for
    /// non-plugin nodes.
    fn inject_plugin_mod_entries(&mut self, _entries: Vec<PluginModEntry>) {}

    /// Called once at compile time to inject `EnvelopeCells` references for all
    /// envelope modulators on the same track.  The node writes the gate state to
    /// these cells on each NoteOn / NoteOff.  Default no-op for non-instrument nodes.
    fn inject_envelope_cells(&mut self, _cells: Vec<Arc<EnvelopeCells>>) {}

    fn latency_samples(&self) -> u32 {
        0
    }

    fn is_source(&self) -> bool {
        self.num_input_ports() == 0
    }

    fn is_sink(&self) -> bool {
        self.num_output_ports() == 0
    }
}
