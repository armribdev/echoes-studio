---
name: architecture-migration
description: Execute a step of the incremental DAW migration (current -> target) without a rewrite. Use when implementing a phase from docs/active/migration/migration_plan.md or any change that advances the target architecture.
---

# Architecture Migration

## Objective
Advance one migration step while honouring the C-1…C-10 constraints and keeping
every shipped step independently correct.

## Workflow
1. Locate the step in `docs/active/migration/migration_plan.md`; read its phase
   notes and the ADR it realizes. Check `state_index.md` for current status.
2. Confirm the constraints in `.claude/rules/migration_rules.md` apply (esp.
   **C-2 bit-identical**, **C-7 extend-not-rewrite**, **C-4 backward compat**,
   **C-9 sandbox opt-in**).
3. Ensure the C-2 offline harness covers the affected area **before** changing
   it; extend `audio_engine/render_harness.rs` first if not.
4. Implement the smallest increment. New routing (MIDI/modulation/ports) defaults
   to a no-op shape so legacy projects stay bit-identical (ADR-011/012).
5. If the step introduces a new decision, write the next ADR and add it to
   `decisions/000-index.md`.
6. Run **safe-refactor** + **rt-validation** as applicable, then **doc-sync**.

## Checklist
- [ ] Step maps to a migration_plan phase + an ADR.
- [ ] Legacy projects load and behave unchanged (C-4).
- [ ] Offline render bit-identical, or divergence is intentional + tested (C-2/C-10).
- [ ] No new mutex on the RT path (C-1); cmd↔audio stays lock-free (C-5).
- [ ] Sandbox/opt-in features are off by default (C-9).
- [ ] `state_index.md` row updated (status / verified date).

## Anti-patterns
- Big-bang replacement of a subsystem instead of variant-by-variant (C-3).
- Gating determinism behind a feature flag (violates C-10).
- Shipping a phase that breaks an earlier one.

## Validation (done when)
- The phase is implemented, its ADR exists, harness is green, and docs +
  state_index are synced.
