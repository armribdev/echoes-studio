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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{atomic::AtomicU32, Arc};

use crate::audio_graph::execution_plan::{
    ExecutionPlan, ExecutionPlanBuilder, ExecutionStep, InputConnection,
};
use crate::audio_graph::graph::{AudioGraph, NodeKind};
use crate::audio_graph::live_params::LiveParams;
use crate::audio_graph::automation::SchedulerAutomationAction;
use crate::audio_graph::node::{
    AudioNode, NodeId, PluginModEntry, PortId, PortKind, PrepareContext, MAX_CONTROL_POINTS,
};
use crate::plugin_system::PluginHost;
use crate::project::AppProject;

/// Shared peak cells for one track meter: (peak_L, peak_R).
pub type MeterCell = (Arc<AtomicU32>, Arc<AtomicU32>);

fn zero_cell() -> Arc<AtomicU32> {
    Arc::new(AtomicU32::new(0.0_f32.to_bits()))
}

const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;
const DEFAULT_MAX_BLOCK_SIZE: usize = 512;
const DEFAULT_NUM_CHANNELS: usize = 2;

/// Parameters that vary per audio device session.
#[derive(Debug, Clone, Copy)]
pub struct CompileConfig {
    pub sample_rate: f32,
    pub max_block_size: usize,
    pub num_channels: usize,
}

