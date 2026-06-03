# ADR-016 — Plugin instance reuse across recompiles

## Status

**Accepted — implemented 2026-06-03 for CLAP + internal formats.** VST3 reuse is
deferred (gated out of the reuse path — see "Scope of edit" and Follow-ups): its
`set_active` / `set_processing` lifecycle still lives in the wrapper `prepare` and
must move to the instance before VST3 can be reused safely. Fixes **point 1** of
[`audits/engine-architecture-audit-2026-06-03.md`](../audits/engine-architecture-audit-2026-06-03.md).
Builds on the per-instance records keying landed in the
[ADR-007 update (2026-06-03)](007-plugin-instance-arc.md#update--2026-06-03-records-keyed-by-instance_key).

---

## Context

`GraphCompiler::compile` → `build_node` → `PluginHost::instantiate`
([`compiler.rs`](../../../src-tauri/src/audio_graph/compiler.rs),
[`host.rs`](../../../src-tauri/src/plugin_system/host.rs)) created a **brand-new**
`PluginInstance` for **every** plugin node on **every** structural recompile:
load/locate the native handle, `activate`, re-query latency, re-apply state.

Consequences (audit point 1):

- **Plugin internal DSP state is destroyed** on every structural edit (reverb /
  delay tails, compressor envelopes, analyzer history) — only the FFI-serialised
  state is replayed, and only on project load, not on recompile.
- **CPU spike on the command thread** (handle setup + `activate`) while the
  `project → engine` locks are held → UI micro-freeze.
- **PDC recompute** from a re-queried latency → potential click / xrun.

The records map is now keyed by a stable, per-placement `instance_key`
(ADR-007 update), which is the **prerequisite** for reuse: reuse keyed by
`device_id` would be unsound the moment a plugin type is duplicated (two graph
nodes would share one handle, processed twice per block).

### Why reuse is not a 5-line change: the displaced-wrapper hazard

Reuse means the new `CompiledGraph` builds a fresh wrapper around
`Arc::clone(existing.instance)` while the **old** wrapper (in the retiring graph)
still exists until the drop ring frees it. The CLAP wrapper used to own the
`start_processing` / `stop_processing` + `activate` / `deactivate` calls, gated by
its per-`prepare` `active: bool` flag, and ran `stop_processing` + `deactivate` in
its `Drop`. With a shared, reused instance:

- the displaced wrapper's `Drop` would call `stop_processing` + `deactivate` on a
  plugin the **new** graph is actively processing → corruption / crash;
- a fresh wrapper's `prepare()` would call `activate` on an already-active
  instance → CLAP/VST3 spec violation (`activate` requires a deactivated plugin).

So reuse requires moving the **activation lifecycle** (`activate`/`deactivate`,
`start`/`stop_processing`) out of the per-wrapper `Drop`/`prepare` and onto the
**shared `PluginInstance`**, owned by the host.

---

## Decision

On compile, **reuse** the existing live `Arc<PluginInstance>` for a plugin node
instead of rebuilding it, when **all** hold (`PluginHost::reuse_existing`):

1. no plugin-state blob is staged (project load wants a fresh instance to apply
   the blob at instantiation — ADR-010 race-free restore);
2. a record exists under the node's `instance_key`, and its `device_id` (type)
   matches the requested node (the slot still hosts the same plugin);
3. the instance is not in a terminal lifecycle state (`Destroyed`/`Error`);
4. the format is **reuse-safe** — Internal or CLAP today (VST3/AU/WASM fall
   through to the current full re-instantiate path).

When reusing, build a fresh wrapper around `Arc::clone(existing.instance)` — **no
new native handle** — so the plugin's internal DSP state is preserved.

### Activation lifecycle moved to the instance (CLAP)

`PluginInstance` now owns CLAP activation
([`instance.rs`](../../../src-tauri/src/plugin_system/instance.rs)):

- `clap_ensure_active(sample_rate, max_block)` — idempotent: no-op when already
  active at the same config; `stop_processing + deactivate` then reactivate on a
  config change; `init` (once) + `activate` + `start_processing` otherwise. Runs
  on the command thread during `prepare`.
- `PluginInstance::Drop` performs `stop_processing + deactivate` (if active)
  before `destroy`.
- The CLAP **wrapper** (`ClapWrapperNode`) and **instrument** (`ClapInstrument`)
  `prepare` now call `clap_ensure_active`; their `Drop` impls were removed — a
  displaced wrapper no longer touches the shared plugin.

Only one graph is live at a time (atomic snapshot swap), so the old and new
wrappers never call `process` on the shared instance concurrently; the displaced
wrapper's drop does no FFI at all.

### Scope of edit

- [`plugin_system/instance.rs`](../../../src-tauri/src/plugin_system/instance.rs)
  — `ClapActivation` state + `clap_ensure_active` + CLAP `Drop` teardown.
- [`plugin_formats/clap/wrapper.rs`](../../../src-tauri/src/plugin_formats/clap/wrapper.rs),
  [`audio_graph/nodes/clap_instrument.rs`](../../../src-tauri/src/audio_graph/nodes/clap_instrument.rs)
  — `prepare` delegates activation to the instance; no `Drop` FFI.
- [`plugin_system/host.rs`](../../../src-tauri/src/plugin_system/host.rs) —
  `reuse_existing` short-circuit in `instantiate` / `instantiate_instrument`.
- The compiler call site is unchanged (`instantiate` still returns
  `Box<dyn AudioNode>`, C-7).

Migrated **variant by variant** (C-3): CLAP + internal now; VST3 deferred.

---

## Real-time impact

| Aspect | Effect |
|---|---|
| `process` hot path | unchanged — same FFI dispatch through `Arc::deref` |
| Per-recompile FFI churn | **removed** for CLAP — strictly improves command-thread cost, removes a xrun source |
| Determinism (offline C-2) | unaffected — the offline harness does a single compile; reuse only changes *live* recompiles |
| Live behaviour | plugin internal DSP state now **persists** across structural edits (the intended fix) |

No allocation, lock, or syscall added to the audio thread. Activation calls run
on the command thread (`prepare`) / drop-ring worker (teardown).

---

## Compatibility (migration constraints)

- **C-2 (bit-identical offline).** Preserved — single-compile render path
  unchanged. Reuse is a live-recompile optimisation only.
- **C-3 (variant-by-variant).** CLAP + internal migrated; VST3 deferred.
- **C-4 (saved projects).** No file-format change.
- **C-6 (one live instance per device).** Reinforced — reuse guarantees the same
  shared `Arc<PluginInstance>` survives recompiles instead of being rebuilt.
- **C-7 (compiler extended, not rewritten).** The reuse decision lives in the
  host; the compiler call site is unchanged.

---

## Alternatives considered

- **Re-instantiate but `getState`/`setState` around it to preserve state.**
  Rejected: still pays handle setup + `activate` cost and the CPU spike; many
  plugins' `getState` is lossy or omits transient DSP state. Doesn't fix churn.
- **Reuse keyed by `device_id`.** Rejected: unsound under duplication — the reason
  step 2 introduced `instance_key`.
- **Keep activation in the wrapper, add a "shared, don't-deactivate" flag.**
  Rejected: leaves two owners of the activation lifecycle (wrapper + instance) —
  the exact duplication ADR-007 §Phase-2b set out to retire; brittle under the
  displaced-wrapper race.

---

## Consequences

- **Positive.** Removes the dominant per-edit CPU spike and the plugin-state loss
  for CLAP; reinforces C-6; completes the wrapper→instance lifecycle unification
  ADR-007 deferred to Phase 2b/4 (CLAP side).
- **Risk.** The activation-lifecycle migration (CLAP) is the delicate part: a
  mistake deactivates a live instance. Mitigation: the C-2 offline harness stays
  green + per-format smoke test (load a CLAP effect, edit graph structure
  mid-playback, assert no click / no crash and tail continuity).
- **Reversibility.** Reuse is a guarded branch in `instantiate`; disabling the
  branch restores the current re-instantiate behaviour.

## Follow-ups

- **VST3 reuse.** Move VST3 `setActive`/`setProcessing` out of the wrapper
  `prepare` into the instance (mirroring CLAP), then add `DeviceFormat::Vst3` to
  the reuse-safe set in `reuse_existing`.
- Pairs with **ADR-017** (per-instance addressing) for the full closure of audit
  points 1-2.
- Sample-rate / buffer-size change is handled in `clap_ensure_active`
  (deactivate + reactivate); an in-place re-`setup` is a later optimisation.
