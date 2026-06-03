//! Serializable graph snapshot for the UI Graph view.
//!
//! Built from `AudioGraph::from_project` — read-only projection of the
//! compiled topology without touching the real-time scheduler.

use serde::Serialize;

use crate::audio_graph::graph::{AudioGraph, GraphNode, NodeKind};
use crate::audio_graph::modulation::ModulationRoute;
use crate::project::{AppProject, LfoWaveform};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSnapshotDto {
    pub nodes: Vec<GraphNodeDto>,
    pub audio_edges: Vec<AudioEdgeDto>,
    pub mod_edges: Vec<ModEdgeDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNodeDto {
    pub id: u64,
    pub kind: String,
    pub label: String,
    pub track_id: Option<String>,
    pub modulator_id: Option<String>,
    pub effect_kind: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioEdgeDto {
    pub from_node: u64,
    pub from_port: u32,
    pub to_node: u64,
    pub to_port: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModEdgeDto {
    pub source_node: u64,
    pub source_port: u32,
    pub target_node: u64,
    pub param: String,
    pub depth: f32,
    pub route_id: Option<String>,
}

impl GraphSnapshotDto {
    /// Build a UI snapshot from the current project topology.
    pub fn from_project(project: &AppProject) -> Self {
        let graph = AudioGraph::from_project(project);
        Self::from_graph(&graph)
    }

    pub fn from_graph(graph: &AudioGraph) -> Self {
        let mut nodes: Vec<GraphNodeDto> = graph.all_nodes().map(node_to_dto).collect();
        nodes.sort_by_key(|n| n.id);

        let audio_edges: Vec<AudioEdgeDto> = graph
            .all_edges()
            .iter()
            .map(|e| AudioEdgeDto {
                from_node: e.from.node.0,
                from_port: e.from.port,
                to_node: e.to.node.0,
                to_port: e.to.port,
            })
            .collect();

        let mod_edges: Vec<ModEdgeDto> =
            graph.modulation_routes().iter().map(route_to_dto).collect();

        Self {
            nodes,
            audio_edges,
            mod_edges,
        }
    }
}

fn node_to_dto(node: &GraphNode) -> GraphNodeDto {
    let (kind, track_id, modulator_id, effect_kind, summary) = describe_kind(&node.kind);

    GraphNodeDto {
        id: node.id.0,
        kind,
        label: node.label.clone(),
        track_id,
        modulator_id,
        effect_kind,
        summary,
    }
}

fn route_to_dto(route: &ModulationRoute) -> ModEdgeDto {
    ModEdgeDto {
        source_node: route.source.node.0,
        source_port: route.source.port,
        target_node: route.target.param.node.0,
        param: route.target.param.name.clone(),
        depth: route.target.depth,
        route_id: route.route_id.clone(),
    }
}

fn describe_kind(
    kind: &NodeKind,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    match kind {
        NodeKind::Track { track_id } => ("track".into(), Some(track_id.clone()), None, None, None),
        NodeKind::Instrument { track_id } => (
            "instrument".into(),
            Some(track_id.clone()),
            None,
            None,
            None,
        ),
        NodeKind::TrackGain { track_id, gain } => (
            "track_gain".into(),
            Some(track_id.clone()),
            None,
            None,
            Some(format!("{:.0}%", gain * 100.0)),
        ),
        NodeKind::Meter { meter_id } => ("meter".into(), Some(meter_id.clone()), None, None, None),
        NodeKind::Master { num_inputs } => (
            "master".into(),
            None,
            None,
            None,
            Some(format!("{num_inputs} entrées")),
        ),
        NodeKind::Modulator {
            modulator_id,
            rate_hz,
            waveform,
            ..
        } => (
            "modulator".into(),
            None,
            Some(modulator_id.clone()),
            None,
            Some(format!("{rate_hz:.2} Hz · {}", waveform_label(waveform))),
        ),
        NodeKind::Eq { params, .. } => (
            "eq".into(),
            None,
            None,
            Some("eq".into()),
            Some(format!(
                "L{:+.0} M{:+.0} H{:+.0} dB",
                params.low_gain_db, params.mid_gain_db, params.high_gain_db
            )),
        ),
        NodeKind::Compressor { params, .. } => (
            "compressor".into(),
            None,
            None,
            Some("compressor".into()),
            Some(format!(
                "{:.0} dB · {:.1}:1",
                params.threshold_db, params.ratio
            )),
        ),
        NodeKind::Reverb { params, .. } => (
            "reverb".into(),
            None,
            None,
            Some("reverb".into()),
            Some(format!("Wet {:.0}%", params.wet * 100.0)),
        ),
        NodeKind::Delay { params, .. } => (
            "delay".into(),
            None,
            None,
            Some("delay".into()),
            Some(format!(
                "{:.0} ms · Wet {:.0}%",
                params.delay_ms,
                params.wet * 100.0
            )),
        ),
        NodeKind::Gain { gain_linear } => (
            "gain".into(),
            None,
            None,
            None,
            Some(format!("{:.0}%", gain_linear * 100.0)),
        ),
        NodeKind::Pan { position } => (
            "pan".into(),
            None,
            None,
            None,
            Some(format!("{position:+.2}")),
        ),
        NodeKind::Bus => ("bus".into(), None, None, None, None),
        NodeKind::Send { send_gain, .. } => (
            "send".into(),
            None,
            None,
            None,
            Some(format!("{:.0}%", send_gain * 100.0)),
        ),
        NodeKind::Return { .. } => ("return".into(), None, None, None, None),
        NodeKind::Plugin { device_id, .. } => {
            ("plugin".into(), None, None, None, Some(device_id.clone()))
        }
    }
}

fn waveform_label(w: &LfoWaveform) -> &'static str {
    match w {
        LfoWaveform::Sine => "sine",
        LfoWaveform::Triangle => "triangle",
        LfoWaveform::Saw => "saw",
        LfoWaveform::Square => "square",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::AppProject;

    #[test]
    fn empty_project_has_master_node() {
        let project = AppProject::new();
        let snap = GraphSnapshotDto::from_project(&project);
        assert!(snap.nodes.iter().any(|n| n.kind == "master"));
    }

    #[test]
    fn track_chain_produces_audio_edges() {
        let mut project = AppProject::new();
        project.tracks.push(crate::project::AppTrack {
            id: "t1".into(),
            name: "Test".into(),
            volume: 80.0,
            pan: 0.0,
            muted: false,
            solo: false,
            clips: vec![],
            effects: vec![],
            modulators: vec![],
            mod_routes: vec![],
            midi_notes: vec![],
            midi_clips: vec![],
            instrument: None,
        });
        let snap = GraphSnapshotDto::from_project(&project);
        assert!(!snap.audio_edges.is_empty());
        let track_nodes: Vec<_> = snap.nodes.iter().filter(|n| n.kind == "track").collect();
        assert_eq!(track_nodes.len(), 1);
    }
}
