# ADR-014 — Capability-driven plugin port sizing (sidechain foundation, Phase 5d)

## Status

Accepted (foundation) + **CLAP introspection/feed landed 2026-09-25** (see the
update at the bottom). Implements the **compiler** half of **Phase 5d** of
[`docs/active/migration/migration_plan.md`](../migration/migration_plan.md) — audit **P0-3**
(capabilities câblées en dur) and **P1-5** (sidechain inexpressible).

Scope: make the graph compiler size plugin input ports from cached plugin
metadata instead of a hardcoded constant, and — as of the update below — make
that metadata real for CLAP and route the aux signal into the plugin. Still
open: **UI routing** of an aux/return track into the port (no ducking is
audible yet — the port exists and is fed, but only ever with silence) and
**VST3 `getBusInfo`** introspection.

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
    declared aux bus (2026-09-25); still gated on the routing item below to be
    audible.
  - **No routing** — `from_project` does not yet connect an aux track to the
    sidechain port (needs project/graph wiring + PortKind routing). This is
    now the *only* thing standing between "the aux port exists and is fed"
    and "a user can hear ducking".
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
- **Routing** — connect an aux/return track to the sidechain port in
  `from_project` (with `PortKind::Audio` aux edges). Still open — this needs a
  return-bus/aux-send data model the project doesn't have yet (cf. critical
  issues §2.4, no `ReturnNode` wiring); until it lands, every aux port a
  plugin declares is real and processed but always reads silence.
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
- **Not done**: routing (above) and VST3 `getBusInfo`. Until routing exists, an
  aux port is real and gets fed to the plugin every block, but always with the
  scheduler's shared silence buffer — structurally correct, silently inert.

Real-FFI proof (both against `crates/test_clap_plugin`, extended to declare
one aux input bus): `plugin_formats/clap/host.rs::audio_ports_probe_tests`
(scanner reads the real count) and
`plugin_formats/clap/wrapper.rs::aux_feed_tests` (the aux buffer's samples
reach the plugin's `process` call verbatim, proven by summing it into the
output).
