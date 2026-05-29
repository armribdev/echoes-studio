# Real-Time Safety Rules

These are absolute constraints. There are no exceptions unless explicitly noted.
Violations cause audio underruns, priority inversions, or crashes.

---

## Prohibited in the Audio Callback

### No heap allocation

```rust
// ✗
let v: Vec<f32> = Vec::new();
let s = String::from("x");
let b = Box::new(Node {});
format!("{}", x)          // allocates internally
```

The system allocator may block on a mutex or call into the OS.
At 48 kHz / 512 frames, the budget is ~10.7 ms per block.
Allocation can take µs to ms — enough to miss the deadline.

**Fix:** Pre-allocate everything in `prepare()`. Use fixed-size arrays.

---

### No blocking locks

```rust
// ✗
some_mutex.lock();
some_mutex.try_lock();   // not strictly lock-free on Windows (SRW lock)
```

`Mutex::lock()` blocks until the lock is available. If the holder is a
low-priority thread, this causes a **priority inversion**: the high-priority
audio thread waits for the low-priority UI thread. `try_lock` is also
forbidden — on Windows `std::sync::Mutex::try_lock` still goes through
`AcquireSRWLockExclusive`, which is not strictly lock-free.

**The approved scheduler-update path:** `RuntimeSnapshot::try_swap` is
backed by an `AtomicPtr<GraphScheduler>` (lock-free) plus a bounded SPSC
ring used to hand the displaced scheduler back to the command thread for
deallocation. The audio thread neither locks nor drops.

---

### No async / await

```rust
// ✗
async fn process_node() { ... }
tokio::spawn(...)
futures::executor::block_on(...)
```

Async runtimes have internal queues and thread pools, none of which are
real-time safe. `block_on` introduces unbounded latency.

---

### No filesystem or network I/O

```rust
// ✗
std::fs::read("audio.wav")
std::net::TcpStream::connect(...)
```

I/O can block indefinitely waiting on the kernel. Even a cached file
read can stall on a page fault.

All audio data must be loaded before streaming starts. Graph recompilation
(command thread) is the mechanism for incorporating new data.

---

### No Tauri event emission

```rust
// ✗
app.emit("engine-event", payload);
```

`app.emit()` acquires an internal Tauri lock. Calling it from the audio
thread blocks and causes underruns.

For real-time feedback to the UI (meters, position), write to atomics:

```rust
// ✓
peak_atomic.store(peak_val, Ordering::Release);
position_atomic.store(pos, Ordering::Release);
```

The UI polls these at 30–60 fps via a Tauri command.

---

### No graph traversal at process time

```rust
// ✗
for node in graph.nodes.values() { ... }  // HashMap + allocation
```

The execution plan pre-computes the traversal order. Traversal at process
time defeats the compiler and may allocate.

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
```

---

## Pre-Merge Checklist for DSP Code

Before any code enters the audio path, verify:

| Check | Rule |
|-------|------|
| No `Vec::push`, `Box::new`, `String::new`, `format!` | No allocation |
| No `drop` of owned heap values (use SPSC return path) | No deallocation |
| No `Mutex::lock()` or `Mutex::try_lock()` | No blocking locks |
| No `.await`, `block_on` | No async |
| No `std::fs::*`, `std::net::*` | No I/O |
| No `app.emit()` | No Tauri events from audio thread |
| No `HashMap::values()` or `BTreeMap` traversal | No runtime graph traversal |
| No `println!` / `eprintln!` | Both allocate internally |
| No `unwrap()` on `Result`/`Option` without guaranteed success | No panic in callback |

---

## Further Reading

- `../realtime-safety-rules.md` (legacy) — thread model, CPAL integration details,
  approved inter-thread mechanisms with code examples
- `../dsp-runtime-rules.md` (legacy) — five prohibitions with extended explanations
- `architecture/threading-model.md` — full thread diagram and synchronization boundaries
