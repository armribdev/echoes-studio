---
name: safe-refactor
description: Refactor engine code without changing behaviour or breaking architectural invariants. Use when restructuring audio_graph, plugin_system, or engine_gateway code where determinism, RT safety, and the bit-identical render must be preserved.
---

# Safe Refactor

## Objective
Change structure, not behaviour. Preserve every hard invariant and keep the
offline render bit-identical.

## Workflow
1. Run the "before" steps of `.claude/rules/completion_protocol.md`: read
   `state_index.md`, the relevant ADRs, and the active doc(s) for the subsystem.
2. Identify the invariants in scope (`.claude/rules/architecture_rules.md`):
   graph-first, immutable snapshots, plugin boundary, single instance, UI/engine
   separation, determinism.
3. Make the smallest change that achieves the goal. Prefer extending over
   rewriting (migration C-7 for the compiler).
4. If the code touches the audio path, also run the **rt-validation** skill.
5. Run the "after" steps: sync docs via `change_impact_mapping.md`, update
   `state_index.md`.

## Checklist
- [ ] No public contract / `NodeKind` / serialized shape changed silently.
- [ ] Topological-sort tiebreak (`NodeId`) and `src_step < step` invariant intact.
- [ ] No format type leaked from `plugin_formats` into `audio_graph`.
- [ ] Offline render still bit-identical (C-2).
- [ ] Tests for the touched area pass.

## Anti-patterns
- "While I'm here" scope creep that mixes refactor with behaviour change.
- Rewriting the graph compiler instead of extending it.
- Renaming/moving a documented type without updating its active doc + ADR.

## Validation (done when)
- Behaviour is provably unchanged (tests + bit-identical render), invariants
  hold, and docs/state_index reflect the new structure.
