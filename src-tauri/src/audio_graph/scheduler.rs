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
//! `run_steps`, the output buffer pool is split at the current step index via
//! `split_at_mut(step_idx)`:
//!
//! - For step `i`, input connections always reference `src_step < i`
//!   (guaranteed by topological ordering in `ExecutionPlan`).
//! - `processed = output_buffers[0..i]` (left half, reborrowed shared) is used
//!   to derive input pointers; `outputs = &mut output_buffers[i]` (first slot
//!   of the right half) is borrowed mutably.  The split makes the disjointness
//!   explicit to the compiler — no aliasing raw-pointer trick on the pool.
//! - Only the `input_ptr_scratch` → `&[&AudioBuffer]` transmute remains
//!   `unsafe`: the pointers were derived from live references whose backing
//!   storage (output buffers, delay-line scratch, silence buffer) outlives the
//!   single `process` call that consumes them.

use std::collections::HashMap;
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::compiler::CompiledGraph;
use crate::audio_graph::execution_plan::ExecutionPlan;
use crate::audio_graph::live_params::RouteCells;
use crate::audio_graph::node::{AudioNode, NodeId, TransportInfo};

/// Minimum scratch capacity for per-step input pointer gathering.
/// Actual size is `max(MIN_INPUT_PTR_SCRATCH, plan.max_input_ports())`.
const MIN_INPUT_PTR_SCRATCH: usize = 32;

// ── PDC delay line ────────────────────────────────────────────────────────────

/// Per-channel ring buffer used to delay one input port for latency
/// compensation.  Allocated at scheduler construction; `process` runs entirely
/// from pre-allocated storage.
struct DelayLine {
    /// One ring buffer per channel (`plan.num_channels` total).
    rings: Vec<Vec<f32>>,
    /// Ring capacity in samples (= delay + max_block).
    cap: usize,
    /// Delay in samples (> 0; zero-delay inputs don't get a `DelayLine`).
    delay: usize,
    /// Cursor into the ring; advances by `block_size` each `process`.
    write_pos: usize,
    /// Scratch output buffer; the scheduler points the node's input slot at
    /// this instead of the upstream raw buffer.
    out: AudioBuffer,
}

impl DelayLine {
    fn new(num_channels: usize, delay: usize, max_block: usize) -> Self {
        debug_assert!(delay > 0, "DelayLine with zero delay is wasteful — skip it");
        let cap = delay + max_block;
        DelayLine {
            rings: (0..num_channels).map(|_| vec![0.0; cap]).collect(),
            cap,
            delay,
            write_pos: 0,
            out: AudioBuffer::new(num_channels, max_block),
        }
    }

    /// Push one block from `src` into the ring; read out the same block delayed
    /// by `self.delay` samples into `self.out`.  Allocation-free.
    fn process(&mut self, src: &AudioBuffer, block_size: usize) {
        let cap = self.cap;
        let delay = self.delay;
        let write_pos = self.write_pos;

        let n_out = self.out.num_channels;
        for ch in 0..n_out {
            let src_ch_idx = ch.min(src.num_channels.saturating_sub(1));
            let src_ch = src.channel(src_ch_idx);
            let ring = &mut self.rings[ch];
            let dst_ch = self.out.channel_mut(ch);
            for i in 0..block_size {
                let w = (write_pos + i) % cap;
                ring[w] = src_ch[i];
                let r = (w + cap - delay) % cap;
                dst_ch[i] = ring[r];
            }
        }
        self.write_pos = (write_pos + block_size) % cap;
    }

