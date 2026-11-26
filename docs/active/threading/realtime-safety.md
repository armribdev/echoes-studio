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

### Audio thread → watcher thread (ADR-037)

```
output events of a CLAP plugin  →  SpscRing<ParamEdit> (per instance)  →  watcher (25 ms)
```

Cost on the audio thread: one relaxed `AtomicBool` load per output event when writing
is not armed; when armed and playing, one ring push (fixed slots, no allocation) per
edit and a lookup in a `HashMap` cloned at compile time. While a drag is open on a
parameter, its automation is not sent (touch).

### MIDI capture threads → Audio thread (ADR-022)

```
MidiLiveInputQueue::push (capture threads, serialized by a producer mutex)
    →  drain_due() in InstrumentNode::process (lock-free consumer)
```

The producer mutex is only ever taken by the capture threads (one per enabled
keyboard); the audio thread reads head/tail atomics only. Keyboard controls
(bend, CC, aftertouch) are `Copy` events in the same pre-allocated ring and
event lists — no new RT cost (ADR-022 amendment 2026-10-13).

Recorded clip controls (ADR-029) are read from the published
`MidiTrackContent` like notes; the catch-up state is a fixed 130-slot array in
`InstrumentNode`, and `push_control` keeps 32 of the 256 per-block event slots
for notes. `MAX_EVENTS_PER_BLOCK` must stay ≤ 512: `sort_by_offset` is a stable
sort whose scratch fits its 4 KB stack buffer only up to 512 eight-byte
events — beyond that it would allocate on the audio thread.

The arpeggiator (ADR-030) keeps the same 256-event list: its output list is a
second pre-allocated `MidiEventList` inside `InstrumentNode`, it emits at most
two events per step, and when fewer than three places remain it skips the step
but never a due `NoteOff` (one note sounds at a time, so one release is ever
pending). Held notes, the step state and the random generator are fixed arrays;
settings are read as relaxed atomics, once per block.

The note effects (ADR-038) add a third pre-allocated `MidiEventList` and ~0.7 KB
of fixed tables to `InstrumentNode`. A chord multiplies events by up to four, so
the stage emits a `NoteOn` only when the list has room for its voices **and
their releases** (`2 × voices`); otherwise the note is skipped whole and not
recorded, so its `NoteOff` is ignored. A `NoteOff` group that does not fit stays
owed and is written first, at offset 0, in the next block: late by one block,
never stuck. The limit stays 256 (ADR-029 §5).

A VST3 instrument (ADR-013 amendment 2026-10-21) hands its notes to the plugin in
a fixed `HostEventList` (512 `Event`s, ~24 KB, boxed in `prepare`), reset and
refilled each block; a full list drops the event, except that a NoteOff it cannot
take is kept for the next block. Its `IMidiMapping` table is read in `prepare`
under the FFI gate; `process_midi` only looks it up and pushes points into the
pre-allocated parameter-change list.

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

// ✓ one compare-and-swap on the export gate per block (ADR-036): never waits;
//   when an export holds the plugins the callback writes silence and returns
let Some(_live_block) = stream_gate.enter_live() else { /* silence */ return; };

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

**The block message (protocol v3, ADR-033 step 4, 2026-11-04).** The message is
`[header 48 B][transport 64 B][notes 12 B × n][parameter events 24 B × m][samples]`
(`plugin_bridge::protocol`), format-neutral; the bridge's CLAP adapter builds the
CLAP events from it. It changes on **both** audio threads, under the same rules:

- same rings, same wake-up, same bounded wait, same single `SetEvent`; no lock,
  no new system call;
- capacities are fixed and allocated before the first block. Engine side: the
  parameter events are a vector of 1024 records and the notes a vector of 256,
  both filled only while `len < capacity` (what does not fit is dropped, like the
  CLAP event pool in-process); the reply buffers (256 edits, 64 MIDI messages) are
  allocated in `BridgeRoundTrip::new`. Bridge side: the reply lists are allocated
  once when `serve_audio` starts, each sized to half of `event_capacity`
  (`ReplyWriter::push_*` returns `false` when full);
