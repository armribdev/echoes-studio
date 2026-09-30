# Architecture Rules (hard invariants)

These properties define the system. Changing any of them requires an **ADR + user
approval** — never a silent edit. See
`docs/active/decisions/architecture-decision-rules.md` for the governance process
and the full "never change silently" table.

**Invariants:**

1. **Graph-first.** Every DSP element is an `AudioNode` in the `AudioGraph`. New
   DSP features = new nodes, never special fields on `AppTrack`. (ADR-001)
2. **Immutable runtime snapshots.** The audio thread never sees a mutable graph;
   updates are compiled and atomically swapped. (ADR-004)
3. **Plugin boundary.** `audio_graph/` must not import `plugin_system` or
   `plugin_formats`. The compiler sees only `Box<dyn AudioNode>`. (ADR-003)
4. **Single plugin instance.** One `Arc<PluginInstance>` per device; no duplicate
   hosting. (ADR-007)
5. **UI/engine separation.** Rust owns all state; the frontend is a reactive
   mirror with optimistic UI. No shared mutable state. (ADR-005)
6. **Event-sourcing scope is bounded.** Not everything is an event — see ADR-002.
7. **Determinism.** Same project + transport = bit-identical output. The
   topological sort tiebreaks by `NodeId`. (underpins migration C-2)

**Before any structural change:** follow `completion_protocol.md` (read
`state_index.md`, relevant ADRs, active docs, scoped `CLAUDE.md`).

**If a change conflicts with an invariant:** stop and raise it with the user.
Do not work around it.

> Decision records: `docs/active/decisions/000-index.md`.
