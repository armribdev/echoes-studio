# Graph Compiler

> **Canonical.** Merges the former `systems/graph-compiler.md` (rationale) and the
> legacy `graph-compiler-design.md` (full algorithm). The legacy flat file is
> archived under `docs/archive/graph-compiler-design.md`.

## Responsibility

The `GraphCompiler` converts an editor-side `AudioGraph` (mutable DAG of nodes
and edges) into a `CompiledGraph` that the `GraphScheduler` can execute without
any graph traversal at runtime.

This pre-computation step is what makes the audio callback allocation-free and
cache-friendly.

---

## Why Compilation Is Necessary

At process time, the scheduler needs to know:
- In what order to call each node's `process()`.
- Which output buffers to pass as inputs to each node.
- How many output buffers to pre-allocate per node.

Deriving this at runtime (traversing a `HashMap<NodeId, GraphNode>`) would
require dynamic memory allocation and non-deterministic iteration order.
Compiling it ahead of time produces a flat `Vec<ExecutionStep>` that the
scheduler iterates with zero overhead.

The graph changes infrequently (on user actions). The callback runs thousands
of times per second. Pre-computing is the right tradeoff.

---

## Inputs and Outputs

```
Input:
  AudioGraph { nodes: HashMap<NodeId, GraphNode>, edges: Vec<Edge> }
  AppProject  (source data for node instantiation — clip samples, track gains)
  CompileConfig { sample_rate, max_block_size, num_channels }

Output:
  CompiledGraph {
      plan:       Arc<ExecutionPlan>,   // immutable, shared with audio thread
      processors: Vec<Box<dyn AudioNode>>,
  }
```

---

## Compilation Algorithm

### 1. Validation

- Every graph must have exactly one `MasterNode` (`graph.master_id == Some`).
  No master = compile error → empty silent graph.
- All edge endpoints must reference existing nodes. Dangling edges = compile
  error → empty silent graph.
- Cycle detection is implicit in step 2 (if Kahn's sort doesn't consume all
  nodes, a cycle exists).

Compilation errors are never fatal. The audio engine always has a valid (if
silent) graph. Errors are logged and surfaced to the UI via future structured
error events.

### 2. Topological Sort (Kahn's BFS)

```
1. Compute in-degree for every node.
2. Seed queue with zero-in-degree nodes, sorted by NodeId (for determinism).
3. BFS: dequeue, reduce neighbour in-degrees, enqueue those reaching zero.
4. If |sorted| < |nodes|: cycle detected → CompileError::CycleDetected.
```

The sort is **stable** (deterministic for identical graphs regardless of
HashMap iteration order) because:
- Nodes at the same topological depth are sorted by `NodeId` (monotonically
  increasing allocation order).
- `NodeId` allocation uses `AtomicU64` with `Relaxed` ordering, so within a
  single thread (all graph building is on the command thread) the order is
  deterministic.

This is required for bit-identical offline export.

### 3. Step Index Assignment

```
step_index: HashMap<NodeId, usize>
```

Maps each `NodeId` to its position in the `steps` array. Used to convert
graph-level `PortId` references (node + port) into flat array indices
(`src_step`, `src_port`) that the scheduler uses to index `output_buffers`.

### 4. ExecutionStep Construction

For each node in topological order:
1. Determine `num_inputs` and `num_outputs` from `NodeKind`.
2. For each input port (`0..num_inputs`), find any edge with
   `to.node == node_id && to.port == port_idx`.
3. If an edge exists, record `Some(InputConnection { src_step, src_port })`.
4. If no edge, record `None` (unconnected → silence buffer at runtime).

```rust
ExecutionStep {
    node_id: NodeId,
    num_outputs: usize,
    input_connections: Vec<Option<InputConnection>>,
}
```

The invariant `src_step < current_step` is guaranteed by topological order.
The scheduler relies on this to safely share buffer references without aliasing.

### 5. Node Instantiation

`build_node(id, kind, project)` dispatches on `NodeKind`:

