//! Graph compiler — converts `AudioGraph` topology into an immutable
//! `ExecutionPlan` and a matching set of `Box<dyn AudioNode>` instances.
//!
//! ## Pipeline
//!
//! ```text
//! AppProject
//!     → AudioGraph::from_project()           topology (editor-side)
//!     → GraphCompiler::compile()             execution plan + node instances
//!     → GraphScheduler::new(plan, nodes)     real-time driver
//! ```
//!
//! ## Compilation steps
//!
//! 1. **Validate** — reject graphs with cycles or disconnected master nodes.
//! 2. **Topological sort** — Kahn's BFS algorithm; produces a deterministic
//!    execution order where every producer precedes its consumers.
//! 3. **Assign step indices** — map `NodeId` → `usize` (position in `steps`).
//! 4. **Build input connections** — for each step, record which earlier step +
//!    port feeds each of its input ports.
//! 5. **Instantiate nodes** — call `NodeFactory::build_node` for each step to
//!    get a `Box<dyn AudioNode>`.
//! 6. **Prepare nodes** — call `node.prepare(ctx)` for each node.
//! 7. **Return** `(ExecutionPlan, Vec<Box<dyn AudioNode>>)`.

use std::collections::{HashMap, VecDeque};
use std::sync::{
    atomic::AtomicU32,
    Arc,
};

use crate::audio_graph::execution_plan::{ExecutionPlan, ExecutionPlanBuilder, ExecutionStep, InputConnection};
use crate::audio_graph::graph::{AudioGraph, NodeKind};
use crate::audio_graph::live_params::LiveParams;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext};
use crate::audio_graph::modulation::ModulationMap;
use crate::plugin_system::PluginHost;
use crate::project::AppProject;

/// Shared peak cells for one track meter: (peak_L, peak_R).
pub type MeterCell = (Arc<AtomicU32>, Arc<AtomicU32>);

fn zero_cell() -> Arc<AtomicU32> {
    Arc::new(AtomicU32::new(0.0_f32.to_bits()))
}

// ── Compile config ─────────────────────────────────────────────────────────────

/// Parameters that vary per audio device session.
#[derive(Debug, Clone, Copy)]
pub struct CompileConfig {
    pub sample_rate: f32,
    pub max_block_size: usize,
    pub num_channels: usize,
}

impl Default for CompileConfig {
    fn default() -> Self {
        CompileConfig {
            sample_rate: 48_000.0,
            max_block_size: 512,
            num_channels: 2,
        }
    }
}

// ── Compile error ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    /// The graph contains at least one directed cycle.
    CycleDetected,
    /// No master output node exists.
    NoMasterNode,
    /// A referenced node does not exist in the graph.
    MissingNode(NodeId),
    /// An edge references a port that the node does not have.
    InvalidPort { node: NodeId, port: u32 },
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::CycleDetected => write!(f, "audio graph contains a cycle"),
            CompileError::NoMasterNode => write!(f, "audio graph has no master output node"),
            CompileError::MissingNode(id) => write!(f, "missing node {}", id),
            CompileError::InvalidPort { node, port } => {
                write!(f, "invalid port {} on node {}", port, node)
            }
        }
    }
}

// ── Compiled output ───────────────────────────────────────────────────────────

/// Everything the `GraphScheduler` needs to run one session.
pub struct CompiledGraph {
    pub plan: Arc<ExecutionPlan>,
    /// Node processors in the same order as `plan.steps`.
    pub processors: Vec<Box<dyn AudioNode>>,
    /// Per-track peak cells: meter_id (= track id) → (peak_L, peak_R).
    /// Written every block by `MeterNode`; read by the position-reporter thread.
    pub meter_cells: HashMap<String, MeterCell>,
    /// Per-route depth cells indexed by project route_id, used by the
    /// scheduler so live depth changes don't require a recompile.
    pub route_cells: HashMap<String, Arc<crate::audio_graph::live_params::RouteCells>>,
}

// ── GraphCompiler ─────────────────────────────────────────────────────────────

pub struct GraphCompiler;

