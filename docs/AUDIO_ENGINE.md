# Audio Engine — Echoes Studio

> Technical specification of the audio engine: processing model, threading, supported formats, and design decisions.

---

## 1. Overview

The audio engine is implemented in Rust and interfaces with the UI via Tauri's IPC bridge. It uses `cpal` for cross-platform audio streams and `hound` for WAV file parsing.

The engine is **graph-based**: every DSP element (tracks, gains, panners, effects) is a node in a directed acyclic graph. The graph is compiled ahead of time into a flat `ExecutionPlan` that the `GraphScheduler` iterates in the audio callback without any runtime traversal.

```
AppProject (project data)
     │  AudioGraph::from_project()
     ▼
AudioGraph (DAG: nodes + edges)
     │  GraphCompiler::compile()
     ▼
CompiledGraph { ExecutionPlan, Vec<Box<dyn AudioNode>> }
     │  GraphScheduler::new()
     ▼
GraphScheduler (pre-allocated buffers + processors)
     │  RuntimeSnapshot::enqueue()
     ▼
Audio thread picks up at next block boundary
     │  GraphScheduler::process_block()
     ▼
Interleaved f32 → CPAL output device
```

---

## 2. Technical Specifications

### 2.1 Audio Parameters

| Parameter | Default | Configurable | Notes |
|-----------|---------|:---:|-------|
| Sample rate | 48,000 Hz | Yes | 44.1, 48, 88.2, 96 kHz supported |
| Internal bit depth | f32 (32-bit float) | No | Normalised range [-1.0, 1.0] |
| Output channels | 2 (stereo) | Yes (1–N) | Configurable in Preferences |
| Buffer size | 512 frames | Yes | 64–2048 frames; auto-fallback if device rejects value |
| Target latency | < 10 ms | — | Driven by buffer size / sample rate |

### 2.2 Supported File Formats

| Format | Status | Library |
|--------|--------|---------|
| WAV (PCM 16/24/32-bit) | ✅ Implemented | `hound` |
| WAV (float 32-bit) | ✅ Implemented | `hound` |
| MP3 | Planned (Phase 2) | `symphonia` |
| FLAC | Planned (Phase 2) | `symphonia` |
| OGG Vorbis | Planned (Phase 2) | `symphonia` |
| AIFF | Planned | `symphonia` |
| M4A/AAC | Planned | `symphonia` |

### 2.3 Audio Backends by Platform

| OS | Default backend | Alternative |
|----|----------------|------------|
| Windows | WASAPI | ASIO (ultra-low latency — Phase 5) |
| macOS | CoreAudio | — |
| Linux | ALSA | JACK, PipeWire |

---

## 3. Data Model

### 3.1 Project Structs (`project.rs`)

```rust
pub struct AppProject {
    pub name: String,
    pub bpm: f32,
    pub numerator: u8,
    pub denominator: u8,
    pub tracks: Vec<AppTrack>,
}

pub struct AppTrack {
    pub id: String,              // stable event-sourced ID
    pub name: String,
    pub volume: f32,             // 0–1 linear, applied by GainNode in graph
    pub pan: f32,                // -1.0 (L) to 1.0 (R)
    pub muted: bool,
    pub solo: bool,
    pub clips: Vec<AppClip>,
}

pub struct AppClip {
    pub id: String,
    pub start: f64,              // timeline position in seconds
    pub end: f64,
    pub file_path: String,
    pub raw_samples: Vec<f32>,   // [-1.0, 1.0]; immutable Arc prefixes during import (ADR-024)
    pub sample_rate: u32,
    pub peaks_64: Vec<[f32; 2]>, // waveform peaks at 3 resolutions for fast rendering
    pub peaks_512: Vec<[f32; 2]>,
    pub peaks_4096: Vec<[f32; 2]>,
}
```

### 3.2 Core DSP Types (`audio_graph/`)

```rust
// Every DSP element implements this trait
pub trait AudioNode: Send + 'static {
    fn node_id(&self) -> NodeId;
    fn num_input_ports(&self) -> usize;
    fn num_output_ports(&self) -> usize;
    fn prepare(&mut self, ctx: PrepareContext);   // only place allocation is allowed
    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [&mut AudioBuffer],
        transport: TransportInfo,
    );
}

// Audio buffer — planar layout: [ch0[0..frames] | ch1[0..frames]]
// All operations are allocation-free (clear, copy_from, mix_from, apply_gain)
pub struct AudioBuffer { /* ... */ }

// Transport state — constructed from atomics, passed to every node each block
pub struct TransportInfo {
    pub is_playing: bool,
    pub position_samples: u64,
    pub position_seconds: f64,
    pub bpm: f32,
    pub sample_rate: f32,
    pub block_size: usize,
}
```