- a block whose sections exceed `event_capacity` is not sent (`Skipped`, the
  caller passes the input through), as before;
- records are copied to and from the ring as bytes (`WireRecord`, no padding,
  unaligned reads); the only `assert!` on the path is in `WireRecord::read`, whose
  callers check the length first.

Round-trip time of a block on the test plugin, before and after: see "Step 4" in
ADR-033.

**Several audio ports per block (protocol v4, ADR-033 step 8b, 2026-11-05).** The
header's `channel_count` is now the channels of the samples **of that message**: a
request carries every input port, a reply every output port, two channels each
(`PORT_CHANNELS`), in the order of the handshake's active ports. Nothing else about
the message changes: same rings, wake-up, bounded wait, single `SetEvent`; no lock,
no allocation and no new system call on either audio thread.

- Sizes are fixed before the first block. The shared region holds `PORT_CHANNELS ×
MAX_PORTS` (18) channels for a VST3 plugin (2 for CLAP); the engine's input and
  output vectors and the bridge's per-port buffers are made at `prepare` / at the
  activation. A block wider than the region is not sent (`Skipped`).
- The bridge reads `BridgeProcessor::output_channels()` before each block (an atomic
  free read of a count fixed at load); the audio thread never asks the plugin.
- A bus the graph does not read stays closed in the plugin and its port plays
  silence (the activation's buffers are new and zeroed, the reply buffer is cleared
  before each block). A block the bridge skips passes the main input through and
  leaves the other ports silent.
- The input side works the same way: a request carries two channels per input port (the main one and the
  sidechains of an effect). The engine copies each graph input into a vector sized at `prepare`; the bridge
  copies each port into a buffer made at the activation and binds them with `Vst3InputBuses::bind`, which
  writes pointers, the mono mix and the silence flags and allocates nothing.
- The set of auxiliary buses to open travels in the `Activate` request
  (`wanted_outputs`, off the audio thread, main thread of the bridge); a different
  set reactivates the plugin, as a restart does.

**VST3 presets in the bridge (ADR-033 step 9).** Nothing changes on either audio thread. Listing
programs, choosing one and loading a preset file's states are control requests on the bridge's
main thread, under the same ADR-010 gate as a state load: the audio thread never waits on it
and passes its block through while the gate is held. A program is given to the processor by a
zero-sample `process` on the main thread with the gate held (the flush the bridge already uses
before a state save), not through the block message.

**VST3 in the bridge (ADR-033 step 5).** The message is the same; the engine side is
unchanged (step 6 below added its parameter section). Bridge side,
`Vst3BlockProcessor::process`: no allocation (the activation sizes the buffers, the bus
tables and the parameter-change list before the first block), no lock, no I/O, no
`drop`, no new system call. The ADR-010 gate is one atomic compare-exchange that the
audio thread **never waits on**: when the bridge's main thread holds it (activation,
`getState`, `setState`) the block is passed through. Measures, 8 sandboxed effects:
ADR-033, "Step 5".

**VST3 parameters in the bridge (ADR-033 step 6).** Still the same message, rings, wake-up,
bounded wait and single non-blocking `SetEvent`. Engine audio thread:
`Vst3ParamEvents::fill_into` fills the preallocated wire section (full means dropped),
`observe_edit` reads at most the 256 edits a reply keeps and keeps a gesture list of fixed
capacity; no allocation, no lock. Bridge audio thread: the parameter-change list, the
output list, the reply slice and the gesture snapshot (`active_gestures` returns an array)
are preallocated or on the stack; the plugin's `restartComponent` / `performEdit` /
`setDirty` callbacks only store atomics (`BridgeSignals`, the SPSC edit queue); the
processor → controller table (`ParamMirror`) is written only while a window watches and
never read by the audio thread. Every control op that touches the plugin (`Params`,
`ParamValue`, `ParamText`, `FlushValues`, `SaveState`, `LoadState`, `Activate`) takes the
main-thread side of the gate; the audio thread skips its block, it does not wait.