impl GraphCompiler {
    /// Compile an `AudioGraph` into an executable `CompiledGraph`.
    ///
    /// Called on a non-real-time thread (command / compiler thread).
    /// Never called from the audio callback.
    pub fn compile(
        graph: &AudioGraph,
        project: &AppProject,
        config: CompileConfig,
        live_params: &LiveParams,
        plugin_host: &mut PluginHost,
    ) -> Result<CompiledGraph, CompileError> {
        // 1. Validate
        if graph.master_id.is_none() {
            return Err(CompileError::NoMasterNode);
        }

        // 2. Topological sort (Kahn's algorithm)
        let order = Self::topological_sort(graph)?;

        // 3. Build step-index map  NodeId → usize
        let step_index: HashMap<NodeId, usize> = order
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();

        // 4. Build ExecutionSteps
        let mut plan_builder = ExecutionPlanBuilder::new(
            config.sample_rate,
            config.max_block_size,
            config.num_channels,
        );

        let mut steps: Vec<ExecutionStep> = Vec::with_capacity(order.len());

        for &node_id in &order {
            let graph_node = graph.nodes.get(&node_id)
                .ok_or(CompileError::MissingNode(node_id))?;

            let num_inputs = Self::num_inputs_for_kind(&graph_node.kind);
            let num_outputs = Self::num_outputs_for_kind(&graph_node.kind);

            // Build input_connections: for each input port, find the upstream edge
            let mut input_connections: Vec<Option<InputConnection>> = vec![None; num_inputs];

            for edge in graph.edges_into(node_id) {
                let dst_port = edge.to.port as usize;
                if dst_port < num_inputs {
                    let src_step = *step_index.get(&edge.from.node)
                        .ok_or(CompileError::MissingNode(edge.from.node))?;
                    input_connections[dst_port] = Some(InputConnection {
                        src_step,
                        src_port: edge.from.port as usize,
                        delay_samples: 0,
                    });
                }
            }

            steps.push(ExecutionStep {
                node_id,
                input_connections,
                num_outputs,
            });
        }

        for step in steps {
            plan_builder.push_step(step);
        }

        // Carry the modulation routing table built by AudioGraph::from_project.
        plan_builder.set_modulation(graph.modulation.clone());

        // Build the plan owned (no Arc yet) — we still need to fill in PDC
        // delays once node latencies are known.
        let mut plan = plan_builder.build();

        // 5. Instantiate processors (Meter nodes get fresh atomic cells)
        let prepare_ctx = PrepareContext {
            sample_rate: config.sample_rate,
            max_block_size: config.max_block_size,
            num_channels: config.num_channels,
        };

        let mut processors: Vec<Box<dyn AudioNode>> = Vec::with_capacity(order.len());
        let mut meter_cells: HashMap<String, MeterCell> = HashMap::new();

        // Resolve depth cells for every modulation route so the scheduler can
        // read depths atomically without consulting `LiveParams` at runtime.
        let mut route_cells: HashMap<String, Arc<crate::audio_graph::live_params::RouteCells>> =
            HashMap::new();
        for route in graph.modulation.all_routes() {
            if let Some(route_id) = &route.route_id {
                let cells = live_params.get_or_init_route(route_id, route.target.depth);
                route_cells.insert(route_id.clone(), cells);
            }
        }

        for &node_id in &order {
            let graph_node = &graph.nodes[&node_id];
            let node: Box<dyn AudioNode> = if let NodeKind::Meter { meter_id } = &graph_node.kind {
                let peak_l = zero_cell();
                let peak_r = zero_cell();
                meter_cells.insert(meter_id.clone(), (Arc::clone(&peak_l), Arc::clone(&peak_r)));
                Box::new(crate::audio_graph::nodes::meter::MeterNode::new(node_id, peak_l, peak_r))
            } else {
                Self::build_node(
                    node_id,
                    &graph_node.kind,
                    project,
                    config.sample_rate,
                    live_params,
                    plugin_host,
                )
            };
            let mut node = node;
            node.prepare(prepare_ctx);
            processors.push(node);
        }

        // 6. Plugin Delay Compensation — compute per-step cumulative latency
        // and per-input delay alignments now that every node has been built.
        Self::compute_latency_compensation(&mut plan, &processors);

        let plan = Arc::new(plan);

        Ok(CompiledGraph { plan, processors, meter_cells, route_cells })
    }

