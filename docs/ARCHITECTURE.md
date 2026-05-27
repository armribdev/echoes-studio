# Echoes Studio — Architecture

> This document describes the application architecture, data flows between layers, and key architectural decisions (ADR).

---

## Overview

Echoes Studio uses a **three-layer architecture** with explicit interfaces between layers:

```
┌─────────────────────────────────────────────────────────────┐
│                        UI Layer                             │
│              SvelteKit 2 + TypeScript + SCSS                │
│   Components ──► Svelte Stores ──► Tauri IPC calls         │
└─────────────────────────┬───────────────────────────────────┘
                          │  IPC (JSON over Tauri commands)
┌─────────────────────────▼───────────────────────────────────┐
│                      Core Layer                             │
│                    Rust / Tauri 2.x                         │
│     engine_gateway ──► EventStore ──► AudioGraphSnapshot    │
└─────────────────────────┬───────────────────────────────────┘
                          │  Rust API (direct function calls)
┌─────────────────────────▼───────────────────────────────────┐
│                    Audio Engine Layer                       │
│                    Rust + cpal + hound                      │
│    AudioGraphSnapshot ──► audio_callback ──► Output Stream  │
└─────────────────────────────────────────────────────────────┘
```

---

## Data pipeline

```
UI (Svelte/TypeScript)
  └─ dispatcher.ts    ← typed command helpers
      └─ invoke('send_command')   ← Tauri IPC
          └─ engine_gateway/api.rs
              ├─ validate payload
              ├─ mutate AppProject (direct, fast path)
              ├─ append ProjectEvent → EventStore
              ├─ recompile audio graph → AudioGraphSnapshot
              │     └─ audio_engine::engine::update_snapshot
              └─ emit EngineEvent via app.emit("engine-event")
                  └─ eventStore.ts  (receives & logs)
                      └─ reducer.ts (applyEvent)
                          └─ ProjectState (derived cache)
                              └─ engineStore.ts (Svelte readable)
                                  └─ selectors/index.ts
```

---

## Scope of event sourcing

| Domain                  | Event-sourced | Reason                                 |
|-------------------------|:------------:|----------------------------------------|
| Tracks                  | ✅            | Structural, undoable                   |
| Clips                   | ✅            | Structural, undoable                   |
| Automation              | ✅ (future)   | Structural, undoable                   |
| FX routing              | ✅ (future)   | Structural, undoable                   |
| Project metadata        | ✅            | name, tempo, time-sig                  |
| UI selection/panels     | ❌            | Ephemeral UI state only                |
| Transport position      | ❌            | Real-time, 60fps                       |
| Audio meters (peak/rms) | ❌            | Real-time feedback only                |
| Drag interactions       | ❌            | Temporary UI state during gesture      |

---

## Key rules

1. **UI never mutates state directly.** All project mutations go through `dispatcher.ts → send_command`.
2. **EventStore is the only source of truth** for project structure. `AppProject` is a cache rebuilt by the reducer.
3. **Audio thread reads only `Arc<AudioGraphSnapshot>`.** No locks, no allocation inside the audio callback.
4. **Undo/redo** = move `EventStore::head` + replay from nearest snapshot.
5. **Batch commands** collapse multiple mutations into a single IPC round-trip and a single graph recompile.

---

## File structure

```
src/
├── shared/
│   ├── protocol/
│   │   ├── commands/index.ts   ← CommandEnvelope + typed payloads
│   │   └── events/index.ts     ← EngineEvent + typed payloads
│   └── project/
│       ├── reducer.ts          ← pure ProjectState reducer
│       └── eventStore.ts       ← frontend event log + state cache
└── frontend/
    ├── store/
    │   └── engineStore.ts      ← Svelte readables from event store
    ├── selectors/
    │   └── index.ts            ← pure selector functions
    └── commands/
        └── dispatcher.ts       ← all mutation entry points

src-tauri/src/
├── engine_gateway/
│   ├── api.rs       ← Tauri commands: send_command, send_batch, undo, redo
│   ├── commands.rs  ← CommandEnvelope + typed payloads
│   ├── events.rs    ← ProjectEvent + EngineEvent
│   └── store.rs     ← EventStore with undo/redo head
├── audio_engine/
│   ├── engine.rs    ← AudioEngine (owns snapshot, transport state)
│   ├── graph.rs     ← AudioGraphCompiler: AppProject → AudioGraphSnapshot
│   └── snapshot.rs  ← Immutable TrackSnapshot, ClipSnapshot (Arc-wrapped)
├── project.rs       ← AppProject, AppTrack (with id), AppClip, AudioDataCache
└── main.rs          ← Tauri builder, legacy commands kept during migration
```

---

## Command latency classification

