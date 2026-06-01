# ADR-015 — Out-of-process plugin sandbox, opt-in per device

## Status

**Proposed (design ratified) — partially implemented (sub-phase 6a only).**

Materialises **ADR-010** of [`docs/target_architecture.md`](../../target_architecture.md)
§9 and **Phase 6 — Sandboxing** of [`docs/migration_plan.md`](../../migration_plan.md)
§6. Addresses `P0-5` (long-term half) of
[`audits/plugin-system-audit-2026-05-31.md`](../audits/plugin-system-audit-2026-05-31.md)
and `8.1–8.4` of `critical_issues.md`.

> Naming note: the target document calls this decision *ADR-010*. The decisions
> directory already used 010 for plugin-state persistence, so the bridge keeps
> the next free number (015). Same decision, different filing index.

This ADR ratifies the **full Phase 6 design** so the threading/process boundary
is not changed silently (architecture-governance rule). Only **sub-phase 6a** is
implemented in the accompanying commit; 6b–6e are scoped here and land in their
own ratifiable commits.

---

## Context

The engine runs every plugin **in-process, on the audio thread itself**
([`scheduler.rs::process_one_step`](../../../src-tauri/src/audio_graph/scheduler.rs)
→ `processor.process(...)`). Phase 1 wrapped that call in `catch_unwind`, which
contains **Rust** panics only. It does **not** contain:

- **C++ structured exceptions** (Windows SEH: access violation, divide-by-zero).
  A misbehaving third-party plugin (Vital, Diva, Serum under extreme
  modulation / corrupt presets) SEGVs → the whole engine process dies → the
  unsaved session is lost.
- **Runaway `process`** (internal deadlock, exponential blow-up). The plugin
  holds the real-time thread hostage → continuous underrun, with no watchdog to
  break out.

There is no isolation boundary: engine memory, the audio-thread stack, and
untrusted plugin code share one address space and one real-time thread. The
otherwise-sound foundations (DAG, atomic snapshot, worker pool) cannot fix this
*in-process* — by construction.

## Decision

Introduce an **optional, per-device** out-of-process bridge,
`plugin_host_bridge.exe`, that hosts the real VST3/CLAP wrapper in a separate
process. The engine talks to it over **pre-allocated shared-memory ring
buffers** (audio in/out, params, events). A **watchdog** marks a timed-out or
crashed device `Dead` and the proxy returns silence for it; the rest of the
graph keeps running.

The bridge is **never enabled by default** (C-9): reliable plugins stay
in-process for performance and zero added latency. The opt-in is a per-device
boolean, persisted in the project (`PluginParams.sandboxed`, default `false`).

The proxy (`BridgedPluginInstance`) exposes the **same** interface as the
in-process `PluginInstance` (Arc-clonable, `ParamQueue`, MIDI queue, state
save/load), so the compiler, snapshot, and scheduler see no new type — only a
different `NativeHandle` variant behind the existing `Arc<PluginInstance>`.

### Real-time strategy (the only RT-sensitive part — sub-phase 6d)

The audio thread must not block unboundedly on another process. Per block, for a
sandboxed device:

1. Write the input block into the shared-mem input ring; publish via an atomic.
2. **Bounded spin** (~10 µs budget) on the completion atomic.
3. On timeout → insert **silence** for this device's outputs this block, push a
   `bridge_timeout` telemetry event, and let the watchdog decide `Dead`.
4. On completion → copy the output ring back.

No mutex, no allocation, no syscall on the hot path beyond the bounded spin and
a shared-memory copy. The bridge process runs at RT priority to avoid priority
inversion (couples with target §6.6 / `1.7`).

### Latency (BP-5)

A sandboxed device costs ~1 block of round-trip latency. The bridge **reports**
that latency through the existing `latency_samples()` path, and the engine's PDC
(already wired in Phase 1) absorbs it. No new PDC code.

## Sub-phase decomposition

| Step | Deliverable | RT risk | Status |
|------|-------------|---------|--------|
| **6a** | This ADR + `PluginParams.sandboxed` opt-in flag, **persisted but inert** (engine always in-process) | nil | **Implemented** |
| 6b | Plumb `sandboxed` project → `NodeKind::Plugin` → compiler → `PluginHost::instantiate` (still always in-process) | nil | Pending |
| 6c | `plugin_host_bridge` crate + shared-memory ring primitive (off-RT, unit-tested in isolation) | nil | Pending |
| 6d | `BridgedPluginInstance` proxy + RT wiring (bounded spin + timeout + silence) | **high** | Pending |
| 6e | Watchdog + `Dead`-marking + prototype on a **single** plugin (Vital), then generalise | high | Pending |

Rationale for the cut: 6a/6b/6c touch **zero** audio-thread code and are
bit-identical (C-2). Only 6d crosses the RT boundary, and it does so against an
interface already ratified here and a transport already validated in 6c.

## Sub-phase 6a — what shipped

- `PluginParams.sandboxed: bool` (`#[serde(default)]`) on the project model
  ([`project.rs`](../../../src-tauri/src/project.rs)).
- The two `PluginParams { … }` literals in
  [`engine_gateway/api.rs`](../../../src-tauri/src/engine_gateway/api.rs)
  completed (`false` for a new effect; the merge path preserves the existing
  value).

**Deliberately untouched** at this step: `NodeKind`, the graph compiler, the
runtime snapshot DTO, the scheduler, `PluginHost`, `PluginInstance`. The flag is
**declared and persisted but not consumed** — mirroring the Phase 1 precedent of
declaring `PortKind` before wiring it to the compiler. This keeps the
graph/RT/snapshot boundaries entirely unchanged while making projects
forward-compatible with the opt-in.

## Consequences

- **C-2 (bit-identical offline):** guaranteed at 6a — no runtime change.
- **C-4 (project backward-compat):** `#[serde(default)]` → pre-Phase-6 projects
  load with `sandboxed = false`; save/load round-trip preserved.
- **C-9 (sandbox opt-in, never default):** default `false`.
- **BP-5 (per-block latency on sandboxed devices):** absorbed by PDC; only
  relevant from 6d.
- **Dual code path (in-process / bridged):** double maintenance surface,
  inherent to C-9, from 6d onward.
- No 32-bit plugin bridging in the initial design (deferred).

## Alternatives rejected

- **Mandatory global bridge** (Bitwig "All Plug-ins in Bridge"): too costly by
  default, breaks bit-identical rendering of existing projects, latency not
  absorbable for small-buffer live monitoring. Rejected per target ADR-010.
- **Win32 `__try/__except` around the in-process FFI call** as the *final*
  answer: catches SEH but leaves engine memory corruptible by a faulty plugin
  and cannot interrupt a runaway `process`. May still be added as a cheap
  in-process hardening layer, but it is not a substitute for process isolation.
- **Spreading the inert `sandboxed` flag through `NodeKind` / snapshot at 6a:**
  rejected — it would touch RT-adjacent boundaries with no consumer, against the
  governance rule. Deferred to 6b where the consumer (instantiate branch)
  actually exists.
