# RT Audio Rules (the law)

Absolute constraints for **any code reachable from the CPAL audio callback**
(`audio_graph/scheduler.rs`, `worker_pool.rs`, every `AudioNode::process`, every
plugin wrapper `process`). No exceptions without an ADR.

**Forbidden on the audio thread:**

- Heap allocation (`Vec::push`, `Box::new`, `String`, `format!`).
- Deallocation / `drop` of owned heap data → hand back via the SPSC drop ring.
- Blocking or `try_lock` on any `Mutex`/`RwLock`.
- `async` / `.await` / `block_on`.
- Filesystem or network I/O.
- `app.emit()` / any Tauri call.
- `HashMap`/`BTreeMap` traversal (use the pre-compiled `ExecutionPlan`).
- `println!` / `eprintln!`.
- `unwrap()`/`expect()` that can panic.

**Allowed:** arithmetic, fixed-size stack arrays, own DSP state, atomics
(Relaxed or Acquire/Release — no SeqCst on the hot path), iterating
pre-allocated slices without resize, `RuntimeSnapshot::try_swap`.

**Where new memory is allowed:** `prepare()` and graph compile time only.

> Full rationale, code examples, thread model, CPAL integration, and the
> pre-merge checklist: **`docs/active/threading/realtime-safety.md`** (authoritative).
> Scheduler execution model: `docs/active/dsp/dsp-runtime.md`.
> Changing any of these is an architectural decision — see `architecture_rules.md`.
