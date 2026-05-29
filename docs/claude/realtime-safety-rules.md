# Real-Time Safety Rules

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

## Approved Inter-Thread Mechanisms

### Command thread → Audio thread

```
RuntimeSnapshot::enqueue(scheduler)  →  try_swap(&mut current) at block boundary
                                        (displaced scheduler returned via
                                         drop ring → freed by command thread)
```

The `RuntimeSnapshot` holds an `AtomicPtr<GraphScheduler>` for the pending
slot and a bounded SPSC ring for displaced schedulers awaiting deallocation.

- The command thread writes via `pending.swap(new_ptr)` (lock-free) and
  drains the drop ring on every `enqueue` — `drop` only ever runs here.
- The audio callback calls `try_swap(&mut current)`, which swaps the pending
  `AtomicPtr` and pushes the displaced `Box<GraphScheduler>` into the drop
  ring.  No mutex, no allocation, no `drop` on the audio path.

### Audio thread → UI thread

```
AtomicU32 / AtomicF32 meter cells  →  UI polls via Tauri command
```

Meter values are written atomically from the audio thread.
The UI reads them periodically (e.g. 30 fps) via a Tauri command that acquires
no locks on the audio path.

### Project state → Audio thread

**Never directly**. Project state always goes through the compilation pipeline:

```
AppProject mutation
    → recompile_if_needed()     (command thread)
    → new GraphScheduler
    → RuntimeSnapshot::enqueue()
    → audio thread picks up at next block
```

## Priority Inversion Prevention

The most dangerous scenario is a **priority inversion**: the high-priority
audio thread blocks waiting for a lock held by the low-priority UI thread.

Prevention strategy:
- Audio thread **never** calls `Mutex::lock()` or `Mutex::try_lock()` (on
  Windows the latter still goes through `AcquireSRWLockExclusive`, which is
  not strictly lock-free).
- All cross-thread data exchange uses lock-free primitives (`AtomicPtr`,
  `AtomicU32`, SPSC ring buffers).
- If `try_swap` finds no pending update, the audio thread continues with its
  current scheduler.  This is always safe: stale graph state produces correct
  (if slightly outdated) audio output.
- The audio thread also **never deallocates**: displaced schedulers ride the
  drop ring back to the command thread.

## Stack Usage

The audio thread's stack is limited on some platforms (e.g. 64KB on some
embedded systems, 256KB on iOS). Guidelines:
- Avoid deep recursion in `process()` implementations.
- Keep per-frame stack allocations small (a few f32s is fine; 512-sample
  arrays on the stack are borderline — prefer pre-allocated heap).
- The `GraphScheduler` pre-allocates all per-block scratch on the heap at
  construction time.

## CPAL Integration Notes

When CPAL streaming is activated (`AudioEngine::start_stream`), the callback
closure must capture:
- `Arc<RuntimeSnapshot>` — for atomic scheduler swap via `try_swap`
- `Box<GraphScheduler>` — the working scheduler, owned locally by the closure
  and updated in place by `try_swap`
- `Arc<SharedTransport>` — atomics for position / playing / sample-rate / BPM
- `Arc<AtomicUsize>` — active output channel count
- `Arc<ChannelLevels>` — per-channel peak cells (AtomicU32)

All cross-thread reads are lock-free. The `TransportInfo` is constructed
inline from atomics — no lock anywhere on the audio path.

## Tauri Event Emission from the Audio Thread

**Never** emit Tauri events from the audio thread.

Tauri's `app.emit()` acquires an internal lock. If the audio thread calls
`emit`, it can block and cause an underrun.

For real-time feedback to the UI (meters, position):
1. Write to `AtomicF32` / `AtomicU64` cells.
2. Have the UI poll these at 30–60 fps via a Tauri command.
3. Or use a dedicated meter relay thread that reads atomics and emits events.

## Summary Checklist

Before putting any code on the audio path:

| Check                               | Rule                                    |
|-------------------------------------|-----------------------------------------|
| No heap allocation                  | Vec::push, Box::new, String::new — ✗   |
| No deallocation                     | drop of owned heap data — ✗            |
| No blocking lock                    | Mutex::lock() / try_lock() — ✗         |
| No async                            | .await, block_on — ✗                   |
| No I/O                              | fs::read, TcpStream — ✗               |
| No Tauri emit                       | app.emit() — ✗                         |
| No HashMap traversal                | HashMap::values() — ✗                  |
| No eprintln! / println!             | Allocates internally — ✗               |
| `RuntimeSnapshot::try_swap`         | The approved scheduler-update path — ✓ |
| Atomic loads/stores                 | Relaxed or Acquire/Release — ✓         |
| Pre-allocated slice iteration       | No resize — ✓                          |
