---
name: doc-sync
description: Keep docs in sync with code after an engine/architecture change. Use after modifying engine behaviour or a documented contract, or when you notice docs and code disagree.
---

# Doc Sync

## Objective

Leave the documentation consistent with the code in the same change — no silent
drift, no information lost.

## Workflow

1. List the files you changed. Look each up in
   `.claude/rules/change_impact_mapping.md` to get the impacted docs + ADRs.
2. Update the impacted `docs/active/` docs to match new behaviour.
3. ADR coherence: amend the affected ADR (date + what changed), or create the
   next ADR + add a row to `docs/active/decisions/000-index.md` for a new decision.
4. Update the affected row(s) in `.claude/state_index.md` (status + verified date).
5. Archive anything now obsolete into `docs/archive/` with a banner pointing to
   the replacement — **never delete**.
6. If a doc and the code disagree and you can't reconcile now, state the
   divergence explicitly in your summary.

## Checklist

- [ ] Every impacted active doc updated.
- [ ] ADRs reconciled / new ADR + index row added if a decision changed.
- [ ] `state_index.md` updated.
- [ ] Obsolete docs archived with banner, not deleted.
- [ ] Root `CLAUDE.md` still compact and correct.
- [ ] No duplication introduced (rules/scoped CLAUDE.md stay pointers).

## Anti-patterns

- "I'll update docs later" — later never comes; do it in the same change.
- Editing an archived doc instead of its canonical replacement.
- Copying detail from `docs/active/` into a rule or scoped `CLAUDE.md`.

## Validation (done when)

- A reader of the docs would correctly predict what the code now does, and the
  state_index reflects reality.
