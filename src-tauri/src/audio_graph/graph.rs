//! Audio graph topology — the editor-side representation.
//!
//! `AudioGraph` stores nodes and edges as a directed acyclic graph (DAG).  It
//! is built from `AppProject` state by `AudioGraph::from_project` and then
//! compiled into an immutable `ExecutionPlan` by `GraphCompiler`.
//!
//! ## Separation of concerns
//!
//! - `AudioGraph`       — **what** the graph contains (topology, node kinds)
//! - `GraphCompiler`    — **how** to execute it (topological order, buffer layout)
//! - `GraphScheduler`   — **when** to execute it (audio callback driver)
//!
//! The graph is **plugin-format agnostic**.  It knows `NodeKind::Plugin { id }`
//! but does not know VST3, AU, or WASM.  The `GraphCompiler` asks the
//! `PluginHost` to produce a concrete `Box<dyn AudioNode>` wrapper for each
//! plugin node.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::audio_graph::modulation::{
    ModulationMap, ModulationRoute, ModulationSource, ModulationTarget, ModulatorKind, ParamId,
};
use crate::audio_graph::node::{NodeId, PortId, PortKind};
use crate::project::{AppProject, EffectParams, EnvelopeParams, LfoWaveform, ModulationTargetRef, ModulatorParams};

static NEXT_NODE_ID: AtomicU64 = AtomicU64::new(1);

fn alloc_node_id() -> NodeId {
    NodeId(NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed))
}

/// Describes the DSP role of a graph node.
///
/// The `GraphCompiler` maps each kind to a concrete `Box<dyn AudioNode>`.
#[derive(Debug, Clone)]
pub enum NodeKind {
    /// Reads timeline clips for one track and outputs audio.
    Track { track_id: String },

    /// Reads MIDI clips for one track, synthesises audio via a built-in instrument.
    Instrument { track_id: String },

    /// Applies gain and panning for one track.
    TrackGain {
        track_id: String,
        /// Volume 0.0–1.0 (linear).
        gain: f32,
    },

    /// Mixes multiple inputs to one output.
    Bus,

    /// Send tap: copies audio to a return bus at a given level.
    Send {
        /// Must match the id of a corresponding `Return` node.
        return_id: String,
        send_gain: f32,
    },

    /// Receives audio from all `Send` nodes with the matching `return_id`.
    Return { return_id: String },

    /// Per-sample gain (linear multiplier, handles both gain and volume faders).
    Gain { gain_linear: f32 },

    /// Stereo panner (−1.0 = hard left, 0.0 = centre, +1.0 = hard right).
    Pan { position: f32 },

    /// Wraps an external or internal plugin.
    Plugin {
        /// Stable, per-placement instance key (= project `effect.id`).  Unlike
        /// `device_id` (the plugin *type*, shared by every use), this is unique
        /// per plugin slot and stable across recompiles, so the `PluginHost` can
        /// key its instance records by it instead of by type — the foundation
        /// for de-duplicating same-plugin-on-two-tracks and for instance reuse
        /// (engine audit points 1-2).
        instance_key: String,
        /// Opaque device ID looked up in `PluginHost`.
        device_id: String,
        /// Per-device out-of-process sandbox opt-in (ADR-015, Phase 6).
        /// Carried from `PluginParams.sandboxed` to the host's `instantiate`
        /// decision point.  **Accepted but inert** until step 6d branches on
        /// it; today every plugin is instantiated in-process regardless.
        sandboxed: bool,
    },

    /// Modulation source (LFO, envelope, macro, audio-rate).
    Modulator {
        /// Project-level modulator ID — used by the compiler to look up the
        /// matching `LiveParams` cells so parameter changes don't trigger a recompile.
        modulator_id: String,
        kind: crate::audio_graph::modulation::ModulatorKind,
        /// Oscillation rate in Hz (LFO only; ignored by other kinds).
        rate_hz: f32,
        /// Waveform shape (LFO only; ignored by other kinds).
        waveform: crate::project::LfoWaveform,
        /// Initial value used to seed `MacroCells` (Macro only; ignored by LFO).
        initial_value: f32,
        /// ADSR seed params (Envelope only; default for other kinds).
        initial_envelope: EnvelopeParams,
        /// Whether the LFO rate is derived from BPM (LFO only).
        tempo_sync: bool,
        /// Beats per LFO cycle when `tempo_sync` is true (LFO only).
        beat_division: f32,
    },

