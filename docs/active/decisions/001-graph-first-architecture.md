# ADR-001 — Graph-First Architecture

## Status

Accepted. Implemented in `audio_graph/`.

---

## Context

A DAW needs to route audio between tracks, buses, sends, effects, and output
devices. There are two common architectural models:

**Track-centric model:** Tracks are first-class objects. Plugins sit inside
tracks as an insert chain. Buses are special track types. Sends are hardcoded
fields. The signal flow is implicitly linear: track → insert chain → bus → master.

**Graph-first model:** Everything is a node in a directed acyclic graph. Tracks,
buses, sends, effects, and modulators are all nodes with input/output ports.
Signal routing is expressed as edges between nodes.

The question was which model to use as the core abstraction of the audio engine.

---

## Decision

The `AudioGraph` DAG is the single core abstraction. Every DSP element
(tracks, gains, panners, buses, sends, effects, modulators) is an `AudioNode`.
The signal path is fully described by the graph topology (nodes + edges).

Tracks are not first-class containers. A track is a `TrackNode` (audio source)
connected to a `GainNode` connected to a `MasterNode` via graph edges.

---

## Consequences

**Positive:**

- **Uniform execution model.** The `GraphScheduler` iterates `ExecutionPlan.steps`
  in topological order. Adding a new DSP primitive requires only implementing
  `AudioNode` and registering a `NodeKind` in the compiler.
- **Arbitrary routing.** Parallel chains, feedback (with cycle detection),
  side-chains, multi-output buses — all are expressible as graph topologies
  without special-casing in the engine.
- **Format-agnostic plugins.** VST3 and AU plugins implement `AudioNode`.
  The scheduler has no knowledge of plugin formats.
- **Modulation is natural.** A modulator is a node with an output port.
  Routing its output to a parameter is just a modulation edge. The graph
  handles it uniformly.
- **Deterministic, reproducible execution.** Topological sort + stable `NodeId`
  ordering guarantees bit-identical output for the same project.

**Negative:**

- **Higher initial complexity.** "Add a gain node on a track" requires graph
  compilation and a scheduler swap, not just setting a field on a struct.
- **Full recompilation on structural changes.** Every track/clip addition
  requires recompiling the full graph. For large projects, this is a
  measurable cost (mitigated by the future dedicated compiler thread).
- **Opaque project model.** `AppProject` stores tracks and clips, not graph
  topology. `AudioGraph::from_project()` rebuilds topology each time. This
  means the graph topology is not persisted — it is derived.

---

## Alternatives Considered

### Track-Centric (Rejected)

Easier to implement initially. But hardcodes the linear insert-chain topology.
Adding sends, parallel buses, or modulation requires hacking the track struct.
As the project grows, complexity migrates into edge-case track fields rather
than emerging naturally from graph topology. This is the architectural debt that
many legacy DAWs carry.

### Hybrid (Tracks as first-class + optional graph override) (Rejected)

Tempting as a "best of both" approach, but it creates two execution models
(linear chain for tracks, graph for "advanced" routing). Future code must handle
both paths. Testing doubles. The graph model is expressive enough to encode
the simple linear case without any penalty.
