# ADR-015 — Out-of-process plugin sandbox, opt-in per device

## Status

**Accepted — implemented for CLAP effects and CLAP track instruments
(amendments 2026-09-25, at the end of this file).**  The sub-phase sections below are the history of how it was
built; type names in them (`BridgeSession`, `serve_with`, …) predate the
amendment.  The amendment describes the current design and closes the two
risks listed in *Known risk*.

Materialises **ADR-010** of [`docs/active/migration/target_architecture.md`](../migration/target_architecture.md)
§9 and **Phase 6 — Sandboxing** of [`docs/active/migration/migration_plan.md`](../migration/migration_plan.md)
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
| **6d-i** | `region::SharedRegion`: named OS shared-memory mapping (Windows first) backing the rings, off-RT, tested in isolation | nil | **Implemented** |
| **6d-ii** | Engine spawns `plugin_host_bridge` + control handshake (`BridgeSession`/`serve`), off-RT, proven against the real binary | nil | **Implemented** |
| **6d-iii-a** | Ring-driven per-block process loop in the bridge with an identity pass-through stand-in; audio frames carried over the rings | nil (separate process) | **Implemented** |
| **6d-iii-b-1** | `BridgeProcessor` seam: the loop deserialises/processes/reserialises `f32` blocks through a trait; `PassthroughProcessor` is the identity impl | nil (separate process) | **Implemented** |
| **6d-iii-b-2** | Real CLAP plugin loaded in the bridge (FFI) driving its `process` vtable as a `BridgeProcessor`; `clap_ffi` extracted as a shared crate | nil (separate process) | **Implemented** (CLAP; VST3 later) |
| **6d-iv** | `BridgedNode` proxy + **RT wiring** (`process_block`: bounded spin + timeout + silence) on the audio thread; compiler routes sandboxed CLAP devices to it | **high** | **Implemented** |
| **6e** | Watchdog: `Dead`-marking after consecutive timeouts → immediate-silence fast path (no spin); real-plugin smoke validation remains manual | low | **Implemented** (watchdog; smoke manual) |

Rationale for the cut: 6a–6c and 6d-i/ii/iii touch **zero** audio-thread code and
are bit-identical (C-2). Only **6d-iv** crosses the RT boundary, and it does so
against an interface already ratified here and a transport already validated in
6c/6d-i. Decomposing 6d this finely is deliberate: wiring a cross-process wait on
the audio thread before the region, spawn, and host loop are individually proven
would be the "break the engine to design better" the constraints forbid.

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

## Sub-phase 6d-i — what shipped

The rings now have a **real cross-process home**. Until now `ShmemRing` was only
ever backed by a heap `OwnedRegion` (single process). 6d-i adds the OS mapping
the production rings live in:

- `plugin_bridge::region::SharedRegion` — a named shared-memory mapping.
  `create` (owner) / `open` (peer) wrap Win32 `CreateFileMappingW` /
  `OpenFileMappingW` + `MapViewOfFile`; `Drop` unmaps and closes. The mapped
  base is allocation-granularity aligned, so it satisfies `ShmemRing`'s 4-byte
  alignment requirement. `unsafe impl Send` (owned exclusively, not `Sync`).
- `plugin_bridge` gains a **Windows-only** `windows-sys` dependency
  (`Win32_System_Memory` + `Win32_Foundation` + `Win32_Security` — the last
  because `CreateFileMappingW`'s signature references `SECURITY_ATTRIBUTES`).
  Same `0.59` the engine already pins → no new resolution. Non-Windows targets
  get a stub returning `io::ErrorKind::Unsupported` so the crate still builds
  everywhere; their POSIX `shm_open`/`mmap` mapping lands with cross-platform
  sandbox work (migration plan §6.8 prioritises Windows).
- The engine re-export (`plugin_system::bridge`) now also surfaces `region`.

**Real-time:** mapping is one-time setup (off the audio thread); the hot path
will only ever touch the region through the lock-free `ShmemRing`. Nothing here
runs in `process_block`.

**Verification:** 3 new Windows tests prove two *independent mappings* of one
named region see each other's writes through `ShmemRing` (`init` on one, `attach`
on the other) — the exact cross-process mechanism, exercised in one process via
two mappings — plus missing-name error handling and base alignment.
`plugin_bridge` 14/14, engine **53/53** unchanged, `clippy --tests -D warnings`
and `fmt` clean.