    /// Peak / RMS meter (side-effect: writes to a meter ring-buffer for the UI).
    Meter { meter_id: String },

    /// The master output node — final stereo sum before device output.
    ///
    /// `num_inputs` equals the number of tracks so each track gets its own
    /// dedicated input port.  This avoids the "all-to-port-0 overwrite" bug.
    Master { num_inputs: usize },

    // `effect_id` is the project-level effect identifier.  The compiler uses it
    // to look up the matching `LiveParams::ParameterBlock` (ADR-009) so hot
    // parameter edits reach the node without a recompile.  Only `Eq` consumes a
    // block today; the field is carried on every variant so the remaining
    // effects migrate variant-by-variant without re-touching this enum.
    Eq {
        effect_id: String,
        params: crate::project::EqParams,
    },
    Compressor {
        effect_id: String,
        params: crate::project::CompressorParams,
    },
    Reverb {
        effect_id: String,
        params: crate::project::ReverbParams,
    },
    Delay {
        effect_id: String,
        params: crate::project::DelayParams,
    },
}

#[derive(Debug, Clone)]
pub struct GraphNode {
    pub id: NodeId,
    pub kind: NodeKind,
    pub label: String,
}

/// A directed connection from one port to another.
///
/// Convention: signal flows  `from` → `to`.
///
/// ## Port typing (ADR-008 / Phase 5a)
///
/// Every edge carries a [`PortKind`].  Legacy connections created via
/// [`AudioGraph::connect`] default to `Audio { channels: 2 }`, which keeps the
/// compiled graph bit-identical to the pre-typing behaviour (target C-2).
/// Typed edges (`Midi`, `Modulation`) are created via
/// [`AudioGraph::connect_typed`] — wired up in Phase 5c/5e once nodes declare
/// real MIDI / modulation ports.  Until then the compiler's
/// `validate_port_kinds` pass rejects any non-`Audio` edge as a guard.
///
/// `Edge` is **not** serialised — it is rebuilt from `AppProject` on every
/// `from_project`, so adding `kind` does not affect the project file format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub from: PortId,
    pub to: PortId,
    pub kind: PortKind,
}

/// The complete audio graph topology (editor-side, non-real-time).
pub struct AudioGraph {
    pub(crate) nodes: HashMap<NodeId, GraphNode>,
    pub(crate) edges: Vec<Edge>,
    /// The single master output node.
    pub master_id: Option<NodeId>,
    /// Modulation routing table — built by `from_project`, consumed by the compiler.
    pub(crate) modulation: ModulationMap,
}

