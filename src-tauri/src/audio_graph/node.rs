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

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::live_params::EnvelopeCells;
use crate::audio_graph::plugin_params::PluginParamControl;

/// Sub-block modulation control period in samples (ADR-019).  Modulation is
/// sampled at absolute sample positions that are multiples of this period, so
/// the modulated render is deterministic for a given block size.
pub const MODULATION_CONTROL_PERIOD: usize = 32;

/// Upper bound on interior control points per parameter per block:
/// `ceil(MAX_BLOCK_FRAMES / MODULATION_CONTROL_PERIOD)` with `MAX_BLOCK_FRAMES = 4096`.
pub const MAX_CONTROL_POINTS: usize = 128;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

impl TimeSignature {
    /// Falls back to 4/4 when either part is 0, so bar math never divides by zero.
    pub fn new(numerator: u8, denominator: u8) -> Self {
        if numerator == 0 || denominator == 0 {
            return Self::default();
        }
        Self { numerator, denominator }
    }

    pub fn quarter_notes_per_bar(self) -> f64 {
        self.numerator as f64 * 4.0 / self.denominator as f64
    }
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self { numerator: 4, denominator: 4 }
    }
}

/// Block-start position on the musical timeline, in quarter notes — the unit
/// both CLAP and VST3 expect from their host.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MusicalPosition {
    pub beats: f64,
    pub bar_start_beats: f64,
    /// Bar 0 starts at the beginning of the project.
    pub bar_number: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct TransportInfo {
    pub is_playing: bool,
    pub position_samples: u64,
    pub position_seconds: f64,
    pub bpm: f32,
    pub time_signature: TimeSignature,
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
            time_signature: TimeSignature::default(),
            sample_rate,
            block_size: Self::DEFAULT_BLOCK_SIZE,
        }
    }

    /// The project has a single tempo, so beats are a linear function of time.
    pub fn musical_position(&self) -> MusicalPosition {
        let beats = self.position_seconds * self.bpm as f64 / 60.0;
        let bar_length = self.time_signature.quarter_notes_per_bar();
        let bar_index = (beats / bar_length).floor();
        MusicalPosition {
            beats,
            bar_start_beats: bar_index * bar_length,
            bar_number: bar_index as i32,
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

    /// Automation value at the block start (ADR-021).  Plugin wrappers treat it
    /// as the parameter's absolute value; the default keeps the additive
    /// modulation path, so internal nodes render exactly as before.
    fn apply_automation(&mut self, param_id: u32, value: f32) {
        self.apply_modulation(param_id, value);
    }

    /// Automation value at an interior control-grid offset (ADR-021).
    fn apply_automation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        self.apply_modulation_at(param_id, sample_offset, value);
    }

    /// Called at every compile for plugin nodes, with the plugin parameters that
    /// modulation or automation targets (possibly none).  Default no-op.
    fn inject_plugin_param_controls(&mut self, _controls: Vec<PluginParamControl>) {}

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
