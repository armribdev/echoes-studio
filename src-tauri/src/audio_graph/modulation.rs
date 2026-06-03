//! Graph-level modulation routing.
//!
//! ## Design principle
//!
//! Modulation is a **graph concern**, not a plugin concern.  Plugins do not own
//! LFOs or envelopes; instead, modulator nodes feed parameter targets through
//! dedicated modulation edges compiled into the `ExecutionPlan`.
//!
//! ## Routing model
//!
//! ```text
//! ModulatorNode  (LFO / Envelope / Macro)
//!      │  modulation edge
//!      ▼
//! ModulationTarget  { node_id, param_id, depth }
//! ```
//!
//! Before a node's `process` is called the scheduler sums all modulation
//! contributions for each parameter and delivers the final value through the
//! `TransportInfo` / parameter side-channel (future work: a dedicated
//! `ParameterBlock` per node).
//!
//! For the current milestone the types are fully defined but modulation
//! edges are not yet wired into the execution pipeline.  All data structures
//! are stable so adding modulation sources only requires populating the maps.

use crate::audio_graph::node::NodeId;

/// Opaque stable identifier for a parameter on a specific node.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParamId {
    pub node: NodeId,
    /// Parameter name / key (e.g. `"gain"`, `"cutoff"`, `"depth"`).
    pub name: String,
}

impl ParamId {
    pub fn new(node: NodeId, name: impl Into<String>) -> Self {
        Self {
            node,
            name: name.into(),
        }
    }
}

/// Identifies which output port of a modulator node carries the modulation
/// signal (normalised −1…+1 f32 per sample).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModulationSource {
    pub node: NodeId,
    /// Output port index on the modulator node.
    pub port: u32,
}

/// A single modulation destination: one parameter on one node.
#[derive(Debug, Clone)]
pub struct ModulationTarget {
    pub param: ParamId,
    /// Modulation depth in the parameter's native range (signed).
    /// e.g. `depth = 6.0` on a gain parameter means ±6 dB of modulation.
    pub depth: f32,
}

/// A complete modulation connection: one source → one target.
#[derive(Debug, Clone)]
pub struct ModulationRoute {
    pub source: ModulationSource,
    pub target: ModulationTarget,
    /// Project-level route ID — used by the compiler to look up the route's
    /// `LiveParams` depth cell so depth changes don't trigger a recompile.
    /// `None` only for routes constructed in tests or future code paths that
    /// don't originate from a project route.
    pub route_id: Option<String>,
}

/// The complete modulation routing table for one compiled execution plan.
///
/// Built by the `GraphCompiler` from modulation edges in the `AudioGraph`.
/// Immutable during playback.
#[derive(Debug, Default, Clone)]
pub struct ModulationMap {
    routes: Vec<ModulationRoute>,
}

impl ModulationMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_route(&mut self, route: ModulationRoute) {
        self.routes.push(route);
    }

    /// All routes whose source originates from `node`.
    pub fn routes_from(&self, node: NodeId) -> impl Iterator<Item = &ModulationRoute> {
        self.routes.iter().filter(move |r| r.source.node == node)
    }

    /// All routes that target `param`.
    pub fn routes_to<'a>(
        &'a self,
        param: &'a ParamId,
    ) -> impl Iterator<Item = &'a ModulationRoute> {
        self.routes.iter().filter(move |r| &r.target.param == param)
    }

    /// All routes targeting `node_id` (called by the scheduler at construction time).
    pub fn routes_targeting_node(&self, node_id: NodeId) -> impl Iterator<Item = &ModulationRoute> {
        self.routes
            .iter()
            .filter(move |r| r.target.param.node == node_id)
    }

    /// Raw slice over all routes (used by the scheduler at construction time).
    pub fn all_routes(&self) -> &[ModulationRoute] {
        &self.routes
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }
}

/// The kind of modulation a source node produces.
///
/// Stored in `AudioGraph` node metadata; used by the UI and compiler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModulatorKind {
    /// Periodic waveform (sine, triangle, saw, square, …)
    Lfo,
    /// ADSR / multi-stage envelope triggered by MIDI or a gate signal.
    Envelope,
    /// User-controlled knob that fans out to multiple targets simultaneously.
    Macro,
    /// Audio-rate modulation (e.g. ring modulation, FM).
    AudioRate,
}
