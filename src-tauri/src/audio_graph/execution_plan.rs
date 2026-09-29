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
    #[allow(dead_code)] // compile-time metadata, asserted by tests
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
    #[cfg(test)]
    pub fn total_latency_samples(&self) -> u32 {
        self.master_step()
            .and_then(|i| self.step_cumulative_latency.get(i).copied())
            .unwrap_or(0)
    }

    /// Last step index (master output), or `None` if empty.
    pub fn master_step(&self) -> Option<usize> {
        self.steps.len().checked_sub(1)
    }

    #[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn step(
        node_id: u64,
        input_connections: Vec<Option<InputConnection>>,
        num_outputs: usize,
    ) -> ExecutionStep {
        ExecutionStep {
            node_id: NodeId(node_id),
            input_connections,
            num_outputs,
        }
    }

    #[test]
    fn empty_plan_has_no_master_step_and_zero_latency() {
        let plan = ExecutionPlan::empty(48_000.0);
        assert_eq!(plan.master_step(), None);
        assert_eq!(plan.total_latency_samples(), 0);
        assert_eq!(plan.total_output_slots(), 0);
        assert_eq!(plan.max_input_ports(), 0);
    }

    #[test]
    fn master_step_is_the_last_step_index() {
        let mut builder = ExecutionPlanBuilder::new(48_000.0, 512, 2);
        builder.push_step(step(1, vec![None], 1));
        builder.push_step(step(
            2,
            vec![Some(InputConnection {
                src_step: 0,
                src_port: 0,
                delay_samples: 0,
            })],
            1,
        ));
        let plan = builder.build();

        assert_eq!(plan.master_step(), Some(1));
    }

    #[test]
    fn total_output_slots_counts_at_least_one_per_step() {
        let mut builder = ExecutionPlanBuilder::new(48_000.0, 512, 2);
        builder.push_step(step(1, vec![None], 0));
        builder.push_step(step(2, vec![None], 3));
        let plan = builder.build();

        // Step 1 declares 0 outputs but still occupies at least one output slot.
        assert_eq!(plan.total_output_slots(), 1 + 3);
    }

    #[test]
    fn max_input_ports_is_the_widest_step() {
        let mut builder = ExecutionPlanBuilder::new(48_000.0, 512, 2);
        builder.push_step(step(1, vec![None], 1));
        builder.push_step(step(2, vec![None, None, None], 1));
        let plan = builder.build();

        assert_eq!(plan.max_input_ports(), 3);
    }

    #[test]
    fn total_latency_samples_reads_the_master_steps_cumulative_latency() {
        let mut builder = ExecutionPlanBuilder::new(48_000.0, 512, 2);
        builder.push_step(step(1, vec![None], 1));
        builder.push_step(step(2, vec![None], 1));
        let mut plan = builder.build();
        plan.step_cumulative_latency = vec![64, 128];

        assert_eq!(plan.total_latency_samples(), 128);
    }

    #[test]
    fn build_preserves_sample_rate_and_block_config() {
        let builder = ExecutionPlanBuilder::new(44_100.0, 256, 1);
        let plan = builder.build();

        assert_eq!(plan.sample_rate, 44_100.0);
        assert_eq!(plan.max_block_size, 256);
        assert_eq!(plan.num_channels, 1);
    }
}
