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

use std::sync::Arc;

use crate::audio_graph::automation::{AutomationAction, CompiledAutomationCurve};
use crate::audio_graph::modulation::{
    ModulationMap, ModulationRoute, ModulationSource, ModulationTarget, ModulatorKind, ParamId,
};
use crate::audio_graph::node::{NodeId, PortId, PortKind};
use crate::audio_graph::plugin_params::plugin_param_name;
use crate::project::{
    AppProject, AudioFollowerParams, EffectParams, EnvelopeParams, LfoWaveform, ModulationTargetRef, ModulatorParams,
    PluginOutputSource,
};

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
        /// Host the plugin in a sandbox process (ADR-015), from
        /// `PluginParams.sandboxed`; `PluginHost::instantiate` decides.
        sandboxed: bool,
        /// Disabled in the effect chain: the plugin stays loaded (settings,
        /// editor) but audio passes through it untouched (ADR-020).
        bypassed: bool,
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
        /// AudioFollower seed params (AudioFollower only; default for other kinds).
        initial_follower: AudioFollowerParams,
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
    /// Resolved automation actions — built by `from_project`, consumed by the compiler.
    pub(crate) automation_actions: Vec<AutomationAction>,
}

impl AudioGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            master_id: None,
            modulation: ModulationMap::new(),
            automation_actions: Vec::new(),
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

        // Sidechain routing (ADR-014 follow-up): resolved in a second pass once
        // every track's post-fader tap point is known, since a plugin's source
        // track can appear later in `project.tracks` than the plugin itself.
        let mut track_tap_nodes: HashMap<String, NodeId> = HashMap::new();
        let mut pending_sidechain: Vec<(String, String, NodeId)> = Vec::new();
        // Multi-out (ADR-014): every plugin placement in the project, and the
        // track inputs waiting for one of their aux outputs.
        let mut plugin_placements: HashMap<String, NodeId> = HashMap::new();
        let mut pending_inputs: Vec<(PluginOutputSource, NodeId)> = Vec::new();

        for (port_idx, track) in project.tracks.iter().enumerate() {
            let mut instrument_node = None;
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
                instrument_node = Some(instr_node);
                let bus_node = graph.add_node(NodeKind::Bus, format!("{} [bus]", track.name));
                graph.connect(audio_node, 0, bus_node, 0);
                graph.connect(instr_node, 0, bus_node, 1);
                bus_node
            } else if !track.midi_clips.is_empty() {
                let instr_node = graph.add_node(
                    NodeKind::Instrument {
                        track_id: track.id.clone(),
                    },
                    format!("{} [instrument]", track.name),
                );
                instrument_node = Some(instr_node);
                instr_node
            } else {
                graph.add_node(
                    NodeKind::Track {
                        track_id: track.id.clone(),
                    },
                    format!("{} [clips]", track.name),
                )
            };
            let track_node = match &track.input_source {
                Some(source) => {
                    let input_node = graph.add_node(NodeKind::Bus, format!("{} [input]", track.name));
                    graph.connect(track_node, 0, input_node, 0);
                    pending_inputs.push((source.clone(), input_node));
                    input_node
                }
                None => track_node,
            };

            let effectively_muted = track.muted || (any_solo && !track.solo);
            let gain_linear = if effectively_muted {
                0.0
            } else {
                (track.volume / 100.0).clamp(0.0, 1.0)
            };

            // Plugin placements a `PluginParam` target can name: effects by id,
            // and the track's plugin instrument by `instrument:{track_id}`.
            let mut effect_node_ids: HashMap<String, NodeId> = HashMap::new();
            if let (Some(node), Some(_)) = (instrument_node, &track.instrument) {
                effect_node_ids.insert(instrument_instance_key(&track.id), node);
            }

            let mut last_node = track_node;
            for effect in &track.effects {
                // Disabled plugins stay in the graph so their instance survives (ADR-020).
                let is_plugin = matches!(effect.params, EffectParams::Plugin(_));
                if !effect.enabled && !is_plugin {
                    continue;
                }
                let sidechain_source = match &effect.params {
                    EffectParams::Plugin(p) => p.sidechain_source.clone(),
                    _ => None,
                };
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
                        bypassed: !effect.enabled,
                    },
                };
                let fx_node = graph.add_node(kind, format!("{} [fx:{}]", track.name, effect.id));
                effect_node_ids.insert(effect.id.clone(), fx_node);
                graph.connect(last_node, 0, fx_node, 0);
                last_node = fx_node;
                if let Some(source_track_id) = sidechain_source {
                    pending_sidechain.push((source_track_id, track.id.clone(), fx_node));
                }
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
            // Post-fader tap: what another track's plugin can sidechain from.
            // Post-fader (not pre-fader) so muting/lowering the source track
            // also quiets the duck it triggers — the intuitive behaviour.
            track_tap_nodes.insert(track.id.clone(), gain_node);

            graph.connect(last_node, 0, gain_node, 0);
            graph.connect(gain_node, 0, meter_node, 0);
            graph.connect(meter_node, 0, master_id, port_idx as u32);
            plugin_placements.extend(effect_node_ids.iter().map(|(id, &node)| (id.clone(), node)));

            let mut mod_node_ids: HashMap<String, NodeId> = HashMap::new();

            for modulator in &track.modulators {
                let (kind, rate_hz, waveform, initial_value, initial_envelope, initial_follower, tempo_sync, beat_division) =
                    match &modulator.params {
                        ModulatorParams::Lfo(p) => (
                            ModulatorKind::Lfo,
                            p.rate_hz,
                            p.waveform.clone(),
                            0.0f32,
                            EnvelopeParams::default(),
                            AudioFollowerParams::default(),
                            p.tempo_sync,
                            p.beat_division,
                        ),
                        ModulatorParams::Macro(p) => (
                            ModulatorKind::Macro,
                            0.0f32,
                            LfoWaveform::default(),
                            p.value,
                            EnvelopeParams::default(),
                            AudioFollowerParams::default(),
                            false,
                            1.0f32,
                        ),
                        ModulatorParams::Envelope(p) => (
                            ModulatorKind::Envelope,
                            0.0f32,
                            LfoWaveform::default(),
                            0.0f32,
                            p.clone(),
                            AudioFollowerParams::default(),
                            false,
                            1.0f32,
                        ),
                        ModulatorParams::AudioFollower(p) => (
                            ModulatorKind::AudioFollower,
                            0.0f32,
                            LfoWaveform::default(),
                            0.0f32,
                            EnvelopeParams::default(),
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
                        initial_follower,
                        tempo_sync,
                        beat_division,
                    },
                    format!("{} [mod:{}]", track.name, modulator.id),
                );
                mod_node_ids.insert(modulator.id.clone(), node_id);

                // AudioFollower taps audio from the track signal chain.
                if let ModulatorParams::AudioFollower(p) = &modulator.params {
                    let source = p.source_effect_id
                        .as_deref()
                        .and_then(|eff_id| effect_node_ids.get(eff_id).copied())
                        .unwrap_or(last_node);
                    graph.connect(source, 0, node_id, 0);
                }
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
                    ModulationTargetRef::PluginParam { native_id, .. } => plugin_param_name(*native_id),
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

            // Resolve automation lanes to NodeId-level actions.
            for lane in &track.automation_lanes {
                let curve = Arc::new(CompiledAutomationCurve::from_lane(lane));
                if curve.is_empty() {
                    continue;
                }
                let (target_node_id, param_name) = match &lane.target {
                    ModulationTargetRef::TrackGain { .. } => (gain_node, "gain".to_string()),
                    ModulationTargetRef::EffectParam { effect_id, param, .. } => {
                        match effect_node_ids.get(effect_id) {
                            Some(&id) => (id, param.clone()),
                            None => continue,
                        }
                    }
                    ModulationTargetRef::PluginParam { effect_id, native_id } => {
                        match effect_node_ids.get(effect_id) {
                            Some(&id) => (id, plugin_param_name(*native_id)),
                            None => continue,
                        }
                    }
                };
                graph.automation_actions.push(AutomationAction {
                    target_node_id,
                    param_name,
                    curve,
                });
            }
        }

        // Wire the sidechain edges collected above, now that every track's tap
        // point exists. `feeds[a]` lists the tracks `a` already feeds, at the
        // track level — used only to reject a candidate edge that would close a
        // cycle (a plugin's aux input port is always index 1, the only aux bus
        // this single-source field can address). A dangling source track, or a
        // routing that would cycle, is dropped silently rather than failing the
        // whole compile: the aux port just keeps reading silence, same as a
        // plugin with no declared aux bus (ADR-014).
        let mut feeds: HashMap<String, Vec<String>> = HashMap::new();
        for (source_track_id, target_track_id, target_node) in &pending_sidechain {
            if source_track_id == target_track_id {
                continue;
            }
            let Some(&source_tap) = track_tap_nodes.get(source_track_id) else {
                continue;
            };
            if sidechain_would_cycle(&feeds, source_track_id, target_track_id) {
                continue;
            }
            feeds
                .entry(source_track_id.clone())
                .or_default()
                .push(target_track_id.clone());
            graph.connect_typed(source_tap, 0, *target_node, 1, PortKind::default_audio_stereo());
        }

        // Plugin aux outputs into track inputs, once every placement exists.
        // Port 0 is excluded (it already feeds the plugin's own track), and so
        // is any routing that would close a cycle — e.g. a track taking an
        // output of one of its own effects.  Both are dropped silently, like an
        // unknown placement: the input just stays silent.  An output the loaded
        // plugin turns out not to have is disconnected by the compiler.
        for (source, input_node) in pending_inputs {
            let Some(&plugin_node) = plugin_placements.get(&source.effect_id) else {
                continue;
            };
            if source.port == 0 || graph.reaches(input_node, plugin_node) {
                continue;
            }
            graph.connect(plugin_node, source.port, input_node, 1);
        }

        graph
    }

    /// Whether `to` can be reached from `from` by following audio edges.
    fn reaches(&self, from: NodeId, to: NodeId) -> bool {
        let mut stack = vec![from];
        let mut visited = std::collections::HashSet::new();
        while let Some(node) = stack.pop() {
            if node == to {
                return true;
            }
            if visited.insert(node) {
                stack.extend(self.edges_from(node).map(|edge| edge.to.node));
            }
        }
        false
    }
}