### 3.3 Built-in Nodes (`audio_graph/nodes/`)

| Node | Role |
|------|------|
| `TrackNode` | Audio source — reads clips, advances position |
| `GainNode` | Volume control |
| `PanNode` | Constant-power stereo panning |
| `MeterNode` | Peak/RMS measurement, writes to atomic cells |
| `BusNode` | Multi-input mixing point |
| `MasterNode` | Final output, soft limiter |
| `SendNode` | Aux send routing (scaffolded, Phase 3) |

---

## 4. Signal Path

### 4.1 Current (Phase 1–2)

`AudioGraph::from_project()` builds a fixed topology for N tracks:

```
TrackNode₁ ──► GainNode₁ ──┐
TrackNode₂ ──► GainNode₂ ──┤
       ...                  ├──► MasterNode ──► CPAL output
TrackNodeₙ ──► GainNodeₙ ──┘
```

`GainNode` applies both volume and pan. `MasterNode` sums all inputs and
applies a soft limiter before writing to the CPAL output buffer.

### 4.2 Phase 3+ (with effects)

Future effect nodes slot into the graph as additional nodes between the gain stage
and the master. The scheduler and buffer system require no changes — topology is
the only thing that expands.

```
TrackNode
   │
   ▼  GainNode (volume + pan)
   │
   ▼  [EqNode, CompressorNode, ReverbNode, ...]   ← graph nodes, not special-cased
   │
   ▼  BusNode
   │
   ▼  MasterNode → CPAL output
```

---

## 5. Threading Model

```
UI Thread (WebView / JavaScript)
     │  Svelte reactivity, DOM events, Tauri IPC calls
     │
     ├──► Web Worker (waveform.worker.ts)
     │        Off-thread waveform peak computation (no UI jank)
     │
     └──► [Tauri IPC bridge]
               │
               ▼
          Rust Main Thread (Tauri command handlers)
               │  engine_gateway: validate → mutate AppProject → append ProjectEvent
               │  Graph recompilation (inline on command thread)
               │
               ├──► Tokio async worker
               │        Audio file loading (hound), project save/load (serde_json)
               │
               └──► CPAL Audio Thread (real-time, OS-scheduled)
                         GraphScheduler::process_block() — the only work here
                         Never: allocate, block, do I/O, call async, emit Tauri events
```

### 5.1 Command Thread → Audio Thread

Graph updates cross the thread boundary through `RuntimeSnapshot`:

```rust
// Command thread: drains the drop ring, then publishes new scheduler
snapshot.enqueue(new_scheduler);        // AtomicPtr::swap + free old box here

// Audio thread: lock-free, allocation-free, drop-free
snapshot.try_swap(&mut current_scheduler);
// Internally: AtomicPtr::swap(null) → take new Box → push old Box into the
// SPSC drop ring → command thread frees it on its next enqueue.
// If nothing was pending, current_scheduler is left untouched (always safe:
// stale graph state still produces correct audio).
```

### 5.2 Audio Thread → UI Thread (meters, transport position)

```rust
// Audio thread: lock-free atomic write
peak_atomic.store(peak_value, Ordering::Release);
position_atomic.store(pos_samples, Ordering::Release);

// UI thread: polling at ~30 fps via Tauri command
// app.emit() is NEVER called from the audio thread — it acquires an internal lock
```

---

## 6. Audio Callback

`GraphScheduler::process_block(transport, output)` is the entire audio callback:

```rust
fn process_block(&mut self, transport: TransportInfo, output: &mut [f32]) {
    // 1. (Done by the CPAL closure *before* calling process_block:
    //     snapshot.try_swap(&mut current_scheduler) — lock-free, no drop)

    // 2. Execute nodes in pre-computed topological order
    for (i, step) in self.plan.steps.iter().enumerate() {
        // Fill input scratch with pointers to upstream output buffers (or silence)
        for (port, conn) in step.input_connections.iter().enumerate() {
            self.input_ptr_scratch[port] = match conn {
                Some(ic) => &self.output_buffers[ic.src_step][ic.src_port],
                None     => &self.silence_buffer,
            };
        }
        self.processors[i].process(inputs, &mut self.output_buffers[i], transport);
    }

    // 3. Write master output to CPAL interleaved buffer
    write_interleaved(&self.output_buffers[master_step], output);
}
```

Real-time safety invariants:
- All buffers pre-allocated in `GraphScheduler::new()` — no allocation in callback
- No `Mutex` of any kind — scheduler swap goes through `AtomicPtr`
- No `drop` on the audio thread — displaced schedulers ride the SPSC drop ring
  back to the command thread
