# ADR-002 — Event Sourcing Scope

## Status

Accepted. Implemented in `engine_gateway/store.rs` and `src/shared/project/`.

---

## Context

Echoes Studio needs undo/redo for project edits. Several approaches were
considered:

1. **Full event sourcing** — every state change (including transport position,
   meters, drag interactions) is an event.
2. **Bounded event sourcing** — only project-structure changes are events.
   Real-time state is managed separately.
3. **Snapshot diffing** — no event log; undo/redo diffs full project snapshots.
4. **Command pattern only** — invertible commands instead of an event log.

---

## Decision

Event sourcing is applied **only to project structure**: tracks, clips, tempo,
time signature, project name, automation curves (future), and FX routing
(future).

Real-time state (transport position, meter levels, playback cursor, UI
selection, drag state) is **never event-sourced**.

The `EventStore` is append-only with a `head` pointer. Undo moves `head`
backward; redo moves it forward. New events discard redo-future entries.

The frontend mirrors the Rust event log locally. The reducer (`applyEvent`) is
a pure function from `(ProjectState, EngineEvent) → ProjectState`.

---

## Consequences

**Positive:**

- **Simple, cheap undo/redo.** Move a pointer, rebuild from nearest snapshot.
  No need to invert complex operations.
- **Reproducible state.** Replaying the event log from scratch always produces
  the same `AppProject`. Useful for debugging and future collaboration features.
- **Clean separation.** The event log contains only meaningful, undoable user
  decisions. The log grows O(user actions), not O(playback time).
- **Forward compatibility.** Unknown event types are silently ignored by the
  reducer. Old project files can be opened with new versions of the app.

**Negative:**

- **Dual state systems.** Real-time state (atomics, Svelte stores) and project
  state (event log) require different mental models. Engineers must know which
  system governs a given piece of state.
- **No transport undo.** "Undo seek" is not supported. This is an intentional
  tradeoff — undoing transport position is not a DAW convention.
- **Full project rebuild on undo/redo.** Replaying the event log reconstructs
  the full `AppProject`, which triggers graph recompilation. For large projects
  this is the most expensive undo/redo implementation possible. Mitigated by
  periodic `AppProject` snapshots in the `EventStore`.

---

## Alternatives Considered

### Full Event Sourcing (Rejected)

Logging `POSITION_UPDATED` at 30 fps generates ~108,000 events per hour of
playback. Undo replay becomes unbounded in time and memory. More critically,
real-time events are lossy — replaying transport history does not reconstruct
the same DSP state.

### Snapshot Diffing (Rejected)

Diffing full `AppProject` snapshots on every change is expensive at large
track/clip counts. Storing full snapshots on every edit is O(project size ×
edit count) memory. The event log is O(edit count) regardless of project size.

### Command Pattern Only (Rejected)

Invertible commands require writing an inverse function for every operation.
For `CLIP_MOVED`, the inverse is straightforward. For `DELETE_TRACK` (which
must recreate a track with all its clips and audio data), the inverse requires
storing the deleted state. This is equivalent to the event log but less
explicit.
