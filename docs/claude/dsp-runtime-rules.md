# DSP Runtime Rules

These rules are **absolute constraints** on all code that runs in the audio
callback. Violations cause audio glitches, priority inversions, or crashes.

## The Five Prohibitions

### 1. No Heap Allocation

```rust
// ✗ FORBIDDEN in process()
let v = Vec::new();
let s = String::from("...");
let b = Box::new(Foo {});
format!("hello {}", x)  // allocates
```

**Why**: The system allocator is not real-time safe. It may block on a
mutex or call into the OS. On macOS/Windows, allocation takes O(µs) to
O(ms) — enough to miss a 512-sample block at 48 kHz (10.7 ms budget).

**What to do instead**: Pre-allocate all buffers, scratch space, and data
structures in `prepare()` or at graph compile time. Use fixed-size arrays.

### 2. No Mutex Locks

```rust
// ✗ FORBIDDEN in process()
let _g = some_mutex.lock();
some_mutex.try_lock()  // even try_lock — it spins on contention
```

**Why**: `Mutex::lock` blocks; `try_lock` still performs atomic operations
that can cause cache-line bouncing between cores. The audio thread must
never wait for any other thread.

**What to do instead**: Use lock-free data structures (atomics, SPSC queues,
`RuntimeSnapshot::try_take`). All inter-thread data exchange must be
designed as producer → consumer with no feedback on the audio path.

**Exception**: `RuntimeSnapshot::try_take` uses `Mutex::try_lock` to pick up
a new scheduler. This is the one permitted lock in the callback, guarded by
`try_lock` (never `lock`) and only taken at the start of the block. If
contended, it is skipped — no audio glitch.

### 3. No Async / Await

```rust
// ✗ FORBIDDEN
async fn process_node(...) { ... }
tokio::spawn(...)
futures::executor::block_on(...)
```

**Why**: Async runtimes have internal queues, lock-free structures, and
thread pools — none of which are real-time safe. `block_on` introduces
unbounded latency.

### 4. No Filesystem or Network I/O

```rust
// ✗ FORBIDDEN in process()
std::fs::read("audio.wav")
std::net::TcpStream::connect(...)
```

**Why**: I/O operations can block indefinitely waiting for the OS kernel.
Even `read` on a cached file can stall on a page fault.

**What to do instead**: Load all audio data before streaming starts.
Changes to project data (new clips, new plugins) trigger a graph recompile
on the command thread; the audio thread picks up the new graph atomically.

### 5. No Graph Traversal

```rust
// ✗ FORBIDDEN in process()
for node in graph.nodes.values() { ... }  // HashMap iteration, allocation
```

**Why**: The execution plan is pre-compiled precisely to avoid runtime
graph traversal. Any traversal at process time defeats the purpose of
the compiler.

**What to do instead**: The `ExecutionPlan::steps` array is the
pre-computed, cache-friendly execution sequence. The scheduler iterates
this flat array.

## What IS Allowed in process()

```rust
// ✓ OK: arithmetic
*output = *input * gain;

// ✓ OK: stack-allocated arrays
let mut scratch = [0.0f32; 512];

// ✓ OK: reading from &self (own DSP state)
self.filter_state += delta;

// ✓ OK: atomic loads (Relaxed / Acquire only, no SeqCst)
let v = self.gain.load(Ordering::Relaxed);

// ✓ OK: writing peak values to atomic cells
self.peak.store(peak_val, Ordering::Release);

// ✓ OK: iterating pre-allocated Vecs (no resize)
for sample in self.buffer.iter_mut() { ... }
```

## Checking Your Code

Use the following mental checklist before merging any DSP code:

1. Does it call any function with `alloc`, `Box`, `Vec::push`, or `String`?
2. Does it call `lock()` or `try_lock()` on any `Mutex`?
3. Does it call `.await` or `block_on`?
4. Does it call `std::fs::*` or `std::net::*`?
5. Does it traverse `HashMap` or `BTreeMap`?
6. Does it use `println!` or `eprintln!` (internally allocates)?

If any answer is yes, the code must be refactored before it enters the audio path.

## Clippy Lints (Recommended)

Add to `src-tauri/Cargo.toml` once the audio thread boundary is cleaner:

```toml
[lints.rust]
# Deny dynamic dispatch in the hot path (optional, strict)
# clippy::dynamic_dispatch = "warn"
```

In the future, consider the `assert_no_alloc` crate for runtime verification
in debug builds.
