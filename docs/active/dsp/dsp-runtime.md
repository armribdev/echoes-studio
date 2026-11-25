# DSP Runtime

> **Canonical.** The runtime _execution model_ (how the scheduler runs a block).
> The audio-thread _prohibitions_ that used to live in the legacy
> `dsp-runtime-rules.md` are now consolidated in
> `../threading/realtime-safety.md`. The legacy flat file is archived under
> `docs/archive/dsp-runtime-rules.md`.

## Runtime Execution Model

The DSP runtime is the `GraphScheduler` running inside the CPAL audio callback.
It does exactly one thing: iterate a flat `Vec<ExecutionStep>` in topological
order and call each node's `process()`.

```rust
fn process_block(&mut self, transport: TransportInfo, output: &mut [f32]) {
    for (step_idx, step) in self.plan.steps.iter().enumerate() {
        // 1. Fill input pointer scratch from upstream output buffers
        for (port, conn) in step.input_connections.iter().enumerate() {
            self.input_ptr_scratch[port] = match conn {
                Some(ic) => &self.output_buffers[ic.src_step][ic.src_port],
                None     => &self.silence_buffer,
            };
        }

        // 2. Call the node
        let inputs  = unsafe { &self.input_ptr_scratch[..step.input_connections.len()] };
        let outputs = &mut self.output_buffers[step_idx];
        self.processors[step_idx].process(inputs, outputs, transport);
    }

    // 3. Write master output to CPAL interleaved buffer
    write_interleaved(&self.output_buffers[master_step], output);
}
```

There is no dynamic dispatch based on node type, no graph traversal, no
allocation. The structure is a tight loop over pre-computed data.

---

## Execution Plan Philosophy

The `ExecutionPlan` is the contract between the compiler and the scheduler.
It is immutable after compilation and shared via `Arc<ExecutionPlan>`.

Key properties:

- **Flat:** `steps` is a `Vec`, not a tree or graph. O(1) index access.
- **Pre-ordered:** topological sort ensures producers always precede consumers.
  The scheduler never needs to check ordering.
- **Self-contained:** `InputConnection { src_step, src_port }` is a direct index
  pair. No node ID lookup at runtime.
- **Correct by construction:** the invariant `src_step < current_step` is
  enforced by the compiler. Buffer aliasing is impossible.

---

## Deterministic Processing

Same graph + same transport state = same output, always.

Determinism sources:

1. **Topological sort** uses `NodeId` as tiebreaker within the same depth.
   `NodeId` is a monotonically incrementing `AtomicU64` assigned at node
   creation on the command thread (single-threaded creation → deterministic).
2. **Planar buffer layout** (`[ch0[0..N] | ch1[0..N]]`) — no interleaving
   means per-channel operations are independent and reproducible.
3. **Transport is passed, not polled** — `TransportInfo` is constructed from
   atomics at the start of each block and passed down to all nodes. Every node
   sees the same transport snapshot for the entire block.

---

## Audio Buffer Lifecycle

### Allocation phase (`GraphScheduler::new`)

```
output_buffers[step][port] — Vec<Vec<AudioBuffer>>
silence_buffer             — single zeroed AudioBuffer, never written
input_ptr_scratch          — Vec<*const AudioBuffer>
```

All memory is allocated once per graph update, never in the callback.

### Processing phase (`process_block`)

- `output_buffers[i]` is written by step `i` and read by all steps `j > i`
  that have `src_step == i`.
- The scheduler clears output buffers only via `node.process()` — nodes are
  responsible for initializing their own output. Most nodes call
  `output.clear()` at the start of `process()`.
- `silence_buffer` is never written. It is a read-only sentinel for unconnected
  inputs.

### Deallocation phase

When a new `GraphScheduler` replaces the old one:

- The audio thread **never** drops the displaced scheduler. It pushes the
  `Box<GraphScheduler>` into `RuntimeSnapshot`'s bounded SPSC drop ring.
- The command thread drains the drop ring on its next `enqueue` call and runs
  the actual `drop` (freeing every node, output buffer, clip-sample `Arc`,
  reverb tail, etc.) off the real-time thread.
- This is the C1 fix from the 2026-05-29 audit (`../audits/audio-engine-audit-2026-05-29.md`).

---

## Scheduling Constraints

- `process_block` must complete within one block period.
  At 48 kHz / 512 frames: `512 / 48000 ≈ 10.7 ms`.
  At 44.1 kHz / 64 frames: `64 / 44100 ≈ 1.45 ms`.
- Nodes with expensive DSP (convolution reverb, resamplers) must be profiled
  against the worst-case block size (64 frames at 44.1 kHz).
