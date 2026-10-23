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

Nodes do not know their own `NodeId`: identity belongs to the graph
(`AudioGraph` / `ExecutionStep.node_id`), so constructors take only DSP
parameters (removed 2026-09-29, ADR-001 unchanged).

The compiler sizes `input_connections` from the node _kind_
(`num_inputs_for_kind`) and widens each step's output count from the prepared
node's `num_output_ports()` (`size_outputs_from_processors`, ADR-014).
`num_input_ports()` is a declared contract asserted by tests; the compiler does
not read it.

- A node with 0 inputs is a **source** (e.g. `TrackNode`).
- A node with 0 outputs is a **sink** (conceptually the `MasterNode`, though it
  declares 1 output for the scheduler's working buffer).

`TrackNode` plays each clip's source starting at `offset` seconds into the file
(the clip's left-trim), and `InstrumentNode` clamps each MIDI note to the clip's
playable window `[start, end]`. Both stem from the clip `offset` field — see
`decisions/018-clip-source-offset-trim.md`.

`InstrumentNode` turns notes into events and hands them to an inner
`InstrumentDevice`, chosen by the compiler (`build_instrument_device`) from
`AppTrack.instrument`: `None` → `PolySynth`; `builtin:sampler` → `Sampler`
(16 drum pads, one mono sample each, built from `AppTrack.sampler`; a new sampler track starts on the factory kit); any other
id → the CLAP plugin, or `SilentInstrument` if it cannot load. A sampler track
gets its `InstrumentNode` even before its first MIDI clip, so pads can be
auditioned; other instrument tracks still need a MIDI clip. See
`decisions/028-builtin-sampler-instrument.md`.

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
without changing the execution model — sidechain is the first case of this:
a plugin effect's `sidechain_source` (`PluginParams`) adds one extra edge from
another track's post-fader `TrackGainNode` into the plugin's aux input port,
resolved in a second pass over `from_project` once every track's tap point
exists (ADR-014 update, 2026-09-25). Multi-out is the second: a track's
`input_source` (`AppTrack`) puts a `BusNode` at the head of that track and
connects one aux output (port `1..`) of a plugin placement into it, so the
signal runs through the track's own effects and fader. It is resolved after
the sidechains; a routing that would close a cycle (exact node reachability)
is dropped. A plugin node's output count is only known once it is loaded:
the compiler widens the step to the prepared node's `num_output_ports()` and
cuts any edge from an output that does not exist (ADR-014 multi-out update).
VST3 plugins work the same way (ADR-014 amendment 2026-10-23), with one extra
step: the compiler tells the host which aux ports an edge reads before building
the nodes, and the plugin opens only those output buses.

Edges are only added during graph compilation on the command thread. The audio
thread never modifies the graph topology.

---

## Compilation Pipeline

Every structural project change triggers a full graph recompile. MIDI clip
content (`SET_CLIP_MIDI_NOTES`, `SET_CLIP_MIDI_CONTROLS`) does not: the command
thread publishes a new `MidiTrackContent` (notes + control points, one unit so
an undo swaps both) into the live instrument, and the audio thread adopts it on
the next block. A selection that does not change notes does not publish anything.
Placing or moving a note in the piano roll also pushes a short audition
(note on, then note off) through a lock-free queue on that track. The live
instrument plays it even when the transport is stopped. Offline export does
not attach the queue, so a render stays independent of audition.

Hardware MIDI input (ADR-022) uses the same queue-injection shape, on a
second, dedicated queue: a MIDI capture thread (owned by `midir`, one per
enabled device) pushes note events for the currently armed track, and
`InstrumentNode` drains them every block alongside the audition queue. Pitch
bend, control changes and aftertouch ride the same queue as
`MidiEventKind::Control` and reach CLAP instruments as `CLAP_EVENT_MIDI`;
built-in instruments ignore them (ADR-022 amendment 2026-10-13). This
is independent of `PortKind::Midi` graph edges, which the compiler still
rejects (Phase 5c, plugin instruments) — live hardware input never goes
through the graph/edge model at all.

The same capture path can record: while `MidiRecorder` is active, each note
and control of the armed track is also stamped with its transport position
and, when the recording ends (record button, pause or stop), becomes a MIDI
clip added through the regular `ADD_MIDI_CLIP` command — nothing new in the
graph.

Clip controls (ADR-029) are stored in beats like notes
(`AppMidiClip.channel_controls`) and cut by the same clip window. Because a
control is a state, `InstrumentNode` keeps a `ControlState`
(`audio_graph/midi_controls.rs`): on a seek, a loop wrap or a content swap it
re-sends the value the timeline holds at the block start; entering a clip sends
the clip's held value at its window start (points hidden by a left-trim count);
leaving a clip, pausing or stopping returns what the timeline moved to neutral
(bend centre, sustain / mod / pressure off). Timeline controls use
`MidiEventList::push_control`, which keeps the last 32 places of the 256-event
block for notes and live input.

The arpeggiator (ADR-030) is a note stage inside `InstrumentNode`, not a graph
node: after the clip notes, audition and live input are merged and sorted,
`Arpeggiator::process` (`audio_graph/arpeggiator.rs`, a pure events-in /
events-out struct with fixed arrays) either lets them through unchanged
(`Routing::Passthrough`, the case for every track without an enabled
arpeggiator) or replaces the note events by its own, one note at a time;
controls pass through. While the transport plays, step _k_ starts at the sample
of beat _k_ × step, with the same beats→samples function as clip notes; while it
is stopped, the grid starts on the chord and counts samples the stage owns.
Settings come from one atomic per setting (`ArpeggiatorCells`, installed per
instrument track at compile). The node's per-pitch counters, the envelope gate and the
recorder keep working on the played notes. This is the node's single note-effect
slot; a second note effect is the trigger for real `PortKind::Midi` edges
(ADR-030 option B).

The metronome click (ADR-026) is the other thing outside the graph: it is added
to the device output after the mix and the meters, so it is never in a track, a
meter or an export.

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
```

`TrackGainNode` reads volume and pan from live atomics and ramps both per
sample. A fader or pan drag streams those atomics (`set_track_mix`) and only
records a project event when the gesture ends, so the graph is not rebuilt.
The volume fader ramps in dB at 4 Hz; volume modulation and automation ride on
top of it with the faster ADR-019 smoother (12 Hz, linear), so an LFO on the
volume keeps its full depth and sharp edges. Mute/solo is a separate gate
(also a 4 Hz dB ramp) applied **after** modulation, so a muted or solo'd-out
track is silent whatever modulates its volume.

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
critical: the graph describes _what to process_, transport describes _where in
time we are_. These concerns must not be mixed.

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
