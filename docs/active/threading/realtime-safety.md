# Real-Time Safety Rules

> **Canonical and authoritative.** Consolidates three former documents: the
> hierarchy `rules/realtime-safety-rules.md`, the legacy flat
> `realtime-safety-rules.md` (thread model + CPAL), and the legacy flat
> `dsp-runtime-rules.md` (the five prohibitions). Both legacy flat files are
> archived under `docs/archive/`.
>
> These are absolute constraints. There are no exceptions unless explicitly
> noted. Violations cause audio underruns, priority inversions, or crashes.

---

## Thread Model

```
UI thread          — Svelte/JS frontend, user interactions
IPC thread         — Tauri command/event bridge
Command thread     — engine_gateway: validates commands, mutates project
Compiler thread    — future: dedicated graph compilation thread
                     (currently inline on the command thread)
Audio thread       — CPAL callback: process_block() only
```

These threads must interact **only** through the approved mechanisms below.
For the full thread-ownership diagram see `threading-model.md`.

---

## Approved Inter-Thread Mechanisms

### Command thread → Audio thread

```
RuntimeSnapshot::enqueue(scheduler)  →  try_swap(&mut current) at block boundary
                                        (displaced scheduler returned via
                                         drop ring → freed by command thread)
```

The `RuntimeSnapshot` holds an `AtomicPtr<GraphScheduler>` for the pending slot
and a bounded SPSC ring for displaced schedulers awaiting deallocation.

- The command thread writes via `pending.swap(new_ptr)` (lock-free) and drains
  the drop ring on every `enqueue` — `drop` only ever runs here.
- The audio callback calls `try_swap(&mut current)`, which swaps the pending
  `AtomicPtr` and pushes the displaced `Box<GraphScheduler>` into the drop ring.
  No mutex, no allocation, no `drop` on the audio path.

### Audio thread → UI thread

```
AtomicU32 / AtomicF32 meter cells  →  UI polls via Tauri command
```

Meter values are written atomically from the audio thread. The UI reads them
periodically (e.g. 30 fps) via a Tauri command that acquires no locks on the
audio path.

### MIDI capture threads → Audio thread (ADR-022)

```
MidiLiveInputQueue::push (capture threads, serialized by a producer mutex)
    →  drain_due() in InstrumentNode::process (lock-free consumer)
```

The producer mutex is only ever taken by the capture threads (one per enabled
keyboard); the audio thread reads head/tail atomics only. Keyboard controls
(bend, CC, aftertouch) are `Copy` events in the same pre-allocated ring and
event lists — no new RT cost (ADR-022 amendment 2026-10-13).

MIDI recording adds three relaxed atomic stores per rendered slice
(`LiveInputClock::set_event_window`: transport position + a seqlock counter,
wait-free) and one per callback (`set_record_latency`: period + CPAL output
latency). The reader is the capture thread; the recorder's buffer and mutex
are never reachable from `process_block`. The position reporter (not the audio
thread) reads `SharedTransport::recording` to skip the end-of-timeline auto-stop.

Metronome and count-in (ADR-026): per slice the callback reads two atomics
(`count_in_remaining`, `metronome_enabled`), renders the click with
`Metronome::render` (arithmetic only, buffers pre-allocated) and adds it to the
output after the meters. Command → audio is atomics only: `start_count_in`,
`set_metronome_enabled`.

### Project state → Audio thread

**Never directly.** Project state always goes through the compilation pipeline:

```
AppProject mutation
    → recompile_if_needed()     (command thread)
    → new GraphScheduler
    → RuntimeSnapshot::enqueue()
    → audio thread picks up at next block
```

---

## Prohibited in the Audio Callback

### 1. No heap allocation

```rust
// ✗ FORBIDDEN in process()
let v: Vec<f32> = Vec::new();
let s = String::from("x");
let b = Box::new(Node {});
format!("{}", x)          // allocates internally
```

**Why:** the system allocator is not real-time safe — it may block on a mutex
or call into the OS. At 48 kHz / 512 frames the budget is ~10.7 ms per block;
allocation can take µs to ms — enough to miss the deadline.

**Fix:** pre-allocate everything in `prepare()` or at graph compile time. Use
fixed-size arrays.

### 2. No deallocation

`drop` of owned heap data on the audio thread is forbidden for the same reason
as allocation. Displaced schedulers ride the SPSC **drop ring** back to the
command thread, which performs the actual free.

### 3. No blocking locks

