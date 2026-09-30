# ADR-005 — UI / Engine Separation

## Status

Accepted. Enforced by the Tauri IPC boundary and the `engine_gateway` module.

---

## Context

Echoes Studio has a SvelteKit frontend (TypeScript) and a Rust audio engine.
These could be coupled tightly (shared memory via Tauri state, direct callback
hooks into the UI) or separated by an explicit protocol boundary.

The question was how strictly to enforce separation between the UI layer and
the DSP engine layer.

---

## Decision

The UI and DSP runtime are strictly separated. They share no mutable state.
All communication crosses the Tauri IPC boundary in one of two directions:

**UI → Engine:** `invoke('send_command', CommandEnvelope)` — all project
mutations go through `engine_gateway/api.rs`.

**Engine → UI:** `app.emit('engine-event', EngineEvent)` — the engine pushes
events to the UI after every mutation.

Real-time feedback (meters, transport position) uses atomics polled by the UI
via periodic Tauri commands — never via event emission from the audio thread.

The UI is a **reactive mirror** of the engine's project state. It holds no
authoritative state that the engine does not know about.

---

## Consequences

**Positive:**

- **Engine evolves independently.** The Rust backend can be refactored, the
  audio graph redesigned, or the compilation model changed without touching
  the frontend — as long as the IPC protocol is preserved.
- **UI evolves independently.** The frontend can be replaced (SvelteKit →
  React, or a future native UI) without touching the audio engine.
- **No UI-thread blocking.** The audio callback has no path to the UI thread.
  There are no callbacks, no shared heap objects, no Svelte store writes from
  Rust.
- **Reproducible state.** Because the UI is a deterministic projection of the
  engine's event log, it can be rebuilt from scratch at any time
  (`PROJECT_RESET` event).
- **Testability.** The engine can be tested in isolation (no UI required).
  The UI can be tested against a mock event stream (no engine required).

**Negative:**

- **Latency for optimistic UI.** Round-trip IPC adds ~1–5 ms per command.
  For operations where the UI should feel instantaneous (knob drag), this
  requires optimistic updates in the frontend that are later confirmed by
  the engine. This complexity must be managed per command type (see the
  latency classification table in `docs/ARCHITECTURE.md`).
- **Protocol versioning.** As new features add new command and event types,
  the `src/shared/protocol/` types must be kept in sync between Rust and
  TypeScript. A mismatch causes silent failures (unknown events are ignored)
  or hard failures (unknown commands return an error).
- **Debugging complexity.** A bug involving a project state mismatch between
  the UI and engine is harder to diagnose than a bug in a single shared object.
  The event log is the source of truth — when in doubt, compare the frontend's
  derived state against the Rust `AppProject`.

---

## Alternatives Considered

### Shared State via Tauri State + Direct Rust Callbacks (Rejected)

Tauri's `State` is accessible from both command handlers and the frontend via
`invoke`. Wrapping `AppProject` in an `Arc<Mutex>` and letting the frontend
read it directly would eliminate the IPC round-trip for reads.

But:

- Reads acquire a lock. The audio thread must never be blocked by a UI read
  of `AppProject`.
- There is no event log. Undo/redo requires either snapshot diffing or
  re-implementing the event log on top of shared state.
- TypeScript has no type-safe access to Rust types at runtime without
  serialization. The "shared state" is always a serialized DTO, so the IPC
  round-trip exists regardless.

### Direct Audio Engine Callbacks into UI (Rejected)

Some DAW toolkits allow plugins to call into the UI directly (e.g., invalidate
a meter widget from the audio callback). This requires the UI to be real-time
safe (no allocation, no blocking) or requires careful synchronization.

Given that the frontend is JavaScript running in a WebView, there is no path
to real-time-safe callbacks. Any cross-thread notification from the audio engine
to the UI must go through the OS event queue — which is equivalent to the
atomic-polling approach already adopted.
