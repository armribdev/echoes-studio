# ADR-007 — Shared `Arc<PluginInstance>` (Phase 2a)

## Status

Accepted. Implements **Phase 2a** of [`docs/active/migration/migration_plan.md`](../migration/migration_plan.md).
Materialises the ADR-006 design in
[`docs/active/migration/target_architecture.md`](../migration/target_architecture.md) (BP-1, C-6).

Subsequent phases (out of scope here):
- **Phase 2b** — unify the lifecycle state (single atomic on the instance).
- **Phase 2c** — refactor `open_vst3_editor` / `open_clap_editor` to clone the
  same `Arc<PluginInstance>` instead of creating a second IComponent /
  ClapPlugin (closes A1 of the plugin-GUI audit).
- **Phase 3** — SPSC `ParamChange` queue carried by `PluginInstance` so
  the host can address parameters on the live plugin.

---

## Context

Pre-Phase-2a layout
([`plugin_system/host.rs`](../../../src-tauri/src/plugin_system/host.rs),
[`plugin_formats/vst3/wrapper.rs`](../../../src-tauri/src/plugin_formats/vst3/wrapper.rs),
[`plugin_formats/clap/wrapper.rs`](../../../src-tauri/src/plugin_formats/clap/wrapper.rs)):

- The **wrapper node** owned the FFI handles directly
  (`NonNull<IComponent>` / `NonNull<IAudioProcessor>` / `*const ClapPlugin`
  plus their `Arc<…Library>` keep-alive).
- The **`PluginHost`** kept a parallel `DeviceRecord` containing only a string
  `id`, a duplicated `LifecycleState`, and a `saved_state: DeviceState` JSON
  blob — **no link** to the live instance.
- The GUI editor (`gui.rs::open_vst3_editor` / `open_clap_editor`) created
  **its own** IComponent / ClapPlugin to drive the editor view.

Direct consequences documented in the audit
([`audits/plugin-system-audit-2026-05-31.md`](../audits/plugin-system-audit-2026-05-31.md)):

- **P1-7** — `PluginHost::set_parameter_normalised` "ne sait pas vers *qui*
  envoyer le paramètre, et `save_device_state` ne peut pas appeler
  `device.save_state()`".
- **P1-2** — two `LifecycleState` sources of truth (host record + wrapper
  field) never synchronised.
- **P1-9** — `PluginInfo` cannot carry capabilities because the host has no
  handle to query them on.
- **A1 (GUI audit)** — two live plugin instances per device: audio path and
  GUI path are different objects; plugins with process-global registration
  (MTS-ESP, MIDI learn, host-tracked CC) misbehave.

Phase 3 (ADR-008 SPSC params) cannot land until the host can address a queue
embedded **next to** the live plugin — i.e. it cannot land until the host and
the wrapper agree on what "the live plugin" is.

## Decision

Land **Phase 2a**: introduce a single shared `PluginInstance` per
`device_id`, held behind an `Arc`. The host keeps one clone, the wrapper
keeps another; (Phase 2c will give the GUI a third).

### Module map

1. **New file
   [`plugin_system/instance.rs`](../../../src-tauri/src/plugin_system/instance.rs)** —
   defines:
   - `PluginInstance { device_id, name, vendor, format, handle, lifecycle: AtomicU8 }`
   - `enum NativeHandle { Internal, Vst3 { component, processor, _lib }, Clap { plugin, _host, _lib } }`
   - Constructors `new_internal`, `new_vst3`, `new_clap` — each returns an
     `Arc<Self>`.
   - Accessors `handle()`, `vst3_handles()`, `vst3_library()`, `clap_plugin()` —
     pattern-match the variant the caller expects. (`vst3_library()` clones the
     `Arc<Vst3Library>` so the editor can re-obtain the factory for
     dual-component plugins — audit V4.)
   - Lifecycle helpers `set_lifecycle()` / `lifecycle()` — atomic, encoded
     into a `u8`.
   - **`Drop`** — performs the format-specific FFI teardown when the **last**
     `Arc` is released. For VST3 this is the full sequence
     `set_processing(0) → set_active(0) → terminate → release × 2`; for CLAP
     it is only `destroy()` (the CLAP wrapper still owns the
     `start/stop_processing` and `activate/deactivate` calls because they
     are gated by its `active: bool` per-prepare flag).