- `TransportInfo` built from atomics — no lock for transport reads
- No graph traversal — the `ExecutionPlan.steps` array is pre-ordered

See `docs/active/threading/realtime-safety.md` for the full checklist.

---

## 7. Graph Compilation

Every structural project change triggers a full recompile on the command thread:

```
CREATE_TRACK / DELETE_TRACK / ADD_CLIP / MOVE_CLIP / REMOVE_CLIP / UPDATE_TRACK / UNDO / REDO
     │
     ▼  AudioGraph::from_project()           O(tracks)
     │
     ▼  GraphCompiler::compile()             O(nodes + edges): topological sort (Kahn BFS)
     │                                       Instantiates nodes, calls prepare()
     ▼  GraphScheduler::new()               O(nodes × ports × block_size): buffer allocation
     │
     ▼  RuntimeSnapshot::enqueue()          O(1): mutex write
```

Transport changes (PLAY / STOP / SEEK) do **not** trigger recompilation.
They update `SharedTransport` atomics which are read by the callback on each block.

---

## 8. Latency Management

### 8.1 Sources of Latency

| Source | Typical | Control |
|--------|---------|---------|
| Audio driver buffer | 1.3–46 ms (64–2048 frames @ 44.1 kHz) | Configurable in Preferences |
| Sample rate mismatch | Negligible if avoided | Match output SR to device SR |
| File decoding | None on audio thread | Decode off RT; immutable PCM prefixes published during import (ADR-024) |
| WASAPI exclusive mode | 1–3 ms | Future (Phase 5) |

### 8.2 Buffer Fallback

If the configured buffer size is not supported by the CPAL device, the engine
falls back to the device's default buffer size. The Preferences panel shows the
estimated latency with a colour-coded gauge (green < 10 ms / yellow < 25 ms / red ≥ 25 ms).

---

## 9. Export / Mixdown

Current implementation uses `OfflineAudioContext` (Web Audio API) in the frontend.
This is a temporary Phase 1–2 approach — it mirrors the project topology but runs
in the browser rather than through the Rust graph.

A Rust offline renderer (same `GraphScheduler`, no CPAL stream, maximum CPU speed)
is planned for Phase 3. It will guarantee that export is bit-identical to playback.

Current export flow:
1. Frontend builds an `OfflineAudioContext` graph from project state
2. `startRendering()` produces a stereo mixdown at full CPU speed
3. `export_wav(path, samples_left, samples_right, sample_rate, bit_depth)` (Tauri command) writes via `hound`

---

## 10. Plugin System

The plugin system (`plugin_system/`, `plugin_formats/`) is scaffolded for Phase 4.

Architecture: every plugin wraps as an `AudioNode`. The graph has no knowledge
of plugin formats. See `docs/active/plugins/plugin-system.md`.

| Module | Status | Contents |
|--------|--------|---------|
| `plugin_system/host.rs` | Scaffolded | `PluginHost`: lifecycle, parameter routing, state |
| `plugin_system/parameter.rs` | Scaffolded | `ParameterMap`, `Param`, `ParameterRange` |
| `plugin_system/lifecycle.rs` | Scaffolded | `LifecycleState` machine |
| `plugin_system/registry.rs` | Scaffolded | `PluginManifest`, `PluginRegistry` |
| `plugin_formats/vst3/` | Scaffolded | VST3 wrapper (FFI, host, wrapper) |
| `plugin_formats/au/` | Scaffolded | AudioUnit wrapper (macOS) |
| `plugin_formats/internal/` | Scaffolded | WASM plugin runtime |

---

## 11. Monitoring & Metering

`MeterNode` computes peak and RMS values per block and writes them to `AtomicF32`
cells. The audio thread never blocks to report these values — the UI polls them
at ~30 fps via a dedicated Tauri command. Frontend VU meter widgets are pending
(Phase 2 remaining).

---

## Links

- [ARCHITECTURE.md](ARCHITECTURE.md) — Overall architecture and IPC pipeline
- [TECH_STACK.md](TECH_STACK.md) — Technology choices and rationale
- [docs/active/architecture/audio-graph.md](active/architecture/audio-graph.md) — Graph-first philosophy
- [docs/active/threading/threading-model.md](active/threading/threading-model.md) — Thread model detail
- [docs/active/threading/realtime-safety.md](active/threading/realtime-safety.md) — Audio thread constraints
- [docs/active/dsp/graph-compiler.md](active/dsp/graph-compiler.md) — Compilation pipeline
- [src-tauri/src/audio_graph/](../src-tauri/src/audio_graph/) — DSP engine source