| Command type         | Strategy         | Notes                                      |
|----------------------|:----------------:|---------------------------------------------|
| PLAY / STOP / PAUSE  | Immediate        | Engine event updates transport store        |
| SEEK                 | Immediate        | Optimistic local update + engine sync       |
| CREATE/DELETE_TRACK  | Round-trip       | Engine is source of truth, no local guess   |
| MOVE_CLIP            | Deferred commit  | Optimistic during drag, committed on drop   |
| ADD_CLIP             | Round-trip       | Audio load required before commit           |
| UPDATE_TEMPO         | Round-trip       | Ruler/transport react via TEMPO_UPDATED     |

---

## Undo / Redo

Events are never deleted. The `EventStore::head` pointer moves:

```
events: [A, B, C, D, E]
                  ↑ head = 3  → current state is A+B+C

undo() → head = 2 → current state is A+B
redo() → head = 3 → current state is A+B+C
```

Rebuild = `snapshot_at(≤ head) + events[snap_idx..head]`.

Audio data is preserved in `AudioDataCache` across undo/redo so CLIP_ADDED
replay never re-reads from disk.

---

## Audio engine (production path)

The Rust `AudioEngine` is structured for CPAL integration:

```rust
audio_callback:
  snapshot = engine.current_snapshot()  // Arc clone, no lock held
  for track in snapshot.tracks:
    for clip in track.clips:
      if clip.is_active_at(position):
        mix samples into output buffer
```

During development, playback is handled by the Web Audio API in the
Svelte frontend. The Rust audio path is activated by calling
`AudioEngine::start_stream` and wiring it to the `AudioEngine::audio_callback`.

---

## Threading

```
Main Thread (UI)
    │  Svelte reactivity, DOM events, Tauri IPC calls
    │
    ├──► Web Worker (Waveform Rendering)
    │        Off-thread canvas rendering to avoid UI jank
    │
    └──► [via Tauri IPC]
              │
              ▼
         Rust Main Thread
              │  Tauri command handlers (engine_gateway)
              │
              ├──► Audio Output Thread (cpal)
              │        Real-time callback — NEVER blocking I/O, NEVER allocate
              │        Reads Arc<AudioGraphSnapshot> — no locks held
              │
              └──► File I/O Thread (tokio async)
                       Background audio file loading, project persistence
```

**Critical rule:** The audio callback (`cpal` output stream) must **never** allocate memory or make blocking calls. All data must be prepared in advance and exchanged via lock-free channels (e.g. `ringbuf` / `Arc` swap).

---

## Architecture Decision Records

### ADR-001 — Tauri over Electron

**Context:** Desktop cross-platform runtime choice.  
**Decision:** Tauri 2.x.  
**Reasons:**
- Binaries 10–20× smaller than Electron
- Lower memory footprint (uses the OS native WebView)
- Native Rust backend = audio performance without Node.js overhead
- Stronger security model (explicit permission system)

**Trade-off:** WebView differences per OS (Chromium on Windows/Linux, WebKit on macOS) — requires cross-platform testing.

---

### ADR-002 — SvelteKit for the frontend

**Context:** UI framework choice.  
**Decision:** SvelteKit 2 (SPA mode, SSR disabled).  
**Reasons:**
- Reactivity without virtual DOM = fluid UI performance for a DAW
- Lighter final bundle than React/Vue for a desktop app
- Native TypeScript
- Svelte stores = state management without external libraries

**Trade-off:** Smaller ecosystem than React, fewer third-party components available.

---

### ADR-003 — Rust for the audio engine

**Context:** Language choice for audio processing.  
**Decision:** Rust + `cpal`.  
**Reasons:**
- Real-time guarantees: no GC, explicit memory control
- `cpal` supports ASIO (Windows), CoreAudio (macOS), ALSA/JACK (Linux)
- Memory safety without a garbage collector
- Native interoperability with Tauri

**Trade-off:** Rust learning curve, type system complexity for concurrent audio code.

---

### ADR-004 — Project file format (open)

**Context:** Project save format.  
**Options:**
- JSON text — human-readable, versionable, but verbose for large timelines
- MessagePack binary — compact, but not human-readable
- SQLite — queryable, history support, but heavy dependency

**Decision:** To be decided in Phase 2.

---

### ADR-005 — Plugin support (open)

**Context:** Third-party audio plugin format.  
**Options:**
- VST3 — industry standard, Windows/macOS/Linux support
- LV2 — open-source, native Linux
- CLAP — new open-source standard, promising
- No third-party plugins — maintain simplicity

**Decision:** To be decided in Phase 4.

---

## Links

- [TECH_STACK.md](TECH_STACK.md) — Detailed technology choices and rationale
- [AUDIO_ENGINE.md](AUDIO_ENGINE.md) — Audio engine specifications
- [UI_GUIDELINES.md](UI_GUIDELINES.md) — UI component architecture and design principles
- [BUILD.md](BUILD.md) — Build and development environment setup
