//! Compiled, immutable execution plan for the audio graph.
//!
//! The `ExecutionPlan` is the output of `GraphCompiler::compile`.  It is
//! produced on a non-real-time thread, wrapped in an `Arc`, and atomically
//! swapped into the `GraphScheduler` at block boundaries.
//!
//! ## Immutability contract
//!
//! Once built, an `ExecutionPlan` is **never mutated**.  The audio thread only
//! reads from it.  All project changes produce a brand-new plan.
//!
//! ## Memory layout
//!
//! The scheduler drives `steps` in order.  For each step:
//!
//!   1. The scheduler looks up `input_connections` to find which earlier
//!      step's output buffer(s) to pass as inputs.
//!   2. It calls `node.process(inputs, outputs, transport)`.
//!   3. The node's outputs become available as inputs for later steps.
//!
//! Because steps are in topological order, all `input_connections[*].src_step`
//! are strictly less than the current step index.  This guarantee is what
//! allows the scheduler to split the buffer array into disjoint read/write
//! regions without locking.

use crate::audio_graph::node::NodeId;
use crate::audio_graph::modulation::ModulationMap;

// ── Connection ─────────────────────────────────────────────────────────────────

/// Describes where a single input port receives its audio.
#[derive(Debug, Clone, Copy)]
pub struct InputConnection {
    /// Index of the upstream step in `ExecutionPlan::steps`.
    /// Always < current step index (topological ordering invariant).
    pub src_step: usize,
    /// Output port index on that upstream step's node.
    pub src_port: usize,
}

// ── Step ──────────────────────────────────────────────────────────────────────

/// One unit of work in the execution plan.
///
/// Processed in `steps[0], steps[1], …` order.
#[derive(Debug, Clone)]
pub struct ExecutionStep {
    /// Identifies which node to process.
    pub node_id: NodeId,

    /// `input_connections[port]` describes where port `port` receives audio.
    ///
    /// `None` means the port is unconnected (the scheduler feeds it a
    /// pre-zeroed silence buffer).
    ///
    /// `input_connections.len() == node.num_input_ports()`
    pub input_connections: Vec<Option<InputConnection>>,

    /// Number of output ports on this node.
    /// Pre-computed to avoid querying the node on the audio thread.
    pub num_outputs: usize,
}

// ── Plan ──────────────────────────────────────────────────────────────────────

/// Fully compiled, immutable execution plan.
///
/// Shared via `Arc<ExecutionPlan>` between the compiler thread and the audio
/// thread.
#[derive(Debug)]
pub struct ExecutionPlan {
    /// Nodes to process, in strict topological (dependency) order.
    pub steps: Vec<ExecutionStep>,

    /// Modulation routing table (built alongside the step list).
    pub modulation: ModulationMap,

    /// Sample rate this plan was compiled for.
    pub sample_rate: f32,

    /// Block size this plan was compiled for.
    pub max_block_size: usize,

    /// Number of audio channels (typically 2 for stereo).
    pub num_channels: usize,
}

impl ExecutionPlan {
    /// An empty plan (silence).  Used as the initial state before the first
    /// project compile.
    pub fn empty(sample_rate: f32) -> Self {
        ExecutionPlan {
            steps: Vec::new(),
            modulation: ModulationMap::new(),
            sample_rate,
            max_block_size: 512,
            num_channels: 2,
        }
    }

    /// The index of the final "master" step (the last step in the plan),
    /// which the engine reads its output from.
    ///
    /// Returns `None` if the plan is empty.
    pub fn master_step(&self) -> Option<usize> {
        if self.steps.is_empty() { None } else { Some(self.steps.len() - 1) }
    }

    /// Total number of output buffer slots required.
    ///
    /// Each step has `num_outputs` slots; the scheduler pre-allocates exactly
    /// this many `AudioBuffer`s in its `output_buffers` Vec.
    pub fn total_output_slots(&self) -> usize {
        self.steps.iter().map(|s| s.num_outputs.max(1)).sum()
    }
}

// ── Build helpers (used by GraphCompiler) ─────────────────────────────────────

/// Builder for an `ExecutionPlan`.  Used exclusively by `GraphCompiler`.
pub struct ExecutionPlanBuilder {
    steps: Vec<ExecutionStep>,
    modulation: ModulationMap,
    sample_rate: f32,
    max_block_size: usize,
    num_channels: usize,
}

impl ExecutionPlanBuilder {
    pub fn new(sample_rate: f32, max_block_size: usize, num_channels: usize) -> Self {
        ExecutionPlanBuilder {
            steps: Vec::new(),
            modulation: ModulationMap::new(),
            sample_rate,
            max_block_size,
            num_channels,
        }
    }

    pub fn push_step(&mut self, step: ExecutionStep) {
        self.steps.push(step);
    }

    pub fn set_modulation(&mut self, map: ModulationMap) {
        self.modulation = map;
    }

    pub fn build(self) -> ExecutionPlan {
        ExecutionPlan {
            steps: self.steps,
            modulation: self.modulation,
            sample_rate: self.sample_rate,
            max_block_size: self.max_block_size,
            num_channels: self.num_channels,
        }
    }
}
