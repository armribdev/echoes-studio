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
│     engine_gateway ──► EventStore ──► RuntimeSnapshot       │
└─────────────────────────┬───────────────────────────────────┘
                          │  Rust API (direct function calls)
┌─────────────────────────▼───────────────────────────────────┐
│                    Audio Engine Layer                       │
│                    Rust + cpal + hound                      │
│    GraphScheduler ──► audio_callback ──► Output Stream      │
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
              ├─ recompile audio graph → RuntimeSnapshot
              │     └─ audio_engine::engine::update_from_project
              └─ emit EngineEvent via app.emit("engine-event")
                  └─ eventStore.ts  (receives & logs)
                      └─ reducer.ts (applyEvent)
                          └─ ProjectState (derived cache)
                              └─ engineStore.ts (Svelte readable)
                                  └─ selectors/index.ts
```

---

## Scope of event sourcing

| Domain                  | Event-sourced | Reason                            |
| ----------------------- | :-----------: | --------------------------------- |
| Tracks                  |      ✅       | Structural, undoable              |
| Clips                   |      ✅       | Structural, undoable              |
| Automation              |  ✅ (future)  | Structural, undoable              |
| FX routing              |  ✅ (future)  | Structural, undoable              |
| Project metadata        |      ✅       | name, tempo, time-sig             |
| UI selection/panels     |      ❌       | Ephemeral UI state only           |
| Transport position      |      ❌       | Real-time, 60fps                  |
| Audio meters (peak/rms) |      ❌       | Real-time feedback only           |
| Drag interactions       |      ❌       | Temporary UI state during gesture |

---

## Key rules

1. **UI never mutates state directly.** All project mutations go through `dispatcher.ts → send_command`.
2. **EventStore is the only source of truth** for project structure. `AppProject` is a cache rebuilt by the reducer.
3. **Audio thread uses `GraphScheduler` swapped in via `RuntimeSnapshot::try_swap()` (lock-free `AtomicPtr`).** No blocking locks, no allocation, and no `drop` inside the audio callback — displaced schedulers ride an SPSC drop ring back to the command thread.
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
│       ├── reducer.ts          ← pure applyEvent(state, event) → state
│       └── eventStore.ts       ← frontend event log + derived state cache
└── frontend/
    ├── store/
    │   └── engineStore.ts      ← Svelte readables derived from event store
    ├── selectors/
    │   └── index.ts            ← pure selector functions
    └── commands/
        └── dispatcher.ts       ← all mutation entry points

src-tauri/src/
├── engine_gateway/
│   ├── api.rs          ← Tauri commands: send_command, send_batch, undo, redo
│   ├── commands.rs     ← CommandEnvelope + typed payloads
│   ├── events.rs       ← ProjectEvent + EngineEvent definitions
│   └── store.rs        ← EventStore: append-only log, head pointer, snapshots
├── audio_engine/
│   ├── engine.rs       ← AudioEngine: owns CPAL stream, SharedTransport, RuntimeSnapshot
│   ├── graph.rs        ← AudioGraphCompiler wrapper (delegates to audio_graph/)
│   ├── transport.rs    ← SharedTransport: atomic play/pause/seek state
│   └── snapshot.rs     ← RuntimeSnapshot: Mutex<Option<GraphScheduler>> swap
├── audio_graph/        ← Core DSP execution engine
│   ├── node.rs         ← AudioNode trait, PrepareContext, TransportInfo
│   ├── buffer.rs       ← AudioBuffer: planar f32, allocation-free operations
│   ├── graph.rs        ← AudioGraph: DAG of NodeId → GraphNode + edges
│   ├── compiler.rs     ← GraphCompiler: topological sort → CompiledGraph
│   ├── execution_plan.rs ← ExecutionPlan: Arc-shared immutable flat plan
│   ├── scheduler.rs    ← GraphScheduler: drives plan in audio callback
│   ├── modulation.rs   ← ModulationRoute, ModulationMap (data ready; runtime pending)
│   └── nodes/          ← Built-in DSP nodes (Track, Gain, Pan, Meter, Bus, Master)
├── plugin_system/      ← Plugin abstraction layer (scaffolded)
│   ├── host.rs         ← PluginHost: lifecycle, parameter routing, state
│   ├── wrapper.rs      ← Device trait (extends AudioNode)
│   ├── parameter.rs    ← ParameterMap, Param, ParameterRange
│   ├── lifecycle.rs    ← LifecycleState machine
│   ├── registry.rs     ← PluginManifest, PluginRegistry
│   └── state.rs        ← DeviceState: opaque plugin state blob
├── plugin_formats/     ← Format-specific wrappers (scaffolded)
│   ├── vst3/           ← VST3 wrapper (ffi.rs, host.rs, wrapper.rs)
│   ├── au/             ← AudioUnit wrapper (macOS)
│   └── internal/       ← WASM plugin runtime
├── project.rs          ← AppProject, AppTrack, AppClip, AudioDataCache
├── audio_clip.rs       ← WAV parsing via hound
├── audio_settings.rs   ← Audio device & sample rate configuration
└── main.rs             ← Tauri builder, native menu, Tauri commands
```

