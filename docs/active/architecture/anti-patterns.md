# Anti-Patterns

Architectural mistakes that have been explicitly identified and must not
be introduced. Each entry explains what the mistake is and why it is dangerous.

---

## 1. Coupling Plugins to Tracks

**The mistake:** Treating plugins as fields on a track struct rather than nodes
in the graph.

```rust
// ✗ Wrong
struct AppTrack {
    id: String,
    volume: f32,
    insert_chain: Vec<PluginInstance>,   // coupled
    send_effects: Vec<PluginInstance>,   // coupled
}
```

**Why it is dangerous:**

- It hardcodes a linear insert chain topology. Parallel processing, feedback
  loops, and complex routing become impossible without hacking the track struct.
- It makes the plugin count a property of the track model, not the graph.
  A track with 0 plugins and a track with 8 plugins are structurally different
  objects in the project model — they should differ only in graph topology.
- It spreads the execution model across two abstractions (track + graph) instead
  of one.

**Correct approach:** A track is a `TrackNode` in the graph. An insert plugin is
another node connected downstream. The graph topology encodes the chain.

---

## 2. Rebuilding the Graph During Playback for Non-Structural Changes

**The mistake:** Triggering full graph recompilation for parameter changes that
don't alter the graph topology.

```rust
// ✗ Wrong: recompile just because gain changed
fn set_track_volume(volume: f32) {
    project.track.volume = volume;
    recompile_graph();   // unnecessary
}
```

**Why it is dangerous:**

- Graph compilation instantiates new nodes and pre-allocates new buffers.
  At large track counts this takes measurable time.
- Doing it on every knob twist blocks the command thread.
- It creates a new scheduler on every parameter change, generating GC pressure
  from dropping the old scheduler.

**Correct approach:** Parameter changes that don't affect topology (gain, pan,
mute, solo on already-existing nodes) should update atomic values read directly
by the node's `process()` call, or queue parameter changes via the approved
mechanism. Graph recompilation is for structural changes only (add/remove
nodes or edges).

---

## 3. UI-Driven DSP Logic

**The mistake:** Placing audio processing logic in the frontend or in IPC
command handlers.

```typescript
// ✗ Wrong: computing gain in the frontend
function applyVolume(samples: Float32Array, volume: number): Float32Array {
	return samples.map((s) => s * volume);
}
```

**Why it is dangerous:**

- The Web Audio API (OfflineAudioContext) is used only for export — not for the
  production audio path, which runs via CPAL.
- UI-side DSP has no access to the real-time graph, sample-accurate transport,
  or the modulation system.
- It creates two code paths for the same operation: one for export (JS) and one
  for playback (Rust). These inevitably diverge.

**Correct approach:** All DSP logic belongs in `audio_graph/nodes/`. The
frontend is a reactive display layer. Export uses `OfflineAudioContext` only as
a temporary measure (Phase 1/2) until the Rust offline renderer is implemented.

---

## 4. Direct Graph Mutation from the UI Thread

**The mistake:** Modifying graph topology or node state directly from Tauri
command handlers without going through the full compile → snapshot pipeline.

```rust
// ✗ Wrong
#[tauri::command]
fn add_plugin(state: State<AppState>, node_id: NodeId, plugin_id: String) {
    let mut scheduler = state.audio_engine.scheduler.lock().unwrap();
    scheduler.processors.push(build_node(plugin_id));  // direct mutation
}
```

**Why it is dangerous:**

- The audio thread is iterating `processors` in `process_block()` at the same
  time. Even with locking, this creates unpredictable timing.
- The execution plan (`plan.steps`) no longer matches the processors array.
  The scheduler will index out-of-bounds or process nodes in wrong order.
- Bypasses graph validation (cycle detection, master-exists check).

**Correct approach:** All structural changes go through
`AudioGraph::from_project() → GraphCompiler::compile() → RuntimeSnapshot::enqueue()`.
The audio thread never touches a graph that is being modified.

---

## 5. Mixing Runtime State and Project State

**The mistake:** Storing real-time playback values inside `AppProject` or the
`EventStore`.

