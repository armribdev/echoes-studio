# Migration History

Append-only log of what has been migrated and which decision realized it. This
is *history* (what happened), not spec. The forward plan is
`docs/active/migration/migration_plan.md`; rationale is in each ADR.

## Planning baseline (2026-05)
- Audits produced: audio-engine (2026-05-29), plugin-system + plugin-GUI
  (2026-05-31) → `docs/active/audits/`.
- Migration framing established: `current_engine_diagram.md` (now archived),
  `critical_issues.md`, `target_architecture.md` (C-1…C-10), `migration_plan.md`.

## Phases delivered
- **Phase 1 — Plugin capabilities + latency reporting** (ADR-006). Capability
  types + PDC foundation; default impl derived from `DeviceInfo` for backward
  compat (C-2/C-4).
- **Phase 2a/2b — Unified `Arc<PluginInstance>`** (ADR-007). Single instance per
  device + lifecycle management (collapses the audio/GUI split direction).
- **Phase 3 — Parameter transport + hot params** (ADR-008 SPSC `ParamChange`
  queue, ADR-009 `ParameterBlock`). Internal effects migrated to `ParameterBlock`.
- **Phase 4 — Plugin state persistence via native FFI** (ADR-010): VST3 first,
  then CLAP.
- **Phase 5 — Typed edges + MIDI + ports** (ADR-011 `PortKind` default `Audio`,
  ADR-012 `MidiNoteSnapshot` in beats, ADR-013 CLAP instruments via
  `InstrumentDevice`, ADR-014 capability-driven port sizing / sidechain
  foundation). Defaults chosen to keep legacy projects bit-identical.
- **Phase 6 — Out-of-process plugin sandbox** (ADR-015), opt-in per device (C-9).
  Sub-phases: 6b core, 6c shared-memory transport + host/shared-lib, 6d
  shared-memory mapping + control handshake + ring-driven audio loop + real CLAP
  in the bridge + RT audio processing, 6e watchdog for plugin timeouts.

## Documentation governance overhaul (2026-06-03)
- Phase 1: restructured docs into `docs/active/` + `docs/archive/`, merged the
  diverged flat/hierarchy doc pairs, added the ADR index.
- Phase 2: added `.claude/rules/` (pointer-based law) + `state_index.md` + sync
  machinery.
- Phase 3: scoped `CLAUDE.md` per engine subdir, `.claude/skills/`, this memory.

## Later work (2026-09)
Sub-block modulation (ADR-019), plugin param modulation + automation engine
(ADR-021), live MIDI hardware input (ADR-022), library browser (ADR-023),
progressive clip PCM (ADR-024), CLAP sidechain / multi-out, sandboxed CLAP
instruments. Docs pass 2026-09-29: June audits archived, `critical_issues.md`
got a verified status table.

## Still open
See the status section of `migration_plan.md` and the table atop
`critical_issues.md` (UI for automation, MIDI recording, VST3 buses/latency/
sandbox, CI, phase 7 cleanup).
