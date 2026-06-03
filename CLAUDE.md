# Claude Code — Echoes Studio

Echoes Studio is a real-time DAW (Tauri + Svelte frontend, Rust audio engine).
**Real-time safety and architectural consistency take priority over short-term
convenience.** The Rust backend owns all project/engine state; the frontend is a
reactive mirror.

The long-term architectural memory lives in [`docs/active/`](docs/active/).
Start at [`docs/active/README.md`](docs/active/README.md), then read the relevant
doc **before** changing that subsystem.

## Routing

| Area | Read first |
|------|-----------|
| Audio graph / DSP core | [`docs/active/architecture/audio-graph.md`](docs/active/architecture/audio-graph.md), [`docs/active/dsp/`](docs/active/dsp/) |
| Real-time safety (audio thread) | [`docs/active/threading/realtime-safety.md`](docs/active/threading/realtime-safety.md) |
| Threading model | [`docs/active/threading/threading-model.md`](docs/active/threading/threading-model.md) |
| Plugin system / hosting | [`docs/active/plugins/`](docs/active/plugins/) |
| Modulation | [`docs/active/modulation/modulation-system.md`](docs/active/modulation/modulation-system.md) |
| State / undo-redo | [`docs/active/architecture/state-management.md`](docs/active/architecture/state-management.md) |
| Migration (current→target) | [`docs/active/migration/`](docs/active/migration/) |
| Architecture decisions | [`docs/active/decisions/000-index.md`](docs/active/decisions/000-index.md) |

## Operating rules (`.claude/`)

Short strict rules live in [`.claude/rules/`](.claude/rules/) and point into
`docs/active/` for detail. Load them when relevant:

- [`rt_audio_rules.md`](.claude/rules/rt_audio_rules.md) — audio-thread law
- [`threading_rules.md`](.claude/rules/threading_rules.md) — thread ownership
- [`architecture_rules.md`](.claude/rules/architecture_rules.md) — hard invariants + when an ADR is required
- [`migration_rules.md`](.claude/rules/migration_rules.md) — C-1…C-10 (no rewrite, C-2 gate)
- [`documentation_rules.md`](.claude/rules/documentation_rules.md) · [`doc_sync_rules.md`](.claude/rules/doc_sync_rules.md) · [`change_impact_mapping.md`](.claude/rules/change_impact_mapping.md)

**Workflow:** before/after any architectural or engine change, follow
[`completion_protocol.md`](.claude/rules/completion_protocol.md) — read
[`state_index.md`](.claude/state_index.md) + relevant ADRs first, then sync the
impacted docs (`doc_sync_rules.md`) after.

## Non-negotiable rules

- **Audio callback:** no allocation, no locks, no I/O, no `drop`, no Tauri emit.
  See [`realtime-safety.md`](docs/active/threading/realtime-safety.md).
- **No silent architectural drift.** Changing a hard constraint (RT safety,
  threading ownership, graph execution model, plugin boundary, state boundaries)
  requires an ADR and user approval — see
  [`decisions/architecture-decision-rules.md`](docs/active/decisions/architecture-decision-rules.md).
- **Never delete docs — archive them.** Superseded docs go to
  [`docs/archive/`](docs/archive/) with a pointer to the canonical replacement.

## Source of truth

- Engine/DSP behaviour → the code under `src-tauri/src/` + the `docs/active/` doc
  for that subsystem (keep both in sync; if they disagree, surface it).
- Decisions already made → [`docs/active/decisions/`](docs/active/decisions/).
- General project docs (build, tech stack, UX) → [`docs/`](docs/) root.

## Code style rules

### Core principle
Write simple, readable and maintainable code. Prioritize clarity over cleverness.

### Comments rules
- Avoid obvious comments.
- Comments must explain intent or non-obvious reasoning only.
- Do NOT describe what the code already shows.
- Remove commented-out code instead of keeping it.
- Prefer self-explanatory code over comments.
- Keep comments short and rare.

### Naming
- Use clear and descriptive names.
- Avoid abbreviations.
- Prefer consistency across the project.

### Functions
- Small and focused (one responsibility).
- No flag arguments.
- Prefer composition over complexity.

### General rules
- Keep it simple.
- Avoid unnecessary abstractions.
- Follow existing project conventions.
- Reduce complexity whenever possible.

### Important
If unsure, prefer simpler code over more “clever” design.


### Strict comment policy
- Prefer zero comments when code is self-explanatory.
- Only add comments for non-obvious business logic or constraints.
- Never explain obvious code.
- If a comment is required, keep it under 1 sentence.