| NodeKind            | Produces                       | Data source        |
|---------------------|--------------------------------|--------------------|
| `Track { id }`      | `TrackNode` with clip data     | `AppProject.tracks`|
| `TrackGain { gain }`| `GainNode(gain)`               | `NodeKind` field   |
| `Gain { linear }`   | `GainNode(linear)`             | `NodeKind` field   |
| `Pan { pos }`       | `PanNode(pos)`                 | `NodeKind` field   |
| `Bus`               | `BusNode(N inputs)`            | constant           |
| `Master`            | `MasterNode(N inputs)`         | constant           |
| `Meter { id }`      | `MeterNode(id)`                | meter ID string    |
| `Plugin { id }`     | `PluginHost::instantiate(id)`, wrapped in `BypassNode` when `bypassed` (ADR-020) | plugin system      |

For unknown plugin IDs, `build_node` logs a warning and returns a
`PassThroughDevice` (silence pass-through) so compilation always succeeds.

### 6. Node Preparation

```rust
for node in &mut processors {
    node.prepare(PrepareContext { sample_rate, max_block_size, num_channels });
}
```

This is the only point where nodes may allocate. Filter coefficients, delay
lines, FFT buffers — all are allocated here, not in `process()`.

### 7. Output sizing from prepared nodes (ADR-014 multi-out)

A plugin's output ports are only known once it is loaded and prepared, after
step 4 sized the steps. `size_outputs_from_processors` then raises each step's
`num_outputs` to its node's `num_output_ports()` when larger (never lowers it —
only multi-out plugin wrappers exceed their static count), and cuts to `None`
any input connection whose `src_port` does not exist on its source step, so an
edge to a missing plugin output reads silence rather than the scheduler's
clamp onto output 0. Runs before latency compensation and level assignment.

---

## ExecutionPlan Structure

```rust
pub struct ExecutionPlan {
    pub steps: Vec<ExecutionStep>,
    pub modulation: ModulationMap,
    pub sample_rate: f32,
    pub max_block_size: usize,
    pub num_channels: usize,
}

pub struct ExecutionStep {
    pub node_id: NodeId,
    pub input_connections: Vec<Option<InputConnection>>,
    pub num_outputs: usize,
}

pub struct InputConnection {
    pub src_step: usize,   // always < current step index (invariant)
    pub src_port: usize,
}
```

The `Arc<ExecutionPlan>` is shared between the command thread (holds a reference
during compilation) and the audio thread (scheduler reads `steps`). Zero copy.

---

## Buffer Allocation Strategy

`GraphScheduler::new(compiled)` pre-allocates:

```
output_buffers[step][port] — one AudioBuffer per output port per step
silence_buffer             — permanently-zeroed, shared across unconnected inputs
input_ptr_scratch          — raw pointer array, sized to MAX_NODE_INPUT_PORTS
```

For a project with 16 tracks, 2 channels, 512 frames:
`~16 nodes × 3 ports × 512 × 4 bytes ≈ 100 KB` — negligible.

No further allocation occurs until the next graph update.

---

## Re-compilation Triggers

A new graph is compiled whenever:
- A track is created or deleted
- A clip is added, moved, or removed
- Track volume changes (gain node needs new value)
- Track mute/solo state changes
- Any structural routing change (plugin insert, bus connection, undo/redo,
  project load)

**Transport changes** (play/stop/seek) do **not** trigger recompilation. They
update `TransportInfo`, passed to `process_block` directly. Volume-knob turns
should ideally be atomic parameter updates rather than full recompiles — see
`../architecture/anti-patterns.md`.

---

## Error Resilience

The compiler never panics. All error paths return `CompileError` and produce an
empty (silent) graph:

- `CycleDetected` → silent graph
- `NoMasterNode` → silent graph
- `MissingNode` in edge → silent graph
- Unknown plugin ID → `PassThroughDevice` (compile succeeds)

This ensures the audio engine always has a valid (if silent) graph to execute.

---

## Future: Incremental Compilation

For projects with many tracks and plugins, full recompilation on every
parameter change is wasteful. Future optimisations:

1. **Dirty marking**: only re-instantiate nodes whose `NodeKind` changed.
2. **State transfer**: copy DSP state from the old node to the new one
   (preserves reverb tails, filter states, etc.).
3. **Dedicated compiler thread**: move compilation off the command thread to
   avoid blocking the gateway during complex projects.

---

## Further Reading

- `../architecture/audio-graph.md` — graph-first philosophy + pipeline overview
- `dsp-runtime.md` — how the scheduler executes the plan
- `../architecture/snapshot-rebuild-flow.md` — compile → enqueue → swap sequence
- `../decisions/001-graph-first-architecture.md` — why graph-first
