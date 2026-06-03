---
name: rt-validation
description: Validate that code on or feeding the audio thread is real-time safe. Use before merging any change to audio_graph/scheduler, worker_pool, an AudioNode::process, a plugin wrapper process(), or audio_engine callback code.
---

# RT Validation

## Objective
Prove a change cannot allocate, lock, block, drop, or panic on the audio path.

## Workflow
1. Identify whether the changed code is reachable from the CPAL callback
   (`audio_engine/engine.rs` → `scheduler.rs` → `AudioNode::process` / wrapper
   `process` / `worker_pool.rs`). If not reachable, this skill doesn't apply.
2. Read `.claude/rules/rt_audio_rules.md` and
   `docs/active/threading/realtime-safety.md`.
3. Walk the checklist below against the diff.
4. Confirm any new buffers/scratch are allocated in `prepare()` or at compile
   time, not in `process()`.
5. Confirm the bit-identical offline harness (`audio_engine/offline.rs`) still
   passes (migration C-2).

## Checklist (any "yes" = block)
- [ ] `Vec::push` / `Box::new` / `String` / `format!` / collect on the path?
- [ ] `drop` of owned heap data (instead of the SPSC return ring)?
- [ ] `Mutex`/`RwLock` `lock()` or `try_lock()`?
- [ ] `.await` / `block_on` / spawn?
- [ ] `std::fs` / `std::net` / any syscall that can block?
- [ ] `app.emit()` or any Tauri call?
- [ ] `HashMap`/`BTreeMap` traversal at process time?
- [ ] `println!` / `eprintln!` / logging that allocates?
- [ ] `unwrap()`/`expect()`/indexing that can panic?
- [ ] `SeqCst` atomics on the hot path (prefer Relaxed/Acq-Rel)?

## Anti-patterns
- "It's just a small Vec / one log line" — still forbidden.
- Holding a lock "briefly" on the audio thread — priority inversion risk.
- Catching panics with `catch_unwind` and assuming safety — it does **not**
  catch C++ SEH exceptions from plugins (see `engine_quirks.md`).

## Validation (done when)
- Every checklist item is "no", new memory is in `prepare()`/compile time, and
  the offline render is bit-identical. If a violation is unavoidable, stop and
  open an ADR (`.claude/rules/architecture_rules.md`).
