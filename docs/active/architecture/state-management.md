# State Management

## Two Distinct State Domains

Echoes Studio has two fundamentally different kinds of state that must not be mixed:

| Domain | Type | Owner | Persistence |
|--------|------|-------|-------------|
| **Project state** | Tracks, clips, tempo, routing, automation | Rust `EventStore` | Saved to `.echoes` file |
| **Runtime state** | Transport position, meter levels, playback cursor | Atomics / Svelte stores | Ephemeral |

Mixing these is one of the most common architectural mistakes. See
`anti-patterns.md` for concrete examples.

---

## Event Sourcing — Scope and Limits

Event sourcing is applied **only to project structure**. The event log records
what the user *decided* about their project, not what the audio engine is doing.

### What IS event-sourced

| Event | Why |
|-------|-----|
| `TRACK_CREATED` / `TRACK_DELETED` | Structural, undoable |
| `CLIP_ADDED` / `CLIP_MOVED` / `CLIP_REMOVED` | Structural, undoable |
| `TRACK_UPDATED` (name, volume, pan) | Structural, undoable |
| `TRACK_MUTED` / `TRACK_SOLOED` | Structural, undoable |
| `TEMPO_UPDATED` | Structural, affects rendering |
| `PROJECT_NAME_UPDATED` | Structural |
| Automation curves (future) | Structural, undoable |
| FX routing (future) | Structural, undoable |

### What is NOT event-sourced

| State | Why NOT |
|-------|---------|
| Transport position | 60fps real-time, never undone |
| Audio meter levels | Real-time feedback, ephemeral |
| UI selection / panels | Ephemeral UI-only state |
| Drag gesture state | Temporary, resolved on drop |
| Window scroll / zoom | UI preference, not project data |

Attempting to event-source real-time state creates an unbounded event log,
breaks the undo/redo model, and adds synchronization overhead to the hot path.

See `decisions/002-event-sourcing-scope.md` for the rationale.

---

## Rust EventStore

```rust
pub struct EventStore {
    events: Vec<ProjectEvent>,           // immutable append-only log
    head: usize,                         // current position
    snapshot: Option<(usize, AppProject)>, // periodic checkpoint
}
```

`head` is a pointer into the events array. Undo moves it back; redo moves it
forward. New events discard everything after `head` (redo-future).

**Snapshot optimization:** Rather than replaying from event 0 every time,
the store periodically saves a full `AppProject` snapshot. On undo/redo, it
replays from the nearest snapshot before `head`.

`AppProject` is a cache — it can be reconstructed from the event log at any
time. The event log is the source of truth.

---

## Frontend Event Store

The frontend mirrors the Rust event log locally:

```typescript
ProjectEventStore {
    _events: EngineEvent[]    // session log
    _state: ProjectState      // derived cache (from reducer)
    _subscribers: Set         // Svelte store contract
}
```

On startup, it seeds from `get_project` (full DTO). It then listens to the
`engine-event` Tauri channel and applies each event via `applyEvent`.

On `PROJECT_RESET` (undo/redo), it receives a full project snapshot from the
Rust side and resets the derived cache wholesale.

---

## Reducer Philosophy

`applyEvent(state, event) → ProjectState` is a pure function:

```typescript
// Pure: no side effects, no mutation, no external calls
export function applyEvent(state: ProjectState, event: EngineEvent): ProjectState {
    switch (event.type) {
        case ev.TRACK_CREATED:  return { ...state, tracks: [...state.tracks, newTrack] };
        case ev.TRACK_DELETED:  return { ...state, tracks: state.tracks.filter(...) };
        // ...
        default:                return state;  // forward-compat: unknown events are ignored
    }
}
```

**Rules:**
- Never mutate `state`. Always return a new object or structurally-shared copy.
- Unknown event types must be silently ignored (forward compatibility with future events).
- The reducer has no knowledge of the UI, the audio engine, or Tauri IPC.

---

## Undo / Redo Model

```
events: [A, B, C, D, E]
                  ↑ head = 3  → current state is A+B+C

undo() → head = 2 → current state is A+B
redo() → head = 3 → current state is A+B+C
new F  → head = 3, future [D, E] discarded → events: [A, B, C, F]
```

The audio graph is recompiled after every undo/redo because the `AppProject`
changes. The frontend receives a `PROJECT_RESET` event with the new full
project state and resets its derived cache.

**Audio data is preserved across undo/redo.** `AudioDataCache` retains loaded
WAV samples indexed by clip ID. A `CLIP_ADDED` event replay does not re-read
from disk.

---

## Derived UI State

UI-only state (selection, hover, scroll, open panels) lives in Svelte stores
in `src/stores/`. These are independent of the `ProjectEventStore` and are
never persisted to the event log.

```
engineStore.ts  ← derived from ProjectEventStore (project truth)
project.ts      ← selected track / clip (UI only)
selection.ts    ← multi-select state (UI only)
transport.ts    ← play/pause/position (real-time, not event-sourced)
preferences.ts  ← user preferences (persisted separately, not events)
```

**Rule:** Svelte stores derived from `engineProject` are read-only computed
views. They must never be written to directly from UI components — mutations
go through `dispatcher.ts` only.

---

## Command Flow Summary

```
User action (e.g. drag clip)
  ─► dispatcher.ts: moveClip(trackId, clipId, newStart, newEnd)
  ─► invoke('send_command', CommandEnvelope{ type: MOVE_CLIP, payload: ... })
  ─► engine_gateway/api.rs: validate → mutate AppProject → append ProjectEvent
  ─► recompile audio graph
  ─► app.emit('engine-event', EngineEvent{ type: CLIP_MOVED, payload: ... })
  ─► eventStore.ts: receive → applyEvent → update _state
  ─► Svelte store subscribers notified → UI re-renders
```

The round-trip is always Rust → frontend, never frontend → Rust for state.

---

## Further Reading

- `../../ARCHITECTURE.md` — data pipeline diagram, key rules, undo/redo sequence
- `../snapshot-rebuild-flow.md` — full command → compile → swap sequence
- `decisions/002-event-sourcing-scope.md` — why event sourcing is bounded
- `decisions/005-ui-engine-separation.md` — why UI and DSP share no state
