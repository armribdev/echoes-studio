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
| **6b** | Plumb `sandboxed` project → `NodeKind::Plugin` → compiler → `PluginHost::instantiate` (still always in-process) | nil | **Implemented** |
| **6c-i** | Shared-memory SPSC ring primitive + wire protocol POD types, off-RT, unit-tested in isolation (`plugin_system/bridge/`) | nil | **Implemented** |
| **6c-ii** | `plugin_host_bridge` binary crate + shared `plugin_bridge` lib crate + workspace | nil | **Implemented** |
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

## Sub-phase 6b — what shipped

The opt-in flag now travels from the project model to the host's instantiation
decision point, **still inert** (always in-process):

- `NodeKind::Plugin` gains a `sandboxed: bool` field
  ([`graph.rs`](../../../src-tauri/src/audio_graph/graph.rs)); the
  `from_project` builder copies it from `PluginParams.sandboxed`.
- The compiler's `build_node` destructures it and forwards it to
  `PluginHost::instantiate(node_id, plugin_id, sandboxed)`
  ([`compiler.rs`](../../../src-tauri/src/audio_graph/compiler.rs)).
- `instantiate` accepts the flag and discards it (`let _ = sandboxed;`) with a
  marker for 6d ([`host.rs`](../../../src-tauri/src/plugin_system/host.rs)).
- The read-only snapshot DTO ignores the field (`{ device_id, .. }`); the UI
  label is unchanged.

Scope: **effect path only**. The instrument path (`instantiate_instrument`)
has its own project source and is sandboxed in a later step. `NodeKind` derives
only `Debug, Clone` — the new field affects no graph identity / recompile-diff
logic, so the compiled graph and offline render stay bit-identical (C-2,
verified: full suite 53/53 green).

## Sub-phase 6c-i — what shipped

The shared-memory transport **core**, off-RT and unit-tested in isolation,
under [`plugin_system/bridge/`](../../../src-tauri/src/plugin_system/bridge/):

- `shmem::ShmemRing` — a region-agnostic SPSC ring that lives entirely inside a
  caller-supplied raw byte region (an OS shared-memory mapping in production; an
  aligned `OwnedRegion` heap backing in tests). Header (`capacity | slot_size |
  head:AtomicU32 | tail:AtomicU32`) + length-prefixed fixed-stride slots.
  `init` (owner) / `attach` (peer), `push` / `pop` with no allocation, no lock,
  fixed-size copies — RT-safe for the audio thread it will run on in 6d.
- `protocol` — `#[repr(C)]` POD message types pinning the cross-process ABI:
  `BRIDGE_PROTOCOL_VERSION`, `BLOCK_MAGIC`, `ChannelRole`, `BlockHeader`,
  `ParamMessage`, with size/alignment assertions so the layout is frozen before
  the rings are wired.

Why a separate module and **not** the existing `ParamQueue`: that queue stores
its slots behind heap pointers (`Box<[UnsafeCell]>`) that are meaningless in
another process's address space. The bridge needs the whole ring state — indices
*and* data — laid out inside one mapped region.

**Deliberately deferred to 6c-ii:** the `plugin_host_bridge` binary crate. The
engine (`src-tauri`) is a standalone Tauri app crate with no workspace; spinning
up a sibling binary crate is a build-system change (workspace root, shared
protocol crate) with no payoff until 6d gives it real IPC to run. Keeping it out
of 6c-i leaves `src-tauri`'s manifest and every runtime path untouched.

**Not wired:** no runtime path references `bridge::*`; the module carries
`#![allow(dead_code)]`. The engine remains bit-identical (full suite 64/64
green, +11 new bridge tests including a 100 k-message two-thread ring exchange
that exercises the SPSC discipline a real cross-process bridge depends on).

## Sub-phase 6c-ii — what shipped

The transport core became a **shared library crate** so the engine and the
bridge host link one definition of the cross-process ABI:

- `src-tauri/crates/plugin_bridge/` *(new lib, std-only, zero deps)* — `shmem` +
  `protocol` were `git mv`'d here unchanged (only two inter-crate doc links
  fixed). Its 11 tests moved with it.
- `src-tauri/crates/plugin_host_bridge/` *(new bin)* — **skeleton** host: it
  links `plugin_bridge`, pins the spawn-time CLI handshake
  (`--protocol-version <N>` / `--print-protocol-version`) and validates
  `BRIDGE_PROTOCOL_VERSION`, then exits. It maps no region, loads no plugin,
  processes no audio — the hosting loop is step 6d. Honest stub, no faked work.
- `src-tauri/src/plugin_system/bridge.rs` — replaces the old `bridge/` module;
  re-exports `plugin_bridge::{shmem, protocol}` so `crate::plugin_system::bridge::…`
  paths stay stable for 6d.
- `src-tauri/Cargo.toml` — `[workspace]` **rooted at this crate** (not the repo
  root) so `target/` stays at `src-tauri/target` and the Tauri build/dev flow is
  untouched; `resolver = "2"` made explicit (already the implicit default) to
  freeze feature resolution; adds the `plugin_bridge` path dependency.
- `scripts/test-all.mjs` — `clippy` / `test` gain `--workspace` so CI covers the
  new crates.

### Why a workspace rooted at `src-tauri`, not at the repo root

A repo-root workspace would relocate the Cargo artifact dir to `<repo>/target`,
diverging from the `src-tauri/target` path the Tauri tooling and `scripts/`
already use. Rooting the workspace at `src-tauri` keeps that path identical, so
the only build-system surface touched is `src-tauri/Cargo.toml` plus the two new
member crates. The `plugin_host_bridge` binary is **not** a dependency of the
engine (it is *spawned* as a process in 6d, never linked), so it builds only
under `--workspace` / `-p`.

### Verification

- Engine bit-identical (C-2): `resolver = "2"` is the standalone edition-2021
  default; `plugin_bridge` shares no transitive deps with the engine's existing
  dependencies, so feature unification cannot shift. Engine suite **53/53**
  unchanged; the 11 transport tests now run under `plugin_bridge` (workspace
  total 64). New crates pass `cargo clippy -D warnings` and `cargo fmt --check`.

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