2. **[`plugin_system/host.rs`](../../../src-tauri/src/plugin_system/host.rs)** —
   - `DeviceRecord { instance: Arc<PluginInstance>, lifecycle, saved_state }`.
   - `instantiate_vst3` / `instantiate_clap` build a `PluginInstance` from
     the freshly created native handles, clone the `Arc` into the record,
     and hand a second clone to the freshly-built wrapper node.
   - New public accessor `instance(device_id) -> Option<Arc<PluginInstance>>`
     — used by Phase 2c (GUI) and future tests.
   - `suspend_device` / `resume_device` / `destroy_device` now propagate
     the new state into `instance.set_lifecycle(…)` as well as the legacy
     `record.lifecycle` (transitional duplication; Phase 2b retires the
     wrapper-side copy).

3. **[`plugin_formats/vst3/wrapper.rs`](../../../src-tauri/src/plugin_formats/vst3/wrapper.rs)** —
   - `VstWrapperNode { node_id, info, params, lifecycle, instance: Arc<PluginInstance>, … }`.
   - No more `component: NonNull<IComponent>`, `processor: NonNull<IAudioProcessor>`,
     `_lib: Arc<Vst3Library>` fields — all moved into the instance.
   - All FFI dispatches read the handle through `self.instance.vst3_handles()`
     (one non-atomic load through `Arc::deref`).
   - **No more `Drop` impl** — `PluginInstance::Drop` is the sole owner of
     VST3 teardown.

4. **[`plugin_formats/clap/wrapper.rs`](../../../src-tauri/src/plugin_formats/clap/wrapper.rs)** —
   - `ClapWrapperNode { node_id, info, params, lifecycle, instance: Arc<PluginInstance>, … }`.
   - No more `plugin: *const ClapPlugin`, `_host: Box<ClapHost>`,
     `_lib: Arc<ClapLibrary>` fields — all moved into the instance.
   - FFI dispatches via `self.instance.clap_plugin()`.
   - **Drop still present** but reduced to `stop_processing` + `deactivate`
     (gated by `self.active`). The instance's `Drop` performs `destroy()`
     after the last `Arc` is released.

5. **[`plugin_system/mod.rs`](../../../src-tauri/src/plugin_system/mod.rs)** —
   re-exports `instance::{PluginInstance, NativeHandle}`.

### Sequencing of `Drop`

For one `device_id`, instances of `Arc<PluginInstance>` are held as follows:

```
                      ┌────────────────────────────────┐
  command thread      │ host.records["id"].instance    │  ← +1
                      └────────────────────────────────┘
                                     │
                                     │ Arc::clone at instantiate time
                                     ▼
                      ┌────────────────────────────────┐
  audio side          │ wrapper.instance               │  ← +1
                      └────────────────────────────────┘
                       (lives inside CompiledGraph
                        → drop ring SPSC on swap)
```

Refcount transitions for a typical recompile cycle:

1. `t0` — `instantiate` builds the instance (count = 1: host clone created).
2. `t0` — compiler embeds a wrapper node with a fresh `Arc::clone`
   (count = 2).
3. `t1` — snapshot swap; new `CompiledGraph` is live on the audio thread.
4. `t2` — user touches a knob, triggering recompile.
5. `t2` — `instantiate` is called again: new `PluginInstance` built; old
   `DeviceRecord` is replaced in `host.records` and dropped on the
   command thread → **old** instance count: 2 → 1.
6. `t3` — second snapshot swap retires the old `CompiledGraph` to the drop
   ring; the drop-ring worker eventually drops it.
7. `t3` — when the old wrapper is dropped on the drop-ring thread:
   - `ClapWrapperNode::drop` calls `stop_processing` + `deactivate`
     (if it was active) via `Arc::deref`.
   - The wrapper's `instance: Arc<PluginInstance>` field is then released,
     count: 1 → 0.
   - `PluginInstance::Drop` runs **on the drop-ring thread** and performs
     final FFI teardown.

If the user removes the plugin from the project entirely:

- `host.destroy_device("id")` removes the record on the command thread,
  releasing the host clone.
- Whichever release happens last (host or audio wrapper) runs
  `PluginInstance::Drop` on its own thread.

**No `Drop` ever runs on the audio thread.** The audio thread releases its
hold on the wrapper only by swapping the snapshot, after which the old graph
travels to the drop ring (`audio_graph::snapshot::RuntimeSnapshot` —
fondation 4 of the audit).

### What this phase does **not** do

