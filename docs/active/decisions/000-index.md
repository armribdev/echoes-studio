# Architecture Decision Records — Index

Central index of all ADRs for Echoes Studio. Each ADR records a significant
architectural decision with its context, decision, alternatives rejected, and
consequences.

**Governance:** how and when to create or amend an ADR is defined in
`architecture-decision-rules.md`. The "never change silently" constraints table
lives there.

---

## How to use this index

- New decision affecting DSP graph, threading, plugin lifecycle, routing,
  modulation, scheduling, or sandboxing → create the next-numbered ADR and add a
  row here in the same change.
- Amending an existing decision → update its ADR (note the date + what changed)
  and, if the theme changed, update its row here.

---

## All ADRs

| # | Title | Theme |
|---|-------|-------|
| [001](001-graph-first-architecture.md) | Graph-First Architecture | Graph / DSP core |
| [002](002-event-sourcing-scope.md) | Event Sourcing Scope | State management |
| [003](003-plugin-wrapper-strategy.md) | Plugin Wrapper Strategy | Plugins |
| [004](004-immutable-runtime-snapshots.md) | Immutable Runtime Snapshots | Threading / RT safety |
| [005](005-ui-engine-separation.md) | UI / Engine Separation | Architecture boundary |
| [006](006-plugin-capabilities-and-pdc.md) | Plugin Capabilities + Latency Reporting (PDC) | Plugins |
| [007](007-plugin-instance-arc.md) | Shared `Arc<PluginInstance>` | Plugins / threading |
| [008](008-param-transport-spsc.md) | Plugin parameter transport via SPSC `ParamChange` queue | Plugins / RT safety |
| [009](009-parameter-block.md) | `ParameterBlock` for internal-effect hot params | Modulation / params |
| [010](010-plugin-state-persistence.md) | Plugin state persistence via native FFI | Plugins / persistence |
| [011](011-portkind-typed-edges.md) | Typed `PortKind` edges, default `Audio` | Graph / routing |
| [012](012-midi-snapshot-beats.md) | `MidiNoteSnapshot` in beats, per-block tempo resolution | Graph / MIDI |
| [013](013-clap-plugin-instruments.md) | CLAP plugin instruments via `InstrumentDevice` adapter | Plugins |
| [014](014-capability-driven-port-sizing.md) | Capability-driven plugin port sizing (sidechain foundation) | Plugins / routing |
| [015](015-plugin-sandbox.md) | Out-of-process plugin sandbox, opt-in per device | Plugins / sandboxing |
| [016](016-plugin-instance-reuse.md) | Plugin instance reuse across recompiles | Plugins / RT safety |
| [017](017-per-instance-plugin-addressing.md) | Per-instance plugin addressing & state persistence | Plugins / state / UI boundary |
| [018](018-clip-source-offset-trim.md) | Clip source offset & trim (audio + MIDI window) | State / MIDI scheduling |
| [019](019-sub-block-modulation.md) | Sub-block deterministic modulation & automation (absolute control grid; amended 2026-09-24: internal-effect bounds, EQ frequencies in octaves) | Modulation / scheduling / determinism |
| [020](020-disabled-plugins-stay-loaded.md) | Disabled plugin effects stay loaded (`BypassNode`) | Plugins / graph / persistence |
| [021](021-plugin-parameter-modulation-and-automation.md) | Plugin parameters: automation sets, modulation offsets (amended 2026-09-24: automatable-only CLAP params modulated by value) | Plugins / modulation |

---

## By theme

- **Graph / DSP core:** 001, 011, 012, 018, 020
- **Threading / RT safety:** 004, 007, 008, 016
- **State / persistence:** 002, 010, 017, 018, 020
- **Architecture boundary:** 005, 017
- **Plugins:** 003, 006, 007, 008, 010, 013, 014, 015, 016, 017, 020, 021
- **Modulation / parameters:** 009, 019, 021
- **Scheduling / determinism:** 019

> Note: several ADRs span more than one theme; listed under each that applies.

---

## Related

- `architecture-decision-rules.md` — ADR governance + hard constraints
- `../README.md` — documentation routing entry point
