# ADR-021 — Plugin parameters: automation sets, modulation offsets

## Status

**Accepted — engine side implemented 2026-09-24** (approved by the user after
the choice was explained in plain terms). The UI to pick a plugin parameter as
a target is the next step. Extends ADR-007/008 (parameter transport), ADR-013
(CLAP instruments) and ADR-019 (sub-block grid).

## Context

The engine could already route modulation and automation to a plugin
parameter (`ModulationTargetRef::PluginParam`), but:

1. **Automation was ignored.** Wrappers only held entries for parameters
   targeted by a *modulation route*; an automation lane on a plugin parameter
   found no entry and was dropped.
2. **Modulation ignored the knob position.** Wrappers sent `shadow + delta`,
   where the shadow defaulted to 0.5 and never followed edits made in the
   plugin's own window. A modulated filter jumped to mid-range.
3. **Instruments** drained and discarded every parameter change.
4. **CLAP effect events were not time-sorted** across parameters, which the
   CLAP spec requires.

Nothing exposed this, because the UI cannot target a plugin parameter yet.

## Decision

- **Automation sets the value.** It is the absolute normalised value of the
  parameter; the plugin's own knob follows it.
- **Modulation offsets the value.** It moves the sound around the parameter's
  current value without changing that value, so the user can still turn the
  knob while modulated (Bitwig model).

### Engine mechanics

- `AudioNode` gains defaulted `apply_automation` / `apply_automation_at`. The
  scheduler calls them for automation actions and the modulation methods for
  routes. The defaults forward to the modulation methods, so internal nodes are
  untouched (their additive automation semantic and the render harness golden
  hashes are unchanged).
- `audio_graph/plugin_params.rs` (format-agnostic): `PluginParamControl` holds,
  per targeted native parameter, separate pre-allocated modulation and
  automation points for the block, the reported range, and a `modulated` flag.
  The compiler injects one set per plugin node — effects and instruments — at
  every compile, built from modulation routes **and** automation lanes
  (`inject_plugin_param_controls`).
- `PluginParam { effect_id: "instrument:{track_id}" }` targets the track's
  plugin instrument (`graph::instrument_instance_key`). `InstrumentNode`
  forwards to `InstrumentDevice::param_controls_mut`.
- **CLAP** (`plugin_formats/clap/params.rs`, shared by effect and instrument):
  modulation → `CLAP_EVENT_PARAM_MOD` (plain-unit amount), sent only to
  parameters flagged `CLAP_PARAM_IS_MODULATABLE`; automation and queued edits →
  `CLAP_EVENT_PARAM_VALUE`. When a recompile drops a modulation route, the
  wrapper sends one `PARAM_MOD` of 0 (`PluginInstance::replace_modulated_params`
  tracks which parameters are modulated across wrapper rebuilds). Notes and
  parameter events share one fixed-size, time-sorted list (`ClapEventBuffer`).
- **VST3** has no host modulation: modulation stays `base + offset` (base =
  the `LiveParams` shadow, still 0.5 by default — see Limits); automation is
  sent as the absolute value and, on the same parameter, wins over modulation.
- `PluginHost::plugin_params` / the `get_plugin_params` command list a
  placement's parameters (`PluginParamInfo`: name, group, plain range, default,
  automatable, modulatable, hidden) for the upcoming UI. CLAP only for now.

## Real-time impact

| Change | RT cost | RT risk |
|---|---|---|
| Separate automation/modulation dispatch | one branch per action | none |
| Controls storage | pre-allocated at compile | none |
| Event sort | `sort_unstable` on ≤ 1280 indices, in place | none — no alloc |
| Released-modulation resets | `Vec::drain` of a compile-time list, once | none — no dealloc |
| `replace_modulated_params` | `Mutex` on the command thread only | none — never on the audio thread |

## Compatibility

- **C-2.** Projects without plugin automation/modulation render as before; the
  harness golden hashes are unchanged. Plugin modulation/automation changes
  behaviour (it was broken), which third-party plugins make non-deterministic
  anyway (ADR-013 known risk #2).
- **C-4.** No schema change: `PluginParam` and `plugin_param` targets already
  existed.

## Consequences

- **Positive.** Automation and modulation of plugin parameters work for CLAP
  effects and CLAP instruments, with standard DAW semantics; the shared layer is
  ready for a VST3 translation.
- **Limits / debt.**
  - VST3 modulation still uses a 0.5 default base that does not follow the
    plugin's own edits. Needs base tracking through `IComponentHandler`.
  - CLAP ranges are read at `prepare`; if the FFI gate is busy then, a
    parameter is treated as unknown (no modulation) until the next compile.
  - The out-of-process bridge (`plugin_host_bridge`) is not covered.
  - An automated parameter is not pushed back to the UI, and edits made in the
    plugin window during playback are not reported to Echoes (plugin output
    events are still discarded).

## Alternatives considered

- **Keep `base + offset` for CLAP too, reading the base from the plugin.**
  Rejected: CLAP defines non-destructive modulation for exactly this, and the
  host would still overwrite edits made in the plugin window.
- **Make automation absolute for internal effects as well.** Deferred: it would
  change existing renders and needs its own decision.