**VST3 editor in the bridge (ADR-033 step 7).** The audio thread of neither process changes: same
message, rings, wake-up, bounded wait and single non-blocking `SetEvent`, no new system call.
The whole view (`createView`, `attached`, `onSize`, `removed`, the `IPlugFrame`) runs on the
bridge's **main** thread; the bridge's audio thread never calls the view nor waits for the
window. The plugin's `resizeView` callback only resizes the window on that thread, and
`performEdit` / `restartComponent` only store atomics. The one controller call added
(`createView`) takes the main-thread side of the gate, so the audio thread skips a block instead
of waiting; the mirror table is still written by the audio thread only while a window watches.

**VST3 instrument in the bridge (ADR-033 step 8a).** The message, the rings, the wake-up, the bounded
wait and the single non-blocking `SetEvent` are unchanged (protocol still 3); the instrument adds no
system call and no wait on either audio thread. On the bridge's audio thread the block's notes go
through `vst3_host::notes` into a `HostEventList` allocated at activation (fixed capacity, a full list
drops the event, a dropped NoteOff is kept), and the raw MIDI controls through `push_mapped_controls`
into the preallocated `IParameterChanges` — the `IMidiMapping` table is read on the main thread, at
activation, under the gate. When the main thread holds the gate the block is skipped as for an effect;
the NoteOffs of that block are kept in a `SoundingNotes` that belongs to the audio thread (a 128-entry
array, no allocation) and sent first in the next block, so no note stays on. The engine's side is the
existing `BridgedInstrument`.

### Sleeping CLAP plugins (2026-10-14)

A CLAP plugin that is silent and has no tail left is not called at all
(`plugin_formats/clap/sleep.rs`, `clap_ffi::ProcessSleep`; rule in
`../plugins/plugin-wrapper-system.md`). On the audio thread this adds:

- plain integers and booleans in the node (no allocation, no lock);
- bounded scans of the block's buffers (frames × channels), only when a
  decision needs them;
- three lock-free flag swaps on the plugin's `ClapHostContext`
  (`request_process`, `tail.changed`; set by the plugin from any thread);
- one FFI call, `clap.tail.get`, only after `tail.changed` (callable from the
  audio thread per the CLAP spec), inside the same FFI gate and
  `audio_thread_scope` as `process`;
- a zero-fill of the outputs instead of the `process` call while asleep.

The decision never reads a clock, so the offline render sleeps at the same
blocks as the live one (C-2). A sandboxed plugin decides inside the bridge;
the engine's bounded wait is unchanged.

Host `clap.params` (2026-10-14) adds no audio-thread work: `request_flush` sets
the `request_process` flag above, and `rescan` is one `fetch_or` on a global
atomic, polled by the plugin-edit watcher. The only new FFI call is
`value_to_text`, made on the command thread under the instance FFI gate like the
other off-RT introspection; while it holds the gate a block in flight bypasses
the plugin, which is why the UI asks at most every 100 ms.

Host `clap.track-info` (2026-10-14) adds no audio-thread work and takes no lock.
`get` is `[main-thread]` in the spec but reachable from any thread, so a plugin
may call it where it should not: it reads a sequence lock on the plugin's
`ClapHostContext` (atomics only, no allocation, bounded retries, never waits for
the writer). The writer and `changed()` run on the command thread (the bridge's
main thread when sandboxed), `changed()` under the instance FFI gate like
`value_to_text` — a block in flight makes it wait up to 3 ms, never the other way
round, and a notification that misses the gate is retried by the plugin-edit
watcher. `changed()` is never called from the audio callback.

Host `clap.note-ports` / `clap.note-name` (2026-10-14) add no audio-thread work
either: `supported_dialects` returns a constant, `rescan(ALL)` is the lock-free
restart flag, and `note-name.changed` is one atomic store (a plugin may call it
from `process`). The plugin's names are read only on the command thread, under
the instance FFI gate, by the `get_plugin_note_names` command.

