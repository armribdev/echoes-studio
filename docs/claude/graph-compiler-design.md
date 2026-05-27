# Graph Compiler Design

## Responsibility

The `GraphCompiler` translates an `AudioGraph` (editor-side DAG) into a
`CompiledGraph` that the `GraphScheduler` can execute without any graph
traversal at runtime.

## Input / Output

```
Input:  AudioGraph { nodes: HashMap<NodeId, GraphNode>, edges: Vec<Edge> }
        AppProject  (for node instantiation data)
        CompileConfig { sample_rate, max_block_size, num_channels }

Output: CompiledGraph {
    plan:       Arc<ExecutionPlan>,
    processors: Vec<Box<dyn AudioNode>>,
}
```

## Compilation Algorithm

### Step 1: Validate

- `graph.master_id` must be `Some` — every graph has exactly one master.
- All edge endpoints must reference existing nodes.
- (Cycle detection is implicit in Step 2.)

### Step 2: Topological Sort (Kahn's BFS)

```
1. Compute in-degree for every node.
2. Seed a queue with all zero-in-degree nodes (sorted by NodeId for determinism).
3. BFS: for each dequeued node, reduce neighbours' in-degrees.
         Enqueue neighbours that reach zero.
4. If |order| < |nodes|, a cycle exists → CompileError::CycleDetected.
```

The sort is **stable** (deterministic for identical graphs regardless of
HashMap iteration order) because:
- Nodes at the same topological depth are sorted by `NodeId` (monotonically
  increasing allocation order).
- `NodeId` allocation uses `AtomicU64` with `Relaxed` ordering, so within a
  single thread (all graph building is on the command thread) the order is
  deterministic.

### Step 3: Assign Step Indices

```
step_index: HashMap<NodeId, usize>
```

Maps each `NodeId` to its position in the `steps` array. Used to convert
graph-level `PortId` references into flat array indices for the scheduler.

### Step 4: Build ExecutionSteps

For each node in topological order:
1. Determine `num_inputs` and `num_outputs` from `NodeKind`.
2. For each of the node's input ports (0..num_inputs), find any edge with
   `to.node == node_id && to.port == port_idx`.
3. If an edge exists, record `InputConnection { src_step, src_port }`.
4. If no edge, record `None` (unconnected → silence buffer at runtime).

### Step 5: Instantiate Nodes

`build_node(id, kind, project)` dispatches on `NodeKind`:

| NodeKind          | Produces                         | Data source           |
|-------------------|----------------------------------|-----------------------|
| Track { id }      | TrackNode with ClipData list     | AppProject.tracks     |
| TrackGain { gain} | GainNode(gain)                   | gain from NodeKind    |
| Gain { linear }   | GainNode(linear)                 | NodeKind field        |
| Pan { pos }       | PanNode(pos)                     | NodeKind field        |
| Bus               | BusNode(N inputs)                | constant              |
| Master            | MasterNode(N inputs)             | constant              |
| Meter { id }      | MeterNode(id)                    | meter_id string       |
| Plugin { id }     | PluginHost::instantiate(id)      | plugin_system         |

For unknown plugin IDs, `build_node` logs a warning and returns a
`PassThroughDevice` (silence pass-through) so compilation always succeeds.

### Step 6: Prepare Nodes

```rust
for node in &mut processors {
    node.prepare(PrepareContext { sample_rate, max_block_size, num_channels });
}
```

`prepare` is the one place where nodes may allocate (filter coefficients,
delay lines, FFT buffers, etc.).

## ExecutionPlan Structure

```rust
ExecutionPlan {
    steps: Vec<ExecutionStep>,
    modulation: ModulationMap,
    sample_rate: f32,
    max_block_size: usize,
    num_channels: usize,
}

ExecutionStep {
    node_id: NodeId,
    input_connections: Vec<Option<InputConnection>>,
    num_outputs: usize,
}

InputConnection {
    src_step: usize,   // always < current step index (invariant)
    src_port: usize,
}
```

## Re-compilation Triggers

A new graph is compiled whenever:
- A track is created or deleted
- A clip is added, moved, or removed
- Track volume changes (gain node needs new value)
- Track mute/solo state changes
- Any structural routing change (plugin insert, bus connection, etc.)

**Transport changes** (play/stop/seek) do **not** trigger recompilation.
They update `TransportInfo` which is passed to `process_block` directly.

## Future: Incremental Compilation

For projects with many tracks and plugins, full recompilation on every
parameter change is wasteful. Future optimisations:

1. **Dirty marking**: only re-instantiate nodes whose `NodeKind` changed.
2. **State transfer**: copy DSP state from the old node to the new one
   (preserves reverb tails, filter states, etc.).
3. **Dedicated compiler thread**: move compilation off the command thread
   to avoid blocking the gateway during complex projects.

## Error Handling

Compilation errors are logged but **never fatal**:
- `CompileError::CycleDetected` → use empty graph (silence)
- `CompileError::NoMasterNode` → use empty graph
- `CompileError::MissingNode` → use empty graph

This ensures the audio engine always has a valid (if silent) graph to execute.
Project errors are reported via `eprintln!` and surfaced to the UI through
future structured error events.
