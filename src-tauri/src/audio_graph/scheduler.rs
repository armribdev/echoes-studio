//! Real-time graph scheduler — drives the execution plan in the audio callback.
//!
//! ## Threading model
//!
//! `GraphScheduler` is **exclusively owned by the audio thread** during
//! playback.  It is swapped atomically via `RuntimeSnapshot` at block
//! boundaries; the audio callback never contends with the compiler thread.
//!
//! ## Memory model
//!
//! All allocations happen at graph construction time (`GraphScheduler::new`).
//! The `process_block` function is allocation-free:
//!
//! - `processors`       — fixed-length Vec, no resize.
//! - `output_buffers`   — pre-allocated AudioBuffer per step per port.
//! - `silence_buffer`   — shared read-only zero buffer for unconnected ports.
//! - `input_ptr_scratch`— pre-sized raw pointer scratch; cleared + filled per step.
//!
//! ## Safety contract for `process_block`
//!
//! The audio callback passes `&mut GraphScheduler` exclusively.  Inside
//! `process_block` we use raw pointers to build a non-aliasing view of the
//! buffer pool:
//!
//! - For step `i`, input connections always reference `src_step < i`
//!   (guaranteed by topological ordering in `ExecutionPlan`).
//! - We read from `output_buffers[src_step]` and write to
//!   `output_buffers[i]`; these are disjoint elements of the Vec.
//! - We transmute the raw pointer scratch into `&[&AudioBuffer]` for the node
//!   call — valid because the pointers were derived from live references.

use std::collections::HashMap;
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::compiler::CompiledGraph;
use crate::audio_graph::execution_plan::ExecutionPlan;
use crate::audio_graph::node::{AudioNode, NodeId, TransportInfo};

/// Maximum number of input ports any single node may declare.
/// Sized conservatively; increase if needed.
const MAX_NODE_INPUT_PORTS: usize = 32;

// ── Modulation action (pre-compiled) ──────────────────────────────────────────

/// One modulation connection, resolved to step indices at compile time.
///
/// Built from `ModulationMap` in `GraphScheduler::new`; read-only during
/// `process_block`.  Grouped by target step index so the scheduler can apply
/// modulation with a single indexed lookup per step.
struct CompiledModAction {
    /// Step that produced the modulation signal (always < target_step by
    /// topological ordering — guaranteed by `GraphCompiler`).
    source_step: usize,
    /// Output port index on the source node (0 for LFO main output).
    source_port: usize,
    /// Parameter name on the target node (e.g. `"gain"`, `"cutoff"`).
    param_name: String,
    /// Modulation depth — the raw signal sample is multiplied by this.
    depth: f32,
}

/// Drives one compiled audio graph in the real-time audio callback.
pub struct GraphScheduler {
    /// Compiled execution plan (immutable, shared with compiler thread).
    plan: Arc<ExecutionPlan>,

    /// DSP processors in the same order as `plan.steps`.
    processors: Vec<Box<dyn AudioNode>>,

    /// `output_buffers[step_idx]` holds the output buffers for that step.
    /// Indexed as `output_buffers[step][port]`.
    /// Pre-allocated at construction; never resized in `process_block`.
    output_buffers: Vec<Vec<AudioBuffer>>,

    /// A permanently-zeroed buffer handed to unconnected input ports.
    silence_buffer: AudioBuffer,

    /// Pre-allocated raw pointer scratch for gathering input pointers.
    /// Capacity = `MAX_NODE_INPUT_PORTS`.  Never reallocated in the callback.
    input_ptr_scratch: Vec<*const AudioBuffer>,

    /// Modulation actions grouped by target step index.
    ///
    /// `mod_actions_by_step[i]` lists all modulation routes whose target node
    /// is at execution step `i`.  Built once at construction; never mutated
    /// during the audio callback.
    mod_actions_by_step: Vec<Vec<CompiledModAction>>,
}

// SAFETY:  GraphScheduler is only ever used on the audio thread.
// The raw pointers in `input_ptr_scratch` are only valid during the scope
// of `process_block` and are never sent to another thread.
unsafe impl Send for GraphScheduler {}

