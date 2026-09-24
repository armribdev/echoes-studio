# Audio Graph Architecture

> **Canonical.** Merges the former `architecture/audio-graph.md` (philosophy) and
> the legacy `audio-graph-architecture.md` (implementation reference).
> The legacy flat file is archived under `docs/archive/audio-graph-architecture.md`.

## Graph-First Philosophy

The `AudioGraph` is the single core abstraction of Echoes Studio's DSP engine.
Tracks, buses, sends, effects, and plugins are all nodes in this graph.
Nothing in the audio engine exists outside of it.

This is a deliberate architectural constraint. It means:

- The engine has one execution model for every kind of processing.
- Adding a new DSP primitive (a new effect type, a new routing topology, a
  modulator) is always the same operation: implement `AudioNode`, register it
  in the graph.
- The graph compiler, scheduler, and buffer allocator are written once and
  apply to all node types uniformly.

Contrast with a **track-centric** model, where tracks are first-class objects
and plugins are subordinate. In a track-centric model, adding a send channel
or a modulator requires special-casing outside the plugin chain. In the
graph-first model, a send is just a node with two outputs.

> **Rule:** When a new DSP feature is needed, the answer is always "add a
> graph node," never "add a special field to AppTrack."

See `decisions/001-graph-first-architecture.md` for why this model was chosen.

---

## Node Abstraction Model

Every DSP element implements the `AudioNode` trait:

```rust
pub trait AudioNode: Send + 'static {
    fn node_id(&self) -> NodeId;
    fn num_input_ports(&self) -> usize;
    fn num_output_ports(&self) -> usize;
    fn prepare(&mut self, ctx: PrepareContext);
    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [&mut AudioBuffer], transport: TransportInfo);
}
```

`prepare` runs once before streaming starts. It is the only place where a node
may allocate memory (filter coefficients, delay lines, FFT buffers).

`process` runs in the audio callback. It must be allocation-free and
non-blocking. See `../threading/realtime-safety.md`.

### Port model

Every node declares `num_input_ports()` and `num_output_ports()`. The compiler
uses these counts to size the `input_connections` array and pre-allocate
`output_buffers[step]`.

- A node with 0 inputs is a **source** (e.g. `TrackNode`).
- A node with 0 outputs is a **sink** (conceptually the `MasterNode`, though it
  declares 1 output for the scheduler's working buffer).

`TrackNode` plays each clip's source starting at `offset` seconds into the file
(the clip's left-trim), and `InstrumentNode` clamps each MIDI note to the clip's
playable window `[start, end]`. Both stem from the clip `offset` field — see
`decisions/018-clip-source-offset-trim.md`.

---

## Routing Philosophy

Nodes are connected by directed edges. An edge from node A port 0 to node B
port 1 means: "the output of A's port 0 is the input of B's port 1."

The current default topology for a project with N tracks is:

```
TrackNode₁ ──► GainNode₁ ──┐
TrackNode₂ ──► GainNode₂ ──┤
       ...                  ├──► MasterNode ──► CPAL output
TrackNodeₙ ──► GainNodeₙ ──┘
```

This is not special-cased. It is the result of `AudioGraph::from_project()`
constructing a DAG of nodes and edges from the `AppProject` model. As routing
complexity grows (buses, sends, side chains), the graph topology changes
without changing the execution model.

Edges are only added during graph compilation on the command thread. The audio
thread never modifies the graph topology.

---

## Compilation Pipeline

Every structural project change triggers a full graph recompile. MIDI note
content (`SET_CLIP_MIDI_NOTES`) does not: the command thread publishes a new
note list into the live instrument, and the audio thread adopts it on the next
block. A selection that does not change notes does not publish anything.

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

### 1. AppProject → AudioGraph

`AudioGraph::from_project(project)` converts the flat project model into a
directed acyclic graph (DAG). Each track becomes:

```
TrackNode ──► TrackGainNode ──► MasterNode

`TrackGainNode` reads volume and pan from live atomics and ramps both per
sample. A fader or pan drag streams those atomics (`set_track_mix`) and only
records a project event when the gesture ends, so the graph is not rebuilt.
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
   each step. The graph is format-agnostic: it calls `NodeFactory` which
   dispatches to built-in nodes or plugin wrappers.
4. **Prepare** — `node.prepare(ctx)` is called for every node; this is the
   only place where nodes may allocate.

Full algorithm in `../dsp/graph-compiler.md`.

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

Transport changes (play/stop/seek) do **not** trigger recompilation. They are
passed directly to `process_block` via `TransportInfo`. This separation is
critical: the graph describes *what to process*, transport describes *where in
time we are*. These concerns must not be mixed.

Full runtime detail in `../dsp/dsp-runtime.md`.

---

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

### Safety model for buffer sharing

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

---

## Immutable Runtime Graph Snapshots

The audio thread never sees a mutable graph. When the project changes, a
completely new `GraphScheduler` is compiled and atomically swapped in at the
next block boundary.

```
Command thread                          Audio thread
───────────────                         ────────────
drain drop ring (free dead boxes)
build new GraphScheduler
RuntimeSnapshot::enqueue()         ───►  try_swap(&mut current) at block start
  AtomicPtr::swap publishes ptr          AtomicPtr::swap(null) → take new Box
                                         push displaced Box to drop ring
                                         (no drop, no alloc, no lock)
```

This means:
- The audio callback has no synchronization overhead on the hot path.
- The graph is always in a consistent state from the audio thread's perspective.
- Partial graph updates are impossible — the swap is all-or-nothing.

The mechanism is `RuntimeSnapshot`: an `AtomicPtr<GraphScheduler>` for the
pending slot plus a bounded SPSC ring used to return displaced schedulers to
the command thread for deallocation. The command thread publishes via
`AtomicPtr::swap` and drains the drop ring on every `enqueue`. The audio thread
calls `try_swap(&mut current)` — lock-free, allocation-free, and **no `drop`
ever runs on the audio thread**. If nothing is pending, `try_swap` is a single
atomic load and the audio thread keeps its current scheduler.

See `snapshot-rebuild-flow.md` and `decisions/004-immutable-runtime-snapshots.md`.

---

## Deterministic Execution Order

The topological sort (Kahn's BFS) produces a stable execution order. Nodes at
the same depth are sorted by `NodeId` (monotonically incrementing). This
guarantees bit-identical output for the same project regardless of `HashMap`
iteration order or system thread scheduling.

Determinism is not just a nice property — it is required for:
- Reproducible offline export (same mixdown every time).
- Future A/B comparison of graph changes.
- Predictable test assertions.

---

## Further Reading

- `../dsp/graph-compiler.md` — full compilation algorithm + execution plan
- `../dsp/dsp-runtime.md` — scheduler loop, buffer lifecycle, isolation rules
- `snapshot-rebuild-flow.md` — command → compile → enqueue → swap sequence
- `../threading/realtime-safety.md` — audio-thread prohibitions
- `decisions/001-graph-first-architecture.md` — why graph-first
- `decisions/004-immutable-runtime-snapshots.md` — why immutable snapshots
