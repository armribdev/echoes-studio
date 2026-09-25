# ADR-014 — Capability-driven plugin port sizing (sidechain foundation, Phase 5d)

## Status

Accepted (foundation) + **CLAP introspection/feed landed 2026-09-25** + **routing
landed 2026-09-25** (see the updates at the bottom). Implements the
**compiler** half of **Phase 5d** of
[`docs/active/migration/migration_plan.md`](../migration/migration_plan.md) — audit **P0-3**
(capabilities câblées en dur) and **P1-5** (sidechain inexpressible).

Scope: make the graph compiler size plugin input ports from cached plugin
metadata instead of a hardcoded constant, and — as of the updates below — make
that metadata real for CLAP, route the aux signal into the plugin, and connect
a track into the aux port. Still open: a **UI control** for
`sidechain_source` (settable today only via `UPDATE_EFFECT`) and **VST3
`getBusInfo`** introspection.

---

## Context

`GraphCompiler` sized every plugin node at a hardcoded `2` inputs
(`num_inputs_for_kind(NodeKind::Plugin) => 2`), never consulting the plugin.
A sidechain compressor's aux bus therefore had no port in the graph, so ducking
— a basic DAW feature — was inexpressible (audit P0-3 / P1-5).

The constraint is ordering, not RT: port counts are computed in the step-build
loop **before** nodes are instantiated/introspected
([`compiler.rs`](../../../src-tauri/src/audio_graph/compiler.rs)). Rather than
reorder the compile pipeline (C-7 risk) or introspect a live instance early,
the plugin's static bus layout belongs in the **scan-time cache** — the
`PluginRegistry` manifest — which is already available to `compile`
(`plugin_host: &mut PluginHost`). This mirrors how Bitwig/Reaper cache plugin
metadata and keeps the pipeline order untouched.

## Decision

`PluginManifest` gains `num_sidechain_inputs: usize` (count of auxiliary input
ports). The compiler adds it to the base port count for `NodeKind::Plugin`.

### Module map

1. **[`plugin_system/registry.rs`](../../../src-tauri/src/plugin_system/registry.rs)**
   - `PluginManifest.num_sidechain_inputs: usize`; `internal()` sets `0`.

2. **[`plugin_formats/clap/host.rs`](../../../src-tauri/src/plugin_formats/clap/host.rs)
   / [`vst3/host.rs`](../../../src-tauri/src/plugin_formats/vst3/host.rs)**
   - Scan registration sets `num_sidechain_inputs: 0` (descriptor-only scan; a
     lazy `clap.audio-ports` / `getBusInfo` probe populates it later).

3. **[`plugin_system/host.rs`](../../../src-tauri/src/plugin_system/host.rs)**
   - `sidechain_input_ports(device_id) -> usize` — manifest lookup, `0` when
     unknown. Unit-tested (manifest aux → N; unknown / built-in → 0).

4. **[`audio_graph/compiler.rs`](../../../src-tauri/src/audio_graph/compiler.rs)**
   - Step build: `num_inputs = num_inputs_for_kind(kind) + (Plugin →
     plugin_host.sidechain_input_ports(device_id))`. All other kinds unchanged.

## Real-time impact

None — all changes are compile-time (command thread). No `process_block`
change, no allocation/lock added.

## Compatibility

- **C-2 (bit-identique offline).** `num_sidechain_inputs` is `0` for every
  device today (`internal()`, both scanners), so plugin nodes are sized at `2`
  exactly as before; the added port count is `2 + 0`. Full suite green.
- **C-4 (saved projects).** No project-schema change — the field lives in the
  runtime registry, rebuilt at scan.
- **C-7 (compiler extended, not rewritten).** One additive term in the existing
  step-sizing expression; the compile pipeline order is untouched.

## Consequences

- **Positive.** The compiler is now capability-aware for plugin input sizing;
  once a sidechain bus is detected, the aux port is allocated automatically.
  No pipeline reorder, no port-model change.
- **Limits / debt (the rest of P0-3 / P1-5).**
  - ~~**Bus introspection not wired**~~ — done for CLAP (2026-09-25, see
    update below); VST3 `getBusInfo` still caches `0`.
  - ~~**Sidechain not functional yet**~~ — the CLAP wrapper now feeds every
    declared aux bus (2026-09-25).
  - ~~**No routing**~~ — `from_project` now connects a `sidechain_source`
    track into aux port 1 (2026-09-25, see the update below). No UI control
    yet — set via `UPDATE_EFFECT`.
  - **Multi-out** — output sizing still fixed at `2`; symmetrical follow-up via
    `num_audio_outputs`, gated on downstream routing for the extra ports.
- **Reversibility.** Additive. With the field at `0` the behaviour is identical
  to pre-5d.

## Alternatives considered

