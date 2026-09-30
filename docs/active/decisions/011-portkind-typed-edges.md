# ADR-011 — Typed `PortKind` edges, default `Audio` (Phase 5a)

## Status

Accepted. Implements **Phase 5a** of
[`docs/active/migration/migration_plan.md`](../migration/migration_plan.md) — the `ADR-008` (`PortKind`)
of [`docs/active/migration/target_architecture.md`](../migration/target_architecture.md) §9. File
numbering continues from the existing `decisions/` series (the conceptual
"ADR-008" of the target maps here to `011`, as `008`–`010` are already taken by
param-transport / parameter-block / state-persistence).

Precondition: **Phase 1** declared the `PortKind` enum but left it inert. This
ADR makes it a property of every graph `Edge` and adds the compiler guard.

Unblocks: **Phase 5c** (real `Midi` edges) and **Phase 5e** (`Modulation`
edges), which relax the guard introduced here.

---

## Context

Audit [`plugin-system-audit-2026-05-31.md`](../audits/plugin-system-audit-2026-05-31.md)
**P1-5** / target **BP-2** — _"Pas de séparation port audio / port modulation /
port MIDI"_:

- `Edge { from: PortId, to: PortId }` carried no type. Every connection was
  implicitly audio.
- Routing MIDI to an instrument plugin (5c) or a modulation signal as a graph
  edge (5e) had no representation, and the compiler could not reject a
  heterogeneous connection.
- `PortKind { Audio { channels }, Midi, Modulation { polarity }, Control }` was
  already declared in [`node.rs`](../../../src-tauri/src/audio_graph/node.rs)
  (Phase 1) precisely so this step would not churn match exhaustiveness across
  the codebase.

`Edge` is a runtime topology type rebuilt by `AudioGraph::from_project` on every
compile; it is **not** serialised, so typing it does not touch the project file
format (BP-3 distinction).

## Decision

Add a `kind: PortKind` field to `Edge`. Legacy construction defaults it to
`Audio { channels: 2 }`, producing a byte-identical compiled graph. The compiler
gains a `validate_port_kinds` pass that **rejects any non-`Audio` edge** as a
fail-fast guard, since no node yet declares MIDI / modulation ports to route
them to.

### Module map

1. **[`audio_graph/graph.rs`](../../../src-tauri/src/audio_graph/graph.rs)**
   - `Edge` gains `kind: PortKind`.
   - `connect(..)` unchanged in signature — delegates to `connect_typed` with
     `PortKind::default_audio_stereo()` (the legacy path).
   - New `connect_typed(.., kind)` — the only constructor of typed edges,
     wired up in 5c/5e.

2. **[`audio_graph/compiler.rs`](../../../src-tauri/src/audio_graph/compiler.rs)**
   - `CompileError::UnsupportedPortKind { from, to, kind }` + `Display`.
   - `validate_port_kinds(graph)` — single O(edges) pass called at the top of
     `compile`, before the topological sort. Accepts `Audio { .. }`, rejects
     everything else.
   - Unit tests: default-audio graph passes; a `Midi` edge and a `Modulation`
     edge are each rejected.

### Why a guard instead of full producer↔consumer validation

Full validation (compare `edge.kind` to the kind each node declares on that
port) needs per-port kind declarations on `AudioNode`, which only land in 5c
when `InstrumentNode` / the wrappers expose real MIDI ports. Until then the
honest minimal check is "only `Audio` is routable" — it cannot misfire today
(nothing produces a typed edge) and it fails loudly the moment a later phase
emits a typed edge before its node-side port exists.

## Real-time impact

| Change                | RT cost                                   | RT risk |
| --------------------- | ----------------------------------------- | ------- |
| `Edge.kind` field     | none — topology type, command thread only | none    |
| `validate_port_kinds` | one O(edges) pass in `compile` (off-RT)   | none    |

C-1 preserved: nothing in `process_block` is touched. The pass runs on the
compiler thread.

## Compatibility

- **C-2 (bit-identique offline).** Preserved — every edge is `Audio`, so the
  DAG, `ExecutionPlan` and render are identical. Full suite green.
- **C-3 (variant-by-variant).** No typed edge is produced yet; 5c/5e opt in via
  `connect_typed`.
- **C-4 (saved projects).** No schema change — `Edge` is not serialised.
- **C-7 (compiler extended, not rewritten).** One validation call added.

## Consequences

- **Positive.** The graph data model is typed; 5c (MIDI→instrument) and 5e
  (modulation edges) have a first-class representation and a single relaxation
  point (`validate_port_kinds` + `connect_typed`).
- **Limits / debt.**
  - `connect_typed` and the `Midi`/`Modulation`/`Control` branches are exercised
    only by tests until 5c/5e.
  - Validation is homogeneity-only (no per-node port-kind table) until 5c.
- **Reversibility.** Additive. Reverting drops the field and the pass; all
  callers used the defaulting `connect`.

## Alternatives considered

- **Separate audio / MIDI sub-graphs.** Rejected (target ADR-008) — duplicates
  the scheduler and breaks the single `ExecutionPlan`.
- **Leave ports untyped, infer MIDI from `NodeKind`.** Rejected — pushes the
  audio/MIDI distinction into ad-hoc compiler special-casing instead of the edge
  itself; sidechain (5d) would remain inexpressible.

## Follow-ups

- **Phase 5c** — `InstrumentNode` and the plugin wrappers declare MIDI ports;
  `connect_typed(.., Midi)` creates note edges; `validate_port_kinds` is
  extended to compare producer/consumer kinds and accept `Midi`.
- **Phase 5e** — `Modulation` edges for Envelope / Macro / Follower.
