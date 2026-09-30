# Project Memory — Echoes Studio

Gotchas, limitations, and history for the engine. This is **memory**, not
architecture truth: it records what's surprising or what happened, and points to
the canonical docs for the rest. Do **not** put specs or architecture decisions
here — those live in `docs/active/` and `docs/active/decisions/`.

> Note: this is the project-level, repo-checked memory. It is distinct from any
> per-user auto-memory. Verify a named file/flag still exists before relying on it.

## Files

- [`engine_quirks.md`](engine_quirks.md) — non-obvious behaviours that trip you up
  (silent XRuns, `catch_unwind` vs SEH, `UPDATE_TEMPO` no-recompile, two plugin
  instances, CLAP/VST3 traps, modulation limits).
- [`recurring_problems.md`](recurring_problems.md) — mistake classes + how to
  avoid them (alloc on audio path, plugin init order, recompile traps, C-2
  breakage, format leaks, doc drift, false crash isolation).
- [`migration_history.md`](migration_history.md) — append-only log of delivered
  phases (ADR-006…015) and the docs governance overhaul.

## Where the truth lives (not here)

- Architecture + subsystems → `docs/active/`
- Decisions → `docs/active/decisions/` (`000-index.md`)
- Rules / protocols → `.claude/rules/`
- Subsystem status map → `.claude/state_index.md`
- Problem catalogue → `docs/active/migration/critical_issues.md`
