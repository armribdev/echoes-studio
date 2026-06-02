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
