//! Real-time graph scheduler — drives the execution plan in the audio callback.
//!
//! ## Threading model
//!
//! `GraphScheduler` is **exclusively owned by the audio thread** during
//! playback.  It is swapped atomically via `RuntimeSnapshot` at block
//! boundaries; the audio callback never contends with the compiler thread.
//!
//! When constructed with `num_workers > 0`, the scheduler also drives a
//! pre-spawned worker pool (`audio_graph::worker_pool::WorkerPool`).  At each
//! block the audio thread walks `ExecutionPlan::levels` and dispatches every
//! level's steps to the pool; steps in the same level have no transitive
//! dependency and are processed concurrently.  The dispatcher participates in
//! the work-stealing loop too so it doesn't sit idle.  See `worker_pool.rs`
//! for the dispatch / completion protocol.
//!
//! ## Memory model
//!
//! All allocations happen at graph construction time (`GraphScheduler::new`).
//! `process_block` is allocation-free:
//!
//! - `processors`         — fixed-length Vec, no resize.
//! - `output_buffers`     — pre-allocated AudioBuffer per step per port.
//! - `silence_buffer`     — shared read-only zero buffer for unconnected ports.
//! - `worker_scratch`     — one `Vec<*const AudioBuffer>` per worker (audio
//!                          thread = worker `num_workers`); pre-sized.
//! - `delay_lines`        — pre-allocated PDC rings.
//! - `mod_actions_by_step`— per-step modulation table, built at construction.
//!
//! ## Safety contract for parallel execution
//!
//! `Workspace` carries raw pointers into the per-step Vecs.  Inside
//! `process_one_step`, every read/write is gated by the level invariant
//! enforced by `GraphCompiler::compute_execution_levels`:
//!
//! - `processors[step_idx]`, `output_buffers[step_idx]`, `delay_lines[step_idx]`
//!   are mutated only by the worker assigned to that `step_idx`.
//! - Audio inputs reference `output_buffers[src_step]` for `src_step` in a
//!   strictly *earlier* level — already fully written and synchronised by the
//!   pool's level-completion barrier.
//! - Modulation source steps are also constrained to strictly-earlier levels
//!   by `compute_execution_levels` (it folds modulation routes into the
//!   dependency relation).
//! - `worker_scratch[worker_id]` is touched only by that worker.
//!
//! The only `unsafe` is the raw-pointer dereference of these disjoint slots
//! plus the `*const AudioBuffer → &AudioBuffer` slice transmute for the
//! `&[&AudioBuffer]` input slice.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::compiler::CompiledGraph;
use crate::audio_graph::execution_plan::ExecutionPlan;
use crate::audio_graph::live_params::RouteCells;
use crate::audio_graph::node::{AudioNode, NodeId, TransportInfo};
use crate::audio_graph::worker_pool::WorkerPool;

const MIN_INPUT_PTR_SCRATCH: usize = 32;
const POOL_MIN_LEVEL_WIDTH: usize = 2; // wider levels use the pool; single-step stays inline

