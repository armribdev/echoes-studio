# Snapshot Rebuild Flow

This document describes the exact sequence of operations when a project change
triggers an audio graph rebuild.

## Trigger Points

The following events trigger `recompile_if_needed()` in `engine_gateway/api.rs`:

- `CREATE_TRACK` / `DELETE_TRACK`
- `UPDATE_TRACK` (volume change)
- `ADD_CLIP` / `MOVE_CLIP` / `REMOVE_CLIP`
- `UNDO` / `REDO` (full project reset)
- Project load

Transport commands (`PLAY`, `STOP`, `SEEK`) do **not** trigger recompilation.

## Full Rebuild Sequence

```
1. UI sends command (e.g. CREATE_TRACK)
       │
2. engine_gateway::api::process_command()
       │  validate payload
       │  mutate AppProject
       │  append ProjectEvent to EventStore
       │  emit EngineEvent to UI
       │
3. recompile_if_needed(&state)
       │
       ├─ lock audio_engine
       ├─ lock project (for reading)
       │       │
       │  4.   AudioEngine::update_from_project(&project)
       │           │
       │      5.   AudioGraph::from_project(&project)
       │               builds topology: track → gain → master
       │           │
       │      6.   GraphCompiler::compile(&graph, &project, config)
       │               topological sort
       │               build ExecutionSteps
       │               instantiate nodes (TrackNode, GainNode, MasterNode)
       │               call prepare() on each node
       │               return CompiledGraph { plan, processors }
       │           │
       │      7.   GraphScheduler::new(compiled)
       │               pre-allocate output_buffers
       │               pre-allocate silence_buffer
       │               pre-allocate input_ptr_scratch
       │           │
       │      8.   RuntimeSnapshot::enqueue(scheduler)
       │               lock pending_mutex (blocking — command thread)
       │               store Some(scheduler)
       │               unlock
       │
       release locks
       │
9. (audio callback runs at some future point)
       │
       ├─ snapshot.try_take()
       │       try_lock pending_mutex (non-blocking)
       │       if Some(s): replace current scheduler with s
       │       if None:    continue with previous scheduler
       │
10. scheduler.process_block(&transport, output)
            run steps in topological order
            write interleaved f32 to CPAL output
```

## Atomicity Guarantees

### Project mutation (step 2)

The command thread holds `state.project` lock during mutation and releases
it before calling `recompile_if_needed`. The audio thread never touches
`AppProject` directly — it only reads the compiled `GraphScheduler`.

### Scheduler swap (step 8 → 9)

The `RuntimeSnapshot` holds `Mutex<Option<GraphScheduler>>`.

- Command thread: `lock()` (blocking) → write `Some(scheduler)` → unlock.
- Audio thread: `try_lock()` (non-blocking) → take → unlock.

If the audio thread's `try_lock` races with the command thread's `lock`,
the audio thread skips the swap for one block (10.7 ms at 48 kHz / 512 frames).
This is imperceptible.

### Overwrite semantics

If the command thread enqueues a second scheduler before the audio thread
has consumed the first, the first scheduler is silently dropped. The audio
thread always picks up the **latest** state. This is correct because:
- The second scheduler is always a superset of the first.
- The dropped scheduler is deallocated on the command thread (no audio glitch).

## Memory Lifetime

```
CompiledGraph { plan: Arc<ExecutionPlan>, processors }
    │
    ▼  GraphScheduler::new() consumes this
GraphScheduler { plan: Arc<ExecutionPlan>, processors, output_buffers, ... }
    │
    ▼  RuntimeSnapshot::enqueue() moves this
RuntimeSnapshot::pending = Some(scheduler)
    │
    ▼  try_take() moves this to audio thread
audio_thread_scheduler = scheduler

The Arc<ExecutionPlan> is shared — zero copy.
The processors (Box<dyn AudioNode>) are moved to the audio thread.
The old scheduler (and its processors) are dropped on whichever thread
last held it (usually the audio thread, or the command thread if overwritten).
```

## Performance Characteristics

| Operation                       | Cost                            |
|---------------------------------|---------------------------------|
| `AudioGraph::from_project`      | O(tracks) — fast                |
| `GraphCompiler::compile`        | O(nodes + edges) — fast         |
| Node instantiation              | O(nodes) — may involve alloc    |
| `node.prepare()`                | O(1) per node — may alloc       |
| `GraphScheduler::new()`         | O(nodes × ports × block_size)   |
| `RuntimeSnapshot::enqueue()`    | O(1) — single mutex + move      |
| `try_take()` in audio callback  | O(1) — single try_lock          |
| `process_block()`               | O(nodes × frames × channels)   |

The dominant cost is `GraphScheduler::new()` which pre-allocates all audio
buffers. For a project with 16 tracks, 2 channels, 512 frames: 
`16 nodes × 3 ports × 512 frames × 4 bytes ≈ 100 KB` — negligible.