```rust
// ✗ FORBIDDEN in process()
some_mutex.lock();
some_mutex.try_lock();   // not strictly lock-free on Windows (SRW lock)
```

`Mutex::lock()` blocks until the lock is available. If the holder is a
low-priority thread, this causes a **priority inversion**: the high-priority
audio thread waits for the low-priority UI thread. `try_lock` is also forbidden
— on Windows `std::sync::Mutex::try_lock` still goes through
`AcquireSRWLockExclusive`, which is not strictly lock-free.

**The approved scheduler-update path:** `RuntimeSnapshot::try_swap` is backed by
an `AtomicPtr<GraphScheduler>` (lock-free) plus the bounded SPSC drop ring. The
audio thread neither locks nor drops.

### 4. No async / await

```rust
// ✗ FORBIDDEN
async fn process_node() { ... }
tokio::spawn(...)
futures::executor::block_on(...)
```

Async runtimes have internal queues and thread pools, none of which are
real-time safe. `block_on` introduces unbounded latency.

### 5. No filesystem or network I/O

```rust
// ✗ FORBIDDEN in process()
std::fs::read("audio.wav")
std::net::TcpStream::connect(...)
```

I/O can block indefinitely waiting on the kernel. Even a cached file read can
stall on a page fault. All audio data must be loaded before streaming starts;
graph recompilation (command thread) is the mechanism for incorporating new
data.

### 6. No Tauri event emission

```rust
// ✗ FORBIDDEN
app.emit("engine-event", payload);
```

`app.emit()` acquires an internal Tauri lock. Calling it from the audio thread
blocks and causes underruns. For real-time feedback to the UI (meters,
position), write to atomics and have the UI poll them at 30–60 fps:

```rust
// ✓
peak_atomic.store(peak_val, Ordering::Release);
position_atomic.store(pos, Ordering::Release);
```

### 7. No graph traversal at process time

```rust
// ✗ FORBIDDEN in process()
for node in graph.nodes.values() { ... }  // HashMap iteration, allocation
```

The execution plan pre-computes the traversal order. Traversal at process time
defeats the compiler and may allocate. The scheduler iterates the flat
`ExecutionPlan::steps` array.

### 8. No `println!` / `eprintln!`

Both allocate internally and can block on stdout/stderr locks.

### 9. No `unwrap()` that can panic

No `unwrap()`/`expect()` on `Result`/`Option` without guaranteed success — a
panic in the callback aborts the audio stream.

---

## Permitted in the Audio Callback

```rust
// ✓ arithmetic and stack-allocated arrays
let mut scratch = [0.0f32; 512];
*output = *input * gain;

// ✓ reading/writing own DSP state (no allocation)
self.filter_state += delta;
self.envelope.advance(block_size);

// ✓ atomic loads and stores (Relaxed or Acquire/Release — no SeqCst)
let gain = self.gain.load(Ordering::Relaxed);
self.peak.store(peak_val, Ordering::Release);

// ✓ iterating pre-allocated Vecs without resize
for sample in self.delay_line.iter_mut() { ... }

// ✓ RuntimeSnapshot::try_swap (lock-free AtomicPtr swap; displaced box
//   handed to the SPSC drop ring, never dropped on this thread)
snapshot.try_swap(&mut current_scheduler);

// ✓ monotonic clock read (std::time::Instant — QueryPerformanceCounter /
//   vDSO clock_gettime / mach_absolute_time: no lock, no allocation).
//   Used once per callback to time live MIDI input (ADR-022); never feeds
//   the offline render, so determinism (C-2) is unaffected.
let callback_start = live_clock.now_nanos();
```

### Sandboxed plugins (ADR-015) — the one bounded wait

A sandboxed plugin's device (`plugin_system/bridged_node.rs`: `BridgedNode`
for an effect, `BridgedInstrument` for a track instrument) hands each block to
another process and **waits for the answer on the audio thread**. This is the
only wait allowed in the callback, and it is bounded:

- a spin on the shared-memory ring, capped at 3/4 of the block period (live);
  past it the block is silence, and 8 late blocks in a row condemn the plugin
  so it costs nothing afterwards;
- one kernel call, the non-blocking `SetEvent` that wakes the bridge — no other
  thread holds anything it could wait on;
- no allocation, no lock: the ring, the scratch buffers and the event lists are
  sized at `prepare`.

Offline (`PrepareContext.live == false`) the node waits up to 10 s per block so
the export does not depend on scheduling (C-2).

### Crash marker (ADR-015)

