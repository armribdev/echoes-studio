# ADR-009 — `ParameterBlock` for internal-effect hot params (Phase 3)

## Status

Accepted. Implements the **ADR-009 ParameterBlock** half of **Phase 3** of
[`docs/migration_plan.md`](../../migration_plan.md) (the `ADR-009` of
[`docs/target_architecture.md`](../../target_architecture.md) §9).

Companion: [`008-param-transport-spsc.md`](008-param-transport-spsc.md) (the
external-plugin equivalent). This ADR covers the **internal** effects.

Migrated: **EQ, Compressor, Reverb, Delay** — the four internal effects, per
migration_plan §3.4 ("Eq d'abord, puis Compressor, Reverb, Delay"). EQ landed
first; Compressor / Reverb / Delay then reused the same infrastructure verbatim
and **dropped their legacy recompile fallback** (the `UPDATE_EFFECT` match over
`EffectParams` is now exhaustive for hot delivery — only structural changes such
as the enable/disable toggle recompile). No internal effect recompiles on a
parameter-only edit anymore.

---

## Context

Audit / target ([`target_architecture.md`](../../target_architecture.md) §4.1,
critical_issues 1.3/1.4/4.6): `UPDATE_EFFECT` calls `recompile_if_needed` on
**every** edit. For a knob sweep on an EQ that means a full graph rebuild each
tick — re-`prepare` on every node, reverb comb / delay ring reallocation,
mid-stream DSP reset, and a 50–200 ms command-thread stall on large projects.
That is the wrong shape for a continuous, modulatable parameter.

## Decision

Each internal effect node registers, at compile time, an `Arc<ParameterBlock>`
of atomic cells (one per hot parameter) in `LiveParams`. The node reads the
cells per block; the gateway writes them on a knob turn. **Recompile is reserved
for structural changes** (add/remove effect, change effect type, enable/disable
toggle, topology, tempo).

### Module map

1. **New
   [`audio_graph/parameter_block.rs`](../../../src-tauri/src/audio_graph/parameter_block.rs)**
   - `ParameterBlock { cells: Vec<AtomicU32> }`. `new(values)` (off-RT alloc),
     `get(idx)` (audio thread, one `Relaxed` load), `set(idx, value)` (command
     thread, one `Relaxed` store). Out-of-range access is silent (returns
     `0.0` / no-op) so a layout mismatch can never abort `process`.
   - Cells hold **plain** values (Hz, dB, Q) bit-cast through `f32::to_bits` —
     internal effects own their ranges, so no denormalise on the hot path
     (contrast ADR-008, where plugin values travel normalised).

2. **[`audio_graph/live_params.rs`](../../../src-tauri/src/audio_graph/live_params.rs)**
   - `LiveParamsInner.effect_params: HashMap<String, Arc<ParameterBlock>>`
     keyed by project `effect_id`.
   - `get_or_init_effect_block(effect_id, values)` — compiler inserts/re-seeds.
   - `effect_block(effect_id)` / `set_effect_param` / `write_effect_block` —
     hot-path writers (return `false` when no block exists → caller recompiles).
   - `prune_to_project` drops blocks for effects no longer in the project.

3. **[`audio_graph/graph.rs`](../../../src-tauri/src/audio_graph/graph.rs)**
   - The four insert-effect `NodeKind` variants carry `effect_id: String`
     (only `Eq` consumes a block today; the others carry it so they migrate
     without re-touching the enum). `from_project` fills it from `effect.id`.

4. **[`audio_graph/nodes/eq.rs`](../../../src-tauri/src/audio_graph/nodes/eq.rs)**
   - Cell-order constants (`EQ_LOW_GAIN_DB` … `EQ_HIGH_FREQ`, `EQ_PARAM_COUNT`)
     + `eq_block_values(&EqParams)` — the single source of truth for the layout,
     shared by compiler, gateway and node.
   - `EqNode` gains `block: Option<Arc<ParameterBlock>>`, `sr_os`, and
     `with_block(block)`. `process` reads the block, compares the flattened
     values to the cached params, and recomputes the three biquads **only on
     change** — preserving filter state (`s1`/`s2`) so a parameter move does not
     reset the running filters.