/// PDC ring per input port; pre-allocated at scheduler construction.
struct DelayLine {
    rings: Vec<Vec<f32>>,
    cap: usize,
    delay: usize,
    write_pos: usize,
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

struct CompiledModAction {
    source_step: usize,
    source_port: usize,
    param_id: u32,
    depth_cells: Option<Arc<RouteCells>>,
    static_depth: f32,
}

#[derive(Clone, Copy)]
struct Workspace {
    plan: *const ExecutionPlan,
    processors_ptr: *mut Box<dyn AudioNode>,
    output_buffers_ptr: *mut Vec<AudioBuffer>,
    delay_lines_ptr: *mut Vec<Option<DelayLine>>,
    mod_actions_ptr: *const Vec<CompiledModAction>,
    silence_ptr: *const AudioBuffer,
    worker_scratch_ptr: *mut Vec<*const AudioBuffer>,
    transport_ptr: *const TransportInfo,
}

// SAFETY: every pointer is into storage exclusively owned by the
// `GraphScheduler`; the scheduler is `&mut self` during `process_block`, so no
// other thread holds an aliasing reference.  Workers only access disjoint
// indices under the level invariant.  See module-level safety contract.
unsafe impl Send for Workspace {}
unsafe impl Sync for Workspace {}

pub struct GraphScheduler {
    plan: Arc<ExecutionPlan>,
    processors: Vec<Box<dyn AudioNode>>,
    output_buffers: Vec<Vec<AudioBuffer>>,
    silence_buffer: AudioBuffer,
    worker_scratch: Vec<Vec<*const AudioBuffer>>,
    mod_actions_by_step: Vec<Vec<CompiledModAction>>,
    delay_lines: Vec<Vec<Option<DelayLine>>>,
    pool: Option<WorkerPool<Workspace>>,
}

// SAFETY: see `Workspace` — raw pointers are scoped to `process_block` calls
// where the scheduler holds `&mut self`.
unsafe impl Send for GraphScheduler {}

impl GraphScheduler {
    pub fn new(compiled: CompiledGraph) -> Self {
        Self::with_workers(compiled, 0)
    }

    /// `num_workers == 0` → sequential execution on the audio thread.
    pub fn with_workers(compiled: CompiledGraph, num_workers: usize) -> Self {
        let plan = compiled.plan;
        let processors = compiled.processors;
        let route_cells = compiled.route_cells;

        let num_channels = plan.num_channels;
        let max_block = plan.max_block_size;

        let output_buffers: Vec<Vec<AudioBuffer>> = plan
            .steps
            .iter()
            .map(|step| {
                let n = step.num_outputs.max(1); // at least one slot even for sinks
                (0..n)
                    .map(|_| AudioBuffer::new(num_channels, max_block))
                    .collect()
            })
            .collect();

        let silence_buffer = AudioBuffer::new(num_channels, max_block);

        let scratch_len = plan.max_input_ports().max(MIN_INPUT_PTR_SCRATCH);
        let worker_scratch: Vec<Vec<*const AudioBuffer>> = (0..(num_workers + 1))
            .map(|_| vec![std::ptr::null(); scratch_len])
            .collect();

        // NodeId → step index — used only at construction, may allocate.
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

        let pool = if num_workers > 0 {
            Some(WorkerPool::<Workspace>::new(num_workers))
        } else {
            None
        };

        GraphScheduler {
            plan,
            processors,
            output_buffers,
            silence_buffer,
            worker_scratch,
            mod_actions_by_step,
            delay_lines,
            pool,
        }
    }

    pub fn num_steps(&self) -> usize {
        self.plan.steps.len()
    }

    pub fn empty(sample_rate: f32) -> Self {
        let plan = Arc::new(ExecutionPlan::empty(sample_rate));
        GraphScheduler {
            plan,
            processors: Vec::new(),
            output_buffers: Vec::new(),
            silence_buffer: AudioBuffer::new(2, 512),
            worker_scratch: vec![vec![std::ptr::null(); MIN_INPUT_PTR_SCRATCH]],
            mod_actions_by_step: Vec::new(),
            delay_lines: Vec::new(),
            pool: None,
        }
    }

    /// Audio thread only.  Output is interleaved `[L0, R0, L1, R1, …]`.
    pub fn process_block(&mut self, transport: &TransportInfo, interleaved_output: &mut [f32]) {
        self.run_levels(transport);

        if let Some(master_step_idx) = self.plan.master_step() {
            self.write_master_output(master_step_idx, transport, interleaved_output);
        } else {
            interleaved_output.fill(0.0);
        }
    }