impl AudioGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            master_id: None,
            modulation: ModulationMap::new(),
        }
    }

    pub fn add_node(&mut self, kind: NodeKind, label: impl Into<String>) -> NodeId {
        let id = alloc_node_id();
        let is_master = matches!(kind, NodeKind::Master { .. });
        self.nodes.insert(
            id,
            GraphNode {
                id,
                kind,
                label: label.into(),
            },
        );
        if is_master {
            self.master_id = Some(id);
        }
        id
    }

    /// Connect output port `from_port` of `from_node` to input port `to_port`
    /// of `to_node`.  Returns the created edge.
    ///
    /// The edge defaults to `PortKind::Audio { channels: 2 }` — the legacy
    /// behaviour.  Use [`AudioGraph::connect_typed`] for MIDI / modulation
    /// edges (Phase 5c/5e).
    pub fn connect(
        &mut self,
        from_node: NodeId,
        from_port: u32,
        to_node: NodeId,
        to_port: u32,
    ) -> Edge {
        self.connect_typed(
            from_node,
            from_port,
            to_node,
            to_port,
            PortKind::default_audio_stereo(),
        )
    }

    /// Connect two ports with an explicit [`PortKind`].
    ///
    /// Phase 5a introduces the typed edge.  The compiler's `validate_port_kinds`
    /// pass currently rejects any `kind` other than `Audio`; the `Midi` /
    /// `Modulation` paths are enabled in later sub-phases once the producing
    /// and consuming nodes declare matching ports.
    pub fn connect_typed(
        &mut self,
        from_node: NodeId,
        from_port: u32,
        to_node: NodeId,
        to_port: u32,
        kind: PortKind,
    ) -> Edge {
        let edge = Edge {
            from: PortId {
                node: from_node,
                port: from_port,
            },
            to: PortId {
                node: to_node,
                port: to_port,
            },
            kind,
        };
        self.edges.push(edge.clone());
        edge
    }

    pub fn remove_node(&mut self, id: NodeId) {
        self.nodes.remove(&id);
        self.edges.retain(|e| e.from.node != id && e.to.node != id);
        if self.master_id == Some(id) {
            self.master_id = None;
        }
    }

    pub fn remove_edge(&mut self, edge: &Edge) {
        self.edges.retain(|e| e != edge);
    }

    pub fn node(&self, id: NodeId) -> Option<&GraphNode> {
        self.nodes.get(&id)
    }

    pub fn edges_into(&self, node: NodeId) -> impl Iterator<Item = &Edge> {
        self.edges.iter().filter(move |e| e.to.node == node)
    }

    pub fn edges_from(&self, node: NodeId) -> impl Iterator<Item = &Edge> {
        self.edges.iter().filter(move |e| e.from.node == node)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Iterate all nodes (for UI snapshot export).
    pub fn all_nodes(&self) -> impl Iterator<Item = &GraphNode> {
        self.nodes.values()
    }

    /// All audio edges in insertion order.
    pub fn all_edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Modulation routes compiled into this graph topology.
    pub fn modulation_routes(&self) -> &[ModulationRoute] {
        self.modulation.all_routes()
    }

    /// Construct a signal-flow graph from an `AppProject`.
    ///
    /// Signal flow per track:
    ///
    /// ```text
    /// TrackNode ──► [FX chain] ──► TrackGainNode ──► MeterNode ──► MasterNode
    ///                                   ▲
    ///                              LFO/Env nodes (modulation edges, not audio edges)
    /// ```
    pub fn from_project(project: &AppProject) -> Self {
        let mut graph = AudioGraph::new();

        let num_track_inputs = project.tracks.len().max(1);
        let master_id = graph.add_node(
            NodeKind::Master {
                num_inputs: num_track_inputs,
            },
            "Master",
        );

        let any_solo = project.tracks.iter().any(|t| t.solo);

        for (port_idx, track) in project.tracks.iter().enumerate() {
            let track_node = if !track.clips.is_empty() && !track.midi_clips.is_empty() {
                // Track has both audio and MIDI clips — run each through its own
                // source node then sum them through a Bus before the FX chain.
                let audio_node = graph.add_node(
                    NodeKind::Track { track_id: track.id.clone() },
                    format!("{} [clips]", track.name),
                );
                let instr_node = graph.add_node(
                    NodeKind::Instrument { track_id: track.id.clone() },
                    format!("{} [instrument]", track.name),
                );
                let bus_node = graph.add_node(NodeKind::Bus, format!("{} [bus]", track.name));
                graph.connect(audio_node, 0, bus_node, 0);
                graph.connect(instr_node, 0, bus_node, 1);
                bus_node
            } else if !track.midi_clips.is_empty() {
                graph.add_node(
                    NodeKind::Instrument {
                        track_id: track.id.clone(),
                    },
                    format!("{} [instrument]", track.name),
                )
            } else {
                graph.add_node(
                    NodeKind::Track {
                        track_id: track.id.clone(),
                    },
                    format!("{} [clips]", track.name),
                )
            };

            let effectively_muted = track.muted || (any_solo && !track.solo);
            let gain_linear = if effectively_muted {
                0.0
            } else {
                (track.volume / 100.0).clamp(0.0, 1.0)
            };

            let mut effect_node_ids: HashMap<String, NodeId> = HashMap::new();

            let mut last_node = track_node;
            for effect in &track.effects {
                if !effect.enabled {
                    continue;
                }
                let kind = match &effect.params {
                    EffectParams::Eq(p) => NodeKind::Eq {
                        effect_id: effect.id.clone(),
                        params: p.clone(),
                    },
                    EffectParams::Compressor(p) => NodeKind::Compressor {
                        effect_id: effect.id.clone(),
                        params: p.clone(),
                    },
                    EffectParams::Reverb(p) => NodeKind::Reverb {
                        effect_id: effect.id.clone(),
                        params: p.clone(),
                    },
                    EffectParams::Delay(p) => NodeKind::Delay {
                        effect_id: effect.id.clone(),
                        params: p.clone(),
                    },
                    EffectParams::Plugin(p) => NodeKind::Plugin {
                        instance_key: effect.id.clone(),
                        device_id: p.plugin_id.clone(),
                        sandboxed: p.sandboxed,
                    },
                };
                let fx_node = graph.add_node(kind, format!("{} [fx:{}]", track.name, effect.id));
                effect_node_ids.insert(effect.id.clone(), fx_node);
                graph.connect(last_node, 0, fx_node, 0);
                last_node = fx_node;
            }

            let gain_node = graph.add_node(
                NodeKind::TrackGain {
                    track_id: track.id.clone(),
                    gain: gain_linear,
                },
                format!("{} [vol]", track.name),
            );
            let meter_node = graph.add_node(
                NodeKind::Meter {
                    meter_id: track.id.clone(),
                },
                format!("{} [meter]", track.name),
            );

            graph.connect(last_node, 0, gain_node, 0);
            graph.connect(gain_node, 0, meter_node, 0);
            graph.connect(meter_node, 0, master_id, port_idx as u32);

            let mut mod_node_ids: HashMap<String, NodeId> = HashMap::new();

            for modulator in &track.modulators {
                let (kind, rate_hz, waveform, initial_value, initial_envelope, tempo_sync, beat_division) =
                    match &modulator.params {
                        ModulatorParams::Lfo(p) => (
                            ModulatorKind::Lfo,
                            p.rate_hz,
                            p.waveform.clone(),
                            0.0f32,
                            EnvelopeParams::default(),
                            p.tempo_sync,
                            p.beat_division,
                        ),
                        ModulatorParams::Macro(p) => (
                            ModulatorKind::Macro,
                            0.0f32,
                            LfoWaveform::default(),
                            p.value,
                            EnvelopeParams::default(),
                            false,
                            1.0f32,
                        ),
                        ModulatorParams::Envelope(p) => (
                            ModulatorKind::Envelope,
                            0.0f32,
                            LfoWaveform::default(),
                            0.0f32,
                            p.clone(),
                            false,
                            1.0f32,
                        ),
                    };
                let node_id = graph.add_node(
                    NodeKind::Modulator {
                        modulator_id: modulator.id.clone(),
                        kind,
                        rate_hz,
                        waveform,
                        initial_value,
                        initial_envelope,
                        tempo_sync,
                        beat_division,
                    },
                    format!("{} [mod:{}]", track.name, modulator.id),
                );
                mod_node_ids.insert(modulator.id.clone(), node_id);
            }

            for route in &track.mod_routes {
                let source_node_id = match mod_node_ids.get(&route.modulator_id) {
                    Some(&id) => id,
                    None => continue, // dangling modulator reference — skip
                };

                let target_node_id = match &route.target {
                    ModulationTargetRef::TrackGain { .. } => gain_node,
                    ModulationTargetRef::EffectParam { effect_id, .. }
                    | ModulationTargetRef::PluginParam { effect_id, .. } => {
                        match effect_node_ids.get(effect_id) {
                            Some(&id) => id,
                            None => continue, // effect not enabled or not found
                        }
                    }
                };

                let param_name = match &route.target {
                    ModulationTargetRef::TrackGain { .. } => "gain".to_string(),
                    ModulationTargetRef::EffectParam { param, .. } => param.clone(),
                    // Sentinel prefix resolved by plugin wrappers in `param_id()`.
                    ModulationTargetRef::PluginParam { native_id, .. } => {
                        format!("plugin_native:{native_id}")
                    }
                };

                graph.modulation.add_route(ModulationRoute {
                    source: ModulationSource {
                        node: source_node_id,
                        port: 0,
                    },
                    target: ModulationTarget {
                        param: ParamId::new(target_node_id, param_name),
                        depth: route.depth,
                    },
                    route_id: Some(route.id.clone()),
                });
            }
        }

        graph
    }
}

impl Default for AudioGraph {
    fn default() -> Self {
        Self::new()
    }
}
