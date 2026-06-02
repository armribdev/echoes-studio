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
       │               drain SPSC drop ring (free dead boxes — command thread)
       │               AtomicPtr::swap publishes the new Box<GraphScheduler>
       │               if swap returned a non-null pointer (previous update
       │                 not yet picked up), free it here on command thread
       │
       release locks
       │
9. (audio callback runs at some future point)
       │
       ├─ snapshot.try_swap(&mut current_scheduler)
       │       AtomicPtr::swap(null)
       │       if non-null: take new Box, push displaced Box to drop ring
       │       if null:     leave current_scheduler untouched
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

`RuntimeSnapshot` holds an `AtomicPtr<GraphScheduler>` for the pending slot
plus a bounded SPSC ring used to return displaced schedulers to the command
thread.

- Command thread: `AtomicPtr::swap(new_ptr)` publishes the new scheduler.
  Lock-free. If the swap returns a non-null pointer (the previous update
  hadn't been picked up yet), it is dropped here.
- Audio thread: `AtomicPtr::swap(null)` takes ownership. Lock-free.
  The displaced `Box<GraphScheduler>` is pushed into the drop ring instead
  of being dropped inline.

There is no contention case to handle: `swap` always succeeds in one step,
no thread ever waits on the other.

### Overwrite semantics

If the command thread enqueues a second scheduler before the audio thread
has consumed the first, the first scheduler is silently dropped on the
command thread (the `swap` in `enqueue` returns the previous pointer and
hands it to `Box::from_raw`). The audio thread always picks up the
**latest** state, and the dropped scheduler never costs the real-time path.

## Memory Lifetime

```
CompiledGraph { plan: Arc<ExecutionPlan>, processors }
    │
    ▼  GraphScheduler::new() consumes this
GraphScheduler { plan: Arc<ExecutionPlan>, processors, output_buffers, ... }
    │
    ▼  RuntimeSnapshot::enqueue() boxes + publishes raw ptr
RuntimeSnapshot::pending: AtomicPtr<GraphScheduler>
    │
    ▼  try_swap() takes ownership of new Box, pushes old Box to drop ring
audio_thread_scheduler: Box<GraphScheduler>   (in CPAL closure)
                                              │
                                              ▼  drop ring → command thread
                                              dropped on next enqueue()

The Arc<ExecutionPlan> is shared — zero copy.
The processors (Box<dyn AudioNode>) are owned by the audio thread for the
lifetime of the scheduler.
**Every** scheduler is dropped on the command thread — never on the audio
thread (this is the C1 fix from the 2026-05-29 audit).
```

## Performance Characteristics

| Operation                       | Cost                            |
|---------------------------------|---------------------------------|
| `AudioGraph::from_project`      | O(tracks) — fast                |
| `GraphCompiler::compile`        | O(nodes + edges) — fast         |
| Node instantiation              | O(nodes) — may involve alloc    |
| `node.prepare()`                | O(1) per node — may alloc       |
| `GraphScheduler::new()`         | O(nodes × ports × block_size)   |
| `RuntimeSnapshot::enqueue()`    | O(1) atomic swap + ring drain   |
| `try_swap()` in audio callback  | O(1) atomic swap + ring push    |
| `process_block()`               | O(nodes × frames × channels)   |

The dominant cost is `GraphScheduler::new()` which pre-allocates all audio
buffers. For a project with 16 tracks, 2 channels, 512 frames: 
`16 nodes × 3 ports × 512 frames × 4 bytes ≈ 100 KB` — negligible.