    // ── PDC ───────────────────────────────────────────────────────────────────

    /// Compute and assign latency compensation delays into `plan.steps[*].input_connections`.
    ///
    /// Walks the plan in topological order (already enforced by the compiler) and:
    ///   - records each step's intrinsic latency from `node.latency_samples()`.
    ///   - computes `cumulative_latency[i]` = `intrinsic[i] + max input arrival`.
    ///   - for each input port `p`, sets `delay_samples` so all converging
    ///     inputs at step `i` arrive aligned at the maximum input arrival time.
    fn compute_latency_compensation(
        plan: &mut ExecutionPlan,
        processors: &[Box<dyn AudioNode>],
    ) {
        let n = plan.steps.len();
        debug_assert_eq!(processors.len(), n, "plan / processors length mismatch");

        plan.step_latencies.clear();
        plan.step_latencies.resize(n, 0);
        plan.step_cumulative_latency.clear();
        plan.step_cumulative_latency.resize(n, 0);

        for i in 0..n {
            plan.step_latencies[i] = processors[i].latency_samples();
        }

        for i in 0..n {
            // Topological invariant: every src_step < i, so cumulative values are
            // already filled before we reach this step.
            let mut arrival: u32 = 0;
            for conn_opt in &plan.steps[i].input_connections {
                if let Some(conn) = conn_opt {
                    let src_lat = plan.step_cumulative_latency[conn.src_step];
                    if src_lat > arrival {
                        arrival = src_lat;
                    }
                }
            }

            for conn_opt in plan.steps[i].input_connections.iter_mut() {
                if let Some(conn) = conn_opt {
                    let src_lat = plan.step_cumulative_latency[conn.src_step];
                    conn.delay_samples = arrival.saturating_sub(src_lat);
                }
            }

            plan.step_cumulative_latency[i] = plan.step_latencies[i].saturating_add(arrival);
        }
    }

    // ── Topological sort (Kahn's BFS) ─────────────────────────────────────────

