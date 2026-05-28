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

use crate::audio_graph::node::{NodeId, PortId};
use crate::project::AppProject;

// ── Node ID allocation ─────────────────────────────────────────────────────────

static NEXT_NODE_ID: AtomicU64 = AtomicU64::new(1);

fn alloc_node_id() -> NodeId {
    NodeId(NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed))
}

// ── Node kinds ─────────────────────────────────────────────────────────────────

/// Describes the DSP role of a graph node.
///
/// The `GraphCompiler` maps each kind to a concrete `Box<dyn AudioNode>`.
#[derive(Debug, Clone)]
pub enum NodeKind {
    /// Reads timeline clips for one track and outputs audio.
    Track {
        track_id: String,
    },

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
    Return {
        return_id: String,
    },

    /// Per-sample gain (linear multiplier, handles both gain and volume faders).
    Gain {
        gain_linear: f32,
    },

    /// Stereo panner (−1.0 = hard left, 0.0 = centre, +1.0 = hard right).
    Pan {
        position: f32,
    },

    /// Wraps an external or internal plugin.
    Plugin {
        /// Opaque device ID looked up in `PluginHost`.
        device_id: String,
    },

    /// Modulation source (LFO, envelope, macro, audio-rate).
    Modulator {
        kind: crate::audio_graph::modulation::ModulatorKind,
    },

    /// Peak / RMS meter (side-effect: writes to a meter ring-buffer for the UI).
    Meter {
        meter_id: String,
    },

    /// The master output node — final stereo sum before device output.
    ///
    /// `num_inputs` equals the number of tracks so each track gets its own
    /// dedicated input port.  This avoids the "all-to-port-0 overwrite" bug.
    Master { num_inputs: usize },
}

// ── Graph node ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GraphNode {
    pub id: NodeId,
    pub kind: NodeKind,
    pub label: String,
}

// ── Edge ──────────────────────────────────────────────────────────────────────

/// A directed audio connection from one port to another.
///
/// Convention: audio flows  `from` → `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub from: PortId,
    pub to: PortId,
}

// ── AudioGraph ────────────────────────────────────────────────────────────────

/// The complete audio graph topology (editor-side, non-real-time).
pub struct AudioGraph {
    pub(crate) nodes: HashMap<NodeId, GraphNode>,
    pub(crate) edges: Vec<Edge>,
    /// The single master output node.
    pub master_id: Option<NodeId>,
}

impl AudioGraph {
    pub fn new() -> Self {
        AudioGraph {
            nodes: HashMap::new(),
            edges: Vec::new(),
            master_id: None,
        }
    }

    // ── Mutation ───────────────────────────────────────────────────────────────

    pub fn add_node(&mut self, kind: NodeKind, label: impl Into<String>) -> NodeId {
        let id = alloc_node_id();
        let is_master = matches!(kind, NodeKind::Master { .. });
        self.nodes.insert(id, GraphNode { id, kind, label: label.into() });
        if is_master {
            self.master_id = Some(id);
        }
        id
    }

    /// Connect output port `from_port` of `from_node` to input port `to_port`
    /// of `to_node`.  Returns the created edge.
    pub fn connect(
        &mut self,
        from_node: NodeId,
        from_port: u32,
        to_node: NodeId,
        to_port: u32,
    ) -> Edge {
        let edge = Edge {
            from: PortId { node: from_node, port: from_port },
            to: PortId { node: to_node, port: to_port },
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

    // ── Query ──────────────────────────────────────────────────────────────────

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

    // ── Project builder ────────────────────────────────────────────────────────

    /// Construct a default signal-flow graph from an `AppProject`.
    ///
    /// Signal flow per track:
    ///
    /// ```text
    /// TrackNode ──► TrackGainNode ──► MasterNode
    /// ```
    ///
    /// This mirrors the current flat-mixer layout.  Routing between buses,
    /// send/returns, and plugins will be added as the project model grows.
    pub fn from_project(project: &AppProject) -> Self {
        let mut graph = AudioGraph::new();

        // ── Master node ─────────────────────────────────────────────────────────
        // num_inputs = one port per track so the master can sum all of them.
        // Using a BusNode would be cleaner long-term; for now each track gets its
        // own dedicated port (port 0 = track 0, port 1 = track 1, …).
        let num_track_inputs = project.tracks.len().max(1);
        let master_id = graph.add_node(
            NodeKind::Master { num_inputs: num_track_inputs },
            "Master",
        );

        // Solo logic: if any track is soloed, mute all non-solo tracks.
        let any_solo = project.tracks.iter().any(|t| t.solo);

        // ── Per-track chain ─────────────────────────────────────────────────────
        for (port_idx, track) in project.tracks.iter().enumerate() {
            let track_node = graph.add_node(
                NodeKind::Track { track_id: track.id.clone() },
                format!("{} [clips]", track.name),
            );

            // Volume 0–100 → 0.0–1.0; honour mute and exclusive solo.
            let effectively_muted = track.muted || (any_solo && !track.solo);
            let gain_linear = if effectively_muted {
                0.0
            } else {
                (track.volume / 100.0).clamp(0.0, 1.0)
            };

            let gain_node = graph.add_node(
                NodeKind::TrackGain { track_id: track.id.clone(), gain: gain_linear },
                format!("{} [vol]", track.name),
            );

            let meter_node = graph.add_node(
                NodeKind::Meter { meter_id: track.id.clone() },
                format!("{} [meter]", track.name),
            );

            // track → gain → meter → master[port_idx]
            graph.connect(track_node, 0, gain_node, 0);
            graph.connect(gain_node, 0, meter_node, 0);
            graph.connect(meter_node, 0, master_id, port_idx as u32);
        }

        graph
    }
}

impl Default for AudioGraph {
    fn default() -> Self {
        Self::new()
    }
}