/// Would adding the track-level edge `source -> target` close a cycle among
/// the sidechain edges already accepted into `feeds`? True exactly when
/// `source` is reachable from `target` by following `feeds`.
fn sidechain_would_cycle(feeds: &HashMap<String, Vec<String>>, source: &str, target: &str) -> bool {
    let mut stack = vec![target.to_string()];
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(node) = stack.pop() {
        if node == source {
            return true;
        }
        if !visited.insert(node.clone()) {
            continue;
        }
        if let Some(next) = feeds.get(&node) {
            stack.extend(next.iter().cloned());
        }
    }
    false
}

/// Placement key of a track's plugin instrument (host records, saved state,
/// `PluginParam` modulation targets).
pub fn instrument_instance_key(track_id: &str) -> String {
    format!("instrument:{track_id}")
}

impl Default for AudioGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use crate::project::{AppMidiClip, AppProject, AppTrack, EffectParams, PluginParams, TrackEffect};

    fn track(id: &str, effects: Vec<TrackEffect>) -> AppTrack {
        AppTrack {
            id: id.into(),
            name: id.into(),
            volume: 80.0,
            pan: 0.0,
            muted: false,
            solo: false,
            clips: vec![],
            effects,
            modulators: vec![],
            mod_routes: vec![],
            midi_notes: vec![],
            midi_clips: vec![],
            instrument: None,
            input_source: None,
            automation_lanes: vec![],
        }
    }

    fn plugin_effect(id: &str, sidechain_source: Option<&str>) -> TrackEffect {
        TrackEffect {
            id: id.into(),
            enabled: true,
            params: EffectParams::Plugin(PluginParams {
                plugin_id: "test.compressor".into(),
                name: "Comp".into(),
                format: "clap".into(),
                params: vec![],
                sandboxed: false,
                sidechain_source: sidechain_source.map(str::to_string),
            }),
        }
    }

    fn gain_node_of(graph: &AudioGraph, track_id: &str) -> NodeId {
        graph
            .all_nodes()
            .find(|n| matches!(&n.kind, NodeKind::TrackGain { track_id: t, .. } if t == track_id))
            .expect("gain node")
            .id
    }

    fn plugin_node_of(graph: &AudioGraph, instance_key: &str) -> NodeId {
        graph
            .all_nodes()
            .find(|n| matches!(&n.kind, NodeKind::Plugin { instance_key: k, .. } if k == instance_key))
            .expect("plugin node")
            .id
    }

    fn has_aux_edge(graph: &AudioGraph, from: NodeId, to: NodeId) -> bool {
        graph
            .all_edges()
            .iter()
            .any(|e| e.from.node == from && e.to.node == to && e.to.port == 1)
    }

    #[test]
    fn sidechain_source_wires_post_fader_tap_into_aux_port() {
        let mut project = AppProject::new();
        project.tracks.push(track("kick", vec![]));
        project.tracks.push(track("duck", vec![plugin_effect("fx0", Some("kick"))]));

        let graph = AudioGraph::from_project(&project);

        let kick_tap = gain_node_of(&graph, "kick");
        let plugin = plugin_node_of(&graph, "fx0");
        assert!(has_aux_edge(&graph, kick_tap, plugin));
    }

    #[test]
    fn sidechain_default_none_adds_no_aux_edge() {
        // C-4/C-2: every project saved before this field existed has `None`
        // here, so the topology must be identical to pre-routing behaviour.
        let mut project = AppProject::new();
        project.tracks.push(track("kick", vec![]));
        project.tracks.push(track("duck", vec![plugin_effect("fx0", None)]));

        let graph = AudioGraph::from_project(&project);
        let plugin = plugin_node_of(&graph, "fx0");
        assert!(graph.edges_into(plugin).all(|e| e.to.port != 1));
    }

    #[test]
    fn sidechain_self_reference_is_dropped() {
        let mut project = AppProject::new();
        project.tracks.push(track("t1", vec![plugin_effect("fx0", Some("t1"))]));

        let graph = AudioGraph::from_project(&project);
        let plugin = plugin_node_of(&graph, "fx0");
        assert!(graph.edges_into(plugin).all(|e| e.to.port != 1));
    }

    #[test]
    fn sidechain_dangling_source_is_dropped() {
        let mut project = AppProject::new();
        project.tracks.push(track("t1", vec![plugin_effect("fx0", Some("does-not-exist"))]));

        let graph = AudioGraph::from_project(&project);
        let plugin = plugin_node_of(&graph, "fx0");
        assert!(graph.edges_into(plugin).all(|e| e.to.port != 1));
    }

    #[test]
    fn sidechain_cycle_is_rejected_keeping_the_first_accepted_edge() {
        // a's plugin sidechains from b, b's plugin sidechains from a: taken
        // together this is a cycle, so only the first one seen (track order)
        // is wired; the second, which would close the loop, is dropped.
        let mut project = AppProject::new();
        project.tracks.push(track("a", vec![plugin_effect("fx_a", Some("b"))]));
        project.tracks.push(track("b", vec![plugin_effect("fx_b", Some("a"))]));

        let graph = AudioGraph::from_project(&project);

        let a_tap = gain_node_of(&graph, "a");
        let b_tap = gain_node_of(&graph, "b");
        let fx_a = plugin_node_of(&graph, "fx_a");
        let fx_b = plugin_node_of(&graph, "fx_b");

        assert!(has_aux_edge(&graph, b_tap, fx_a), "first candidate edge (b -> fx_a) must be accepted");
        assert!(!has_aux_edge(&graph, a_tap, fx_b), "second candidate edge would close the cycle and must be dropped");
    }

    fn drum_track(id: &str) -> AppTrack {
        let mut track = track(id, vec![]);
        track.instrument = Some("clap:drums".into());
        track.midi_clips = vec![AppMidiClip { id: "c".into(), start: 0.0, end: 1.0, offset: 0.0, notes: vec![] }];
        track
    }

    fn fed_from(track_id: &str, effect_id: &str, port: u32) -> AppTrack {
        let mut track = track(track_id, vec![]);
        track.input_source = Some(PluginOutputSource { effect_id: effect_id.into(), port });
        track
    }

    fn input_node_of(graph: &AudioGraph, track_name: &str) -> Option<NodeId> {
        let label = format!("{track_name} [input]");
        graph.all_nodes().find(|n| n.label == label).map(|n| n.id)
    }

    fn instrument_node_of(graph: &AudioGraph, track_id: &str) -> NodeId {
        graph
            .all_nodes()
            .find(|n| matches!(&n.kind, NodeKind::Instrument { track_id: t } if t == track_id))
            .expect("instrument node")
            .id
    }

    fn feeds_input(graph: &AudioGraph, from: NodeId, from_port: u32, input: NodeId) -> bool {
        graph
            .all_edges()
            .iter()
            .any(|e| e.from.node == from && e.from.port == from_port && e.to.node == input && e.to.port == 1)
    }

    #[test]
    fn a_track_input_takes_an_instrument_aux_output() {
        let mut project = AppProject::new();
        project.tracks.push(drum_track("drums"));
        project.tracks.push(fed_from("snare", "instrument:drums", 2));

        let graph = AudioGraph::from_project(&project);

        let input = input_node_of(&graph, "snare").expect("input mixer at the head of the track");
        assert!(feeds_input(&graph, instrument_node_of(&graph, "drums"), 2, input));
    }

    #[test]
    fn a_track_without_input_source_has_no_input_mixer() {
        // C-4/C-2: every project saved before multi-out keeps its topology.
        let mut project = AppProject::new();
        project.tracks.push(drum_track("drums"));
        project.tracks.push(track("snare", vec![]));

        let graph = AudioGraph::from_project(&project);
        assert!(input_node_of(&graph, "snare").is_none());
    }

    #[test]
    fn the_main_output_cannot_be_taken_as_an_input() {
        let mut project = AppProject::new();
        project.tracks.push(drum_track("drums"));
        project.tracks.push(fed_from("snare", "instrument:drums", 0));

        let graph = AudioGraph::from_project(&project);
        let input = input_node_of(&graph, "snare").expect("input mixer");
        assert_eq!(graph.edges_into(input).count(), 1, "only the track's own source");
    }

    #[test]
    fn an_input_from_an_own_effect_is_dropped_as_a_cycle() {
        let mut project = AppProject::new();
        let mut track = fed_from("t1", "fx0", 1);
        track.effects = vec![plugin_effect("fx0", None)];
        project.tracks.push(track);

        let graph = AudioGraph::from_project(&project);
        let input = input_node_of(&graph, "t1").expect("input mixer");
        assert!(!feeds_input(&graph, plugin_node_of(&graph, "fx0"), 1, input));
    }

    #[test]
    fn an_input_from_an_unknown_placement_is_dropped() {
        let mut project = AppProject::new();
        project.tracks.push(fed_from("t1", "gone", 1));

        let graph = AudioGraph::from_project(&project);
        let input = input_node_of(&graph, "t1").expect("input mixer");
        assert_eq!(graph.edges_into(input).count(), 1, "only the track's own source");
    }
}