impl GraphScheduler {
    /// Build a new scheduler from a freshly compiled graph.
    ///
    /// Called on the compiler/command thread; may allocate freely.
    pub fn new(compiled: CompiledGraph) -> Self {
        let plan = compiled.plan;
        let processors = compiled.processors;

        let num_channels = plan.num_channels;
        let max_block = plan.max_block_size;

        // Pre-allocate output buffers: one Vec<AudioBuffer> per step,
        // one AudioBuffer per output port.
        let output_buffers: Vec<Vec<AudioBuffer>> = plan
            .steps
            .iter()
            .map(|step| {
                let n = step.num_outputs.max(1); // at least one slot even for sinks
                (0..n).map(|_| AudioBuffer::new(num_channels, max_block)).collect()
            })
            .collect();

        let silence_buffer = AudioBuffer::new(num_channels, max_block);

        // Pre-allocate input pointer scratch to avoid any allocation in the callback
        let input_ptr_scratch = vec![std::ptr::null(); MAX_NODE_INPUT_PORTS];

        // ── Pre-compile modulation actions ────────────────────────────────────────
        // NodeId → step index lookup (used only at construction — may allocate).
        let node_to_step: HashMap<NodeId, usize> = plan
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.node_id, i))
            .collect();

        let num_steps = plan.steps.len();
        let mut mod_actions_by_step: Vec<Vec<CompiledModAction>> =
            (0..num_steps).map(|_| Vec::new()).collect();

        for route in plan.modulation.all_routes() {
            let source_step = match node_to_step.get(&route.source.node) {
                Some(&i) => i,
                None => continue, // source not in plan — skip
            };
            let target_step = match node_to_step.get(&route.target.param.node) {
                Some(&i) => i,
                None => continue, // target not in plan — skip
            };
            mod_actions_by_step[target_step].push(CompiledModAction {
                source_step,
                source_port: route.source.port as usize,
                param_name: route.target.param.name.clone(),
                depth: route.target.depth,
            });
        }

        GraphScheduler {
            plan,
            processors,
            output_buffers,
            silence_buffer,
            input_ptr_scratch,
            mod_actions_by_step,
        }
    }

    /// Number of nodes in the current execution plan (0 = empty graph).
    pub fn num_steps(&self) -> usize {
        self.plan.steps.len()
    }

    /// An empty scheduler that produces silence.
    pub fn empty(sample_rate: f32) -> Self {
        let plan = Arc::new(ExecutionPlan::empty(sample_rate));
        GraphScheduler {
            plan,
            processors: Vec::new(),
            output_buffers: Vec::new(),
            silence_buffer: AudioBuffer::new(2, 512),
            input_ptr_scratch: vec![std::ptr::null(); MAX_NODE_INPUT_PORTS],
            mod_actions_by_step: Vec::new(),
        }
    }

    // ── Audio callback entry point ─────────────────────────────────────────────

    /// Process one block.  Must only be called from the audio thread.
    ///
    /// After this returns, `interleaved_output` contains the rendered audio
    /// in `[L0, R0, L1, R1, …]` interleaved format.
    pub fn process_block(
        &mut self,
        transport: &TransportInfo,
        interleaved_output: &mut [f32],
    ) {
        let _block_size = transport.block_size;
        let _num_channels = self.plan.num_channels;

        // Run every step in topological order.
        self.run_steps(transport);

        // Write final output (master step = last step) to the output buffer.
        if let Some(master_step_idx) = self.plan.master_step() {
            // The master node is a sink (0 outputs), so we read from its
            // *last* upstream step via the master's input connections.
            // Actually: master has 0 outputs but its inputs are the mixed
            // signals — we grab the signal from the last non-sink step.
            //
            // Convention: the node immediately before master sums all tracks.
            // If the master IS the last step and has 0 outputs, we look at
            // the step that feeds into it.
            self.write_master_output(master_step_idx, transport, interleaved_output);
        } else {
            // Empty graph — fill with silence
            interleaved_output.fill(0.0);
        }
    }

    // ── Internal step execution ────────────────────────────────────────────────

    /// Execute every step in the plan.
    ///
    /// # Safety
    ///
    /// For each step `i`, all `input_connections[*].src_step < i` (enforced by
    /// the compiler's topological sort).  This ensures that when we take a raw
    /// pointer to `output_buffers[src_step]` and a mutable reference to
    /// `output_buffers[i]`, they refer to distinct, non-overlapping memory.
    fn run_steps(&mut self, transport: &TransportInfo) {
        let pool_ptr: *mut Vec<AudioBuffer> = self.output_buffers.as_mut_ptr();
        let num_steps = self.plan.steps.len();

        for step_idx in 0..num_steps {
            let step = &self.plan.steps[step_idx];
            let num_inputs = step.input_connections.len();

            // ── Gather input pointers ──────────────────────────────────────────
            // Fill `input_ptr_scratch[0..num_inputs]` with pointers to the
            // upstream output buffers (or silence for unconnected ports).
            //
            // SAFETY: src_step < step_idx for all connections (compiler invariant).
            // The source buffers are in `output_buffers[src_step]`, which is a
            // distinct element from `output_buffers[step_idx]`.
            for (port, conn_opt) in step.input_connections.iter().enumerate() {
                self.input_ptr_scratch[port] = match conn_opt {
                    Some(conn) => unsafe {
                        let src_bufs: &Vec<AudioBuffer> = &*pool_ptr.add(conn.src_step);
                        let src_port = conn.src_port.min(src_bufs.len().saturating_sub(1));
                        &src_bufs[src_port] as *const AudioBuffer
                    },
                    None => &self.silence_buffer as *const AudioBuffer,
                };
            }

            // ── Build &[&AudioBuffer] from pointer scratch ─────────────────────
            // SAFETY: All pointers in scratch[0..num_inputs] point to live
            // AudioBuffer values.  We build a slice of references for the node.
            let inputs: &[&AudioBuffer] = unsafe {
                std::slice::from_raw_parts(
                    self.input_ptr_scratch.as_ptr() as *const &AudioBuffer,
                    num_inputs,
                )
            };

            // ── Clear output buffers before processing ─────────────────────────
            // SAFETY: output_buffers[step_idx] is a distinct element from all
            // source buffers referenced above (src_step < step_idx).
            let outputs: &mut Vec<AudioBuffer> = unsafe { &mut *pool_ptr.add(step_idx) };
            for buf in outputs.iter_mut() {
                buf.clear();
            }

            // ── Modulation dispatch ────────────────────────────────────────────
            // Apply all modulation routes targeting this step before calling
            // process.  Source buffers are already filled (src_step < step_idx
            // by topological order — compiler invariant).
            //
            // SAFETY: `mod_actions_by_step` is never modified during
            // `run_steps`.  We take a raw pointer to it so we can simultaneously
            // hold a mutable borrow on `self.processors[step_idx]`.
            // The two fields are disjoint; this mirrors the `pool_ptr` pattern.
            let actions_ptr: *const Vec<Vec<CompiledModAction>> =
                &self.mod_actions_by_step as *const _;
            // SAFETY: `mod_actions_by_step` is never modified during `run_steps`.
            // We first bind the raw pointer to a safe reference, then index —
            // this avoids the implicit-autoref-through-raw-pointer lint.
            let actions: &[CompiledModAction] = unsafe {
                let outer: &Vec<Vec<CompiledModAction>> = &*actions_ptr;
                &outer[step_idx]
            };

            if !actions.is_empty() {
                self.processors[step_idx].reset_modulation();
                for action in actions {
                    let mod_signal: f32 = unsafe {
                        let src_bufs: &Vec<AudioBuffer> =
                            &*pool_ptr.add(action.source_step);
                        if action.source_port < src_bufs.len()
                            && src_bufs[action.source_port].num_frames > 0
                        {
                            // PROVISIONAL (Phase 3): block-rate modulation — samples the first
                        // sample of the modulator's output buffer per block.  This
                        // introduces up to one block of latency and is not suitable for
                        // audio-rate modulation (FM, ring-mod).  Replace with sample-accurate
                        // interleaved dispatch when that requirement arises.
                            src_bufs[action.source_port].channel(0)[0]
                        } else {
                            0.0
                        }
                    };
                    self.processors[step_idx]
                        .apply_modulation(&action.param_name, mod_signal * action.depth);
                }
            }

            // ── Dispatch ───────────────────────────────────────────────────────
            self.processors[step_idx].process(inputs, outputs, transport);
        }
    }

    /// Read the final stereo signal and write to `interleaved_output`.
    ///
    /// `MasterNode::process` already sums all track inputs, applies soft-clip,
    /// and writes the result to `output_buffers[master_step_idx][0]`.  We read
    /// that buffer directly — no need to re-sum upstream nodes here.
    fn write_master_output(
        &self,
        master_step_idx: usize,
        transport: &TransportInfo,
        interleaved_output: &mut [f32],
    ) {
        let block_size = transport.block_size;
        let num_channels = self.plan.num_channels.min(2); // stereo out max

        interleaved_output.fill(0.0);

        // output_buffers[master_step_idx][0] was written by MasterNode::process
        // during run_steps (it declared num_outputs = 1 so the scheduler
        // pre-allocated exactly this slot).
        let master_bufs = &self.output_buffers[master_step_idx];
        if master_bufs.is_empty() {
            return; // safety guard — should never happen
        }
        let master_buf = &master_bufs[0];

        for ch in 0..num_channels {
            let ch_slice = if ch < master_buf.num_channels {
                master_buf.channel(ch)
            } else {
                master_buf.channel(0) // fold extra channels to L
            };

            for (frame, &sample) in ch_slice.iter().take(block_size).enumerate() {
                let idx = frame * num_channels + ch;
                if idx < interleaved_output.len() {
                    interleaved_output[idx] = sample;
                }
            }
        }
    }

    // ── Graph swap ─────────────────────────────────────────────────────────────

    /// Returns the current execution plan (used for introspection / debugging).
    pub fn plan(&self) -> &Arc<ExecutionPlan> {
        &self.plan
    }

    /// Reset all node DSP state (e.g. on transport stop).
    pub fn reset(&mut self) {
        for processor in &mut self.processors {
            processor.reset();
            processor.reset_modulation();
        }
        for bufs in &mut self.output_buffers {
            for b in bufs.iter_mut() {
                b.clear();
            }
        }
        self.silence_buffer.clear();
    }
}