    fn run_levels(&mut self, transport: &TransportInfo) {
        if self.plan.steps.is_empty() {
            return;
        }

        let plan_arc = Arc::clone(&self.plan);
        let workspace = Workspace {
            plan: Arc::as_ptr(&plan_arc),
            processors_ptr: self.processors.as_mut_ptr(),
            output_buffers_ptr: self.output_buffers.as_mut_ptr(),
            delay_lines_ptr: self.delay_lines.as_mut_ptr(),
            mod_actions_ptr: self.mod_actions_by_step.as_ptr(),
            silence_ptr: &self.silence_buffer,
            worker_scratch_ptr: self.worker_scratch.as_mut_ptr(),
            transport_ptr: transport as *const TransportInfo,
        };

        for level_indices in plan_arc.levels.iter() {
            if level_indices.is_empty() {
                continue;
            }

            match &self.pool {
                Some(pool) if level_indices.len() >= POOL_MIN_LEVEL_WIDTH => {
                    pool.dispatch(level_indices, workspace, step_thunk);
                }
                _ => {
                    // Single-step levels skip pool dispatch to avoid unpark/barrier overhead.
                    for &step_idx in level_indices {
                        unsafe { process_one_step(0, step_idx, workspace) };
                    }
                }
            }
        }
    }

