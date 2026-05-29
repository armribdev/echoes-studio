# ADR-004 — Immutable Runtime Snapshots

## Status

Accepted. Implemented in `audio_engine/snapshot.rs` and `audio_graph/snapshot.rs`.

---

## Context

The audio graph must be updatable at runtime (user adds a track, changes volume,
triggers undo) without interrupting the CPAL audio callback.

This requires a safe handoff mechanism between the command thread (which
compiles the new graph) and the audio thread (which is executing the current
graph in a tight real-time loop).

Three approaches were considered:

1. **In-place mutation with fine-grained locking.** The scheduler holds a
   `RwLock<AudioGraph>`. Structural changes acquire the write lock; the callback
   acquires the read lock.
2. **Pause and resume.** Stop the CPAL stream, update the graph, restart the
   stream.
3. **Immutable snapshot swap.** Compile a completely new `GraphScheduler` on
   the command thread. The audio callback picks it up atomically at the next
   block boundary.

---

## Decision

**Immutable snapshot swap.** Every structural project change produces a fresh
`GraphScheduler` (pre-allocated buffers, pre-compiled execution plan). The
command thread deposits it in `RuntimeSnapshot`. The audio callback picks it
up at the start of the next block via a lock-free `AtomicPtr::swap`.

```
Command thread                          Audio thread
──────────────                          ────────────
drain SPSC drop ring (free dead boxes)
compile new GraphScheduler
RuntimeSnapshot::enqueue(sched)  ───►  try_swap(&mut current) at block boundary
  AtomicPtr publishes new box            AtomicPtr swap; new box owned here
                                         displaced box → drop ring
                                         (command thread frees it next enqueue)
```

The audio thread **never** drops the displaced scheduler — it hands it back
through a bounded SPSC ring buffer, and the actual `drop` runs on the
command thread.

> **Revision history**: the original v1 implementation used
> `Mutex<Option<GraphScheduler>>` with `try_lock` on the audio thread, and
> the old scheduler was dropped inline by the callback. The 2026-05-29 audit
> identified both as RT-safety violations (issues C1 and C2). The current
> design — `AtomicPtr` + SPSC drop ring — supersedes that approach.

---

## Consequences

**Positive:**

- **Zero synchronization on the hot path.** The callback's `try_swap` is
  O(1) — a single `AtomicPtr::swap`, never blocks, no mutex anywhere.
- **No deallocation on the audio thread.** Displaced schedulers ride the
  SPSC drop ring back to the command thread, which `drop`s them off the
  real-time path.
- **No audio glitches on update.** The callback always has a valid, complete
  scheduler. There is no "partially updated" state.
- **No pausing required.** The stream continues uninterrupted during project
  edits. Perceived latency of a track change is one block duration (~10 ms).
- **Correct by construction.** The new scheduler is fully compiled and prepared
  before being handed to the audio thread. It cannot be in an intermediate state.
- **Overwrite semantics are safe.** If two commands arrive before the audio
  thread picks up the first scheduler, the `AtomicPtr::swap` in the second
  `enqueue` returns the previous pointer and frees it on the command thread.
  The audio thread always gets the latest state.

**Negative:**

- **Full reallocation on every structural change.** Even a small change
  (add one track) triggers a full buffer pre-allocation. For large projects
  this is dominated by `GraphScheduler::new()` which allocates
  `O(nodes × ports × block_size)` buffers.
- **No DSP state transfer.** The new nodes are freshly initialized. A reverb's
  tail, a delay line's buffer, and a filter's state are lost on graph update.
  This is an audible click for active effects. Mitigated in future by state
  transfer during compilation (copy DSP state from old node to new node with
  matching `NodeId`).
- **Drop-ring capacity is a static constant.** In the (currently unreachable
  by construction) case of overflow, the audio thread leaks the displaced
  box rather than dropping it on the RT path. Capacity is sized well above
  the worst-case steady-state depth.

---

## Alternatives Considered

### Fine-Grained Locking (Rejected)

Even with `RwLock`, a write-lock acquisition can block an arbitrary number of
read-lock holders (or vice versa depending on the fairness policy). The audio
callback cannot afford to wait for any lock held by the UI thread. Priority
inversion risk is unacceptable.

### Pause and Resume (Rejected)

Stopping the CPAL stream introduces an audible gap (the output buffer drains
to silence). Restarting has OS-dependent latency (milliseconds to hundreds of
milliseconds). This is unacceptable for any operation as common as changing a
track's volume.