- No SPSC param transport (Phase 3, ADR-008).
- No state save/load via FFI (Phase 4).
- No GUI editor consuming the shared `Arc` (Phase 2c).
- No capability introspection at `prepare` time (Phase 5d).
- No unification of `LifecycleState` into a single field (Phase 2b — the
  `set_lifecycle` calls in this PR are deliberately *redundant* with the
  wrapper's own `lifecycle: LifecycleState` field, so the wrapper API
  observable behaviour is unchanged).

## Real-time impact

| Change | RT cost | RT risk |
|---|---|---|
| `Arc::clone` at instantiate | none — off-RT | none |
| `Arc::deref` to reach handle in `process` | one non-atomic load (≡ field read) | none |
| `Arc::drop` on snapshot retire | runs on drop-ring worker (off-RT) | none — same path as previous wrapper drop |
| `PluginInstance::Drop` FFI teardown | runs on drop-ring or command thread | none — same calls as the old wrapper `Drop` |
| `AtomicU8` lifecycle store on transitions | off-RT (suspend/resume from command thread) | none |
| `AtomicU8` lifecycle load | reserved for off-RT consumers (host, future GUI) | none — never called from `process` |

The `process` call path is **mechanically identical** to the pre-Phase-2a
code: same FFI sequence, same pointer scratch, same buffer routing. The only
difference is that the handle pointer is reached through `Arc::deref`
instead of through `self.component`/`self.processor`/`self.plugin` directly
— same memory access pattern, same instruction count.

C-1 (zero mutex / zero alloc / zero syscall in `process`) is preserved.

## Compatibility

- **C-2 (bit-identique offline).** Preserved. The DSP path is unchanged: the
  same FFI process call is made with the same arguments. No new state, no new
  filter, no new computation.
- **C-3 (variant-by-variant migration).** Achieved: each format wrapper opts
  in independently. `PassThroughDevice` (Internal format) does **not** route
  through `PluginInstance` for its own state — only the host's per-device
  record gets an `Arc::new_internal(...)` clone, so the audit's "thin device"
  pattern is preserved.
- **C-4 (saved projects).** No schema change. `DeviceState` is unchanged.
- **C-5 (lock-free cmd ↔ audio).** Arc is the canonical lock-free shared
  ownership primitive. No mutex, no channel introduced.
- **C-6 (one live plugin per device).** Achieved on the host/wrapper side.
  The GUI still creates its own instance until Phase 2c — that closure of A1
  is the explicit Phase 2c deliverable.