- Deep recursion in `process()` risks stack overflow on constrained platforms.
  Keep per-block stack usage below a few KB.

---

## Parallel Level Execution (opt-in)

By default the scheduler runs steps sequentially on the audio thread. When
`ECHOES_AUDIO_WORKERS > 0` (env var, default `0`), the compiler groups
independent steps into `plan.levels` and a pre-spawned `WorkerPool`
(`audio_graph/worker_pool.rs`) processes each level in parallel. The audio
thread joins the pool as the last worker and spin-waits on a completion barrier
between levels.

Determinism is unchanged: every step within a level writes its own output
buffer, so execution order across workers cannot affect the result (the
`parallel_pool_produces_same_output_as_sequential` test guards this).

Each worker, **once at start-up** (off the audio path), mirrors the audio
callback thread so worker-run DSP behaves identically:

- enables FTZ/DAZ (`enable_flush_to_zero`) — MXCSR is per-thread, so without this
  a denormal on a worker would not be flushed (CPU spike) **and** could diverge
  from the FTZ-enabled sequential path;
- raises its scheduling priority (Windows: `THREAD_PRIORITY_TIME_CRITICAL`,
  best-effort; no-op elsewhere) so a normal-priority thread cannot preempt it
  mid-block while the audio thread spin-waits (see
  `../threading/realtime-safety.md` → Priority Inversion Prevention).

A spurious `park()` wake-up with no published job re-parks instead of panicking,
so a worker can never die and stall every later dispatch on the barrier. Full RT
priority (MMCSS "Pro Audio" / `SCHED_FIFO`) is a later, dedicated change.

---

## Offline Export (`audio_engine/offline.rs`)

The export drives the same `GraphScheduler` from the command thread, with two
rules the live callback does not have (2026-10-26) and two guards on the plugins
it shares with the live stream (2026-11-25, ADR-036):

- **Plugins held.** The caller holds the `LiveStreamGate` first
  (`AudioEngine::hold_stream_for_export`), so the live callback is silent and no
  other caller reaches the shared plugin instances while the export renders.
- **Clean start.** `render_project_offline` calls `GraphScheduler::reset()` on the
  freshly compiled scheduler, so a plugin forgets what the live stream played.

- **Latency.** The master output lags the timeline by
  `GraphScheduler::latency_samples()` (the plan's cumulative latency at the master
  step, ADR-006). `render_project_offline` renders that many extra frames and
  drops the first `N`, so the file starts at timeline zero.
- **Tail.** After the last clip (`timeline_end_secs`) it keeps rendering until the
  output has stayed under −80 dBFS for 2.5 s or the tail reaches 10 s, then drops
  the trailing silence. The file ends at the later of the last clip's end and the
  last audible sample. The 2.5 s hold is longer than the internal delay's maximum
  time (2 s); a plugin echo spaced further apart would be cut.

`render_project_in_blocks` (the C-2 harness path) applies none of these: it renders
exactly the timeline length, without reset or gate, so the golden hashes do not move. Both share
`compile_for_render` and `BlockRenderer`. The live transport still stops at
`timeline_end_secs` (`duration_samples`); only the export extends.

---

## Runtime Isolation Rules

1. The audio thread never reads from `AppProject`. It only reads from the
   `ExecutionPlan` and the pre-allocated node state.
2. The audio thread never writes to `AppProject` or `EventStore`.
3. The audio thread never calls Tauri IPC.
4. The only output path is: node writes to `output_buffers[step]` →
   scheduler writes interleaved to CPAL buffer → OS delivers to device.
5. The only input to the runtime is: `TransportInfo` (from atomics) +
   `GraphScheduler` (from `RuntimeSnapshot::try_swap()`).

---

## Snapshot Swap at Runtime

At the start of each audio block the CPAL closure calls:

```rust
// `scheduler` is a Box<GraphScheduler> owned by the closure.
snapshot.try_swap(&mut scheduler);
scheduler.process_block(&transport, output);
```

`try_swap` performs a single `AtomicPtr::swap(null)`. If a new scheduler was
pending, it is moved into `scheduler` and the previous box is pushed into the
SPSC drop ring (freed later by the command thread). If nothing was pending, the
current scheduler is left untouched and the block runs against stale-but-valid
state — imperceptible at one block (~10 ms at 48 kHz / 512 frames).

The audio thread never blocks, never allocates, and never drops.

---

## Further Reading

- `../threading/realtime-safety.md` — full audio-thread prohibition list + checklist
- `graph-compiler.md` — how the `ExecutionPlan` is built
- `../architecture/audio-graph.md` — buffer safety model, port invariants
- `../architecture/snapshot-rebuild-flow.md` — how a project change reaches the runtime
