# Completion Protocol

The standard loop for any change to engine behaviour, architecture, or a
documented contract. (Trivial, non-architectural edits — typos, comments,
formatting — skip this.)

## Before modifying

1. Read `state_index.md` — find the subsystem, its source-of-truth doc, and status.
2. Read the relevant **ADRs** (`decisions/000-index.md`) and the **active doc(s)**
   for that subsystem.
3. Read the **scoped `CLAUDE.md`** in the directory you're editing, if present.
4. Confirm the change does not break a hard invariant (`architecture_rules.md`).
   If it does → stop and raise it with the user (ADR required).

## While modifying

5. Obey `rt_audio_rules.md` / `threading_rules.md` for anything on the RT path.
6. Preserve migration constraints, especially **C-2 bit-identical offline**
   (`migration_rules.md`).

## After modifying

7. Run the sync procedure (`doc_sync_rules.md`): update impacted docs via
   `change_impact_mapping.md`, reconcile ADRs, update `state_index.md`, archive
   stale docs.
8. **Summarise the architecture impact** in your final message: what invariant
   area was touched, which docs/ADRs you updated, and any divergence you could
   not resolve.
9. If the change introduced a new decision, ensure its ADR + index row exist.

> This protocol is referenced from the root `CLAUDE.md`. It is the contract that
> keeps docs and code from drifting over time.