    fn topological_sort(graph: &AudioGraph) -> Result<Vec<NodeId>, CompileError> {
        let node_ids: Vec<NodeId> = graph.nodes.keys().copied().collect();

        // In-degree per node
        let mut in_degree: HashMap<NodeId, usize> =
            node_ids.iter().map(|&id| (id, 0)).collect();

        // Adjacency list (from → [to])
        let mut adj: HashMap<NodeId, Vec<NodeId>> =
            node_ids.iter().map(|&id| (id, Vec::new())).collect();

        for edge in &graph.edges {
            *in_degree.entry(edge.to.node).or_insert(0) += 1;
            adj.entry(edge.from.node).or_default().push(edge.to.node);
        }

        // Seed queue with zero-in-degree nodes
        let mut queue: VecDeque<NodeId> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&id, _)| id)
            .collect();

        // Sort for determinism (stable order regardless of HashMap iteration)
        let mut queue_vec: Vec<NodeId> = queue.drain(..).collect();
        queue_vec.sort_unstable();
        queue.extend(queue_vec);

        let mut order: Vec<NodeId> = Vec::with_capacity(node_ids.len());

        while let Some(node_id) = queue.pop_front() {
            order.push(node_id);

            if let Some(neighbors) = adj.get(&node_id) {
                let mut next: Vec<NodeId> = Vec::new();
                for &nb in neighbors {
                    let deg = in_degree.get_mut(&nb).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        next.push(nb);
                    }
                }
                next.sort_unstable(); // determinism
                queue.extend(next);
            }
        }

        if order.len() != node_ids.len() {
            return Err(CompileError::CycleDetected);
        }

        Ok(order)
    }

    // ── Port count helpers ────────────────────────────────────────────────────

    fn num_inputs_for_kind(kind: &NodeKind) -> usize {
        match kind {
            NodeKind::Track { .. }              => 0, // source node
            NodeKind::Instrument { .. }         => 0, // source node
            NodeKind::TrackGain { .. }          => 1,
            NodeKind::Bus                       => 8, // up to 8 mix inputs
            NodeKind::Send { .. }               => 1,
            NodeKind::Return { .. }             => 0, // collects from sends internally
            NodeKind::Gain { .. }               => 1,
            NodeKind::Pan { .. }                => 1,
            NodeKind::Plugin { .. }             => 2, // default stereo in
            NodeKind::Modulator { .. }          => 0, // modulator = source
            NodeKind::Meter { .. }              => 1,
            // num_inputs is set per-project so every track gets its own port.
            NodeKind::Master { num_inputs }     => *num_inputs,
            NodeKind::Eq { .. } | NodeKind::Compressor { .. }
            | NodeKind::Reverb { .. } | NodeKind::Delay { .. } => 1,
        }
    }

    fn num_outputs_for_kind(kind: &NodeKind) -> usize {
        match kind {
            NodeKind::Track { .. }       => 1,
            NodeKind::Instrument { .. }  => 1,
            NodeKind::TrackGain { .. } => 1,
            NodeKind::Bus              => 1,
            NodeKind::Send { .. }      => 0, // side-effect node
            NodeKind::Return { .. }    => 1,
            NodeKind::Gain { .. }      => 1,
            NodeKind::Pan { .. }       => 1,
            NodeKind::Plugin { .. }    => 2, // default stereo out
            NodeKind::Modulator { .. } => 1, // one modulation output
            NodeKind::Meter { .. }     => 1, // pass-through
            // Master produces one mixed stereo output that the scheduler reads
            // directly.  Declaring it as 1 (instead of 0) means the scheduler
            // pre-allocates the buffer and `write_master_output` can read it.
            NodeKind::Master { .. }    => 1,
            NodeKind::Eq { .. } | NodeKind::Compressor { .. }
            | NodeKind::Reverb { .. } | NodeKind::Delay { .. } => 1,
        }
    }

    // ── Node factory ─────────────────────────────────────────────────────────

    /// Instantiate the concrete `AudioNode` for a given graph node kind.
    ///
    /// `engine_sample_rate` is passed so clip positions are computed in the
    /// correct sample-rate space (avoids the old hardcoded-48 kHz bug).
    fn build_node(
        id: NodeId,
        kind: &NodeKind,
        project: &AppProject,
        engine_sample_rate: f32,
        live_params: &LiveParams,
        plugin_host: &mut PluginHost,
    ) -> Box<dyn AudioNode> {
        use crate::audio_graph::nodes;
        use std::sync::Arc as StdArc;

        match kind {
            NodeKind::Instrument { track_id } => {
                use crate::audio_graph::midi::MidiNoteSnapshot;
                use crate::audio_graph::nodes::instrument::InstrumentNode;
                use crate::audio_graph::nodes::poly_synth::PolySynth;

                let mut notes: Vec<MidiNoteSnapshot> = Vec::new();

                if let Some(track) = project.tracks.iter().find(|t| &t.id == track_id) {
                    for clip in &track.midi_clips {
                        for note in &clip.notes {
                            notes.push(MidiNoteSnapshot {
                                pitch: note.pitch,
                                velocity: note.velocity,
                                clip_start_sec: clip.start,
                                start_beats: note.start,
                                end_beats: note.start + note.duration,
                            });
                        }
                    }
                }

                Box::new(InstrumentNode::new(
                    id,
                    StdArc::new(notes),
                    Box::new(PolySynth::new()),
                ))
            }

            NodeKind::Track { track_id } => {
                // Gather clip data from the project for this track.
                // Clip positions are in seconds; convert to engine-sample-rate
                // frames so the TrackNode can compare them directly against the
                // atomic `position_samples` counter (which is also in frames).
                // `samples` is `Arc<Vec<f32>>` shared with the project — no
                // deep clone of the (potentially huge) decoded audio buffer.
                let clips = project
                    .tracks
                    .iter()
                    .find(|t| &t.id == track_id)
                    .map(|track| {
                        track.clips.iter().map(|c| nodes::track::ClipData {
                            start_sample: (c.start * engine_sample_rate as f64) as u64,
                            end_sample:   (c.end   * engine_sample_rate as f64) as u64,
                            samples: StdArc::clone(&c.raw_samples),
                            native_sample_rate: c.sample_rate,
                        }).collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                Box::new(nodes::track::TrackNode::new(id, clips))
            }

            NodeKind::TrackGain { track_id, gain } => {
                // Look up the project track so we seed `LiveParams` with the
                // current mute/solo/pan state if its entry didn't already
                // exist.  After this call, the compiler and the gateway both
                // hold `Arc<TrackParams>` pointing at the same atomics.
                let (volume, pan, muted, solo) = project
                    .tracks
                    .iter()
                    .find(|t| &t.id == track_id)
                    .map(|t| {
                        let v = (t.volume / 100.0).clamp(0.0, 2.0);
                        (v, t.pan, t.muted, t.solo)
                    })
                    .unwrap_or((*gain, 0.0, false, false));
                let cells = live_params.get_or_init_track(track_id, volume, pan, muted, solo);
                Box::new(nodes::track_gain::TrackGainNode::new(
                    id,
                    cells,
                    Arc::clone(&live_params.any_solo),
                ))
            }

            NodeKind::Gain { gain_linear } => {
                Box::new(nodes::gain::GainNode::new(id, *gain_linear))
            }

            NodeKind::Pan { position } => {
                Box::new(nodes::pan::PanNode::new(id, *position))
            }

            NodeKind::Bus => {
                Box::new(nodes::bus::BusNode::new(id, Self::num_inputs_for_kind(kind)))
            }

            NodeKind::Master { num_inputs } => {
                Box::new(nodes::master::MasterNode::new(
                    id,
                    *num_inputs,
                    Arc::clone(&live_params.master_soft_clip),
                ))
            }

            NodeKind::Meter { .. } => {
                // Handled before build_node is called (see compile loop above).
                // Fallback: silence pass-through if reached unexpectedly.
                Box::new(nodes::gain::GainNode::new(id, 1.0))
            }

            NodeKind::Send { send_gain, .. } => {
                Box::new(nodes::send::SendNode::new(id, *send_gain))
            }

            NodeKind::Return { .. } => {
                // Return node collects from a shared ring — future work
                Box::new(nodes::bus::BusNode::new(id, 1))
            }

            NodeKind::Plugin { device_id } => {
                // Hand off to `PluginHost` so the appropriate device wrapper
                // (internal, VST3, AU, CLAP, WASM) is instantiated.  The host
                // registers the resulting record so subsequent parameter
                // updates and lifecycle calls can find it.  If the device is
                // unknown (registry miss) or fails to instantiate, fall back
                // to a unity-gain pass-through so the graph still compiles.
                match plugin_host.instantiate(id, device_id) {
                    Ok(node) => node,
                    Err(e) => {
                        eprintln!(
                            "[audio] Plugin '{device_id}' instantiation failed ({e}) — pass-through utilisé"
                        );
                        Box::new(nodes::gain::GainNode::new(id, 1.0))
                    }
                }
            }

            NodeKind::Modulator { modulator_id, kind, rate_hz, waveform } => {
                use crate::audio_graph::modulation::ModulatorKind;
                match kind {
                    ModulatorKind::Lfo => {
                        let cells = live_params.get_or_init_modulator(modulator_id, *rate_hz);
                        Box::new(
                            nodes::modulator::LfoNode::new(id, *rate_hz, waveform.clone())
                                .with_rate_cells(cells),
                        )
                    }
                    _ => {
                        // Other modulator kinds not yet implemented — silent source
                        Box::new(nodes::modulator::LfoNode::new(id, 0.0, crate::project::LfoWaveform::Sine))
                    }
                }
            }

            NodeKind::Eq { params } =>
                Box::new(nodes::eq::EqNode::new(id, params.clone())),
            NodeKind::Compressor { params } =>
                Box::new(nodes::compressor::CompressorNode::new(id, params.clone())),
            NodeKind::Reverb { params } =>
                Box::new(nodes::reverb::ReverbNode::new(id, params.clone())),
            NodeKind::Delay { params } =>
                Box::new(nodes::delay::DelayNode::new(id, params.clone())),
        }
    }
}
