# Echoes Studio — Active Architecture Reference

This directory (`docs/active/`) is the long-term architectural memory for Echoes
Studio. It is written for Claude Code sessions, not end users.

**Purpose:** preserve architectural consistency, prevent regressions, and guide
future implementations without repeating analysis that's already been done.

Obsolete material is never deleted — it is moved to `docs/archive/` with a
banner pointing at its canonical replacement.

---

## Directory Layout

```
docs/active/
├── README.md                ← you are here (routing entry point)
│
├── architecture/            ← what the system is and why
│   ├── audio-graph.md       ← graph-first philosophy + pipeline + buffer safety
│   ├── state-management.md  ← event sourcing, undo/redo, derived state
│   ├── snapshot-rebuild-flow.md ← command → compile → enqueue → swap sequence
│   └── anti-patterns.md     ← architectural mistakes + why they are dangerous
│
├── dsp/                     ← DSP graph execution
│   ├── graph-compiler.md    ← compilation pipeline + execution plan
│   └── dsp-runtime.md       ← scheduler, buffers, execution model
│
├── threading/              ← thread roles + real-time safety
│   ├── threading-model.md   ← thread ownership + synchronization boundaries
│   └── realtime-safety.md   ← audio-thread prohibitions (authoritative)
│
├── plugins/                ← plugin abstraction + hosting
│   ├── plugin-system.md     ← Device/PluginHost model, parameter system
│   └── plugin-wrapper-system.md ← VST3/AU/CLAP wrapper rules
│
├── modulation/             ← parameter modulation
│   └── modulation-system.md ← ownership rules, data types, execution
│
├── migration/              ← current → target evolution (no rewrite)
│   ├── critical_issues.md   ← catalogued problems of the current system
│   ├── target_architecture.md ← realistic target + migration strategy
│   └── migration_plan.md    ← incremental plan + current status and next steps
│
├── audits/                 ← point-in-time engine/plugin audits (+ FFI lifecycle validation)
│
└── decisions/              ← ADR-style architecture decision records
    ├── 000-index.md         ← ADR index (start here)
    ├── architecture-decision-rules.md ← ADR governance + hard constraints
    └── 001..024-*.md        ← individual ADRs
```

Sibling trees: `docs/archive/` (superseded docs, including the June 2026
« reality audit » series) and `.claude/` (`rules/`, `skills/`, `state_index.md`).

General project docs live at the `docs/` root: `ARCHITECTURE.md`,
`AUDIO_ENGINE.md`, `BUILD.md`, `TECH_STACK.md`, `UI_GUIDELINES.md`,
`library-browser-ux.md`, `library-browser-roadmap.md`.

---

## Quick Reference: What Governs What

| Concern                                           | Go to                                                                                                         |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| Can I allocate in `process()`?                    | `threading/realtime-safety.md`                                                                                |
| What threads exist and what can they do?          | `threading/threading-model.md`                                                                                |
| Should plugins know about tracks?                 | `architecture/audio-graph.md`                                                                                 |
| Why is the graph compiled ahead of time?          | `dsp/graph-compiler.md` + `decisions/001-graph-first-architecture.md`                                         |
| How does the scheduler run a block?               | `dsp/dsp-runtime.md`                                                                                          |
| How does a project change reach the audio thread? | `architecture/snapshot-rebuild-flow.md`                                                                       |
| How does undo/redo work?                          | `architecture/state-management.md`                                                                            |
| How does VST3/AU/CLAP connect to the graph?       | `plugins/plugin-wrapper-system.md`                                                                            |
| Who owns plugin parameters and modulation?        | `plugins/plugin-system.md`, `modulation/modulation-system.md`                                                 |
| What mistakes must I avoid?                       | `architecture/anti-patterns.md`                                                                               |
| Can I change the threading/graph/plugin model?    | `decisions/architecture-decision-rules.md`                                                                    |
| What decisions have already been made?            | `decisions/000-index.md`                                                                                      |
| What is done and what comes next?                 | `migration/migration_plan.md` (status section), `../library-browser-roadmap.md`                               |
| What is the target architecture?                  | `migration/target_architecture.md` (pre-migration structure archived: `../archive/current_engine_diagram.md`) |
| Library / browser UX (panel, pop-up, drops)?      | `../library-browser-ux.md` + `decisions/023-library-browser-ux.md`                                            |

---

## Conventions

- **Canonical vs archived:** every active doc is the single source of truth for
  its topic. Superseded versions live in `docs/archive/` and are marked
  do-not-edit.
- **ADRs:** any significant decision about the DSP graph, threading, plugin
  lifecycle, routing, modulation, scheduling, or sandboxing gets an ADR. See
  `decisions/architecture-decision-rules.md`.
- **No silent drift:** changing a hard constraint requires an ADR and user
  approval (see the constraints table in `decisions/architecture-decision-rules.md`).