5. **[`audio_graph/compiler.rs`](../../../src-tauri/src/audio_graph/compiler.rs)**
   - `build_node(NodeKind::Eq { effect_id, params })` seeds the block from the
     project values via `get_or_init_effect_block` and hands the node a clone.

6. **[`engine_gateway/api.rs`](../../../src-tauri/src/engine_gateway/api.rs)**
   - `UPDATE_EFFECT` dual-path (local `HotEffectUpdate`): an enabled EQ
     parameter-only edit calls `write_effect_block` and **skips recompile**; a
     toggle (structural) or an effect with no registered block recompiles
     (T4 fallback).

## Real-time impact

| Change | RT cost | RT risk |
|---|---|---|
| `ParameterBlock::get` ×7 in `EqNode::process` | 7 `Relaxed` atomic loads | none |
| Change-detection compare | 7-element `[f32;7]` equality | negligible |
| Biquad recompute on change | 3 biquads (trig) — only when a value moved | bounded; one-off per edit |
| `write_effect_block` | off-RT (command thread) | none |
| `effect_params` map | inserted at compile (off-RT); audio side holds an `Arc<ParameterBlock>` clone captured at compile | none — no map lookup at runtime (BP-8) |

C-1 preserved: no allocation, no lock in `process`. `Relaxed` ordering is
sound — each cell is independent; a torn read across two cells resolves on the
next block, identical to a one-block-late automation value.

## Compatibility

- **C-2 (bit-identique offline).** Preserved. Offline export issues no
  `UPDATE_EFFECT`, so the EQ reads its seeded values and computes identically.
  When `block` is `None` (not yet wired) the node keeps the pre-ADR coefficient
  freeze exactly.
- **C-3 (variant-by-variant).** Migrated one variant at a time (EQ, then
  Compressor → Reverb → Delay); each landed independently against the same
  infrastructure. All four internal effects are now hot.
- **C-4 (saved projects).** No schema change — `EffectParams` unchanged. The
  block is a runtime mirror seeded from the project (the source of truth); the
  project is still updated on every edit for persistence/undo, only the
  recompile is skipped.
- **C-5 / C-7.** Lock-free `Arc<ParameterBlock>` clone captured at compile; the
  compiler is extended (one `get_or_init` call), not rewritten.

## Consequences

- **Positive.** EQ knob edits no longer recompile → no edit-time xrun, no UI
  stall (quick-win "Eq freq sans recompile"). Establishes the
  `parameter_block` + `LiveParams.effect_params` infrastructure the remaining
  internal effects reuse verbatim.
- **Limits / debt.**
  - Abrupt coefficient jumps on a large parameter step can zipper (BP-7). The
    biquad state is preserved across recompute, which smooths most musical
    moves; explicit per-parameter smoothing is a later refinement.
  - The block re-seeds from the project on each structural recompile, so the
    block and project cannot drift.
  - `effect_id` rides on all four effect `NodeKind` variants and all four now
    read a block — the asymmetry noted for the EQ-only step is closed.
  - Reverb `dry` and delay `dry` ride in the block for a complete round-trip but
    remain unused by the DSP (`dry = 1.0 - wet`, unchanged); delay `ping_pong`
    is encoded as a 0.0/1.0 cell.
- **Reversibility.** Additive. Dropping `with_block` (or never registering the
  block) returns the node to the legacy frozen-coefficient behaviour; the
  gateway's `applied_hot` simply stays `false` and recompiles.

## Alternatives considered

- **Diff-compile (recompile only touched nodes).** Rejected (target ADR-009):
  invalidates the atomic snapshot, complicates the compiler, marginal gain vs
  a `ParameterBlock`.
- **Per-effect typed structs instead of a generic cell bank.** Rejected — the
  generic `Vec<AtomicU32>` serves every effect; the index→meaning map lives in
  the node (`eq::EQ_*`), keeping the container layout-agnostic for the
  variant-by-variant rollout.
- **Recompute biquads every block unconditionally.** Rejected — the
  change-detection compare avoids per-block trig in the common (static) case
  for free.

## Follow-ups

- **Phase 3 (next variants)** — ✅ done: Compressor → Reverb → Delay migrated to
  their own `ParameterBlock`s on the same infrastructure; their
  `recompile_if_needed` fallback for parameter edits is removed.
- **Phase 5e** — automation lanes write into these same cells block-rate
  (`AutomationEvaluator`), sharing the cell with UI and modulation.
