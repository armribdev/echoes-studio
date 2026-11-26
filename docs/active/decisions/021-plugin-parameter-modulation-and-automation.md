# ADR-021 — Plugin parameters: automation sets, modulation offsets

## Status

**Accepted — engine side implemented 2026-09-24** (approved by the user after
the choice was explained in plain terms). The UI to pick a plugin parameter as
a modulation target exists since 2026-09-24 (`ModulationTargetPicker.svelte`). Extends ADR-007/008 (parameter transport), ADR-013
(CLAP instruments) and ADR-019 (sub-block grid).

**Amendment 2026-09-24 — value modulation for automatable-only CLAP
parameters** (approved by the user). Vital declares none of its 903 parameters
`CLAP_PARAM_IS_MODULATABLE`, so no LFO could reach it. For a CLAP parameter that
is automatable but not modulatable, the host now **moves the parameter**: each
grid point sends `PARAM_VALUE = base + offset` (normalised, clamped to 0…1).
This deliberately breaks "modulation never moves the parameter" for those
parameters only; modulatable parameters keep `PARAM_MOD`.

- **Base.** `ValueModulatedParam.base` (`audio_graph/plugin_params.rs`, the
  `LiveParams` shadow `Arc`). Seeded from `clap.params.get_value` when the
  parameter first becomes modulated (under the FFI gate; if the gate is busy
  the previous shadow value is kept).
- **User edits.** The plugin's output events reach `observe_output_event`
  (`plugin_formats/clap/params.rs`). Between `PARAM_GESTURE_BEGIN` and
  `PARAM_GESTURE_END` the modulation pauses and the dragged `PARAM_VALUE`
  becomes the new base. Values outside a gesture are ignored, so a plugin that
  echoes host values cannot make the base drift.
- **Route removed.** One `PARAM_VALUE` of the base, sent once.
- **State save.** `PluginInstance::save_state_blob` flushes value-modulated
  parameters to their base (`clap.params.flush`) before `clap.state.save`, so the
  saved preset holds the user's setting. The next block resumes the modulation.
- **Automation wins** over value modulation on the same parameter (as VST3).
- **RT.** Output-event callback: linear search + atomic store, no allocation.
  Released list drained with `Vec::drain` (no deallocation). The per-instance
  list is a command-thread `Mutex`, never touched by the audio thread.
- **Known limits.** `clap.params.flush` is called from the command thread while
  the plugin is active; the CLAP spec asks for the audio thread in that case.
  The FFI gate guarantees it never runs concurrently with `process` (verified
  with Vital). A preset change made inside the plugin window does not update
  the base (no gesture), so the next block puts the parameter back on it.
  VST3 and the out-of-process bridge are unchanged.

## Context

The engine could already route modulation and automation to a plugin
parameter (`ModulationTargetRef::PluginParam`), but:

1. **Automation was ignored.** Wrappers only held entries for parameters
   targeted by a _modulation route_; an automation lane on a plugin parameter
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

| Change                                  | RT cost                                     | RT risk                          |
| --------------------------------------- | ------------------------------------------- | -------------------------------- |
| Separate automation/modulation dispatch | one branch per action                       | none                             |
| Controls storage                        | pre-allocated at compile                    | none                             |
| Event sort                              | `sort_unstable` on ≤ 1280 indices, in place | none — no alloc                  |
| Released-modulation resets              | `Vec::drain` of a compile-time list, once   | none — no dealloc                |
| `replace_modulated_params`              | `Mutex` on the command thread only          | none — never on the audio thread |

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
  - The out-of-process bridge (`plugin_host_bridge`) is not covered. _(CLAP
    since ADR-015; VST3 since the amendment of 2026-11-04 below.)_
  - An automated parameter is not pushed back to the UI, and edits made in the
    plugin window during playback are not reported to Echoes (plugin output
    events are still discarded). _2026-11-26: a drag in a CLAP plugin window can now be
    written into a lane, [ADR-037](037-live-automation-writing.md); an automated
    parameter is still not pushed back to the UI._

## Alternatives considered

- **Keep `base + offset` for CLAP too, reading the base from the plugin.**
  Rejected: CLAP defines non-destructive modulation for exactly this, and the
  host would still overwrite edits made in the plugin window.
- **Make automation absolute for internal effects as well.** Deferred: it would
  change existing renders and needs its own decision. _2026-09-29: decided in
  [ADR-025](025-absolute-automation-internal-targets.md) (accepted)._

## Amendment 2026-11-04 — VST3 parameters in the sandbox (ADR-033 step 6)

The rules above hold unchanged for a VST3 plugin that runs in the bridge; only where
the logic lives is new.

- **The engine keeps the bases, the bridge computes nothing.** The engine sends the
  _effective_ normalised value of each point (queued edit, automation, or the base
  plus the modulation offset, clamped to `0..1`, automation first) as a `PARAM_VALUE`
  record with its sample offset. `PARAM_MODULATION` is never sent to a VST3 plugin and
  the VST3 adapter ignores it. The logic is `emit_control_changes`
  (`plugin_formats/vst3/param_delivery.rs`), the same function as in process, behind a
  `Vst3ParamSink`.
- **Edits come back.** What the user does in the plugin window reaches the bridge's
  `IComponentHandler`; the bridge forwards it to its processor and reports it in the
  block reply (`PARAM_VALUE`, `PARAM_GESTURE_BEGIN` / `_END`). The engine moves the base
  of a modulated parameter on an edit and pauses its modulation while the gesture
  lasts (`Vst3ParamEvents::observe_edit`). The engine hears of a gesture one block
  after it started; the edit is last in the bridge's block, so the user's value wins
  meanwhile.
- **The base before a state save.** `save_state_blob` sends `FlushValues` with the bases
  (normalised); the bridge puts them in the plugin with a zero-sample `process` and
  applies them again inside the `SaveState` it receives next, so no block can land in
  between. The bases are kept in `PluginInstance::value_modulated_params` with the
  range `0..1`.
- Units: VST3 values are normalised end to end, CLAP values are plain. The two never
  share a record type or an event path; the handshake declares the unit
  (`ParamUnit::Normalised`) and the engine refuses another one.
- Debt closed: the VST3 bridge is covered; the window's edits are reported to Echoes
  through `BLOCK_PLUGIN_EDITED`. What a processor writes to `outputParameterChanges` is
  not an edit and stays for its controller, as in process.
