# ADR-006 — Plugin Capabilities Types + Latency Reporting (Phase 1)

## Status

Accepted. Implements Phase 1 of [`docs/active/migration/migration_plan.md`](../migration/migration_plan.md).
Decisions 1 and 2 superseded 2026-09-29 — see the amendment at the end.

---

## Context

The plugin-system audit ([`../audits/plugin-system-audit-2026-05-31.md`](../audits/plugin-system-audit-2026-05-31.md))
documented six P0 issues. Two of them — **P0-3 (capabilities hardcoded to
2/2)** and **P0-6 (`latency_samples()` always 0)** — are blockers that the
later phases cannot proceed without:

- P0-3 demands a stable type vocabulary for everything the graph compiler
  must learn from a plugin (bus layout, MIDI ports, parameter set, GUI /
  state support, instrument flag). Without it, Phases 2-5 ship behind a
  moving target.
- P0-6 means every linear-phase EQ, convolution reverb, and lookahead
  compressor silently misaligns its track. The PDC walk in the graph
  compiler is already correct ([`compiler.rs:255-293`](../../src-tauri/src/audio_graph/compiler.rs#L255-L293))
  but the wrappers feed it zero.

A third issue — **P0-5 short-term containment** — is reachable in the same
phase and would otherwise leave a known unwind path uncaught:

- A Rust-side panic raised from inside an `AudioNode::process` call
  currently unwinds the audio callback (or a pool worker). That kills
  CPAL or poisons the worker pool and crashes the engine.

A type-level addition — `PortKind` — is also brought forward because
Phase 5 ratifies it in ADR-008. Declaring the enum now (without enforcing
it in the compiler) fixes the variant set and lets later phases extend
behaviour without an exhaustiveness churn across the codebase. See
target architecture [`docs/active/migration/target_architecture.md`](../migration/target_architecture.md)
BP-2.

## Decision

Phase 1 lands four additive, behaviour-preserving changes:

1. **`plugin_system::capabilities` module.** New file
   [`plugin_system/capabilities.rs`](../../../src-tauri/src/plugin_system/capabilities.rs)
   defining `DeviceCapabilities`, `AudioBusInfo`, `MidiPortInfo`,
   `ParameterInfo`, `ParameterFlags`, `NativeParamId`, `SpeakerLayout`.
   No call site reads these yet; Phase 2+ wrappers populate them at
   `prepare()` time via VST3 `IComponent::getBusInfo` /
   `IEditController::getParameterInfo` and CLAP `clap.audio-ports` /
   `clap.params`.

2. **`Device::capabilities()` trait method with safe default.** The
   default impl derives a `DeviceCapabilities` from the legacy `DeviceInfo`
   fields (single main bus per direction, optional MIDI ports, no
   parameters). Existing `Device` implementors compile unchanged.

3. **Latency introspection in both wrappers.**
   - VST3 ([`plugin_formats/vst3/wrapper.rs`](../../../src-tauri/src/plugin_formats/vst3/wrapper.rs)):
     reads `IAudioProcessor::getLatencySamples` once after
     `setupProcessing`, caches into `reported_latency: u32`, overrides
     `AudioNode::latency_samples()` to return the cached value.
   - CLAP ([`plugin_formats/clap/wrapper.rs`](../../../src-tauri/src/plugin_formats/clap/wrapper.rs)):
     queries the `clap.latency` extension after `activate` (spec: main
     thread only, valid only post-activation), caches into the same
     `reported_latency: u32`, same override.
   - The graph compiler picks the value up at compile time
     (`step_latencies[i] = processors[i].latency_samples()`,
     [`compiler.rs:268`](../../../src-tauri/src/audio_graph/compiler.rs#L268)),
     so PDC alignment becomes correct for any plugin that reports a
     non-zero latency. **No code on the audio thread changes.**

4. **`catch_unwind` around `AudioNode::process` in the scheduler.** The
   per-step dispatch in [`scheduler.rs::process_one_step`](../../../src-tauri/src/audio_graph/scheduler.rs)
   now wraps the call in `std::panic::catch_unwind(AssertUnwindSafe(…))`.
   On panic the output buffers are re-zeroed defensively (they were already
   pre-zeroed before the call, see [`scheduler.rs:583-585`](../../../src-tauri/src/audio_graph/scheduler.rs#L583-L585)).
   The processor is left live for the next block — proper Dead-marking and
   process isolation are Phase 6 work (ADR-010, bridge IPC).

5. **`PortKind` enum declared in `audio_graph::node`.** Variants `Audio
{ channels }`, `Midi`, `Modulation { polarity }`, `Control`. **Not** read
   by the compiler. Phase 5 (ADR-008) adds a `kind` field on `Edge`,
   defaults legacy connections to `Audio`, and switches the compiler to
   typed validation.

## Real-time impact

| Change                              | RT cost                           | RT risk                            |
| ----------------------------------- | --------------------------------- | ---------------------------------- |
| `DeviceCapabilities` types          | none                              | none — types only                  |
| `Device::capabilities()` default    | none                              | called only off-RT (compile / UI)  |
| VST3 `getLatencySamples` at prepare | none on hot path                  | off-RT call                        |
| CLAP `clap.latency.get` at prepare  | none on hot path                  | off-RT call                        |
| `latency_samples()` override        | none on hot path                  | plain field read, off-RT consumer  |
| `catch_unwind` around process       | ~ns / node / block (setjmp frame) | no allocation, no syscall, no lock |
| `PortKind` enum                     | none                              | type-level only                    |

All RT constraints from `../threading/realtime-safety.md` and target C-1
hold. No mutex, no allocation, no UI call introduced on the audio path.

## Compatibility

- **C-2 (bit-identique offline)** — preserved for projects that load no
  external plugin, or only plugins reporting zero latency. For plugins that
  report a non-zero latency, the PDC now aligns the track grid (this is the
  correctness fix). The CI bit-identique baseline will need to be
  re-snapped once after the merge; cf. migration plan §1.8.
- **C-3 (variant-by-variant migration)** — `Device::capabilities()` has a
  default, so each wrapper opts in independently.
- **C-4 (saved projects)** — no project schema change. No serialised field
  added.
- **C-5 (lock-free cmd ↔ audio)** — capabilities are read off-RT;
  latency cache is a plain `u32` field set during `prepare`. No new
  channel, no new lock.

## Consequences

- **Positive.** Linear-phase EQs, lookahead compressors, convolution
  reverbs now declare their latency and PDC aligns them correctly. Rust
  panics no longer crash the engine. Phase 2 can target a stable
  `DeviceCapabilities` type. Phase 5 inherits the `PortKind` enum without
  re-ratifying the variant set.
- **Limits.** `catch_unwind` does **not** capture C++ structured
  exceptions (Windows SEH `STATUS_ACCESS_VIOLATION`, divide-by-zero); a
  misbehaving plugin's native crash still aborts the process. ADR-010
  (Phase 6) addresses this with an out-of-process bridge.
- **Debt added.** `DeviceCapabilities` is populated only with the legacy
  shape + the freshly-introspected latency; Phase 2 fills the rest.
  `Device::set_parameter_normalised` still writes to a JSON blob never
  read by the audio thread — explicitly Phase 3 work (ADR-007).
- **Reversibility.** All changes are local and additive. Each can be
  reverted independently by a focused PR.

## Known risk — surfaced 2026-06-03 (architecture review), not yet closed

The `catch_unwind` guard is weaker than the "RT cost ~ns, no risk" table row
implies. Three caveats are tracked debt, to be addressed at the next contact with
the scheduler / plugin process path:

1. **Conditional on `panic = "unwind"`.** The guard is a **no-op** under
   `panic = "abort"`. The build currently uses the default (unwind) so the guard
   is live, but nothing pins it — see the build-profile note in
   `architecture-decision-rules.md`. Do not switch to `panic = "abort"` without
   first replacing this guard (ADR-015 bridge).
2. **Corrupted DSP state after a caught panic.** Re-zeroing the output buffers
   does **not** reset the node's internal state. A node that panicked mid-`process`
   (half-updated filter coefficients, partial ring write) is left live; the next
   block may emit NaN/instability. Proper recovery is `Dead`-marking the node
   (deferred to Phase 6 / ADR-015), not "leave it live".
3. **Cost not measured.** The per-node, per-block landing-pad cost and its effect
   on dispatch inlining were asserted, not benchmarked. Measure before relying on
   the "~ns" figure on the hot path.

These do not change the Phase 1 decision; they record that the guard is a
_containment stopgap_, and the real isolation is ADR-015.

## Alternatives considered

- **Bundle Phase 2 (`Arc<PluginInstance>`) into Phase 1.** Rejected:
  Phase 2 is a large structural refactor (target BP-1) and would invalidate
  the bit-identique baseline. Phase 1 must stay small and safely
  ratifiable.
- **Replace `catch_unwind` with per-step `Dead` flag + skip.** Rejected
  for Phase 1: extends scope. The migration plan calls for the minimal
  containment; Phase 6 brings proper Dead-marking via the bridge.
- **Wire `DeviceCapabilities` into the graph compiler immediately.**
  Rejected: depends on `PluginInstance` being shared (Phase 2) so the host
  can hand capabilities to the compiler. Trying to do it now would
  duplicate the host-side state and create a divergence path.

## Follow-ups (Phase 2+)

- Capabilities introspected from VST3 buses and CLAP audio ports
  (Phase 5d).
- Parameter enumeration via `IEditController` / `clap.params`
  (Phase 2, ADR-007).
- `Arc<PluginInstance>` shared between host, audio node, and editor
  (Phase 2, ADR-006).
- Handle VST3 `restart(kLatencyChanged)` and `clap_host_latency.changed`
  notifications to re-trigger PDC recompile when latency changes at
  runtime (Phase 6 ramp-up).
  **Update 2026-09-25 — CLAP done** (in-process and sandboxed): the plugin's
  `request_restart` triggers a recompile that reactivates the reused instance
  and re-reads `clap.latency` (see ADR-016 update and
  `plugins/plugin-wrapper-system.md` § Latency Handling). VST3 still open.
- SEH / structured-exception capture and out-of-process sandbox
  (Phase 6, ADR-010).

## Amendment 2026-09-29 — capabilities types removed

Decisions 1 and 2 never got a consumer. Capability data reached the compiler by
another route instead (ADR-014): the scan-time `clap.audio-ports` probe fills
`PluginManifest.num_sidechain_inputs`, and multi-out reads the loaded
instance's output ports (`plugin_formats/clap/audio_ports.rs`). The
`DeviceCapabilities` vocabulary and the `Device` trait carrying
`capabilities()` stayed dead code alongside that working path.

Removed: `plugin_system/capabilities.rs`, the `Device` trait and `DeviceInfo`
(`plugin_system/wrapper.rs`), the `ParameterMap` / `Param` / `ParameterRange`
model (`plugin_system/parameter.rs`, replaced in practice by `PluginParamInfo`,
ADR-021), and the per-wrapper `Device` impls. Decisions 3–5 (latency via
`AudioNode::latency_samples()`, `catch_unwind`, `PortKind`) are unchanged.

A future capability source (VST3 `getBusInfo`, AU, WASM) extends the
`PluginManifest` / `PluginInstance` route rather than reviving a parallel type.
No runtime behaviour changed; the C-2 harness (golden hashes) stays green.
