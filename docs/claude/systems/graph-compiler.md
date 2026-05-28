# Graph Compiler

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

## Compilation Stages

### 1. Validation

- Every graph must have exactly one `MasterNode`. No master = compile error →
  empty silent graph.
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
4. If |sorted| < |nodes|: cycle detected → CompileError.
```

Stable sort by `NodeId` (monotonically incrementing `AtomicU64`) ensures
identical execution order across compilations for the same graph structure.
This is required for bit-identical offline export.

### 3. Step Index Assignment

Each `NodeId` is mapped to its position in the steps array. Used to convert
graph-level `PortId` references (node + port) into flat array indices
(`src_step`, `src_port`) that the scheduler uses to index `output_buffers`.

### 4. ExecutionStep Construction

For each node in topological order, build `ExecutionStep`:

```rust
ExecutionStep {
    node_id: NodeId,
    num_outputs: usize,
    input_connections: Vec<Option<InputConnection>>,
    // None = unconnected port → silence buffer at runtime
    // Some(InputConnection { src_step, src_port }) = upstream buffer
}
```

The invariant `src_step < current_step` is guaranteed by topological order.
The scheduler relies on this to safely share buffer references without aliasing.

### 5. Node Instantiation

`build_node(id, kind, project)` dispatches on `NodeKind`:

| NodeKind | Produces | Data source |
|----------|----------|------------|
| `Track { id }` | `TrackNode` with clip data | `AppProject.tracks` |
| `TrackGain { gain }` | `GainNode(gain)` | `NodeKind` field |
| `Pan { pos }` | `PanNode(pos)` | `NodeKind` field |
| `Bus` | `BusNode` | constant |
| `Master` | `MasterNode` | constant |
| `Meter { id }` | `MeterNode` | meter ID string |
| `Plugin { id }` | `PluginHost::instantiate(id)` | plugin system |

For unknown plugin IDs, `build_node` returns a `PassThroughDevice` (silence)
so compilation always produces a runnable graph.

### 6. Node Preparation

```rust
for node in &mut processors {
    node.prepare(PrepareContext { sample_rate, max_block_size, num_channels });
}
```

This is the only point where nodes may allocate. Filter coefficients, delay
lines, FFT buffers — all are allocated here, not in `process()`.

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
```

The `Arc<ExecutionPlan>` is shared between the command thread (holds reference
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

Recompile on structural changes: create/delete track, add/move/remove clip,
undo/redo, project load.

Do **not** recompile on: transport changes (play/stop/seek), volume knob turns
(these should be atomic parameter updates, not full recompiles — see
`rules/anti-patterns.md` §2).

---

## Error Resilience

The compiler never panics. All error paths return `CompileError` and produce
an empty (silent) graph:

- `CycleDetected` → silent graph
- `NoMasterNode` → silent graph
- `MissingNode` in edge → silent graph
- Unknown plugin ID → `PassThroughDevice` (compile succeeds)

---

## Further Reading

- `../graph-compiler-design.md` — full algorithm with code examples
- `../snapshot-rebuild-flow.md` — how compilation fits into the command → audio pipeline
- `architecture/audio-graph.md` — graph-first philosophy
