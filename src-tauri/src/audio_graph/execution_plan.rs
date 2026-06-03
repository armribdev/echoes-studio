//! Compiled, immutable execution plan for the audio graph.
//!
//! Output of `GraphCompiler::compile`.  Built off the audio thread, shared via
//! `Arc`, swapped into `GraphScheduler` at block boundaries.  Never mutated
//! after build — project changes produce a new plan.
//!
//! Steps run in topological order: `input_connections[*].src_step` is always
//! less than the current step index (no locks between read/write regions).

use crate::audio_graph::modulation::ModulationMap;
use crate::audio_graph::node::NodeId;

const DEFAULT_MAX_BLOCK_SIZE: usize = 512;
const DEFAULT_NUM_CHANNELS: usize = 2;

/// Upstream source and PDC delay for one input port.
#[derive(Debug, Clone, Copy)]
pub struct InputConnection {
    /// Upstream step index (< current step).
    pub src_step: usize,
    pub src_port: usize,
    /// PDC padding in samples (0 = none).  From `AudioNode::latency_samples()`.
    pub delay_samples: u32,
}

/// One node invocation in topological order.
#[derive(Debug, Clone)]
pub struct ExecutionStep {
    pub node_id: NodeId,
    /// `None` → scheduler feeds silence.  Length == `num_input_ports()`.
    pub input_connections: Vec<Option<InputConnection>>,
    /// Pre-computed; avoids querying the node on the audio thread.
    pub num_outputs: usize,
}

/// Immutable plan shared between compiler and audio threads.
#[derive(Debug)]
pub struct ExecutionPlan {
    pub steps: Vec<ExecutionStep>,
    pub modulation: ModulationMap,
    pub sample_rate: f32,
    pub max_block_size: usize,
    pub num_channels: usize,
    /// Per-step `AudioNode::latency_samples()` at compile time.
    pub step_latencies: Vec<u32>,
    /// Output cumulative latency: `step_latencies[i] + max cumulative[input]`.
    pub step_cumulative_latency: Vec<u32>,
    /// R1 parallel levels: `level[i] = 1 + max(level[deps])`, sources at 0.
    pub step_levels: Vec<u32>,
    /// Step indices grouped by level, ascending.
    pub levels: Vec<Vec<usize>>,
}

impl ExecutionPlan {
    pub fn empty(sample_rate: f32) -> Self {
        Self {
            steps: Vec::new(),
            modulation: ModulationMap::new(),
            sample_rate,
            max_block_size: DEFAULT_MAX_BLOCK_SIZE,
            num_channels: DEFAULT_NUM_CHANNELS,
            step_latencies: Vec::new(),
            step_cumulative_latency: Vec::new(),
            step_levels: Vec::new(),
            levels: Vec::new(),
        }
    }

    /// End-to-end latency at the master step (0 if empty).
    pub fn total_latency_samples(&self) -> u32 {
        self.master_step()
            .and_then(|i| self.step_cumulative_latency.get(i).copied())
            .unwrap_or(0)
    }

    /// Last step index (master output), or `None` if empty.
    pub fn master_step(&self) -> Option<usize> {
        self.steps.len().checked_sub(1)
    }

    pub fn total_output_slots(&self) -> usize {
        self.steps.iter().map(|s| s.num_outputs.max(1)).sum()
    }

    pub fn max_input_ports(&self) -> usize {
        self.steps
            .iter()
            .map(|s| s.input_connections.len())
            .max()
            .unwrap_or(0)
    }
}

pub struct ExecutionPlanBuilder {
    steps: Vec<ExecutionStep>,
    modulation: ModulationMap,
    sample_rate: f32,
    max_block_size: usize,
    num_channels: usize,
}

impl ExecutionPlanBuilder {
    pub fn new(sample_rate: f32, max_block_size: usize, num_channels: usize) -> Self {
        Self {
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
        let n = self.steps.len();
        ExecutionPlan {
            steps: self.steps,
            modulation: self.modulation,
            sample_rate: self.sample_rate,
            max_block_size: self.max_block_size,
            num_channels: self.num_channels,
            step_latencies: vec![0; n],
            step_cumulative_latency: vec![0; n],
            step_levels: vec![0; n],
            levels: Vec::new(),
        }
    }
}
