# Audio Graph Architecture

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
non-blocking. See `rules/realtime-safety-rules.md`.

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

## Immutable Runtime Graph Snapshots

The audio thread never sees a mutable graph. When the project changes, a
completely new `GraphScheduler` is compiled and atomically swapped in at the
next block boundary.

```
Command thread                    Audio thread
──────────────                    ────────────
modify AppProject
compile new GraphScheduler  ───►  try_swap(&mut current) at block start
                                  AtomicPtr::swap; old box → drop ring
                                  continue processing
```

This means:
- The audio callback has no synchronization overhead on the hot path.
- The graph is always in a consistent state from the audio thread's perspective.
- Partial graph updates are impossible — the swap is all-or-nothing.

The mechanism is `RuntimeSnapshot`: an `AtomicPtr<GraphScheduler>` for the
pending slot plus a bounded SPSC ring used to return displaced schedulers
to the command thread for deallocation. The command thread publishes via
`AtomicPtr::swap` and drains the drop ring on every `enqueue`. The audio
thread calls `try_swap(&mut current)` — lock-free, allocation-free, and
**no `drop` ever runs on the audio thread**.

See `systems/graph-compiler.md` and `../snapshot-rebuild-flow.md` for the full
compilation and swap sequence.

---

## Graph Compilation Lifecycle

Every structural project change triggers a full graph recompile:

```
AppProject
  ─► AudioGraph::from_project()    topology: nodes + edges
  ─► GraphCompiler::compile()      topological sort → ExecutionPlan
  ─► GraphScheduler::new()         pre-allocate all buffers
  ─► RuntimeSnapshot::enqueue()    atomic swap into audio thread
```

Transport changes (play/stop/seek) do **not** trigger recompilation. They are
passed directly to `process_block` via `TransportInfo`.

This separation is critical: the graph describes *what to process*, transport
describes *where in time we are*. These concerns must not be mixed.

---

## Deterministic Execution

The topological sort (Kahn's BFS) produces a stable execution order. Nodes at
the same depth are sorted by `NodeId` (monotonically incrementing). This
guarantees bit-identical output for the same project regardless of HashMap
iteration order or system thread scheduling.

Determinism is not just a nice property — it is required for:
- Reproducible offline export (same mixdown every time).
- Future A/B comparison of graph changes.
- Predictable test assertions.

---

## Further Reading

- `../audio-graph-architecture.md` — buffer safety model, execution step details,
  port connection invariants, atomic snapshot swap internals
- `systems/graph-compiler.md` — full compilation algorithm
- `decisions/001-graph-first-architecture.md` — why this model was chosen
