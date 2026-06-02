# ADR-014 — Capability-driven plugin port sizing (sidechain foundation, Phase 5d)

## Status

Accepted (foundation). Implements the **compiler** half of **Phase 5d** of
[`docs/active/migration/migration_plan.md`](../migration/migration_plan.md) — audit **P0-3**
(capabilities câblées en dur) and **P1-5** (sidechain inexpressible).

Scope: make the graph compiler size plugin input ports from cached plugin
metadata instead of a hardcoded constant. **Functional sidechain** (the wrapper
feeding the aux bus + UI routing of an aux track into it) and **real bus
introspection** (the scanner currently caches `0`) are explicit follow-ups.

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
  - **Bus introspection not wired** — the scanner caches `0`; a lazy
    `clap.audio-ports` / VST3 `getBusInfo` probe must populate
    `num_sidechain_inputs`. Unverifiable without a real plugin (smoke test).
  - **Sidechain not functional yet** — the wrapper still reads `inputs[0]`
    only; it must feed the aux audio buffer into the plugin's aux bus.
  - **No routing** — `from_project` does not yet connect an aux track to the
    sidechain port (needs project/graph wiring + PortKind routing).
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

- **Bus introspection** — lazy `clap.audio-ports` / `getBusInfo` probe →
  populate `num_sidechain_inputs` (+ main/out bus channel counts).
- **Wrapper aux feed** — route `inputs[1..]` into the plugin's aux audio bus.
- **Routing** — connect an aux/return track to the sidechain port in
  `from_project` (with `PortKind::Audio` aux edges).
- **Multi-out** — size outputs from the manifest once downstream routing reads
  the extra ports.
