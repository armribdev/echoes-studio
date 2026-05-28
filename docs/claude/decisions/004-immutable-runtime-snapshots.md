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
up at the start of the next block via non-blocking `try_lock`.

```
Command thread                          Audio thread
──────────────                          ────────────
compile new GraphScheduler
RuntimeSnapshot::enqueue(sched)  ───►  try_take() at block boundary
                                        Some → swap in, continue
                                        None → continue with current
```

The old scheduler is dropped on whichever thread last held it. Dropping on
the audio thread is unavoidable but happens at block boundaries, not mid-callback.

---

## Consequences

**Positive:**

- **Zero synchronization on the hot path.** The callback's `try_lock` is
  O(1). If uncontested (the common case), it is a single atomic operation.
- **No audio glitches on update.** The callback always has a valid, complete
  scheduler. There is no "partially updated" state.
- **No pausing required.** The stream continues uninterrupted during project
  edits. Perceived latency of a track change is one block duration (~10 ms).
- **Correct by construction.** The new scheduler is fully compiled and prepared
  before being handed to the audio thread. It cannot be in an intermediate state.
- **Overwrite semantics are safe.** If two commands arrive before the audio
  thread picks up the first scheduler, the second overwrites the first.
  The audio thread always gets the latest state. The dropped scheduler is
  deallocated on the command thread.

**Negative:**

- **Full reallocation on every structural change.** Even a small change
  (add one track) triggers a full buffer pre-allocation. For large projects
  this is dominated by `GraphScheduler::new()` which allocates
  `O(nodes × ports × block_size)` buffers.
- **Drop on audio thread.** Dropping the old scheduler (and its `Vec<Vec<AudioBuffer>>`)
  happens on the audio thread. Memory deallocation is not real-time safe, but
  occurs at block boundaries where a one-time stall is acceptable.
- **No DSP state transfer.** The new nodes are freshly initialized. A reverb's
  tail, a delay line's buffer, and a filter's state are lost on graph update.
  This is an audible click for active effects. Mitigated in future by state
  transfer during compilation (copy DSP state from old node to new node with
  matching `NodeId`).

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
