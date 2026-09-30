# Threading Rules

Thread ownership is fixed. Code must not blur these boundaries.

| Thread                                     | Owns / may do                                                                                               | Must NOT                                                  |
| ------------------------------------------ | ----------------------------------------------------------------------------------------------------------- | --------------------------------------------------------- |
| UI (Svelte/JS)                             | user input, reactive mirror of state                                                                        | mutate engine state directly                              |
| IPC (Tauri)                                | command/event bridge                                                                                        | run DSP                                                   |
| Command (`engine_gateway/`)                | validate commands, mutate `AppProject`, compile graph, enqueue snapshot, run `drop` of displaced schedulers | touch the audio callback; block on the audio thread       |
| Audio (CPAL callback)                      | `process_block` only, read atomics, `try_swap`                                                              | allocate, lock, drop, I/O, emit (see `rt_audio_rules.md`) |
| Worker pool (`audio_graph/worker_pool.rs`) | parallel node processing within a block                                                                     | escape RT constraints — same law as the audio thread      |

**Approved cross-thread mechanisms only:**

- Command → Audio: `RuntimeSnapshot::enqueue` / `try_swap` (lock-free `AtomicPtr` + SPSC drop ring).
- Audio → UI: atomic meter/position cells, polled by the UI (never `emit` from audio).
- Project → Audio: **never directly** — always recompile → enqueue.
- Plugin params Command → Audio: SPSC `ParamChange` queue (`plugin_system/param_queue.rs`, ADR-008).

**Priority inversion** is the cardinal sin: the audio thread must never wait on a
lock held by a lower-priority thread.

> Full diagram + synchronization boundaries: **`docs/active/threading/threading-model.md`**.
> Audio-thread prohibitions: `rt_audio_rules.md` → `docs/active/threading/realtime-safety.md`.
> Snapshot swap rationale: ADR-004.