---

## Command latency classification

| Command type        |    Strategy     | Notes                                                                  |
| ------------------- | :-------------: | ---------------------------------------------------------------------- |
| PLAY / STOP / PAUSE |    Immediate    | Engine event updates transport store                                   |
| SEEK                |    Immediate    | Optimistic local update + engine sync                                  |
| CREATE/DELETE_TRACK |   Round-trip    | Engine is source of truth, no local guess                              |
| MOVE_CLIP           | Deferred commit | Optimistic during drag, committed on drop                              |
| ADD_CLIP            |   Round-trip    | May commit on first PCM prefix; silence past decoded samples (ADR-024) |
| UPDATE_TEMPO        |   Round-trip    | Ruler/transport react via TEMPO_UPDATED                                |

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

The `AudioEngine` runs a graph-based DSP pipeline. Every structural project
change compiles a new `GraphScheduler` and atomically swaps it into the CPAL
callback at the next block boundary:

```
Command thread                        Audio thread
──────────────                        ────────────
AppProject mutation
  → AudioGraph::from_project()
  → GraphCompiler::compile()          try_swap(&mut current) at block start:
  → GraphScheduler::new()    ──────►  AtomicPtr swap; old box → drop ring
  → RuntimeSnapshot::enqueue()        process_block(transport, output)
    (drains drop ring, frees
     boxes returned by audio)
```

The audio callback (`GraphScheduler::process_block`) iterates a pre-sorted
flat `Vec<ExecutionStep>`, calling each node's `process()` in topological
order. No graph traversal, no allocation, no blocking locks occur in the
callback. See [AUDIO_ENGINE.md](AUDIO_ENGINE.md) for full details.

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

### ADR-005 — Plugin support

**Context:** Third-party audio plugin format.  
**Options considered:**

- VST3 — industry standard, Windows/macOS/Linux support
- LV2 — open-source, native Linux
- CLAP — new open-source standard, promising
- No third-party plugins — maintain simplicity

**Decision:** VST3 as primary format; AU on macOS. Scaffolding is done
(`plugin_formats/vst3/`, `plugin_formats/au/`, `plugin_system/`). Full
implementation deferred to Phase 4. Plugins integrate as thin `AudioNode`
wrappers — the graph runtime remains format-agnostic.

See `docs/active/decisions/003-plugin-wrapper-strategy.md` for rationale.

---

## Links

- [TECH_STACK.md](TECH_STACK.md) — Detailed technology choices and rationale
- [AUDIO_ENGINE.md](AUDIO_ENGINE.md) — Audio engine specifications
- [UI_GUIDELINES.md](UI_GUIDELINES.md) — UI component architecture and design principles
- [BUILD.md](BUILD.md) — Build and development environment setup
- [claude/README.md](claude/README.md) — Deep-dive architecture reference (audio graph, threading, ADRs, rules)