    fn reset(&mut self) {
        for ring in self.rings.iter_mut() {
            for s in ring.iter_mut() {
                *s = 0.0;
            }
        }
        self.write_pos = 0;
        self.out.clear();
    }
}

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
    /// Node-local parameter identifier resolved by `AudioNode::param_id` at
    /// scheduler construction.  Replaces the previous `String` (cloned per
    /// route, string-compared each block on the hot path — see audit I12).
    param_id: u32,
    /// Modulation depth.  When `Some`, read per-block from the atomic so the
    /// command thread can change depth without a recompile.  When `None`,
    /// `static_depth` is used (routes constructed without a `route_id`).
    depth_cells: Option<Arc<RouteCells>>,
    static_depth: f32,
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
    /// Sized to the plan's largest input port count at construction.
    /// Never reallocated in the callback.
    input_ptr_scratch: Vec<*const AudioBuffer>,

    /// Modulation actions grouped by target step index.
    ///
    /// `mod_actions_by_step[i]` lists all modulation routes whose target node
    /// is at execution step `i`.  Built once at construction; never mutated
    /// during the audio callback.
    mod_actions_by_step: Vec<Vec<CompiledModAction>>,

    /// Plugin Delay Compensation lines, indexed `[step_idx][port_idx]`.
    /// `None` means the input port has zero compensation delay (the common
    /// case) and is passed through directly.  When `Some`, the scheduler
    /// pushes the upstream buffer through the line and points the node's
    /// input at the delay's internal `out` buffer.
    delay_lines: Vec<Vec<Option<DelayLine>>>,
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
        let route_cells = compiled.route_cells;

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

        // Size scratch to the plan's widest node (e.g. master with one port per track).
        let input_scratch_len = plan.max_input_ports().max(MIN_INPUT_PTR_SCRATCH);
        let input_ptr_scratch = vec![std::ptr::null(); input_scratch_len];

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
            // Resolve the parameter name to a node-local u32 ID once, here.
            // Unknown names are dropped at compile time (matches the previous
            // semantics where `apply_modulation` silently ignored them).
            let param_id = match processors[target_step].param_id(&route.target.param.name) {
                Some(id) => id,
                None => continue,
            };
            let depth_cells = route
                .route_id
                .as_ref()
                .and_then(|id| route_cells.get(id).cloned());
            mod_actions_by_step[target_step].push(CompiledModAction {
                source_step,
                source_port: route.source.port as usize,
                param_id,
                depth_cells,
                static_depth: route.target.depth,
            });
        }

        // ── Pre-allocate PDC delay lines ──────────────────────────────────────
        // Walk every step's input connections; allocate a `DelayLine` for each
        // port that the compiler tagged with `delay_samples > 0`.
        let delay_lines: Vec<Vec<Option<DelayLine>>> = plan
            .steps
            .iter()
            .map(|step| {
                step.input_connections
                    .iter()
                    .map(|conn_opt| match conn_opt {
                        Some(conn) if conn.delay_samples > 0 => Some(DelayLine::new(
                            num_channels,
                            conn.delay_samples as usize,
                            max_block,
                        )),
                        _ => None,
                    })
                    .collect()
            })
            .collect();

        GraphScheduler {
            plan,
            processors,
            output_buffers,
            silence_buffer,
            input_ptr_scratch,
            mod_actions_by_step,
            delay_lines,
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
            input_ptr_scratch: vec![std::ptr::null(); MIN_INPUT_PTR_SCRATCH],
            mod_actions_by_step: Vec::new(),
            delay_lines: Vec::new(),
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
    /// Disjointness is expressed to the compiler via `split_at_mut(step_idx)`:
    /// `processed` (left half) covers every prior step that input connections
    /// may reference, and `outputs` (first slot of the right half) is the
    /// current step's writable buffers.  No raw-pointer aliasing on the pool.
    fn run_steps(&mut self, transport: &TransportInfo) {
        let Self {
            plan,
            processors,
            output_buffers,
            silence_buffer,
            input_ptr_scratch,
            mod_actions_by_step,
            delay_lines,
        } = self;

        let num_steps = plan.steps.len();
        let silence_ptr: *const AudioBuffer = &*silence_buffer;

        for step_idx in 0..num_steps {
            let step = &plan.steps[step_idx];
            let num_inputs = step.input_connections.len();
            debug_assert!(
                num_inputs <= input_ptr_scratch.len(),
                "input_ptr_scratch too small: need {num_inputs}, have {}",
                input_ptr_scratch.len()
            );

            // Split the pool: `processed` is read-only and covers every step
            // an input may reference (src_step < step_idx, topological order);
            // `outputs` is this step's writable buffer slot.
            let (processed, tail) = output_buffers.split_at_mut(step_idx);
            let outputs: &mut Vec<AudioBuffer> = &mut tail[0];

            // ── Gather input pointers ──────────────────────────────────────────
            // Fill `input_ptr_scratch[0..num_inputs]` with pointers to the
            // upstream output buffers (or silence for unconnected ports).
            for (port, conn_opt) in step.input_connections.iter().enumerate() {
                input_ptr_scratch[port] = match conn_opt {
                    Some(conn) => {
                        let src_bufs: &Vec<AudioBuffer> = &processed[conn.src_step];
                        let src_port = conn.src_port.min(src_bufs.len().saturating_sub(1));
                        &src_bufs[src_port] as *const AudioBuffer
                    }
                    None => silence_ptr,
                };
            }

            // ── Apply PDC delay lines ──────────────────────────────────────────
            // For every input port the compiler tagged with non-zero
            // `delay_samples`, push the gathered upstream buffer through its
            // delay line and redirect the input pointer to the line's internal
            // `out` buffer.
            let step_delays = &mut delay_lines[step_idx];
            for (port, dl_opt) in step_delays.iter_mut().enumerate() {
                if let Some(dl) = dl_opt {
                    // SAFETY: scratch[port] was filled above with a pointer to
                    // a live AudioBuffer (either an upstream output in
                    // `processed`, which is still borrowed, or the long-lived
                    // `silence_buffer`).
                    let src_ref: &AudioBuffer = unsafe { &*input_ptr_scratch[port] };
                    dl.process(src_ref, transport.block_size);
                    input_ptr_scratch[port] = &dl.out as *const AudioBuffer;
                }
            }

            // ── Build &[&AudioBuffer] from pointer scratch ─────────────────────
            // SAFETY: All pointers in scratch[0..num_inputs] point to live
            // AudioBuffer values whose backing storage (entries in `processed`,
            // delay-line `out` buffers in `delay_lines[step_idx]`, or
            // `silence_buffer`) is disjoint from `outputs` and outlives the
            // `process` call below.
            let inputs: &[&AudioBuffer] = unsafe {
                std::slice::from_raw_parts(
                    input_ptr_scratch.as_ptr() as *const &AudioBuffer,
                    num_inputs,
                )
            };

            // ── Clear output buffers before processing ─────────────────────────
            for buf in outputs.iter_mut() {
                buf.clear();
            }

            // ── Modulation dispatch ────────────────────────────────────────────
            // Apply all modulation routes targeting this step before calling
            // process.  Source buffers live in `processed` (src_step < step_idx
            // by topological order), disjoint from the processor being mutated.
            let actions: &[CompiledModAction] = &mod_actions_by_step[step_idx];
            let processor = &mut processors[step_idx];

            // Always reset before re-summing modulation contributions for this
            // block: if a route is removed (or its actions list otherwise
            // shrinks to empty) between blocks, the previous block's
            // `mod_delta` must not persist.  Default impl is a no-op, so this
            // costs nothing for nodes without modulated parameters.
            processor.reset_modulation();
            for action in actions {
                let src_bufs: &Vec<AudioBuffer> = &processed[action.source_step];
                let mod_signal: f32 = if action.source_port < src_bufs.len()
                    && src_bufs[action.source_port].num_frames > 0
                {
                    // PROVISIONAL (Phase 3): block-rate modulation — samples
                    // the first sample of the modulator's output buffer per
                    // block.  Up to one block of latency; not suitable for
                    // audio-rate modulation (FM, ring-mod).  Replace with
                    // sample-accurate interleaved dispatch when needed.
                    src_bufs[action.source_port].channel(0)[0]
                } else {
                    0.0
                };
                let depth = match &action.depth_cells {
                    Some(cells) => cells.depth(),
                    None => action.static_depth,
                };
                processor.apply_modulation(action.param_id, mod_signal * depth);
            }

            // ── Dispatch ───────────────────────────────────────────────────────
            processor.process(inputs, outputs, transport);
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
        for step_lines in &mut self.delay_lines {
            for dl_opt in step_lines.iter_mut() {
                if let Some(dl) = dl_opt {
                    dl.reset();
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::buffer::AudioBuffer;
    use crate::audio_graph::execution_plan::{
        ExecutionPlan, ExecutionStep, InputConnection,
    };
    use crate::audio_graph::modulation::ModulationMap;
    use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
    use crate::audio_graph::compiler::CompiledGraph;
    use std::collections::HashMap;

    /// Source node: emits a stored sample on frame 0 of the next block, then silence.
    /// Used to inject a single impulse to verify PDC alignment.
    struct ImpulseSource {
        id: NodeId,
        pending: f32,
    }
    impl ImpulseSource {
        fn new(id: u64, value: f32) -> Self {
            ImpulseSource { id: NodeId(id), pending: value }
        }
    }
    impl AudioNode for ImpulseSource {
        fn node_id(&self) -> NodeId { self.id }
        fn num_input_ports(&self) -> usize { 0 }
        fn num_output_ports(&self) -> usize { 1 }
        fn prepare(&mut self, _ctx: PrepareContext) {}
        fn process(
            &mut self,
            _inputs: &[&AudioBuffer],
            outputs: &mut [AudioBuffer],
            _transport: &TransportInfo,
        ) {
            let buf = &mut outputs[0];
            for ch in 0..buf.num_channels {
                let s = buf.channel_mut(ch);
                if !s.is_empty() {
                    s[0] = self.pending;
                }
            }
            self.pending = 0.0;
        }
        fn reset(&mut self) {}
    }

    /// Single-input pass-through node that reports a fixed intrinsic latency
    /// (but does not actually delay its signal).  Lets the test verify that
    /// the compiler/scheduler insert compensation downstream of *other* paths.
    struct LatencyReporter {
        id: NodeId,
        latency: u32,
    }
    impl LatencyReporter {
        fn new(id: u64, latency: u32) -> Self {
            LatencyReporter { id: NodeId(id), latency }
        }
    }
    impl AudioNode for LatencyReporter {
        fn node_id(&self) -> NodeId { self.id }
        fn num_input_ports(&self) -> usize { 1 }
        fn num_output_ports(&self) -> usize { 1 }
        fn prepare(&mut self, _ctx: PrepareContext) {}
        fn process(
            &mut self,
            inputs: &[&AudioBuffer],
            outputs: &mut [AudioBuffer],
            _transport: &TransportInfo,
        ) {
            outputs[0].copy_from(inputs[0]);
        }
        fn reset(&mut self) {}
        fn latency_samples(&self) -> u32 { self.latency }
    }

    /// Two-input summing node — joins parallel paths so PDC must align them.
    struct Sum2 { id: NodeId }
    impl Sum2 { fn new(id: u64) -> Self { Sum2 { id: NodeId(id) } } }
    impl AudioNode for Sum2 {
        fn node_id(&self) -> NodeId { self.id }
        fn num_input_ports(&self) -> usize { 2 }
        fn num_output_ports(&self) -> usize { 1 }
        fn prepare(&mut self, _ctx: PrepareContext) {}
        fn process(
            &mut self,
            inputs: &[&AudioBuffer],
            outputs: &mut [AudioBuffer],
            _transport: &TransportInfo,
        ) {
            outputs[0].copy_from(inputs[0]);
            outputs[0].mix_from(inputs[1], 1.0);
        }
        fn reset(&mut self) {}
    }

    /// Build a tiny plan + processors with the topology:
    ///
    /// ```text
    ///   src_a ──► reporter(latency=N) ──┐
    ///                                    ├──► sum ──► (master output)
    ///   src_b ──────────────────────────┘
    /// ```
    ///
    /// `sum` has two inputs at different cumulative latencies — the scheduler
    /// must delay `src_b` by `N` samples so the two impulses land in the same
    /// output frame.
    fn build_test_compiled(latency: u32, block: usize) -> CompiledGraph {
        let steps = vec![
            // 0: src_a
            ExecutionStep {
                node_id: NodeId(1),
                input_connections: vec![],
                num_outputs: 1,
            },
            // 1: src_b
            ExecutionStep {
                node_id: NodeId(2),
                input_connections: vec![],
                num_outputs: 1,
            },
            // 2: reporter (consumes src_a)
            ExecutionStep {
                node_id: NodeId(3),
                input_connections: vec![Some(InputConnection {
                    src_step: 0,
                    src_port: 0,
                    delay_samples: 0,
                })],
                num_outputs: 1,
            },
            // 3: sum (consumes reporter, src_b)
            ExecutionStep {
                node_id: NodeId(4),
                input_connections: vec![
                    Some(InputConnection { src_step: 2, src_port: 0, delay_samples: 0 }),
                    Some(InputConnection { src_step: 1, src_port: 0, delay_samples: 0 }),
                ],
                num_outputs: 1,
            },
        ];

        let mut plan = ExecutionPlan {
            steps,
            modulation: ModulationMap::new(),
            sample_rate: 48_000.0,
            max_block_size: block,
            num_channels: 2,
            step_latencies: vec![0; 4],
            step_cumulative_latency: vec![0; 4],
        };

        let processors: Vec<Box<dyn AudioNode>> = vec![
            Box::new(ImpulseSource::new(1, 1.0)),
            Box::new(ImpulseSource::new(2, 1.0)),
            Box::new(LatencyReporter::new(3, latency)),
            Box::new(Sum2::new(4)),
        ];

        // Replicate the compiler's PDC pass: arrival[i] = max input cumlat;
        // input.delay = arrival - cumlat[src]; cumlat[i] = intrinsic + arrival.
        for i in 0..plan.steps.len() {
            plan.step_latencies[i] = processors[i].latency_samples();
        }
        for i in 0..plan.steps.len() {
            let arrival = plan.steps[i]
                .input_connections
                .iter()
                .filter_map(|c| c.as_ref().map(|c| plan.step_cumulative_latency[c.src_step]))
                .max()
                .unwrap_or(0);
            for conn_opt in plan.steps[i].input_connections.iter_mut() {
                if let Some(conn) = conn_opt {
                    let src_lat = plan.step_cumulative_latency[conn.src_step];
                    conn.delay_samples = arrival.saturating_sub(src_lat);
                }
            }
            plan.step_cumulative_latency[i] =
                plan.step_latencies[i].saturating_add(arrival);
        }

        CompiledGraph {
            plan: Arc::new(plan),
            processors,
            meter_cells: HashMap::new(),
            route_cells: HashMap::new(),
        }
    }

    fn run_one_block(sched: &mut GraphScheduler, block: usize) -> Vec<f32> {
        let transport = TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            sample_rate: 48_000.0,
            block_size: block,
        };
        let master_step = sched.plan.master_step().unwrap();
        sched.run_steps(&transport);
        // Read sum's output buffer directly (port 0) and return left channel.
        sched.output_buffers[master_step][0].channel(0).to_vec()
    }

    #[test]
    fn pdc_aligns_two_parallel_paths() {
        let latency = 5u32;
        let block = 32usize;
        let compiled = build_test_compiled(latency, block);

        // Confirm the compiler-side computation produced the expected delays:
        // src_b (step 1) should be delayed by `latency` samples; the reporter
        // path is unchanged.
        let sum_step = &compiled.plan.steps[3];
        assert_eq!(sum_step.input_connections[0].as_ref().unwrap().delay_samples, 0);
        assert_eq!(sum_step.input_connections[1].as_ref().unwrap().delay_samples, latency);
        assert_eq!(compiled.plan.step_cumulative_latency[3], latency);

        let mut sched = GraphScheduler::new(compiled);

        // First block: src_a's impulse hits the reporter at frame 0 → reaches
        // sum's input 0 at frame 0.  src_b's impulse is delayed by `latency`,
        // so it lands on sum's input 1 at frame `latency`.  Because the sources
        // are not aligned by intent (both fire at frame 0), the *naive* sum
        // without PDC would put both impulses at frame 0.  With PDC active,
        // the src_b impulse is pushed to frame `latency` to match the path
        // through the latent node.
        let out = run_one_block(&mut sched, block);

        assert_eq!(out[0], 1.0, "src_a impulse must arrive at frame 0");
        for i in 1..(latency as usize) {
            assert_eq!(out[i], 0.0, "silence expected at frame {i} (waiting on delayed path)");
        }
        assert_eq!(out[latency as usize], 1.0, "src_b impulse must arrive at frame `latency`");
        for i in (latency as usize + 1)..block {
            assert_eq!(out[i], 0.0, "silence expected after both impulses at frame {i}");
        }
    }

    #[test]
    fn pdc_zero_latency_is_passthrough() {
        let block = 16usize;
        let compiled = build_test_compiled(0, block);

        // No node reports latency → every input.delay_samples must be 0 and
        // the scheduler must not allocate any DelayLines.
        for step in compiled.plan.steps.iter() {
            for conn in step.input_connections.iter().flatten() {
                assert_eq!(conn.delay_samples, 0);
            }
        }
        let mut sched = GraphScheduler::new(compiled);
        assert!(sched.delay_lines.iter().all(|s| s.iter().all(|d| d.is_none())));

        let out = run_one_block(&mut sched, block);
        // Both src_a and src_b impulses fire at frame 0 → sum = 2.0.
        assert_eq!(out[0], 2.0);
        for s in &out[1..] {
            assert_eq!(*s, 0.0);
        }
    }

    #[test]
    fn pdc_delay_carries_across_block_boundary() {
        // Latency larger than one block — the delayed impulse must surface on
        // a subsequent block.
        let latency = 10u32;
        let block = 4usize;
        let compiled = build_test_compiled(latency, block);
        let mut sched = GraphScheduler::new(compiled);

        let b0 = run_one_block(&mut sched, block);
        let b1 = run_one_block(&mut sched, block);
        let b2 = run_one_block(&mut sched, block);

        // src_a's impulse fired only on the first block (ImpulseSource::pending
        // is consumed), so b1 / b2 contain only the delayed src_b impulse.
        assert_eq!(b0[0], 1.0);
        let delayed_block = (latency as usize) / block;
        let delayed_offset = (latency as usize) % block;
        let stream = [b0, b1, b2];
        assert_eq!(stream[delayed_block][delayed_offset], 1.0,
            "delayed src_b impulse must land at absolute frame {latency}");
    }
}
