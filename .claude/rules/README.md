# .claude/rules — operating rules for Claude Code

Short, strict, non-duplicating rules. They are the "law"; the detailed
explanations live in `docs/active/`. These files are referenced from the root
`CLAUDE.md`.

| File                       | Governs                                                                 |
| -------------------------- | ----------------------------------------------------------------------- |
| `rt_audio_rules.md`        | Audio-thread prohibitions (allocation, locks, I/O, …)                   |
| `threading_rules.md`       | Thread ownership + approved cross-thread mechanisms                     |
| `architecture_rules.md`    | Hard architectural invariants + when an ADR is required                 |
| `migration_rules.md`       | The C-1…C-10 migration constraints (no rewrite, C-2 gate)               |
| `documentation_rules.md`   | How docs are structured and kept canonical                              |
| `doc_sync_rules.md`        | Keeping docs in sync with code after every change                       |
| `change_impact_mapping.md` | Code area → docs/ADRs to update (sync lookup table)                     |
| `completion_protocol.md`   | Before/while/after-change checklist                                     |
| `rust_self_rules.md`       | When `Self { … }` is safe vs must use an explicit type (safe refactors) |

Companion index (not a rule): `../state_index.md` — subsystem → source-of-truth →
code → status.

**Principle:** these files must never restate what a `docs/active/` doc already
says — they point to it. If a rule and a doc disagree, the doc is the detail and
must be reconciled (see `doc_sync_rules.md`).