## Sub-phase 6d-ii — what shipped

The bridge process now has a **lifecycle**, off-RT, in `plugin_bridge::session`:

- A fixed **control block** at the head of the shared region
  (`magic | protocol | region_bytes | status:AtomicU32`, [`CONTROL_BYTES`] = 16;
  rings start after it). `CONTROL_MAGIC` = `"EsCt"`, distinct from the per-block
  `BLOCK_MAGIC`.
- `BridgeSession::spawn` (engine/owner) — create region, stamp the control block
  `INIT`, `Command::spawn` the bridge with `--region-name/--region-bytes`, then
  poll `status` (bounded by a timeout) until `READY`; fails cleanly if the child
  dies, reports `ERROR`, or times out. `Drop` sets `SHUTDOWN` and reaps the child
  (grace period, then kill).
- `serve` (bridge/peer) — open the region, validate magic + protocol + size,
  signal `READY`, then block until `SHUTDOWN`. **No plugin is loaded and no audio
  is processed** — that is 6d-iii / 6d-iv.
- `plugin_host_bridge` gains a `--region-name <N> --region-bytes <N>` serve mode.

Scope discipline: the engine's `PluginHost` is **not** wired to spawn a session
yet (that call site belongs with the `BridgedPluginInstance` proxy in 6d-iv).
6d-ii delivers and **proves** the mechanism without touching the engine's runtime
path → engine stays bit-identical.

**Verification:** control-block read/write/transition unit-tested cross-platform
on a heap region (no OS mapping needed). The full spawn→READY→SHUTDOWN handshake
is integration-tested **against the real binary** in the bridge crate's own
`tests/` (where `CARGO_BIN_EXE_plugin_host_bridge` is available), including a
repeatability case. `plugin_bridge` 17/17, `plugin_host_bridge` 2/2, engine
**53/53** unchanged; `clippy --tests -D warnings` and `fmt` clean.

## Sub-phase 6d-iii-a — what shipped

Real audio now flows through the rings, processed by a loop — just not yet by a
real plugin. The mechanism the 6d-iv RT proxy will drive is built and proven:

- `protocol::BlockHeader::{to_bytes, from_bytes}` + `BLOCK_HEADER_BYTES`, with a
  const size assertion freezing the 32-byte layout.
