# engine_gateway/ — command thread & state boundary

The bridge between the UI (Tauri IPC) and the engine. Validates commands,
mutates `AppProject`, triggers recompiles, and emits events. This is where
UI/engine separation is enforced.

## Local hard constraints
- Rust owns all state; the frontend is a **reactive mirror** (ADR-005). Never
  expose mutable engine state to the UI; reply with derived snapshots/events.
- Event-sourcing scope is bounded — not every mutation is an event (ADR-002).
  Don't widen the event model without an ADR.
- Structural changes recompile the graph and `enqueue` a new snapshot; transport
  changes do **not** recompile (they update atomics).
- This thread runs the `drop` of displaced schedulers (off the audio thread).
- Watch the recompile cost: `UPDATE_EFFECT`/`UPDATE_TEMPO` quirks live in
  `.claude/memory/engine_quirks.md`.

## Key files
`api.rs` (Tauri commands) · `commands.rs` (command handling/validation) ·
`store.rs` (state/event store) · `events.rs` (outbound events).

## Read before changing
- `docs/active/architecture/state-management.md`
- `docs/active/threading/threading-model.md`
- ADRs 002, 005 · then run `.claude/rules/completion_protocol.md`.