- **C-7 (compiler extended, not rewritten).** `PluginHost::instantiate` still
  returns `Box<dyn AudioNode>` to the compiler — the compiler call site at
  [`compiler.rs:608`](../../../src-tauri/src/audio_graph/compiler.rs#L608)
  is unchanged.

## Consequences

- **Positive.**
  - Host can now address the live plugin by `device_id` →
    `Arc<PluginInstance>` (`host.instance(...)`). Phase 3 SPSC and Phase 4
    state save/load become tractable.
  - Lifecycle propagation works (the host's `suspend_device` no longer
    leaves the wrapper in `Running`).
  - `PluginInstance::Drop` is now the **single** point of FFI teardown,
    eliminating the previous duplication risk between wrapper and host.
  - The unsafe `Send` claim is centralised in `instance.rs`, alongside the
    discipline that justifies it.

- **Limits.**
  - GUI editor still uses its own short-lived IComponent / ClapPlugin
    (A1 still open). Phase 2c is the closure.
  - `set_parameter_normalised` still writes to a JSON blob the audio thread
    never reads — Phase 3 (ADR-008) replaces it with an SPSC drain.
  - `lifecycle` is still duplicated (wrapper field + instance atomic).
    Intentional for Phase 2a — Phase 2b unifies.

- **Debt added.**
  - The host's `record.lifecycle` and `record.saved_state` fields are
    transitional. Both will be retired in subsequent phases; the API surface
    (`suspend_device`, `lifecycle_of`, `save_device_state`,
    `load_device_state`) is kept stable to avoid touching call sites
    needlessly.
  - `debug_assert!` guards in the wrapper constructors are a developer-only
    check that the host wires the correct format to the correct wrapper. A
    bug at the construction site would otherwise surface as a panic in
    `vst3_handles()` / `clap_plugin()`. Acceptable: it's a programmer error,
    not a runtime condition.

- **Reversibility.**
  - The five edited files are local and additive.
  - Wrapper constructors keep their old shape *semantically* — only the
    inputs change. Reverting is straightforward at the cost of giving up the
    host↔wrapper link.

## Alternatives considered

- **Land all of Phase 2 (2a + 2b + 2c) in one PR.** Rejected — the migration
  plan §2.7 explicitly prescribes the 3-sub-PR split to bound rollback risk
  on a structural refactor.
- **`Arc<Mutex<PluginInstance>>`.** Rejected: a mutex on a host-shared handle
  would force the audio thread to take a lock to dispatch FFI on the
  wrapper's hot path. Direct violation of C-1.
- **`Weak<PluginInstance>` on the host side, `Arc<…>` on the wrapper.**
  Rejected: when the audio side drops first (e.g. user deletes a track), the
  host loses the ability to address the still-valid (per its own bookkeeping)
  device. The audit's P1-7 / P1-9 needs a strong reference on the host side.
- **Trait object `dyn NativeHandle` instead of enum.** Rejected: variants
  are closed (two formats today, plus the no-op Internal); the dispatch
  matches on a `u8`-sized tag. Trait objects would add a vtable indirection
  and obscure the format-specific FFI cleanup logic.

## Update — 2026-06-03 (records keyed by `instance_key`)

Phase 2a keyed `PluginHost.records` by `device_id` (the plugin *type*), which
collapsed two instances of the same plugin onto one record — the **R-2**
collision flagged in
[`audits/engine-architecture-audit-2026-06-03.md`](../audits/engine-architecture-audit-2026-06-03.md)
(points 1-2). The map is now keyed by a stable **per-placement** `instance_key`
(project `effect.id`, or `instrument:{track_id}` for a track instrument), carried
on `NodeKind::Plugin` and passed to `PluginHost::instantiate` /
`instantiate_instrument`.

What changed, and what deliberately did **not**:

- **Changed (engine-internal only):** `records` key; `retain_devices` now matches
  on each record's `instance.device_id`; the `device_id`-addressed methods
  (`push_plugin_param`, `save_device_state`, `instance`, `lifecycle_of`,
  `suspend/resume/destroy_device`) resolve via the new `record_by_device_id`
  helper. Behaviour is identical in the one-instance-per-type case.
- **Unchanged (out of scope here):** the project file format
  (`plugin_state_blobs` stays keyed by `device_id`) and the UI/IPC parameter
  protocol. Fully per-instance addressing — so two duplicates can be targeted
  independently from the UI and persisted separately — remains a follow-up
  (audit point 2, "step 4"), which touches the serialized format (C-4 migration)
  and the IPC surface and therefore needs its own ADR.
- **Not yet done:** instance *reuse* across recompiles (audit point 1). The new
  key is its prerequisite; reuse additionally requires avoiding a double
  `activate` in the format wrappers and is gated on that work.

C-1/C-2/C-5/C-6 are unaffected: all of this runs on the command thread at compile
time; the `process` path and the determinism guarantees are untouched.

## Follow-ups

- **Phase 2b** — fold `LifecycleState` into the instance's `AtomicU8`;
  remove the wrapper-side duplicate.
- **Phase 2c** — `gui::open_vst3_editor` and `open_clap_editor` clone the
  `Arc<PluginInstance>` returned by `host.instance(...)` instead of creating
  a fresh IComponent / ClapPlugin. Closes A1 of the plugin-GUI audit.
- **Phase 3 (ADR-008)** — `PluginInstance` gains an SPSC `ParamChange`
  queue; the host pushes, the wrapper drains in `process()`.
- **Phase 5d** — `PluginInstance` exposes a `capabilities()` method
  populated by the wrapper at `prepare()` from VST3 `getBusInfo` /
  `IEditController` and CLAP `clap.audio-ports` / `clap.params`.

## Update 2026-09-29 — unused host lifecycle API removed

`PluginHost::suspend_device`, `resume_device` and `lifecycle_of` never had a
production caller and were removed with the `Device` trait (ADR-006 amendment).
The lifecycle atomic on `PluginInstance` is unchanged; it is driven by
instantiation, `retain_devices` and the wrappers' activation paths.
`destroy_device`, `save_device_state` and `device_count` are now test-only
(`#[cfg(test)]`). The Phase 5d follow-up above was delivered through
`PluginManifest` + `clap/audio_ports.rs` (ADR-014), not a `capabilities()`
method.

Also removed the same day, never read: `PluginInstance.name` / `vendor` (the constructors no longer take them) and the `PluginManifest` fields `version`, `num_audio_inputs`, `num_audio_outputs`, `has_midi_input`, `has_midi_output`. Port counts reach the compiler through `num_sidechain_inputs` and the loaded instance's ports (ADR-014). `instance_id` now exists only in test builds, where it proves instance reuse.
