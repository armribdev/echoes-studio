# audio_graph/ — DSP graph core

This is the heart of the engine: the compiler, scheduler, buffers, and node
model. Most code here runs **on or feeding the audio thread**.

## Local hard constraints
- Code reachable from `scheduler.rs` / `worker_pool.rs` is RT-critical: no alloc,
  no locks, no I/O, no `drop`. See `.claude/rules/rt_audio_rules.md`.
- The graph the audio thread executes is **immutable**. Mutations go through
  recompile → `RuntimeSnapshot::enqueue` → `try_swap` (ADR-004).
- The compiler is **extended, never rewritten** (migration C-7).
- Determinism is mandatory: topo sort tiebreaks by `NodeId`; same project +
  transport ⇒ bit-identical output (migration C-2).
- `unsafe` for buffer aliasing is justified only by the `src_step < step`
  invariant — keep that assertion intact.

## Key files
`compiler.rs`, `execution_plan.rs` (compile pipeline) · `scheduler.rs`,
`worker_pool.rs` (runtime) · `graph.rs`, `node.rs` (model) · `buffer.rs` (planar
buffers) · `snapshot*.rs` (swap) · `modulation.rs`, `parameter_block.rs`,
`live_params.rs`, `midi.rs` (params/mod/MIDI).

## Read before changing
- `docs/active/dsp/graph-compiler.md`, `docs/active/dsp/dsp-runtime.md`
- `docs/active/architecture/audio-graph.md`, `.../snapshot-rebuild-flow.md`
- `docs/active/threading/realtime-safety.md`
- ADRs 001, 004, 009, 011, 012, 014 · then run `.claude/rules/completion_protocol.md`.
