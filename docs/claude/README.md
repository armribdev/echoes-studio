# Echoes Studio — Architecture Reference

This directory contains the long-term architectural memory for Echoes Studio.
It is written for Claude Code sessions, not end users.

**Purpose:** preserve architectural consistency, prevent regressions, and guide
future implementations without repeating analysis that's already been done.

---

## Directory Layout

```
docs/claude/
├── README.md                      ← you are here
│
├── architecture/                  ← what the system is and why
│   ├── audio-graph.md             ← graph-first DSP philosophy
│   ├── plugin-system.md           ← plugin abstraction model
│   ├── threading-model.md         ← thread roles + synchronization boundaries
│   └── state-management.md        ← event sourcing, undo/redo, derived state
│
├── rules/                         ← hard constraints, no exceptions
│   ├── architecture-decision-rules.md  ← governance: when/how to create ADRs
│   ├── realtime-safety-rules.md   ← audio thread prohibitions
│   ├── anti-patterns.md           ← architectural mistakes + why they are dangerous
│   └── modulation-rules.md        ← parameter modulation ownership
│
├── systems/                       ← deep dives into specific subsystems
│   ├── graph-compiler.md          ← compilation pipeline + execution plan
│   ├── plugin-wrapper-system.md   ← VST3/AU wrapper design
│   └── dsp-runtime.md             ← scheduler, buffers, execution model
│
└── decisions/                     ← ADR-style architecture decision records
    ├── 001-graph-first-architecture.md
    ├── 002-event-sourcing-scope.md
    ├── 003-plugin-wrapper-strategy.md
    ├── 004-immutable-runtime-snapshots.md
    └── 005-ui-engine-separation.md
```

Legacy flat files (pre-reorganisation) remain in `docs/claude/` as detailed
implementation references. The new hierarchy above is the canonical entry point.

---

## Legacy Files (detailed implementation references)

| File | Content |
|------|---------|
| `audio-graph-architecture.md` | Full DSP pipeline with buffer safety model |
| `plugin-system-architecture.md` | Device trait, PluginHost, parameter system |
| `graph-compiler-design.md` | Kahn's sort, execution steps, node factory |
| `snapshot-rebuild-flow.md` | Command → compile → enqueue → swap sequence |
| `realtime-safety-rules.md` | Thread model + approved inter-thread mechanisms |
| `dsp-runtime-rules.md` | The five prohibitions + checklist |
| `modulation-system-design.md` | ModulationRoute, ModulationMap, execution |
| `vst-wrapper-rules.md` | Buffer translation, parameter mirroring, transport |

---

## Quick Reference: What Governs What

| Concern | Go to |
|---------|-------|
| Can I allocate in process()? | `rules/realtime-safety-rules.md` |
| Should plugins know about tracks? | `architecture/audio-graph.md` |
| How does undo/redo work? | `architecture/state-management.md` |
| What threads exist and what can they do? | `architecture/threading-model.md` |
| Why is the graph compiled ahead of time? | `decisions/001-graph-first-architecture.md` |
| Why is event sourcing limited? | `decisions/002-event-sourcing-scope.md` |
| How does VST3 connect to the graph? | `systems/plugin-wrapper-system.md` |
| What are the scheduler execution rules? | `systems/dsp-runtime.md` |
| What mistakes must I avoid? | `rules/anti-patterns.md` |
| Can I change the threading model? | `rules/architecture-decision-rules.md` |
