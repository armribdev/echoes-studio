# DSP Runtime

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
- The audio thread **never** dropps the displaced scheduler. It pushes the
  `Box<GraphScheduler>` into `RuntimeSnapshot`'s bounded SPSC drop ring.
- The command thread drains the drop ring on its next `enqueue` call and
  runs the actual `drop` (freeing every node, output buffer, clip-sample
  `Arc`, reverb tail, etc.) off the real-time thread.
- This is the C1 fix from the 2026-05-29 audit.

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
pending, it is moved into `scheduler` and the previous box is pushed into
the SPSC drop ring (freed later by the command thread). If nothing was
pending, the current scheduler is left untouched and the block runs against
stale-but-valid state — imperceptible at one block (~10 ms at 48 kHz / 512
frames).

The audio thread never blocks, never allocates, and never drops.

---

## Further Reading

- `../dsp-runtime-rules.md` — full prohibition list with code examples
- `../realtime-safety-rules.md` — thread model, CPAL integration notes
- `../audio-graph-architecture.md` — buffer safety model, port invariants
- `../snapshot-rebuild-flow.md` — how a project change reaches the runtime