In-process plugin wrappers mark the running plugin in a memory-mapped file
(`plugin_system/crash_marker.rs`) through the `rt_try_enter` guard: one atomic
store per block, no lock, no allocation. The page is file-backed, so after the
OS writes it back the next store may take a soft page fault (no I/O).

---

## Priority Inversion Prevention

The most dangerous scenario is a **priority inversion**: the high-priority audio
thread blocks waiting for a lock held by the low-priority UI thread.

Prevention strategy:

- Audio thread **never** calls `Mutex::lock()` or `Mutex::try_lock()`.
- All cross-thread data exchange uses lock-free primitives (`AtomicPtr`,
  `AtomicU32`, SPSC ring buffers).
- If `try_swap` finds no pending update, the audio thread continues with its
  current scheduler. This is always safe: stale graph state produces correct
  (if slightly outdated) audio output.
- The audio thread also **never deallocates**: displaced schedulers ride the
  drop ring back to the command thread.

### Parallel worker pool (opt-in)

When parallel level execution is enabled (`ECHOES_AUDIO_WORKERS > 0`, see
`../dsp/dsp-runtime.md`), the audio thread spin-waits on a completion barrier for
the worker threads. To keep that wait from becoming an inversion, each worker
raises its own scheduling priority at start-up (Windows:
`THREAD_PRIORITY_TIME_CRITICAL`, best-effort; no-op on other platforms) so a
normal-priority thread cannot preempt it mid-block. Workers also enable FTZ/DAZ
once at start-up, matching the audio thread, and a spurious `park()` return
re-parks instead of panicking (a dead worker would otherwise stall every later
block on the barrier). This is a mitigation, not full RT scheduling — MMCSS
"Pro Audio" / `SCHED_FIFO` is a later, dedicated change.

---

## Stack Usage

The audio thread's stack is limited on some platforms (e.g. 64 KB on some
embedded systems, 256 KB on iOS). Guidelines:

- Avoid deep recursion in `process()` implementations.
- Keep per-frame stack allocations small (a few f32s is fine; 512-sample arrays
  on the stack are borderline — prefer pre-allocated heap).
- The `GraphScheduler` pre-allocates all per-block scratch on the heap at
  construction time.

---

## CPAL Integration Notes

When CPAL streaming is activated (`AudioEngine::start_stream`), the callback
closure must capture:

- `Arc<RuntimeSnapshot>` — for atomic scheduler swap via `try_swap`
- `Box<GraphScheduler>` — the working scheduler, owned locally by the closure
  and updated in place by `try_swap`
- `Arc<SharedTransport>` — atomics for position / playing / sample-rate / BPM / time signature / playback loop (enabled, start, end)
- `Arc<AtomicUsize>` — active output channel count
- `Arc<ChannelLevels>` — per-channel peak cells (AtomicU32)

All cross-thread reads are lock-free. The `TransportInfo` is constructed inline
from atomics — no lock anywhere on the audio path.

---

## Pre-Merge Checklist for DSP Code

Before any code enters the audio path, verify:

| Check                                                 | Rule                              |
| ----------------------------------------------------- | --------------------------------- |
| No `Vec::push`, `Box::new`, `String::new`, `format!`  | No allocation                     |
| No `drop` of owned heap values (use SPSC return path) | No deallocation                   |
| No `Mutex::lock()` or `Mutex::try_lock()`             | No blocking locks                 |
| No `.await`, `block_on`                               | No async                          |
| No `std::fs::*`, `std::net::*`                        | No I/O                            |
| No `app.emit()`                                       | No Tauri events from audio thread |
| No `HashMap::values()` / `BTreeMap` traversal         | No runtime graph traversal        |
| No `println!` / `eprintln!`                           | Both allocate internally          |
| No `unwrap()` without guaranteed success              | No panic in callback              |
| Atomic loads/stores use Relaxed or Acquire/Release    | No SeqCst on hot path             |

---

## Future: Runtime Verification

- Consider the `assert_no_alloc` crate to catch allocations on the audio thread
  in debug builds.
- Once the audio-thread boundary is cleaner, add strict Clippy lints to
  `src-tauri/Cargo.toml` (e.g. deny dynamic dispatch in the hot path).

---

## Further Reading

- `threading-model.md` — full thread-ownership diagram and synchronization boundaries
- `../dsp/dsp-runtime.md` — the scheduler execution model
- `../architecture/audio-graph.md` — buffer safety model
- `../decisions/004-immutable-runtime-snapshots.md` — why the snapshot swap is lock-free
