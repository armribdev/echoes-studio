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
                           │ RuntimeSnapshot::enqueue() (AtomicPtr::swap;
                           │ drains drop ring, frees dead boxes here)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ CPAL Audio Thread (real-time, OS-scheduled)                     │
│   GraphScheduler::process_block() — the only thing here        │
│   Reads TransportInfo from atomics                             │
│   Picks up new scheduler via RuntimeSnapshot::try_swap()       │
│     (lock-free AtomicPtr; displaced box → SPSC drop ring)      │
│   Writes interleaved f32 to CPAL output buffer                 │
│   InstrumentNode drains its MidiLiveInputQueue every block     │
│     (ADR-022) — same SPSC-drain shape as piano-roll audition   │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│ MIDI Capture Thread(s) (owned by `midir`, ADR-022)              │
│   One per open hardware MIDI input device                       │
│   Parses raw bytes → Note On/Off, reads the armed-track id,     │
│     pushes into that track's MidiLiveInputQueue (SPSC)         │
│   Never touches the audio thread, CPAL, or each other directly  │
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

**Can do:** Read atomics, call `RuntimeSnapshot::try_swap` (lock-free),
process DSP.

**Cannot do:** Everything listed in `realtime-safety.md` — and
in particular: **never `Mutex::lock`/`try_lock`, never `drop` heap-owning
values, never allocate.**

The audio thread is scheduled by the OS at elevated priority. Any blocking
operation risks a **priority inversion** (see below) or an underrun.

When an input device is available, a **second** CPAL stream runs on its own
callback thread (ADR-027). Same RT rules: peaks + SPSC ring pushes only. The
output callback may drain the monitor ring and mix after the graph (with the
metronome click).

### MIDI Capture Thread(s) (ADR-022)

**Owns:** One `midir::MidiInputConnection` per enabled hardware MIDI input
device. Not spawned/managed by Echoes directly — `midir` drives the thread
internally and invokes our callback on it.

**Can do:** Parse raw MIDI bytes, read the armed-track `Mutex<Option<String>>`,
push into that track's `MidiLiveInputQueue` (lock-free SPSC, command-thread
style — this thread is not on the RT audio path so a `Mutex` read is fine).

**Cannot do:** Touch the audio thread, CPAL, or the `GraphScheduler` directly.
The only channel out of this thread is the per-track `MidiLiveInputQueue`.

MIDI _output_ (Local Control On/Off, ADR-022 amendment) is not sent from this
thread: the command thread opens a short-lived `midir` output connection,
sends, and closes it.

### MIDI Hotplug Watcher (ADR-022 amendment)

**Owns:** nothing persistent — a named thread (`midi-hotplug-watcher`, spawned
once at startup) that re-enumerates visible MIDI ports every second and emits
`MIDI_DEVICES_EVENT` to the frontend when the set changes. `midir`/WinMM give
no OS-level connect/disconnect callback, so this is a plain poll, not a true
event subscription.

**Can do:** Lock `MidiInputRegistry` briefly to compare/reconcile, same as any
other command-thread-adjacent access. Never touches the audio thread.

---

## Synchronization Boundaries

### Command thread → Audio thread

The only crossing point is `RuntimeSnapshot`:

```rust
// Command thread (lock-free, but blocking would be OK here)
snapshot.enqueue(new_scheduler);
//   drains the SPSC drop ring (frees displaced boxes from the audio thread)
//   then AtomicPtr::swap publishes the new Box<GraphScheduler>

// Audio thread (lock-free, allocation-free, drop-free)
snapshot.try_swap(&mut current_scheduler);
//   AtomicPtr::swap(null) → take ownership of new Box
//   push displaced Box into the drop ring; command thread frees it later
//   if nothing was pending, current_scheduler is untouched
```

This is the **only approved scheduler-update path** in the audio callback.
There is no contention case: every step is a single atomic operation.

Playback loop bounds live on the same atomics. The callback splits a block that crosses the loop end and stores the wrapped position; it does not emit. With the loop on, the reporter does not auto-stop at the end of the timeline.

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

### MIDI capture thread → Audio thread (ADR-022)

```rust
// MIDI capture thread (midir callback): lock-free push
live_params.push_live_input(&armed_track_id, message);

// Audio thread, inside InstrumentNode::process(): drains every block
self.drain_live_input(); // same shape as drain_preview()
```

Same ring shape as piano-roll audition (`MidiPreviewQueue`), but a distinct
queue per track (1024 slots, sized for dense pitch-bend / CC streams — ADR-022
amendment 2026-10-13), and multi-producer: each enabled keyboard has its own capture
thread, so pushes are serialized by a producer mutex inside the queue (never
taken by the audio thread, whose drain stays lock-free).

Timing (ADR-022 amendment): the capture thread stamps each event with the
shared `LiveInputClock`; the CPAL callback reads the same clock once per
callback and publishes, per rendered slice, the wall time mapped to its first
sample. `drain_due` places each event one callback period after its arrival —
constant latency, no jitter — and leaves events due later in the queue.

Recording (ADR-022 amendment 2026-09-29): the callback also publishes the
transport sample mapped to that same wall time (`set_event_window`, seqlock).
While a recording runs, the capture thread turns an event's arrival time into a
transport position (`transport_position_at`) and appends it to the
`MidiRecorder` buffer behind a `Mutex` shared only with the command thread. The
finished clip reaches the UI through `midi-recording-finished`, emitted from the
command thread (record button, `PAUSE`, `STOP`) — never from the audio thread.
Each recorded note is also announced (`midi-recording-note`) from the capture
thread so the UI can draw the growing clip; the capture thread is not real-time.

Metronome / count-in (ADR-026): the command thread arms the count-in
(`start_count_in`) and toggles the click (`set_metronome_enabled`) through
`SharedTransport` atomics; the CPAL callback consumes them. Nothing is sent back.

Audio recording (ADR-027): a dedicated `audio-record-drain` thread pops the
record ring and appends into `AudioRecorder` (skipped during count-in). Finish
writes a WAV under app-data `recordings/` and emits `audio-recording-finished`
from the command thread (● / PAUSE / STOP) — never from a CPAL callback.
Software monitoring is a session atomic (`input_monitor_enabled`) mixed in the
output callback outside the project graph.

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

- Audio thread **never** calls `Mutex::lock()` or `Mutex::try_lock()` (the
  latter is not strictly lock-free on Windows).
- All cross-thread data exchange uses lock-free primitives (`AtomicPtr`,
  `AtomicU32`, SPSC ring buffers).
- The audio thread also never deallocates — displaced schedulers are handed
  back to the command thread through `RuntimeSnapshot`'s drop ring.

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
`realtime-safety.md` for enforcement.

---

## Further Reading

- `realtime-safety.md` — full checklist for audio thread code
- `../dsp/dsp-runtime.md` — the five prohibitions in detail
- `../dsp/dsp-runtime.md` — how the scheduler drives execution
- `decisions/004-immutable-runtime-snapshots.md` — why snapshot swap instead of in-place mutation