- `session::BridgeLayout` — carves the region into `[control][audio-in][audio-out]`,
  computing each ring's offset and size from `block_size × channel_count ×
  capacity` so owner and peer agree without extra messages.
- `BridgeSession` now sizes the region from the layout, initialises both audio
  ring headers (owner side), and exposes `try_send_input` / `try_recv_output`
  (the off-RT shape of what the 6d-iv proxy will call on the audio thread).
- `session::serve` is now the **process loop**: attach the rings, signal READY,
  then `pop(audio-in) → identity → push(audio-out)` until SHUTDOWN, validating
  each block's `BlockHeader` magic (torn-region guard) and dropping malformed
  blocks. A preallocated scratch buffer means no allocation in the loop.
- `plugin_host_bridge` serve CLI is now layout-driven
  (`--block-size/--channels/--audio-capacity`).

### Why an identity stand-in, not the real plugin

Loading a real VST3/CLAP plugin in the bridge process needs the engine's
`plugin_formats` FFI stack (DLL load, COM/`clap_entry` instantiation). That code
lives deep in `src-tauri` and would have to be extracted into a shared crate or
the bridge would have to depend on the whole Tauri app crate — a large refactor
of the engine's plugin core, exactly the "break the engine to design better" the
constraints forbid in one step. The *ring-driven loop* is separable from *what
runs inside it*, so 6d-iii-a proves the loop with an identity copy; 6d-iii-b
slots the real `process()` into the same place.

**Real-time:** the loop runs in the bridge process; the engine-side helpers are
off-RT (a small scratch allocation, documented for replacement by a preallocated
path in 6d-iv). `ShmemRing`, the RT-validated primitive, was **not modified**.

**Verification:** `BlockHeader` (de)serialisation unit-tested. The full path —
engine spawns the real binary, sends 16 distinct audio blocks, reads each back
**identical** through the bridge's loop — is integration-tested, plus a
per-block sequence/ordering check. `plugin_bridge` 19/19,
`plugin_host_bridge` 2/2, engine **53/53** unchanged; `clippy --tests -D warnings`
and `fmt` clean.

## Sub-phase 6d-iii-b-1 — what shipped

The bridge loop now processes audio at the **`f32` sample level through a typed
seam** — the exact interface a real plugin implements:

- `session::BridgeProcessor` — `process(&mut self, input: &[f32], output: &mut
  [f32], ctx: &BlockContext)`, plus `BlockContext` decoded from the `BlockHeader`.
- `session::PassthroughProcessor` — identity impl (the stand-in that keeps the
  transport verifiable without an installed plugin).
- `serve` now deserialises each block's samples into a preallocated `f32` buffer,
  calls the processor, and re-serialises the output (header echoed, samples
  written back). `serve_with(.., &mut dyn BridgeProcessor)` is the entry point a
  real plugin processor uses in 6d-iii-b-2; the loop and transport are identical.
  Buffers (`scratch`, `in_samples`, `out_samples`) are preallocated — no
  allocation in the loop.

### Why the seam, not the real plugin, in this step

The engine's CLAP FFI (`plugin_formats/clap/ffi.rs`) is, helpfully,
**self-contained** (no `use crate::` — pure C-ABI bindings), so it *is*
extractable. But loading a real plugin in this step is blocked on two things the
constraints don't allow to rush:

1. **Engine-core churn** — sharing that FFI means either moving it out of
   `src-tauri` (rewiring `instance.rs` / `wrapper.rs` / `host.rs` imports) or
   duplicating ~200 lines of `unsafe`. Either is an engine-plugin-core change,
   not a localized one.
2. **Untestable here** — "load a real plugin and process" can't be verified in
   the harness without a guaranteed `.clap` on disk. That is exactly audit B4
   (embed a minimal Rust-written CLAP plugin for tests), a prerequisite that
   belongs with 6d-iii-b-2.

So 6d-iii-b-1 builds the seam (testable now with identity + a unit-test gain
processor proving the loop transforms), and 6d-iii-b-2 implements
`BridgeProcessor` for a real loaded plugin against an embedded test plugin —
a localized change behind this trait.

**Real-time:** loop runs in the bridge process with preallocated buffers; the
sample copies are fixed-size and allocation-free. Engine and `ShmemRing`
untouched.

**Verification:** `PassthroughProcessor` (identity) and a test `GainProcessor`
(×2) unit-tested directly, proving the loop drives the seam rather than echoing
bytes. The end-to-end round-trip (real binary, 16 blocks) now flows through the
f32 seam and still returns identical samples. `plugin_bridge` 21/21,
`plugin_host_bridge` 2/2, engine **53/53** unchanged; `clippy --tests -D warnings`
and `fmt` clean.

## Sub-phase 6d-iii-b-2 — what shipped

The bridge now loads and drives a **real CLAP plugin** (the identity stand-in is
replaced by an actual `process()` call):

- **`clap_ffi` crate** — the engine's `plugin_formats/clap/ffi.rs` (846 lines,
  already `use crate::`-free) was `git mv`'d into a standalone crate. The engine's
  `ffi.rs` became a one-line re-export shim (`pub use clap_ffi::*;`), so **every**
  existing in-engine import path is unchanged. Both the engine and the bridge link
  one CLAP ABI definition.
- **`ClapProcessor`** (in `plugin_host_bridge`) implements `BridgeProcessor`:
  `load` does the `clap_entry → factory → create → init → activate →
  start_processing` sequence; `process` marshals the planar `f32` block into
  `clap_audio_buffer` channel-pointer arrays + `clap_process` and calls the
  plugin's `process` vtable (replicating the engine wrapper's proven sequence);
  `Drop` tears down (`stop_processing → deactivate → destroy`). Params/MIDI are
  passed as empty event lists for now.
- The bridge CLI gains a real-plugin serve mode
  (`--sample-rate --plugin-dll --plugin-id`).

### Engine bit-identical (the extraction)

The CLAP FFI moved out of `src-tauri` but the engine compiles and behaves
identically: only `ffi.rs` changed (→ shim), every consumer import is untouched,
`clap_ffi` is std-only so feature resolution can't shift. Engine suite **51/51**
(the 2 FFI unit tests moved with the code into `clap_ffi`; 51 + 2 = the prior 53).

### What is tested vs. smoke-only

- **Tested (deterministic, no DLL):** `ClapProcessor::process` is unit-tested
  against a **synthetic CLAP plugin** — a real `clap_plugin`-shaped vtable whose
  `process` applies ×2 gain — asserting correct per-channel marshalling, plus an
  oversized-block guard. This validates the risky part (building `clap_process`
  correctly), the new code, without needing an installed plugin.
- **Smoke-only (audit V5):** `ClapProcessor::load` of a real `.clap` reuses
  `clap_ffi`'s loader (proven in the engine) and the CLI plugin mode; these run
  only with a plugin on disk, so they are manual/smoke, not CI.

### Why CLAP and not VST3 here

CLAP's FFI is a self-contained C ABI (extractable cleanly, no engine coupling);
VST3 is COM with deeper engine entanglement. Prototyping the sandbox on one
format (CLAP) matches the plan's "single plugin first"; the VST3 processor is a
later, analogous step behind the same `BridgeProcessor` seam.

**Real-time:** all of this runs in the bridge process; `process` uses only the
preallocated channel-pointer vectors (no allocation). Engine runtime and
`ShmemRing` untouched. `plugin_bridge` 21/21, `plugin_host_bridge` 4/4
(2 synthetic-CLAP unit + 2 spawn/round-trip integration), `clap_ffi` 2/2, engine
**51/51**; `clippy --tests -D warnings` and `fmt` clean.

## Sub-phase 6d-iv — what shipped (the RT crossing)

The engine now hosts a sandboxed plugin **out-of-process, on the audio thread**:

- **`BridgeSession::process_block`** (in `plugin_bridge`) — the real-time
  round-trip. Formats the planar input block into a **preallocated** scratch
  (no allocation), pushes it to the audio-in ring, **bounded-spins** on the
  audio-out ring up to a `spin_budget`, and copies the result into `output`. On
  timeout (or a full ring) it fills `output` with **silence** and bumps a
  `timeout_count` atomic. No mutex, no syscall (only the lock-free SPSC rings +
  a userspace `Instant` deadline). It drains stale outputs from a prior
  timed-out block first, so sends and receives stay paired. `BridgeSession`
  also gained `spawn_plugin` (spawns the bridge in real-CLAP mode).
- **`BridgedNode`** (engine `plugin_system::bridged_node`) — an `AudioNode`
  (1 multichannel port in/out, mirroring the in-process wrapper). `prepare`
  (off-RT) spawns the `plugin_host_bridge` child loading the real plugin;
  `process` marshals the graph's planar buffers through `process_block`. If the
  bridge can't be spawned (no binary, unknown DLL, error), or for a partial
  final block, it **falls back to transparent pass-through** so the graph keeps
  running.
- **Compiler wiring** — `build_node` routes a `NodeKind::Plugin { sandboxed:
  true }` CLAP device to `BridgedNode::try_new`; anything not bridgeable falls
  through to the existing in-process path. Gated on `sandboxed` (default false)
  → **non-sandboxed projects are bit-identical** (engine 53/53, the +2 are the
  new `BridgedNode` tests).

### Real-time honesty

`process` deliberately *waits* — out-of-process hosting cannot be synchronous
otherwise — but the wait is **bounded** and degrades to silence, which is the
whole P0-5 win: a crashed/stalled plugin can no longer take the engine down. The
budget (5 ms placeholder) and a watchdog that marks a device `Dead` after
repeated timeouts are 6e. The round-trip is synchronous, so it adds **no** block
of latency when it completes in budget (simpler than the async model BP-5
assumed).

### What is tested vs. smoke

- **Tested (real binary, deterministic):** `process_block` is exercised
  end-to-end against the real `plugin_host_bridge` (passthrough) — 32 planar
  blocks round-tripped exactly, `timeout_count == 0`. `BridgedNode`'s
  pass-through fallback (no bridge) is unit-tested.
- **Smoke-only (audit V5):** the full chain *sandboxed CLAP device → spawn
  bridge with the real plugin → audio-thread round-trip* needs a real `.clap` +
  the colocated bridge binary + `sandboxed = true`, so it is manual.

`plugin_bridge` 21/21, `plugin_host_bridge` 5/5 (2 synthetic-CLAP unit +
3 spawn/round-trip integration), `clap_ffi` 2/2, engine **53/53**;
`clippy --tests -D warnings` and `fmt` clean.

### Phase 6 status

Everything up to and including the RT boundary is now built and (where CI can)
tested: opt-in flag → region → spawn/handshake → process loop → real CLAP plugin
→ **audio-thread round-trip with bounded-spin + silence**. Only **6e**
(watchdog / `Dead`-marking + real-plugin smoke validation) remains, plus the
later VST3 processor behind the same seam.

## Sub-phase 6e — what shipped

A watchdog so a crashed/hung plugin stops costing the audio thread anything:

- `BridgeSession` tracks `consecutive_timeouts`; after
  `DEAD_AFTER_CONSECUTIVE_TIMEOUTS` (8) in a row it sets a `dead` `AtomicBool`.
  A successful block clears the run, so a one-off scheduling hiccup (which does
  not repeat *consecutively*) never trips it.
- `process_block` **short-circuits** when dead: it fills silence and returns
  immediately, with **no send and no spin** — so a dead bridge costs ~nothing per
  block instead of burning the full spin budget. `is_dead()` is exposed off-RT
  for UI/telemetry ("plugin disabled").
- No new syscalls on the audio thread: a crashed child is detected purely through
  its repeated timeouts (the rings stop being serviced), not by polling the
  process — keeping the hot path RT-safe.

**Detection is intentionally timeout-based, not respawn.** A condemned device
stays silent for the session; automatic respawn/recovery is a later refinement.

**Verification (deterministic):** an integration test drives `process_block`
with a **zero** spin budget against the real bridge — every block then times out
by construction — and asserts the watchdog flips `is_dead()` after exactly the
threshold, that timed-out blocks are silence, and that a dead session
short-circuits even under a generous budget. The healthy round-trip test asserts
`is_dead() == false`. `plugin_bridge` 21/21, `plugin_host_bridge` 6/6
(2 synthetic-CLAP unit + 4 integration), `clap_ffi` 2/2, engine **53/53**;
`clippy --tests -D warnings` and `fmt` clean.

## Phase 6 — overall status

The out-of-process plugin sandbox (ADR-015) is **functionally complete for
CLAP**, end to end and RT-safe:

```
opt-in flag (6a) → graph plumbing (6b) → shmem ring (6c-i) → shared crates (6c-ii)
→ OS region (6d-i) → spawn + handshake (6d-ii) → ring process loop (6d-iii-a)
→ real CLAP plugin (6d-iii-b-2) → audio-thread round-trip (6d-iv) → watchdog (6e)
```

Every step kept the engine **bit-identical** for non-sandboxed projects (the
flag defaults off) and the audio-thread crossing is bounded-spin + silence +
watchdog, so a misbehaving sandboxed plugin can no longer take the engine down
(the P0-5 goal).

**Remaining (out of this ADR's core scope):**
- VST3 processor behind the same `BridgeProcessor` seam (CLAP shipped first
  because its FFI is a clean C ABI; VST3 is COM).
- Manual smoke validation on a real `.clap` (audit V5) — CI cannot assume an
  installed plugin.
- Automatic respawn/recovery of a condemned device; spin-budget tuning;
  param/MIDI rings to the bridge.

## Known risk — surfaced 2026-06-03 (architecture review) — closed 2026-09-25

> Items 1 and 2 are resolved by the amendment below; item 3 is mitigated
> (restart on recompile, crash reported to the UI).

The crash-isolation goal (P0-5) is met and the decomposition is sound. Two
RT-strategy items must be resolved **before the sandbox is enabled on real
content** (it is off by default, so non-blocking for current dev):

1. **Spin budget contradicts the block period.** The decision text says ~10 µs;
   6d-iv ships a **5 ms placeholder**. At 48 kHz / 64 samples a block lasts
   ~1.33 ms, so a 5 ms busy-spin on the audio thread *guarantees* underrun, and
   per-device spins add up. A busy-wait also burns RT-thread CPU and, without a
   guaranteed core pin (not portable to Linux without RT privileges, or macOS),
   can starve the very bridge process it waits on (priority/affinity inversion).
   Tune the budget to a fraction of the block period **and** prefer the
   **asynchronous pipelined** model (process block N−1 while computing N; the one
   block of latency is exactly what PDC/BP-5 already absorbs) over the current
   synchronous spin. The synchronous model was chosen to avoid added latency —
   the wrong trade-off against the project's stated "robustesse runtime > pureté"
   priority.
2. **Offline render + sandbox is unspecified.** During an offline render the
   bridge round-trip timing is non-deterministic (OS scheduling of a separate
   process); a budget expiry injects silence, making the render non-reproducible.
   Offline must use a **blocking, un-timed** round-trip (acceptable — offline is
   not real-time) or force sandboxed devices in-process for rendering. Until
   specified, a sandboxed device breaks C-2. Governed by the determinism
   invariant (`architecture-decision-rules.md`).

3. *(minor)* A `Dead`-marked device stays silent for the whole session
   (no auto-respawn) — a transient hiccup permanently mutes a plugin. UX choice
   to revisit.

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

## Amendment — 2026-09-25: usable sandbox (state, parameters, editor, RT strategy)

Before this amendment the bridge was reachable only by editing a project file,
was re-spawned on every recompile, lost the plugin's state on save, ignored
parameters/automation/modulation, and waited on the audio thread with a 5 ms
spin against a bridge that polled every 200 µs (so timeouts, then a condemned
plugin, were the normal case).  What changed:

### A sandboxed plugin is a `PluginInstance`

As the original decision required, the proxy is **a variant of the native
handle**, not a separate node type chosen by the compiler:
`NativeHandle::Bridged(BridgedPlugin)` (`plugin_system/{instance,bridge}.rs`).
`PluginHost::instantiate` builds it when `sandboxed` is set on a CLAP plugin
(other formats ignore the flag).  Consequences, all through the existing paths:

- **Reuse across recompiles** (ADR-016): the bridge process survives structural
  edits.  Reuse requires the sandbox mode to match, so toggling it rebuilds the
  instance and carries the state across (`restore_from`, R-4).
- **State** (ADR-010): `save_state_blob` / `load_state_blob` become control
  requests; project save and load work unchanged.
- **Parameters** (ADR-007/021): the node reuses `ClapParamEvents` — queued
  edits, automation, modulation, value-modulation, output-event observation —
  and ships the resulting CLAP events with each block.
- **Editor**: opened *in the bridge process* (top-level window, per-monitor DPI,
  same host callbacks as in-process).  A crash in the editor kills only the bridge.
- **Unsaved-changes flag**: the bridge forwards `mark_dirty` and the plugin's
  output events with each block.
- **No silent fallback**: a plugin that cannot start sandboxed is reported
  unavailable; it is never loaded in-process instead.

### Transport

- **Audio**: one ring message per block, `[header][events][samples]`
  (protocol v2, `plugin_bridge::protocol`).  Events are opaque CLAP bytes
  (transport + parameter events in, plugin output events back).
- **Control**: request/response frames over the child's stdin/stdout
  (`plugin_bridge::control`): activate, state, parameter list/value/flush,
  open editor.  Off the audio thread; each request has a timeout, and a timeout
  closes the channel (a late reply would desynchronise it).
- **Wake-up**: a named auto-reset event.  The engine signals it after queuing a
  block; the bridge's audio thread (time-critical priority) sleeps on it.

### Real-time strategy (closes risk 1)

The round-trip stays **synchronous** — no added latency, arbitrary block sizes
(loop wraps split blocks) — but the wait now fits the block:

- live: bounded spin of **3/4 of the block period**; a late block is silence;
  8 consecutive late blocks condemn the plugin, which then costs nothing;
- the only kernel call on the audio thread is the **non-blocking `SetEvent`**
  (no lock is held by another thread that could make it wait).  This is a
  deliberate, bounded exception to "no syscall" for the sandbox path only.

The asynchronous pipelined model (+1 block latency) was not adopted: it needs a
fixed block size to report a constant latency, which the engine does not have.

### Offline render (closes risk 2)

`PrepareContext.live` (new, from `CompileConfig.live`, formerly `audition`)
tells the node it renders offline.  It then waits up to 10 s per block, so a
sandboxed plugin renders the same whatever the OS scheduling (C-2).  Projects
without sandboxed plugins are untouched: the harness stays green with its
golden hashes.

### Crash handling (mitigates risk 3)

A condemned plugin is reported with `UnavailableReason::Crashed` — pushed to the
UI within 250 ms (`plugin_system::bridge::crash_count`, polled by the
plugin-edit watcher) — and restarted by the next recompile ("Redémarrer" in the
effect chain), from its last cached state.

### Verification

- `plugin_bridge` unit tests: protocol, control frames, the OS event, and the
  full owner/peer round-trip in one process.
- `crates/test_clap_plugin`: a real CLAP effect (gain, state, latency, "hang",
  "crash", "touch" parameters).  `plugin_host_bridge/tests/bridge.rs` and
  `plugin_system/sandbox_tests.rs` drive it through the real bridge binary:
  processing, parameter changes and modulation, state round-trip, sandbox
  toggle keeping state, reuse across recompiles, edits reported, crash and hang
  contained, crash reported and recovered.
- Manual (not in CI): a real third-party plugin and its editor.

### Global preference (2026-09-25)

Preferences → Audio → *Isolation des plugins*: **Aucun** (everything
in-process), **Choisis** (default: the effects marked « Isoler », i.e.
`PluginParams.sandboxed`, plus the plugins marked « Toujours isoler » —
right-click on an effect — wherever they are used), **Tous** (every CLAP
effect).  The "always isolate" list is per plugin type and per machine, like
the mode.  It is an app
preference, not a project setting — it says how much this machine trusts its
plugins — so it lives in the frontend preferences and reaches the engine through
`set_plugin_isolation` at startup and on change (`PluginHost::set_isolation`).
A change recompiles and closes open in-process editors.  C-9 holds: the default
isolates nothing unless the user asks.

### Crash attribution: offering to isolate the culprit (2026-09-25)

When an **in-process** plugin kills Echoes, the next start names it and offers
« Toujours isoler … » (or, for VST3, explains it cannot be isolated yet).

`plugin_system/crash_marker.rs` maps a 4 KiB file from the app data folder.
Every call into an in-process plugin marks its tag (FNV-1a of the plugin id) in
the slot of the calling thread — audio (`rt_try_enter` guard), control
(`offrt_try_acquire` guard, instantiation, activation, teardown) or UI (editor
open/close, `on_main_thread`, DPI).  The pages of a file mapping belong to the
OS, so they survive the crash.  At startup, a session still marked *running*
with a non-zero slot gives the suspect (`take_crash_suspect`), matched against
the registry; a normal exit (`RunEvent::Exit`) marks the session clean.  A
second Echoes instance cannot open the file (exclusive write) and marks nothing.

On the audio thread this is one atomic store per plugin block into the mapped
page — no lock, no allocation, no kernel call; after the OS writes the page
back, the next store may take a soft page fault (no I/O).

Blind spots: a crash inside the plugin's own window procedure or on a thread
the plugin created is not attributed (no marked call is in progress), so no
suggestion is made — never a wrong one.

### Crashed plugins in the effect chain (2026-09-25)

`PluginInstance::has_crashed` covers both a condemned sandbox and an in-process
crash caught by the SEH guard (`crash_guard`, lifecycle `Error`); both are
reported as `UnavailableReason::Crashed` and counted by
`crash_marker::note_crash` so the UI learns within 250 ms.  The effect block
turns red with a « Recharger » button in its header, visible even collapsed;
it calls `reload_plugins` (a recompile).  The rebuild restores the **last saved
state**, never the crashed instance's live state: calling into a plugin whose
memory may be corrupt could take Echoes down.

### Track instruments (2026-09-25)

A CLAP track instrument runs sandboxed when the isolation preference says so:
**Tous**, or **Choisis** with the plugin in the « Toujours isoler » list
(right-click on the instrument, or the crash dialog — which already offered it
for any CLAP plugin but had no effect on an instrument).  There is no
per-track « Isoler » flag: that would be a project-format and event change, and
« this synth is unstable » is a property of the plugin, which the per-type list
already expresses.

- `PluginHost::instantiate_instrument` asks `isolates(plugin_id, false)` and
  builds the instance with `create_bridged_instance` (shared with effects).
  Reuse (ADR-016) requires the same sandbox mode, so a preference change
  rebuilds the instance and carries its state (`restore_from`).
- `BridgedInstrument` (`plugin_system/bridged_node.rs`) is the
  `InstrumentDevice`; it shares the per-block round-trip (`BridgeRoundTrip`)
  with `BridgedNode`.  The notes join the parameter events in one time-sorted
  list, the block has no audio input (`BLOCK_HAS_INPUT` clear), and a block that
  cannot be sent is silent with its notes dropped — as in-process.
- `InstrumentDevice::prepare` now takes the `PrepareContext`, so the
  instrument knows it renders offline and waits like an effect (C-2).
- The bridge process needed no change: it already zeroed the input and passed
  `audio_inputs_count = 0` for a block without input, and forwards note events.

Tests (`plugin_system/sandbox_tests.rs`, real bridge + test plugin, which now
plays velocity × gain for a held note when it has no input): notes and
parameters reach the sandboxed instrument, the preference moves it in and out
of the sandbox with its state, and a crash is contained, reported and restarted.

### Still open

- A per-track « Isoler » choice for an instrument (today: the preference only).
- **VST3** behind the same handle.
- **Installer**: `tauri build` builds the bridge next to the app binary, but
  the installer does not bundle it yet (`bundle.externalBin`).
- The live wait still spins a core for the plugin's processing time.
