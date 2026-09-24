# State Index

The map from subsystem → source-of-truth doc → code → status. **Read this first**
(per `completion_protocol.md`) before changing a subsystem, and **update the
relevant row** after (per `doc_sync_rules.md`).

Status legend: **stable** (production, don't change silently) ·
**in-migration** (actively evolving — check the migration plan) ·
**scaffolded** (types/plumbing exist, runtime not fully wired).

_Last full review: 2026-06-03. Each row's date is when its status was last verified._

| Subsystem | Source of truth (`docs/active/`) | Code | Status | Verified |
|-----------|----------------------------------|------|--------|----------|
| Graph-first model & pipeline | `architecture/audio-graph.md` | `audio_graph/{graph,node,mod}.rs` | stable | 2026-06-03 |
| Graph compiler | `dsp/graph-compiler.md` | `audio_graph/{compiler,execution_plan}.rs` | stable | 2026-06-03 |
| DSP runtime / scheduler | `dsp/dsp-runtime.md` | `audio_graph/{scheduler,worker_pool,buffer}.rs` | stable | 2026-06-05 |
| Immutable snapshot swap | `architecture/snapshot-rebuild-flow.md` | `audio_graph/snapshot*.rs`, `audio_engine/snapshot.rs` | stable | 2026-06-03 |
| Real-time safety | `threading/realtime-safety.md` | (cross-cutting; audio thread) | stable | 2026-06-05 |
| Threading model | `threading/threading-model.md` | `audio_engine/engine.rs`, `engine_gateway/` | stable | 2026-06-03 |
| Transport | `threading/realtime-safety.md` | `audio_engine/transport.rs` | stable — carries time signature; tempo resynced on every recompile | 2026-09-23 |
| Offline render (C-2 harness) | `migration/*` (+ ADR-019 amendment) | `audio_engine/{offline,render_harness}.rs` | stable — harness V1 built: block-size independence + golden hashes (5 reference projects) (Windows x86_64 only; cross-OS open) | 2026-09-23 |
| Modulation | `modulation/modulation-system.md` (+ ADR-019) | `audio_graph/{modulation,parameter_block,live_params,scheduler,node,nodes/*}.rs` | sub-block grid modulation fully implemented (ADR-019 Tranches 1–3): plugins, Gain/TrackGain, and all 4 internal effects; `mod_smooth.rs` now unused; render proven block-size independent (held off-grid value + grid-aware gain smoother, ADR-019 amendment); internal-effect modulated values clamped to per-param ranges; EQ frequencies modulated in octaves via `*_freq_octaves` targets (ADR-019 amendment, harness project `eq_octave_modulation`); LFOs assignable from the UI (track volume, internal effects, plugin effects + instrument) | 2026-09-24 |
| MIDI | `architecture/audio-graph.md` (+ ADR-012, ADR-018) | `audio_graph/midi.rs` | in-migration (Phase 5b; clip-window cull ADR-018) | 2026-06-04 |
| State management / event sourcing | `architecture/state-management.md` | `engine_gateway/{store,commands,events,api}.rs`, `project.rs` | stable (clip `offset`/trim ADR-018) | 2026-06-04 |
| Plugin system core | `plugins/plugin-system.md` | `plugin_system/{host,lifecycle,instance,registry}.rs` | stable | 2026-06-03 |
| Plugin parameters / transport | `plugins/plugin-system.md` (+ ADR-008/009) | `plugin_system/{param_queue,parameter}.rs` | stable | 2026-06-03 |
| Plugin capabilities / PDC | `plugins/plugin-system.md` (+ ADR-006/014) | `plugin_system/capabilities.rs` | partial (sidechain foundation) | 2026-06-03 |
| Plugin state persistence | `plugins/plugin-system.md` (+ ADR-010) | `plugin_system/state.rs` | stable (R-4: restore carried across controlled re-instantiation; missing plugins keep their state on save) | 2026-09-23 |
| Plugin wrappers (VST3/AU/CLAP/internal) | `plugins/plugin-wrapper-system.md` | `plugin_system/wrapper.rs`, `plugin_formats/` | in-migration (VST3/CLAP active; AU partial; CLAP gets host transport, VST3 does not yet; missing plugins reported to UI, missing instrument = silence; disabled plugins stay loaded, ADR-020; CLAP param automation sets / modulation offsets via PARAM_MOD, instruments included, ADR-021; automatable-only CLAP params (e.g. Vital) modulated by value around a base that follows plugin-window drags and is restored before state save, ADR-021 amendment; edits in a CLAP plugin window mark the project unsaved (host clap.state + output events); CLAP instrument now takes the FFI gate) | 2026-09-24 |
| Plugin editor/GUI hosting | `plugins/plugin-wrapper-system.md`, `audits/plugin-gui-audit-2026-05-31.md` | `plugin_system/gui.rs` | in-migration (V4 dual-component done) | 2026-06-04 |
| Plugin sandbox (out-of-process) | `plugins/plugin-wrapper-system.md` (+ ADR-015) | `plugin_system/{bridge,bridged_node}.rs` | in-migration (ADR-015 Phase 6) | 2026-06-03 |

**Notes**
- Statuses reflect what was true at the verified date — confirm against the code
  before relying on a row (especially `in-migration` rows).
- Migration ordering and phase status: `docs/active/migration/migration_plan.md`.
- Outstanding problems by area: `docs/active/migration/critical_issues.md`.