Host `clap.timer-support` (2026-10-14) adds no audio-thread work and takes no
lock. `register_timer` / `unregister_timer` are `[main-thread]` in the spec but a
plugin may call them from `process`: they only touch a fixed table of atomics in
the plugin's `ClapHostContext` (compare-and-swap on a slot, no allocation, no
lock, no wait). `on_timer` is never called from a host callback nor from the
audio thread: it comes from the editor window's `WM_TIMER` (Win32 thread) or the
bridge's main loop, and only while the editor is open. Unlike the other off-RT
calls it does **not** take the instance FFI gate — a periodic call holding it
would make the audio thread bypass the blocks it overlaps; it only skips a tick
while another off-RT caller holds the gate. The offline render opens no editor,
so it never calls `on_timer` (C-2).

Host `clap.preset-load` (2026-10-14) adds no audio-thread work and takes no lock:
`loaded` is one atomic store (the unsaved-changes flag); `on_error` prints, but
returns first when called from the audio thread.

Preset discovery (ADR-031, 2026-10-15) touches no audio-thread path: it runs on a
blocking worker thread without the engine lock, behind the SEH guard, one listing
at a time, and neither takes nor waits on any instance FFI gate.

Loading a preset (ADR-031 step 2) is an off-RT live-FFI caller and follows the
ADR-010 rule: `PluginInstance::load_preset` claims the instance FFI gate
(`offrt_try_acquire`, 50 ms) and holds it for the whole `from_location`. The audio
thread never waits: `rt_try_enter` fails and the wrapper skips the block with its
transparent bypass (an effect passes through, an instrument is silent) until the
load is done. A load can therefore cut a plugin's sound for as long as the plugin
takes to apply the preset; a block in flight past the budget refuses the load
instead. Sandboxed plugins are loaded through the bridge and need no gate. The
host's `loaded` stays one atomic store.

VST3 presets (ADR-031 amendment, 2026-10-22) follow the same rule and add nothing to
the audio thread. Loading a `.vstpreset` file reads and checks the file **before**
the gate is taken, then holds the gate (`offrt_try_acquire`, 50 ms) for
`setState` / `setComponentState` / `setState` of the controller; the plugin is
bypassed meanwhile. Choosing a program holds the gate for the controller call, then
releases it and waits (a bounded sleep, up to 200 ms, on the command-side worker
thread) for the audio thread to drain the parameter queue — the audio thread only
does what it already does with a queued change — before taking the gate again for
`getState` / `setComponentState`. Listing programs reads the controller under the
gate; listing files touches the disk without any gate or engine lock.

VST3 controller follow-up (ADR-008 amendment, 2026-10-23) adds three things the
audio thread does and one it never does:

- **Publishes** to `ParamMirror` (`vst3/param_mirror.rs`): per parameter, one
  relaxed store of the value and one release store of a flag into a preallocated
  table of 2048 slots. A slot is claimed with a compare-and-swap on its id;
  probing is bounded (32), so a full table drops the value. No allocation, lock
  or wait. When no editor is open, one acquire load of the watcher count and
  nothing else.
- **Fills** a second preallocated `HostParameterChanges` per wrapper (the plugin's
  `outputParameterChanges`, about 200 KB boxed in `prepare`), emptied each block
  with `reset()`; the plugin's `addParameterData` / `addPoint` use the same
  allocation-free code as the input list, and a plugin that writes more than 128
  parameters gets a null queue. The last point of each queue is then published.
- It does **not** call the controller. The window thread does, from its
  `WM_TIMER`, without the FFI gate: a held gate makes the audio thread bypass the
  blocks it overlaps, which a 60 Hz call would turn into audible dropouts.

A VST3 state save (ADR-010 amendment, 2026-10-23) runs a zero-sample `process` on
the **command thread**, only with the FFI gate held, so it never overlaps a block
(the audio thread bypasses the plugin meanwhile, like any other off-RT FFI call).
It is sent only when a parameter is modulated by value.

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