- **Reorder the compile pipeline** (build/prepare processors before step sizing,
  read introspected ports). Rejected for this step — larger blast radius on the
  scheduler buffer model (the wrapper reports 1 port vs. the compiler's 2) and a
  C-7 risk, for no extra capability the scan cache cannot provide.
- **Probe a live instance during step sizing.** Rejected — no instance exists on
  first compile; probing would double-instantiate.

## Follow-ups

- **Bus introspection** — ~~lazy `clap.audio-ports` / `getBusInfo` probe~~
  **done for CLAP (2026-09-25)**, see the update below. VST3 `getBusInfo` is
  still open.
- **Wrapper aux feed** — ~~route `inputs[1..]` into the plugin's aux audio
  bus.~~ **done for CLAP (2026-09-25)**, see the update below.
- **Routing** — ~~connect an aux/return track to the sidechain port in
  `from_project`~~ **done (2026-09-25)**, see the update below. Deliberately
  *not* the generic return-bus/aux-send model critical issues §2.4 describes
  (no `ReturnNode` wiring) — a plugin names one source track directly, which
  covers the standard "duck compressor" case without that larger data model.
- **UI control** — `sidechain_source` is settable via `UPDATE_EFFECT` but has
  no picker in `EffectChain.svelte` yet. Still open.
- **Multi-out** — size outputs from the manifest once downstream routing reads
  the extra ports. Still open.

## Update — 2026-09-25: real CLAP bus introspection + wrapper feed

The two items above that only needed the compiler-side plumbing this ADR
already landed are now real, CLAP only (VST3 `getBusInfo` untouched):

- **Scanner** (`plugin_formats/clap/host.rs::register_clap`). For every
  non-instrument plugin found, a throwaway instance is created, `init`'d,
  queried via `clap.audio-ports` (`count`/`get` on input ports, aux = any port
  without `CLAP_AUDIO_PORT_IS_MAIN`) and destroyed, all inside one
  [`crash_guard`](../../../src-tauri/src/plugin_formats/crash_guard.rs)-guarded
  call so a plugin that crashes during this probe can't take the scanner down
  with it (falls back to `0`, same as "no `clap.audio-ports` support"). This
  is a one-time, off-RT scan-time cost, not a hot path. Instruments are not
  probed (no meaningful aux *input*, and probing would trigger the same
  load-on-create cost a real instantiation does).
- **Port base fixed.** `num_inputs_for_kind(NodeKind::Plugin)` was `2`
  ("default stereo in") even though both wrappers' real `num_input_ports()`
  return `1` and only ever read `inputs[0]` — a always-empty, always-silent
  phantom second port existed for every plugin node before this pass (VST3
  included, since the two formats share this match arm). Changed to `1`; the
  aux term from this ADR now sits at ports `1..` instead of stacking on top of
  the phantom slot. Behaviour-identical for every device with `0` aux inputs
  (the removed slot was never connected or read) — `renders_match_golden_hashes`
  stayed green (C-2).
- **`ClapWrapperNode`** (`plugin_formats/clap/wrapper.rs`) takes `num_aux_inputs`
  at construction (from `PluginHost::sidechain_input_ports`, both the fresh-
  instantiate and the ADR-016 reuse path) and now builds one `ClapAudioBuffer`
  per input port — port 0 from `inputs[0]` (main), ports `1..` from
  `inputs[1..]` (aux) — instead of a single hardcoded buffer. `num_input_ports()`
  returns the real total. Every port is nc-channel like the rest of the graph's
  buffers (no per-port channel model exists anywhere else in the graph either).
- **Not done** (at the time of this update): routing, and VST3 `getBusInfo`
  (still open). Until routing existed, an aux port was real and fed every
  block, but always with the scheduler's shared silence buffer — structurally
  correct, silently inert.

Real-FFI proof (both against `crates/test_clap_plugin`, extended to declare
one aux input bus): `plugin_formats/clap/host.rs::audio_ports_probe_tests`
(scanner reads the real count) and
`plugin_formats/clap/wrapper.rs::aux_feed_tests` (the aux buffer's samples
reach the plugin's `process` call verbatim, proven by summing it into the
output).

## Update — 2026-09-25: sidechain routing (a track into the aux port)

The remaining open item — connecting a track's audio into the aux port this
ADR already sizes and feeds — is now real, scoped to a single named source
track per plugin rather than the full return-bus/aux-send model critical
issues §2.4 describes:

- **Project schema.** `PluginParams.sidechain_source: Option<String>`
  (`project.rs`) names the track feeding this plugin's aux port 1. `None` for
  every project saved before this field existed (C-4); no other schema
  change, so `renders_match_golden_hashes` (C-2) stayed green untouched.
- **Command.** `UPDATE_EFFECT` gained `sidechain_source: Option<Option<String>>`
  (the same present-vs-absent-vs-null idiom as `UpdateModulatorPayload.source_effect_id`)
  — absent leaves it unchanged, `null` clears it. `patch_effect`
  (`engine_gateway/api.rs`) rejects a track sidechaining from itself outright
  (`Err`, no partial state) and marks the edit structural (recompile) when the
  value actually changes; replay goes through the existing `EFFECT_UPDATED`
  event, which already carries the whole serialized `PluginParams`, so no new
  apply/replay path was needed.
- **Graph wiring.** `AudioGraph::from_project` (`audio_graph/graph.rs`) records
  each track's post-fader tap (its `TrackGain` node — chosen so muting or
  lowering the source track also quiets the duck it triggers) in a first pass,
  then resolves every plugin's `sidechain_source` into a
  `connect_typed(tap, 0, plugin_node, 1, PortKind::Audio{..})` edge in a
  second pass, once every tap exists (a source track can be listed after the
  plugin sidechaining from it). A self-reference or a source that would close
  a track-level cycle (via chained `sidechain_source` references) is dropped
  silently rather than failing the whole compile — an unmet routing intent
  stays inert, the same fallback as a plugin with no declared aux bus. A
  dangling source track id (never existed, or was deleted) is dropped the same
  way. Port 1 only: a plugin with more than one declared aux bus gets its
  first bus fed; further buses are unaddressed by this single-source field.
- **Not done**: a UI control (`sidechain_source` is only settable via
  `UPDATE_EFFECT` today) and VST3 `getBusInfo`.

Tests: `audio_graph/graph.rs::sidechain_tests` — edge lands on port 1 from the
source's `TrackGain` node; `None` adds no edge (C-4); self-reference dropped;
dangling source dropped; a two-track cycle keeps the first accepted edge and
drops the second. No end-to-end "load Pro-C 2, hear ducking" manual pass yet —
the unit tests cover topology, not perceptual audio.