impl Default for CompileConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            max_block_size: DEFAULT_MAX_BLOCK_SIZE,
            num_channels: DEFAULT_NUM_CHANNELS,
        }
    }
}

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
    /// An edge declares a [`PortKind`] the compiler cannot route yet
    /// (anything other than `Audio`).  Guards Phase 5a until the MIDI /
    /// modulation paths (5c/5e) wire the producing & consuming ports.
    UnsupportedPortKind {
        from: PortId,
        to: PortId,
        kind: PortKind,
    },
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
            CompileError::UnsupportedPortKind { from, to, kind } => write!(
                f,
                "edge {:?} → {:?} declares port kind {:?}, which the compiler cannot route yet \
                 (only Audio is supported in Phase 5a)",
                from, to, kind
            ),
        }
    }
}

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
    /// Pre-compiled automation actions, consumed by `GraphScheduler::new`.
    pub automation_actions: Vec<SchedulerAutomationAction>,
}

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
        if graph.master_id.is_none() {
            return Err(CompileError::NoMasterNode);
        }
        Self::validate_port_kinds(graph)?;

        let order = Self::topological_sort(graph)?;

        let step_index: HashMap<NodeId, usize> =
            order.iter().enumerate().map(|(i, &id)| (id, i)).collect();

        let mut plan_builder = ExecutionPlanBuilder::new(
            config.sample_rate,
            config.max_block_size,
            config.num_channels,
        );

        let mut steps: Vec<ExecutionStep> = Vec::with_capacity(order.len());

        for &node_id in &order {
            let graph_node = graph
                .nodes
                .get(&node_id)
                .ok_or(CompileError::MissingNode(node_id))?;

            // Sidechain aux inputs from scan manifest (Phase 5d); 0 until bus introspection.
            let num_inputs = {
                let base = Self::num_inputs_for_kind(&graph_node.kind);
                match &graph_node.kind {
                    NodeKind::Plugin { device_id, .. } => {
                        base + plugin_host.sidechain_input_ports(device_id)
                    }
                    _ => base,
                }
            };
            let num_outputs = Self::num_outputs_for_kind(&graph_node.kind);

            let mut input_connections: Vec<Option<InputConnection>> = vec![None; num_inputs];

            for edge in graph.edges_into(node_id) {
                let dst_port = edge.to.port as usize;
                if dst_port < num_inputs {
                    let src_step = *step_index
                        .get(&edge.from.node)
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

        plan_builder.set_modulation(graph.modulation.clone());
        let mut plan = plan_builder.build();

        let prepare_ctx = PrepareContext {
            sample_rate: config.sample_rate,
            max_block_size: config.max_block_size,
            num_channels: config.num_channels,
        };

        let mut processors: Vec<Box<dyn AudioNode>> = Vec::with_capacity(order.len());
        let mut meter_cells: HashMap<String, MeterCell> = HashMap::new();

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
                Box::new(crate::audio_graph::nodes::meter::MeterNode::new(
                    node_id, peak_l, peak_r,
                ))
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

        // Wire EnvelopeCells from envelope modulators to InstrumentNodes on the same track.
        // Both sides hold the same Arc, allowing the InstrumentNode to write the gate
        // state on the audio thread and the EnvelopeNode to read it lock-free.
        {
            use crate::project::ModulatorParams;

            let mut track_to_envelope_cells: HashMap<
                &str,
                Vec<Arc<crate::audio_graph::live_params::EnvelopeCells>>,
            > = HashMap::new();
            for track in &project.tracks {
                for m in &track.modulators {
                    if let ModulatorParams::Envelope(params) = &m.params {
                        let cells = live_params.get_or_init_envelope(&m.id, params);
                        track_to_envelope_cells
                            .entry(track.id.as_str())
                            .or_default()
                            .push(cells);
                    }
                }
            }

            for (step_idx, &node_id) in order.iter().enumerate() {
                if let NodeKind::Instrument { track_id } = &graph.nodes[&node_id].kind {
                    if let Some(cells) = track_to_envelope_cells.get(track_id.as_str()) {
                        processors[step_idx].inject_envelope_cells(cells.clone());
                    }
                }
            }
        }

        // Inject plugin modulation entries into plugin wrapper nodes.
        // For each modulation route targeting a Plugin node (identified by the
        // "plugin_native:{id}" sentinel in param.name), create a shadow cell in
        // LiveParams and hand a pre-allocated Vec<PluginModEntry> to the wrapper.
        // This allocates once here (command thread) so process() is allocation-free.
        {
            let node_instance_keys: HashMap<NodeId, &str> = graph
                .nodes
                .values()
                .filter_map(|n| {
                    if let NodeKind::Plugin { instance_key, .. } = &n.kind {
                        Some((n.id, instance_key.as_str()))
                    } else {
                        None
                    }
                })
                .collect();

            let mut entries_by_step: HashMap<usize, HashMap<u32, PluginModEntry>> = HashMap::new();
            for route in graph.modulation.all_routes() {
                let target_node_id = route.target.param.node;
                let Some(&instance_key) = node_instance_keys.get(&target_node_id) else {
                    continue;
                };
                let Some(native_id) = route
                    .target
                    .param
                    .name
                    .strip_prefix("plugin_native:")
                    .and_then(|s| s.parse::<u32>().ok())
                else {
                    continue;
                };
                let step = step_index[&target_node_id];
                let shadow = live_params.get_or_init_plugin_param_shadow(instance_key, native_id, 0.5);
                entries_by_step
                    .entry(step)
                    .or_default()
                    .entry(native_id)
                    .or_insert_with(|| PluginModEntry {
                        native_id,
                        delta: 0.0,
                        shadow,
                        points: [(0, 0.0); MAX_CONTROL_POINTS],
                        point_count: 0,
                    });
            }
            for (step, map) in entries_by_step {
                let entries: Vec<PluginModEntry> = map.into_values().collect();
                processors[step].inject_plugin_mod_entries(entries);
            }
        }

        // Resolve automation actions: NodeId → step index + param_id.
        let automation_actions: Vec<SchedulerAutomationAction> = graph
            .automation_actions
            .iter()
            .filter_map(|action| {
                let step = *step_index.get(&action.target_node_id)?;
                let param_id = processors[step].param_id(&action.param_name)?;
                Some(SchedulerAutomationAction {
                    target_step: step,
                    param_id,
                    curve: Arc::clone(&action.curve),
                })
            })
            .collect();

        Self::compute_latency_compensation(&mut plan, &processors);
        Self::compute_execution_levels(&mut plan);

        // ADR-007 removal path: drop host records for placements no longer in the graph.
        // Keyed by instance_key (per-placement id), not device_id: two placements of the
        // same plugin type have distinct keys, so removing one does not keep the other's
        // orphaned record alive.
        let mut active_keys: HashSet<String> = HashSet::new();
        for graph_node in graph.nodes.values() {
            match &graph_node.kind {
                NodeKind::Plugin { instance_key, .. } => {
                    active_keys.insert(instance_key.clone());
                }
                NodeKind::Instrument { track_id } => {
                    let has_plugin = project
                        .tracks
                        .iter()
                        .find(|t| &t.id == track_id)
                        .and_then(|t| t.instrument.as_ref())
                        .is_some();
                    if has_plugin {
                        active_keys.insert(format!("instrument:{track_id}"));
                    }
                }
                _ => {}
            }
        }
        plugin_host.retain_devices(&active_keys);

        let plan = Arc::new(plan);

        Ok(CompiledGraph {
            plan,
            processors,
            meter_cells,
            route_cells,
            automation_actions,
        })
    }

    /// Compute and assign latency compensation delays into `plan.steps[*].input_connections`.
    ///
    /// Walks the plan in topological order (already enforced by the compiler) and:
    ///   - records each step's intrinsic latency from `node.latency_samples()`.
    ///   - computes `cumulative_latency[i]` = `intrinsic[i] + max input arrival`.
    ///   - for each input port `p`, sets `delay_samples` so all converging
    ///     inputs at step `i` arrive aligned at the maximum input arrival time.
    fn compute_latency_compensation(plan: &mut ExecutionPlan, processors: &[Box<dyn AudioNode>]) {
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
            let mut arrival: u32 = 0;
            for conn in plan.steps[i].input_connections.iter().flatten() {
                let src_lat = plan.step_cumulative_latency[conn.src_step];
                if src_lat > arrival {
                    arrival = src_lat;
                }
            }

            for conn in plan.steps[i].input_connections.iter_mut().flatten() {
                let src_lat = plan.step_cumulative_latency[conn.src_step];
                conn.delay_samples = arrival.saturating_sub(src_lat);
            }

            plan.step_cumulative_latency[i] = plan.step_latencies[i].saturating_add(arrival);
        }
    }

    /// Assign each step a "level" index such that any two steps at the same
    /// level have no transitive dependency on each other and can be processed
    /// in parallel.
    ///
    /// The dependency relation includes:
    ///   - audio edges (`input_connections[*].src_step`)
    ///   - modulation edges (`route.source.node → route.target.param.node`)
    ///
    /// Source steps with no dependencies sit at level 0; every other step is
    /// placed at `1 + max(level[dep])`.  Steps are then grouped per level
    /// (ascending step index for determinism) and stored on the plan.
    pub(crate) fn compute_execution_levels(plan: &mut ExecutionPlan) {
        let n = plan.steps.len();
        plan.step_levels.clear();
        plan.step_levels.resize(n, 0);
        plan.levels.clear();
        if n == 0 {
            return;
        }

        let node_to_step: HashMap<NodeId, usize> = plan
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.node_id, i))
            .collect();

        // Fold modulation deps into level assignment (scheduler reads sources from prior levels).
        let mut mod_deps: Vec<Vec<usize>> = vec![Vec::new(); n];
        for route in plan.modulation.all_routes() {
            let (Some(&src_step), Some(&tgt_step)) = (
                node_to_step.get(&route.source.node),
                node_to_step.get(&route.target.param.node),
            ) else {
                continue;
            };
            if src_step != tgt_step {
                mod_deps[tgt_step].push(src_step);
            }
        }

        for i in 0..n {
            let mut lvl: u32 = 0;
            for conn in plan.steps[i].input_connections.iter().flatten() {
                let dl = plan.step_levels[conn.src_step].saturating_add(1);
                if dl > lvl {
                    lvl = dl;
                }
            }
            for &src in &mod_deps[i] {
                let dl = plan.step_levels[src].saturating_add(1);
                if dl > lvl {
                    lvl = dl;
                }
            }
            plan.step_levels[i] = lvl;
        }

        let num_levels = (*plan.step_levels.iter().max().unwrap_or(&0) as usize) + 1;
        plan.levels = vec![Vec::new(); num_levels];
        for (i, &lvl) in plan.step_levels.iter().enumerate() {
            plan.levels[lvl as usize].push(i);
        }
    }

    /// Validate that every edge carries a routable [`PortKind`] (ADR-008, 5a).
    ///
    /// Today only `Audio` is routable.  A non-`Audio` edge means a later phase
    /// produced a typed edge before its node-side ports exist — we fail fast
    /// rather than silently treating a note stream as audio.  With all edges
    /// defaulting to `Audio` (legacy path) this pass is a no-op, preserving the
    /// bit-identical guarantee (C-2).
    fn validate_port_kinds(graph: &AudioGraph) -> Result<(), CompileError> {
        for edge in &graph.edges {
            match edge.kind {
                PortKind::Audio { .. } => {}
                other => {
                    return Err(CompileError::UnsupportedPortKind {
                        from: edge.from,
                        to: edge.to,
                        kind: other,
                    });
                }
            }
        }
        Ok(())
    }

    fn topological_sort(graph: &AudioGraph) -> Result<Vec<NodeId>, CompileError> {
        let node_ids: Vec<NodeId> = graph.nodes.keys().copied().collect();

        let mut in_degree: HashMap<NodeId, usize> = node_ids.iter().map(|&id| (id, 0)).collect();
        let mut adj: HashMap<NodeId, Vec<NodeId>> =
            node_ids.iter().map(|&id| (id, Vec::new())).collect();

        for edge in &graph.edges {
            *in_degree.entry(edge.to.node).or_insert(0) += 1;
            adj.entry(edge.from.node).or_default().push(edge.to.node);
        }

        let mut queue: VecDeque<NodeId> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&id, _)| id)
            .collect();

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
                next.sort_unstable();
                queue.extend(next);
            }
        }

        if order.len() != node_ids.len() {
            return Err(CompileError::CycleDetected);
        }

        Ok(order)
    }

    fn num_inputs_for_kind(kind: &NodeKind) -> usize {
        match kind {
            NodeKind::Track { .. } => 0,      // source node
            NodeKind::Instrument { .. } => 0, // source node
            NodeKind::TrackGain { .. } => 1,
            NodeKind::Bus => 8, // up to 8 mix inputs
            NodeKind::Send { .. } => 1,
            NodeKind::Return { .. } => 0, // collects from sends internally
            NodeKind::Gain { .. } => 1,
            NodeKind::Pan { .. } => 1,
            NodeKind::Plugin { .. } => 2,    // default stereo in
            NodeKind::Modulator { kind, .. } => match kind {
                crate::audio_graph::modulation::ModulatorKind::AudioFollower => 1,
                _ => 0,
            },
            NodeKind::Meter { .. } => 1,
            NodeKind::Master { num_inputs } => *num_inputs,
            NodeKind::Eq { .. }
            | NodeKind::Compressor { .. }
            | NodeKind::Reverb { .. }
            | NodeKind::Delay { .. } => 1,
        }
    }

    fn num_outputs_for_kind(kind: &NodeKind) -> usize {
        match kind {
            NodeKind::Track { .. } => 1,
            NodeKind::Instrument { .. } => 1,
            NodeKind::TrackGain { .. } => 1,
            NodeKind::Bus => 1,
            NodeKind::Send { .. } => 1,
            NodeKind::Return { .. } => 1,
            NodeKind::Gain { .. } => 1,
            NodeKind::Pan { .. } => 1,
            NodeKind::Plugin { .. } => 2,    // default stereo out
            NodeKind::Modulator { .. } => 1, // one modulation output
            NodeKind::Meter { .. } => 1,     // pass-through
            // Master produces one mixed stereo output that the scheduler reads
            // directly.  Declaring it as 1 (instead of 0) means the scheduler
            // pre-allocates the buffer and `write_master_output` can read it.
            NodeKind::Master { .. } => 1,
            NodeKind::Eq { .. }
            | NodeKind::Compressor { .. }
            | NodeKind::Reverb { .. }
            | NodeKind::Delay { .. } => 1,
        }
    }

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
                use crate::audio_graph::nodes::instrument::{InstrumentDevice, InstrumentNode, SilentInstrument};
                use crate::audio_graph::nodes::poly_synth::PolySynth;

                let mut notes: Vec<MidiNoteSnapshot> = Vec::new();
                let mut instrument_id: Option<&str> = None;

                if let Some(track) = project.tracks.iter().find(|t| &t.id == track_id) {
                    for clip in &track.midi_clips {
                        // Notes anchor to the content origin (`start - offset`) so a
                        // left-trim hides leading notes without moving the rest. The
                        // playable window `[start, end]` (seconds, tempo-independent)
                        // gates which notes sound.
                        let content_origin = clip.start - clip.offset;
                        let window_start_sample = (clip.start * engine_sample_rate as f64) as u64;
                        let window_end_sample = (clip.end * engine_sample_rate as f64) as u64;
                        for note in &clip.notes {
                            notes.push(MidiNoteSnapshot {
                                pitch: note.pitch,
                                velocity: note.velocity,
                                clip_start_sec: content_origin,
                                start_beats: note.start,
                                end_beats: note.start + note.duration,
                                window_start_sample,
                                window_end_sample,
                            });
                        }
                    }
                    instrument_id = track.instrument.as_deref();
                }

                // Phase 5c: CLAP instrument, built-in PolySynth when none is chosen.
                // A chosen plugin that fails to load stays silent (the host reports
                // it as unavailable) rather than being swapped for another sound.
                let instr_key = format!("instrument:{track_id}");
                let device: Box<dyn InstrumentDevice> = match instrument_id {
                    Some(device_id) => match plugin_host
                        .instantiate_instrument(&instr_key, device_id)
                    {
                        Ok(dev) => dev,
                        Err(e) => {
                            eprintln!("[audio] Instrument '{device_id}' unavailable ({e}) — track silent");
                            Box::new(SilentInstrument)
                        }
                    },
                    None => Box::new(PolySynth::new()),
                };

                Box::new(InstrumentNode::new(id, StdArc::new(notes), device))
            }

            NodeKind::Track { track_id } => {
                let clips = project
                    .tracks
                    .iter()
                    .find(|t| &t.id == track_id)
                    .map(|track| {
                        track
                            .clips
                            .iter()
                            .map(|c| nodes::track::ClipData {
                                start_sample: (c.start * engine_sample_rate as f64) as u64,
                                end_sample: (c.end * engine_sample_rate as f64) as u64,
                                offset_sample: (c.offset * engine_sample_rate as f64) as u64,
                                samples: StdArc::clone(&c.raw_samples),
                                native_sample_rate: c.sample_rate,
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                Box::new(nodes::track::TrackNode::new(id, clips))
            }

            NodeKind::TrackGain { track_id, gain } => {
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

            NodeKind::Pan { position } => Box::new(nodes::pan::PanNode::new(id, *position)),

            NodeKind::Bus => Box::new(nodes::bus::BusNode::new(
                id,
                Self::num_inputs_for_kind(kind),
            )),

            NodeKind::Master { num_inputs } => Box::new(nodes::master::MasterNode::new(
                id,
                *num_inputs,
                Arc::clone(&live_params.master_soft_clip),
            )),

            NodeKind::Meter { .. } => Box::new(nodes::gain::GainNode::new(id, 1.0)),

            NodeKind::Send { send_gain, .. } => {
                Box::new(nodes::send::SendNode::new(id, *send_gain))
            }

            NodeKind::Return { .. } => Box::new(nodes::bus::BusNode::new(id, 1)),

            NodeKind::Plugin {
                instance_key,
                device_id,
                sandboxed,
            } => {
                if *sandboxed {
                    if let Some(node) = crate::plugin_system::bridged_node::BridgedNode::try_new(
                        id,
                        device_id,
                        plugin_host,
                    ) {
                        return node;
                    }
                    eprintln!(
                        "[audio] Plugin '{device_id}' marked sandboxed but cannot be bridged — \
                         using in-process host"
                    );
                }

                match plugin_host.instantiate(id, instance_key, device_id, *sandboxed) {
                    Ok(node) => node,
                    Err(e) => {
                        eprintln!(
                            "[audio] Plugin '{device_id}' instantiation failed ({e}) — pass-through utilisé"
                        );
                        Box::new(nodes::gain::GainNode::new(id, 1.0))
                    }
                }
            }

            NodeKind::Modulator {
                modulator_id,
                kind,
                rate_hz,
                waveform,
                initial_value,
                initial_envelope,
                initial_follower,
                tempo_sync,
                beat_division,
            } => {
                use crate::audio_graph::modulation::ModulatorKind;
                match kind {
                    ModulatorKind::Lfo => {
                        let cells = live_params.get_or_init_modulator(
                            modulator_id,
                            *rate_hz,
                            *tempo_sync,
                            *beat_division,
                        );
                        Box::new(
                            nodes::modulator::LfoNode::new(id, *rate_hz, waveform.clone())
                                .with_rate_cells(cells),
                        )
                    }
                    ModulatorKind::Macro => {
                        let cells = live_params.get_or_init_macro(modulator_id, *initial_value);
                        Box::new(nodes::modulator::MacroNode::new(id, cells))
                    }
                    ModulatorKind::Envelope => {
                        let cells = live_params.get_or_init_envelope(modulator_id, initial_envelope);
                        Box::new(nodes::modulator::EnvelopeNode::new(id, cells))
                    }
                    ModulatorKind::AudioFollower => {
                        Box::new(nodes::modulator::AudioFollowerNode::new(id, initial_follower.clone()))
                    }
                    _ => Box::new(nodes::modulator::LfoNode::new(
                        id,
                        0.0,
                        crate::project::LfoWaveform::Sine,
                    )),
                }
            }

            NodeKind::Eq { effect_id, params } => {
                let block = live_params
                    .get_or_init_effect_block(effect_id, &nodes::eq::eq_block_values(params));
                Box::new(nodes::eq::EqNode::new(id, params.clone()).with_block(block))
            }
            NodeKind::Compressor { effect_id, params } => {
                let block = live_params.get_or_init_effect_block(
                    effect_id,
                    &nodes::compressor::compressor_block_values(params),
                );
                Box::new(
                    nodes::compressor::CompressorNode::new(id, params.clone()).with_block(block),
                )
            }
            NodeKind::Reverb { effect_id, params } => {
                let block = live_params.get_or_init_effect_block(
                    effect_id,
                    &nodes::reverb::reverb_block_values(params),
                );
                Box::new(nodes::reverb::ReverbNode::new(id, params.clone()).with_block(block))
            }
            NodeKind::Delay { effect_id, params } => {
                let block = live_params
                    .get_or_init_effect_block(effect_id, &nodes::delay::delay_block_values(params));
                Box::new(nodes::delay::DelayNode::new(id, params.clone()).with_block(block))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::graph::AudioGraph;

    fn two_node_graph() -> (AudioGraph, NodeId, NodeId) {
        let mut g = AudioGraph::new();
        let a = g.add_node(
            NodeKind::Meter {
                meter_id: "a".into(),
            },
            "a",
        );
        let b = g.add_node(
            NodeKind::Meter {
                meter_id: "b".into(),
            },
            "b",
        );
        (g, a, b)
    }

    #[test]
    fn default_audio_edges_pass_validation() {
        let (mut g, a, b) = two_node_graph();
        g.connect(a, 0, b, 0);
        assert_eq!(GraphCompiler::validate_port_kinds(&g), Ok(()));
    }

    #[test]
    fn midi_edge_is_rejected_until_phase_5c() {
        let (mut g, a, b) = two_node_graph();
        g.connect_typed(a, 0, b, 0, PortKind::Midi);
        assert!(matches!(
            GraphCompiler::validate_port_kinds(&g),
            Err(CompileError::UnsupportedPortKind {
                kind: PortKind::Midi,
                ..
            })
        ));
    }

    #[test]
    fn modulation_edge_is_rejected_until_phase_5e() {
        use crate::audio_graph::node::Polarity;
        let (mut g, a, b) = two_node_graph();
        g.connect_typed(
            a,
            0,
            b,
            0,
            PortKind::Modulation {
                polarity: Polarity::Bipolar,
            },
        );
        assert!(matches!(
            GraphCompiler::validate_port_kinds(&g),
            Err(CompileError::UnsupportedPortKind { .. })
        ));
    }

    /// End-to-end runtime proof: a `NodeKind::Plugin` is (1) instantiated by the
    /// host, (2) registered, (3) inserted into the compiled plan as a processor,
    /// and (4) executed by the scheduler without panic.  Uses the built-in
    /// `echoes.gain` (PassThroughDevice) so no real plugin DLL is required.
    #[test]
    fn plugin_node_is_instantiated_inserted_and_scheduled() {
        use crate::audio_graph::node::TransportInfo;
        use crate::audio_graph::scheduler::GraphScheduler;
        use crate::plugin_system::PluginHost;
        use crate::project::AppProject;

        let mut host = PluginHost::new();
        let live = LiveParams::new();
        let project = AppProject::new();

        let mut g = AudioGraph::new();
        let master = g.add_node(NodeKind::Master { num_inputs: 1 }, "Master");
        let plug = g.add_node(
            NodeKind::Plugin {
                instance_key: "fx0".into(),
                device_id: "echoes.gain".into(),
                sandboxed: false,
            },
            "plug",
        );
        g.connect(plug, 0, master, 0);

        let compiled =
            GraphCompiler::compile(&g, &project, CompileConfig::default(), &live, &mut host)
                .expect("compile must succeed");

        assert!(host.instance("echoes.gain").is_some());
        assert!(compiled.processors.iter().any(|n| n.label() == "PassThrough"));

        let mut sched = GraphScheduler::new(compiled);
        let block = 16usize;
        let mut out = vec![0.0_f32; block * 2];
        let transport = TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: 48_000.0,
            block_size: block,
        };
        sched.process_block(&transport, &mut out);
    }

    /// Recompiling a graph that no longer contains the plugin must prune the
    /// host record (the teardown path that previously leaked).
    #[test]
    fn recompile_without_plugin_prunes_host_record() {
        use crate::plugin_system::PluginHost;
        use crate::project::AppProject;

        let mut host = PluginHost::new();
        let live = LiveParams::new();
        let project = AppProject::new();

        let mut g1 = AudioGraph::new();
        let m1 = g1.add_node(NodeKind::Master { num_inputs: 1 }, "Master");
        let p1 = g1.add_node(
            NodeKind::Plugin {
                instance_key: "fx0".into(),
                device_id: "echoes.gain".into(),
                sandboxed: false,
            },
            "plug",
        );
        g1.connect(p1, 0, m1, 0);
        GraphCompiler::compile(&g1, &project, CompileConfig::default(), &live, &mut host)
            .expect("compile A");
        assert!(host.instance("echoes.gain").is_some());

        let mut g2 = AudioGraph::new();
        g2.add_node(NodeKind::Master { num_inputs: 1 }, "Master");
        GraphCompiler::compile(&g2, &project, CompileConfig::default(), &live, &mut host)
            .expect("compile B");

        assert!(
            host.instance("echoes.gain").is_none(),
            "stale plugin record must be pruned on recompile"
        );
        assert_eq!(host.device_count(), 0);
    }
}