```rust
// ✗ Wrong
struct AppProject {
    tracks: Vec<AppTrack>,
    current_position_samples: u64,   // runtime state leaking in
    output_peak_left: f32,           // runtime state leaking in
}
```

**Why it is dangerous:**

- `AppProject` is serialized to disk and replayed for undo/redo. Runtime values
  corrupt the undo history.
- The event store would log `POSITION_UPDATED` thousands of times per second,
  creating an unbounded log.
- Undo of "seek to bar 4" is not a meaningful user operation.

**Correct approach:** Runtime state (transport position, meters) lives in
atomics owned by `AudioEngine`. The UI reads them via polling commands or a
dedicated meter relay thread. They are never part of the event log.

---

## 6. Excessive Event Sourcing

**The mistake:** Event-sourcing state that changes at real-time rates or that
has no meaningful undo semantics.

```typescript
// ✗ Wrong: emitting and storing position events
eventStore.append({ type: 'POSITION_UPDATED', position: 48000 });
eventStore.append({ type: 'PEAK_METER', left: 0.8, right: 0.7 });
```

**Why it is dangerous:**

- An event log entry is written for every `POSITION_UPDATED` at 30 fps.
  Over a 10-minute session this is ~18,000 events — all meaningless for undo.
- The frontend's `ProjectEventStore` replays all events on `PROJECT_RESET`.
  A bloated log makes undo/redo slow.
- Real-time events are lossy by nature. Replaying them produces different state
  than the original (the position at undo time is not the same as at record time).

**Correct approach:** `POSITION_UPDATED`, `PEAK_METER`, `CLIP_PLAYING`, and
`LATENCY_WARNING` are **runtime events**. They are forwarded directly to the
UI via `engineRuntimeEvents` (a separate Svelte store) and never persisted in
the `ProjectEventStore`.

---

## 7. Plugin-Internal Modulation

**The mistake:** Implementing modulators (LFOs, envelopes) inside a plugin node.

```rust
// ✗ Wrong: LFO inside a plugin
fn process(&mut self, ...) {
    let mod_val = self.lfo.advance();
    let gain = self.base_gain + mod_val * self.depth;
    ...
}
```

**Why it is dangerous:**

- The modulator is invisible to the graph's modulation routing system.
- It cannot be reused across parameters or across nodes.
- It cannot be automated from the timeline.
- It couples DSP topology to plugin implementation, defeating the graph model.

**Correct approach:** The plugin has a `gain` parameter. An `LfoNode` exists in
the graph and connects to that parameter via a modulation edge. The scheduler
applies the modulation value before calling `process()`. See
`../modulation/modulation-system.md`.

---

## 8. Allocating Before the First `prepare()` Call is Complete

**The mistake:** Deferring pre-allocation to the first `process()` call.

```rust
// ✗ Wrong: lazy initialization in process()
fn process(&mut self, ...) {
    if self.filter_coeffs.is_none() {
        self.filter_coeffs = Some(compute_coeffs(self.cutoff));  // allocates!
    }
    ...
}
```

**Why it is dangerous:**

- The first block after the graph is swapped in will allocate, which may stall
  the audio callback and cause a click or underrun.
- There is no guarantee about when the first `process()` call occurs relative
  to scheduler setup.

**Correct approach:** All allocation happens in `prepare(PrepareContext)`. By
the time `process()` is called, the node is fully initialized.

---

## 9. Hardcoding Channel Count

**The mistake:** Writing DSP code that assumes stereo (2 channels) rather than
reading from `PrepareContext` or `AudioBuffer::num_channels()`.

```rust
// ✗ Wrong
outputs[0].channel_mut(0)[i] = left;
outputs[0].channel_mut(1)[i] = right;  // panics if num_channels != 2
```

**Why it is dangerous:**

- The user may configure a mono output device or a multichannel interface.
- The channel count is part of the `CompileConfig` passed to the compiler
  and propagated to every node via `PrepareContext`.

**Correct approach:** Read `ctx.num_channels` in `prepare()` and loop over
channels dynamically.