    fn write_master_output(
        &self,
        master_step_idx: usize,
        transport: &TransportInfo,
        interleaved_output: &mut [f32],
    ) {
        let block_size = transport.block_size;
        let num_channels = self.plan.num_channels.min(2); // stereo out max

        interleaved_output.fill(0.0);

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

    pub fn plan(&self) -> &Arc<ExecutionPlan> {
        &self.plan
    }

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

fn step_thunk(worker_id: usize, step_idx: usize, ws: Workspace) {
    unsafe { process_one_step(worker_id, step_idx, ws) }
}

/// # Safety
/// `step_idx` is exclusive per level; lower levels finished; `ws` valid for `process_block`.
unsafe fn process_one_step(worker_id: usize, step_idx: usize, ws: Workspace) {
    let plan = &*ws.plan;
    let step = &plan.steps[step_idx];
    let num_inputs = step.input_connections.len();
    let transport = &*ws.transport_ptr;

    let scratch = &mut *ws.worker_scratch_ptr.add(worker_id);
    debug_assert!(
        num_inputs <= scratch.len(),
        "input_ptr_scratch too small: need {num_inputs}, have {}",
        scratch.len()
    );

    for (port, conn_opt) in step.input_connections.iter().enumerate() {
        scratch[port] = match conn_opt {
            Some(conn) => {
                let src_bufs = &*ws.output_buffers_ptr.add(conn.src_step);
                let src_port = conn.src_port.min(src_bufs.len().saturating_sub(1));
                &src_bufs[src_port] as *const AudioBuffer
            }
            None => ws.silence_ptr,
        };
    }

    let step_delays = &mut *ws.delay_lines_ptr.add(step_idx);
    for (port, dl_opt) in step_delays.iter_mut().enumerate() {
        if let Some(dl) = dl_opt {
            let src_ref: &AudioBuffer = &*scratch[port];
            dl.process(src_ref, transport.block_size);
            scratch[port] = &dl.out as *const AudioBuffer;
        }
    }

    let inputs: &[&AudioBuffer] =
        std::slice::from_raw_parts(scratch.as_ptr() as *const &AudioBuffer, num_inputs);

    let outputs = &mut *ws.output_buffers_ptr.add(step_idx);
    for buf in outputs.iter_mut() {
        buf.clear();
    }

    let actions: &[CompiledModAction] = &*ws.mod_actions_ptr.add(step_idx);
    let processor = &mut *ws.processors_ptr.add(step_idx);

    // Always reset before re-summing modulation contributions for this block:
    // if a route is removed (or its actions list otherwise shrinks to empty)
    // between blocks, the previous block's `mod_delta` must not persist.
    processor.reset_modulation();
    for action in actions {
        let src_bufs = &*ws.output_buffers_ptr.add(action.source_step);
        let mod_signal: f32 =
            if action.source_port < src_bufs.len() && src_bufs[action.source_port].num_frames > 0 {
                // PROVISIONAL (Phase 3): block-rate modulation — samples the
                // first sample of the modulator's output buffer per block.  Up to
                // one block of latency; not suitable for audio-rate modulation
                // (FM, ring-mod).  Replace with sample-accurate interleaved
                // dispatch when needed.
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

    // Phase 1 / P0-5: catch Rust panics so a buggy
    // node (or a Rust-side panic raised from a third-party wrapper) does not
    // unwind the audio thread or a pool worker, which would crash the engine.
    //
    // The output buffers were cleared above; on panic they stay silent for
    // this block.  The processor itself may be left in an inconsistent state
    // — proper isolation (sandbox / Dead-marking) is Phase 6 work.
    //
    // Note: this only captures Rust unwinding panics.  C++ structured
    // exceptions (Windows SEH access violations) still abort the process;
    // see the audit's P0-5 long-term entry and ADR-010 for the bridged-host
    // plan that addresses them.
    //
    // Cost on the hot path: one setjmp-style frame setup per node per block,
    // measured in single-digit ns on x86_64.  Negligible compared to a real
    // plugin's `process` (µs–ms).  No allocation, no syscalls.
    let result = catch_unwind(AssertUnwindSafe(|| {
        processor.process(inputs, outputs, transport);
    }));
    if result.is_err() {
        // SAFETY: the closure consumed the previous `&mut Vec<AudioBuffer>`
        // borrow when it returned (whether normally or by unwind), so it is
        // safe to re-acquire a fresh exclusive borrow here.  The level
        // barrier still guarantees exclusive access to step_idx.
        let outs = &mut *ws.output_buffers_ptr.add(step_idx);
        for buf in outs.iter_mut() {
            buf.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::buffer::AudioBuffer;
    use crate::audio_graph::compiler::{CompiledGraph, GraphCompiler};
    use crate::audio_graph::execution_plan::{ExecutionPlan, ExecutionStep, InputConnection};
    use crate::audio_graph::modulation::ModulationMap;
    use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
    use std::collections::HashMap;

    /// Source node: emits a stored sample on frame 0 of the next block, then silence.
    /// Used to inject a single impulse to verify PDC alignment.
    struct ImpulseSource {
        id: NodeId,
        pending: f32,
    }
    impl ImpulseSource {
        fn new(id: u64, value: f32) -> Self {
            ImpulseSource {
                id: NodeId(id),
                pending: value,
            }
        }
    }
    impl AudioNode for ImpulseSource {
        fn node_id(&self) -> NodeId {
            self.id
        }
        fn num_input_ports(&self) -> usize {
            0
        }
        fn num_output_ports(&self) -> usize {
            1
        }
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
            LatencyReporter {
                id: NodeId(id),
                latency,
            }
        }
    }
    impl AudioNode for LatencyReporter {
        fn node_id(&self) -> NodeId {
            self.id
        }
        fn num_input_ports(&self) -> usize {
            1
        }
        fn num_output_ports(&self) -> usize {
            1
        }
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
        fn latency_samples(&self) -> u32 {
            self.latency
        }
    }

    /// Two-input summing node — joins parallel paths so PDC must align them.
    struct Sum2 {
        id: NodeId,
    }
    impl Sum2 {
        fn new(id: u64) -> Self {
            Sum2 { id: NodeId(id) }
        }
    }
    impl AudioNode for Sum2 {
        fn node_id(&self) -> NodeId {
            self.id
        }
        fn num_input_ports(&self) -> usize {
            2
        }
        fn num_output_ports(&self) -> usize {
            1
        }
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
                    Some(InputConnection {
                        src_step: 2,
                        src_port: 0,
                        delay_samples: 0,
                    }),
                    Some(InputConnection {
                        src_step: 1,
                        src_port: 0,
                        delay_samples: 0,
                    }),
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
            step_levels: vec![0; 4],
            levels: Vec::new(),
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
            plan.step_cumulative_latency[i] = plan.step_latencies[i].saturating_add(arrival);
        }

        // Compute levels via the public compiler entry point so the test
        // mirrors the production code path.
        GraphCompiler::compute_execution_levels(&mut plan);

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
        sched.run_levels(&transport);
        sched.output_buffers[master_step][0].channel(0).to_vec()
    }

    #[test]
    fn pdc_aligns_two_parallel_paths() {
        let latency = 5u32;
        let block = 32usize;
        let compiled = build_test_compiled(latency, block);

        let sum_step = &compiled.plan.steps[3];
        assert_eq!(
            sum_step.input_connections[0]
                .as_ref()
                .unwrap()
                .delay_samples,
            0
        );
        assert_eq!(
            sum_step.input_connections[1]
                .as_ref()
                .unwrap()
                .delay_samples,
            latency
        );
        assert_eq!(compiled.plan.step_cumulative_latency[3], latency);

        let mut sched = GraphScheduler::new(compiled);
        let out = run_one_block(&mut sched, block);

        assert_eq!(out[0], 1.0, "src_a impulse must arrive at frame 0");
        for i in 1..(latency as usize) {
            assert_eq!(
                out[i], 0.0,
                "silence expected at frame {i} (waiting on delayed path)"
            );
        }
        assert_eq!(
            out[latency as usize], 1.0,
            "src_b impulse must arrive at frame `latency`"
        );
        for i in (latency as usize + 1)..block {
            assert_eq!(
                out[i], 0.0,
                "silence expected after both impulses at frame {i}"
            );
        }
    }

    #[test]
    fn pdc_zero_latency_is_passthrough() {
        let block = 16usize;
        let compiled = build_test_compiled(0, block);

        for step in compiled.plan.steps.iter() {
            for conn in step.input_connections.iter().flatten() {
                assert_eq!(conn.delay_samples, 0);
            }
        }
        let mut sched = GraphScheduler::new(compiled);
        assert!(sched
            .delay_lines
            .iter()
            .all(|s| s.iter().all(|d| d.is_none())));

        let out = run_one_block(&mut sched, block);
        assert_eq!(out[0], 2.0);
        for s in &out[1..] {
            assert_eq!(*s, 0.0);
        }
    }

    #[test]
    fn pdc_delay_carries_across_block_boundary() {
        let latency = 10u32;
        let block = 4usize;
        let compiled = build_test_compiled(latency, block);
        let mut sched = GraphScheduler::new(compiled);

        let b0 = run_one_block(&mut sched, block);
        let b1 = run_one_block(&mut sched, block);
        let b2 = run_one_block(&mut sched, block);

        assert_eq!(b0[0], 1.0);
        let delayed_block = (latency as usize) / block;
        let delayed_offset = (latency as usize) % block;
        let stream = [b0, b1, b2];
        assert_eq!(
            stream[delayed_block][delayed_offset], 1.0,
            "delayed src_b impulse must land at absolute frame {latency}"
        );
    }

    #[test]
    fn level_computation_groups_parallel_sources() {
        // src_a (lvl 0), src_b (lvl 0) → reporter (lvl 1) → sum (lvl 2).
        // src_b lives at level 0 too even though sum receives it directly,
        // because the level of each step is computed off its inputs.
        let compiled = build_test_compiled(0, 16);
        let plan = &compiled.plan;
        assert_eq!(plan.step_levels, vec![0, 0, 1, 2]);
        assert_eq!(plan.levels.len(), 3);
        assert_eq!(plan.levels[0], vec![0, 1]);
        assert_eq!(plan.levels[1], vec![2]);
        assert_eq!(plan.levels[2], vec![3]);
    }

    #[test]
    fn parallel_pool_produces_same_output_as_sequential() {
        // Build two identical plans; one runs sequential, one with a pool.
        let latency = 7u32;
        let block = 32usize;

        let mut seq_sched = GraphScheduler::with_workers(build_test_compiled(latency, block), 0);
        let mut par_sched = GraphScheduler::with_workers(build_test_compiled(latency, block), 3);

        let seq = run_one_block(&mut seq_sched, block);
        let par = run_one_block(&mut par_sched, block);

        assert_eq!(seq, par, "parallel execution must match sequential output");
    }

    #[test]
    fn parallel_pool_handles_multiple_blocks() {
        let block = 8usize;
        let mut sched = GraphScheduler::with_workers(build_test_compiled(0, block), 2);
        for _ in 0..16 {
            let _ = run_one_block(&mut sched, block);
        }
    }
}
