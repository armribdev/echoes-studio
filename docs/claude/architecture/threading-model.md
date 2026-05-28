# Threading Model

## Thread Inventory

```
┌─────────────────────────────────────────────────────────────────┐
│ UI Thread (WebView / JavaScript)                                │
│   Svelte reactivity, DOM events, user interactions             │
│   Tauri IPC calls (invoke / listen)                            │
│                                                                 │
│   ┌──────────────────────────────────────────────────────┐     │
│   │ Web Worker (waveform.worker.ts)                       │     │
│   │   Off-thread waveform peak computation               │     │
│   │   Canvas rendering (OffscreenCanvas)                 │     │
│   └──────────────────────────────────────────────────────┘     │
└──────────────────────────┬──────────────────────────────────────┘
                           │ Tauri IPC (JSON)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ Rust Main Thread (Tauri command handlers)                       │
│   engine_gateway::api: validates and processes commands        │
│   EventStore: appends events, manages undo/redo head           │
│   AppProject: authoritative project state                      │
│   Graph recompilation (inline, currently synchronous)          │
│                                                                 │
│   ┌──────────────────────────────────────────────────────┐     │
│   │ Tokio Async Worker (file I/O)                         │     │
│   │   Audio file loading (hound WAV parsing)             │     │
│   │   Project file save/load (serde_json)                │     │
│   └──────────────────────────────────────────────────────┘     │
└──────────────────────────┬──────────────────────────────────────┘
                           │ RuntimeSnapshot::enqueue() (blocking lock OK)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ CPAL Audio Thread (real-time, OS-scheduled)                     │
│   GraphScheduler::process_block() — the only thing here        │
│   Reads TransportInfo from atomics                             │
│   Picks up new scheduler via RuntimeSnapshot::try_take()       │
│   Writes interleaved f32 to CPAL output buffer                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## Thread Responsibilities

### UI Thread

**Owns:** Svelte component tree, all Svelte stores, all DOM state.

**Can do:** React to store changes, call `dispatcher.ts` helpers,
listen to Tauri events, update UI-local state (selection, drag, scroll).

**Cannot do:** Mutate `AppProject` directly (all mutations go through IPC).

### Rust Main Thread

**Owns:** `AppProject`, `EventStore`, `AudioEngine` (and its scheduler lock).

**Can do:** Block on locks, allocate freely, do I/O (via tokio), compile graphs.

**Cannot do:** Hold `audio_engine` lock while doing expensive I/O — this would
block the graph swap path indirectly.

### CPAL Audio Thread

**Owns:** The current `GraphScheduler` and its pre-allocated buffers.

**Can do:** Read atomics, call `try_lock` (non-blocking only), process DSP.

**Cannot do:** Everything listed in `rules/realtime-safety-rules.md`.

The audio thread is scheduled by the OS at elevated priority. Any blocking
operation risks a **priority inversion** (see below) or an underrun.

---

## Synchronization Boundaries

### Command thread → Audio thread

The only crossing point is `RuntimeSnapshot`:

```rust
// Command thread (blocking OK)
snapshot.enqueue(new_scheduler);       // lock() → write → unlock

// Audio thread (non-blocking only)
if let Some(sched) = snapshot.try_take() {
    current_scheduler = sched;
}
// If try_lock fails, silently continue with existing scheduler.
// The new scheduler will be picked up on the next block.
```

This is the **one approved locking point** in the audio callback.
`try_lock` failure is not an error — it defers the update by one block (~10 ms
at 48 kHz / 512 frames), which is imperceptible.

### Audio thread → UI thread (meters, transport)

```rust
// Audio thread: lock-free write
peak_atomic.store(peak_val, Ordering::Release);
position_atomic.store(pos_samples, Ordering::Release);

// UI thread: poll at ~30 fps via Tauri command
let peak = peak_atomic.load(Ordering::Acquire);
```

Tauri events are **never emitted from the audio thread**. `app.emit()` acquires
an internal lock — calling it from a real-time thread causes underruns.

### Project state → Audio thread

**Never directly.** Project state always goes through the compilation pipeline:

```
AppProject mutation
  → recompile_if_needed()     (command thread)
  → new GraphScheduler
  → RuntimeSnapshot::enqueue()
  → audio thread picks up at next block boundary
```

---

## Priority Inversion Prevention

A priority inversion occurs when the high-priority audio thread blocks waiting
for a lock held by the low-priority command thread.

Prevention rules:
- Audio thread **never** calls `Mutex::lock()`.
- All cross-thread data exchange uses `try_*` variants or lock-free primitives.
- If `try_lock` fails, the audio thread uses stale-but-valid state for one block.

---

## Future: Dedicated Compiler Thread

Currently, graph compilation runs synchronously on the command thread. For
projects with many tracks and plugins, this can block the IPC handler briefly.

The planned improvement is a dedicated compiler thread:

```
Command thread: mutates AppProject, enqueues compile request
Compiler thread: dequeues, compiles, enqueues RuntimeSnapshot
Audio thread: picks up at next block (unchanged)
```

This change is additive — the `RuntimeSnapshot` mechanism is already designed
for it. The command thread does not need to wait for compilation to complete
before returning a response to the UI.

---

## Async Boundaries

Tokio async is used for file I/O (audio loading, project save/load). The
`#[tauri::command]` handlers that involve I/O are `async fn`. These run on
Tokio's thread pool, not on the audio thread.

No async code ever runs on the audio thread. No `block_on` is called
from a synchronous context on the audio path. See
`rules/realtime-safety-rules.md` for enforcement.

---

## Further Reading

- `../realtime-safety-rules.md` — full checklist for audio thread code
- `../dsp-runtime-rules.md` — the five prohibitions in detail
- `systems/dsp-runtime.md` — how the scheduler drives execution
- `decisions/004-immutable-runtime-snapshots.md` — why snapshot swap instead of in-place mutation
