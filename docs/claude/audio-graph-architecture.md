# Audio Graph Architecture

## Core Principle

The audio graph is the central system of Echoes Studio. Everything else — plugins,
tracks, buses, automation — are implementations of graph nodes.

```
AppProject (editor state)
    │
    ▼  AudioGraph::from_project()
AudioGraph (topology: nodes + edges)
    │
    ▼  GraphCompiler::compile()
CompiledGraph { plan: Arc<ExecutionPlan>, processors: Vec<Box<dyn AudioNode>> }
    │
    ▼  GraphScheduler::new()
GraphScheduler (owns buffers + processors)
    │
    ▼  RuntimeSnapshot::enqueue()
Audio thread picks up at next block boundary
    │
    ▼  GraphScheduler::process_block()
Interleaved f32 output → CPAL device
```

## Pipeline Stages

### 1. AppProject → AudioGraph

`AudioGraph::from_project(project)` converts the flat project model into a
directed acyclic graph (DAG).  Each track becomes:

```
TrackNode ──► TrackGainNode ──► MasterNode
```

Future routing (sends, buses, plugins) adds more nodes and edges without
changing downstream stages.

### 2. AudioGraph → CompiledGraph

`GraphCompiler::compile(graph, project, config)`:

1. **Topological sort** (Kahn's BFS) — produces a deterministic execution order
   where every producer strictly precedes its consumers.
2. **Step construction** — each node becomes an `ExecutionStep` with its
   `input_connections` list and `num_outputs`.
3. **Node instantiation** — `build_node()` creates a `Box<dyn AudioNode>` for
   each step.  The graph is format-agnostic: it calls `NodeFactory` which
   dispatches to built-in nodes or plugin wrappers.
4. **Prepare** — `node.prepare(ctx)` is called for every node; this is the
   only place where nodes may allocate.

### 3. CompiledGraph → GraphScheduler

`GraphScheduler::new(compiled)` pre-allocates:
- `output_buffers[step][port]` — one `AudioBuffer` per output port per step
- `silence_buffer` — a permanently-zeroed sentinel for unconnected ports
- `input_ptr_scratch` — raw pointer array sized to `MAX_NODE_INPUT_PORTS`

No further allocation occurs until the next graph update.

### 4. GraphScheduler in the Audio Callback

`process_block(transport, output)`:

1. **For each step** in topological order:
   - Fill `input_ptr_scratch` with pointers to upstream output buffers (or silence).
   - Call `node.process(inputs, outputs, transport)`.
2. **Write master output** — read the master node's summed signal into
   `interleaved_output`.

## Buffer System

Buffers use **planar layout**: channel 0 samples, then channel 1 samples, etc.

```rust
data = [ ch0[0..frames] | ch1[0..frames] | … ]
```

Key operations (all allocation-free):
- `clear()` — zero all samples
- `copy_from(src)` — memcpy
- `mix_from(src, gain)` — accumulate (used for bus mixing)
- `apply_gain(gain)` — multiply in-place
- `mono_to_all_channels()` — duplicate ch0 to all channels

## Safety Model for Buffer Sharing

The scheduler uses raw pointers to create non-aliasing buffer views:

```
output_buffers[0] ──► TrackNode output   (read by downstream nodes)
output_buffers[1] ──► GainNode output    (currently being written)
output_buffers[2] ──► MasterNode output
```

**Safety invariant**: for step `i`, all `input_connections[*].src_step < i`
(guaranteed by topological sort). Therefore the write target (`output_buffers[i]`)
and all read sources (`output_buffers[src_step]`) are distinct elements.

This invariant is enforced by the `GraphCompiler` and asserted at compile time.

## Atomic Snapshot Swap

```
Compiler thread              Audio thread
───────────────              ────────────
build new GraphScheduler
RuntimeSnapshot::enqueue() ──► try_take() at block start
                                if Some → swap in new scheduler
                                if None → continue with current
```

`try_take` uses `Mutex::try_lock` (non-blocking). If the compiler thread holds
the lock at the exact block boundary (extremely rare), the audio thread silently
skips the update for one block. No audio glitch occurs.

## Node Port Model

Every node declares:
- `num_input_ports()` — how many audio inputs it reads
- `num_output_ports()` — how many audio outputs it produces

The compiler uses these counts to size the `input_connections` array and
pre-allocate `output_buffers[step]`.

A node with 0 inputs is a **source** (e.g. `TrackNode`).
A node with 0 outputs is a **sink** (conceptually the `MasterNode`, though it
declares 1 output for the scheduler's working buffer).

## Deterministic Execution Order

Kahn's algorithm produces a stable topological order:
- Nodes with no incoming edges come first.
- Among nodes at the same depth, they are sorted by `NodeId` (monotonically
  incrementing) for bit-identical renders.

This makes the audio output reproducible for the same project file regardless
of `HashMap` iteration order or threading.
