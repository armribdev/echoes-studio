# ADR-010 — Plugin state persistence via native FFI (Phase 4b VST3 + 4c CLAP)

## Status

Accepted. Implements the **VST3 half (sub-phase 4b)** and the **CLAP half
(sub-phase 4c)** of **Phase 4 — Séparation DSP/UI stricte (clôture)** of
[`docs/active/migration/migration_plan.md`](../migration/migration_plan.md) §4 (the `P0-4` of
[`audits/plugin-system-audit-2026-05-31.md`](../audits/plugin-system-audit-2026-05-31.md)).

Scope of this ADR: **VST3 + CLAP.** AU/WASM are deferred and continue to use the
empty/cache fallback. Phase 4a (single GUI/audio `Arc<PluginInstance>`, closing
A1 of the GUI audit) shipped in Phase 2c and is a precondition here.

Sub-phase 4c reuses **verbatim** the `PluginInstance` / `PluginHost` / project
plumbing introduced for VST3: only a CLAP-native stream pair and a dispatch
branch were added (see *Module map* and *CLAP addendum* below). The host API,
project file schema, and save/load orchestration are format-agnostic and were
not touched.

---

## Context

Audit P0-4: plugin state was never serialised. `VstWrapperNode::save_state`
returned `DeviceState::empty(...)`, `load_state` was a no-op, and
`PluginHost::save_device_state` only cloned a perpetually-empty cache. Every
project save dropped the plugin's internal state; every reload reverted external
plugins to their default preset. The project file carried no plugin state at
all.

The native APIs were already partly reachable: `IComponent` exposes
`set_state` / `get_state` in the FFI vtable
([`vst3/ffi.rs`](../../../src-tauri/src/plugin_formats/vst3/ffi.rs)). Both take an
`IBStream*` the host must supply, and the project links no VST3 SDK, so the host
had no concrete stream to pass.

## Decision

1. **Host-implemented `IBStream`.** A `Vec<u8>`-backed `HostStream`
   (`vtbl` at offset 0, FUnknown + read/write/seek/tell) lives in `vst3/ffi.rs`,
   alongside the existing host-side COM objects (`HostParameterChanges`,
   `IComponentHandler`, `IPlugFrame`). Two safe-ish helpers wrap it:
   `read_component_state(comp) -> Option<Vec<u8>>` (calls `getState`) and
   `write_component_state(comp, &[u8]) -> bool` (calls `setState`).

2. **State lives on `PluginInstance`.** `save_state_blob()` / `load_state_blob()`
   dispatch on the shared `NativeHandle`. VST3 → FFI; CLAP/internal → `None` /
   `false`. The single shared `Arc<PluginInstance>` (ADR-006/007) is the natural
   home: the command-thread host and the audio-thread wrapper reach the *same*
   `IComponent`.

3. **Host API does the real thing.** `PluginHost::save_device_state` returns a
   `DeviceState::Binary` from FFI (falling back to the `saved_state` cache for
   formats without an FFI path); `load_device_state` pushes the blob via
   `setState`. `device_ids()` lets the save path enumerate live devices.

4. **Project file carries the blobs.** `ProjectFile.plugin_state_blobs:
   HashMap<device_id, base64>` with `#[serde(default)]` (backward compatible — a
   pre-4b save loads with an empty map → previous behaviour). On save the live
   state is harvested from the host before serialisation; on load it is applied
   **after** the graph compiles (the instances must exist first). Base64 (a
   ~40-line self-contained RFC 4648 codec in `state.rs`, round-trip tested) keeps
   large synth chunks compact and adds no dependency.

### Module map

| File | Change |
|---|---|
| `plugin_formats/vst3/ffi.rs` | `HostStream` (`IBStream`) + `IID_IBSTREAM` + `read/write_component_state` |
| `plugin_formats/clap/ffi.rs` | **(4c)** `CLAP_EXT_STATE` + `ClapPluginState`/`ClapIStream`/`ClapOStream` vtables + `ClapStateStream` (`Vec<u8>`-backed) + `read/write_plugin_state` + stream round-trip test |
| `plugin_system/instance.rs` | `save_state_blob` / `load_state_blob` (VST3 dispatch; **4c** adds the CLAP branch) |
| `plugin_system/host.rs` | `save/load_device_state` via FFI; `device_ids()` (format-agnostic — unchanged by 4c) |
| `plugin_system/state.rs` | `encode_base64` / `decode_base64` + tests |
| `project.rs` | `AppProject.plugin_state_blobs` + `ProjectFile.plugin_state_blobs` (serde default); `to_file` / `from_file` |
| `engine_gateway/api.rs` | `snapshot_project` carries the (empty) field |
| `main.rs` | harvest on save; restore after recompile on load |

### CLAP addendum (sub-phase 4c)

CLAP's `clap.state` extension is the direct analogue of VST3 `IComponent::get/setState`.
The difference is the stream shape: VST3 needs a host-implemented `IBStream` over
FUnknown (query_interface/add_ref/release/read/write/seek/tell); CLAP streams are
pure C — a `ctx` back-pointer plus a single `read` (`clap_istream`) or `write`
(`clap_ostream`) function pointer, the same shape the existing `FixedEventList`
already uses. `ClapStateStream` is a boxed `Vec<u8>` buffer whose two embedded
vtables point their `ctx` back at the box; `read_plugin_state`/`write_plugin_state`
fetch the `clap.state` extension via `get_extension`, run `save`/`load`
synchronously, and drop the stream immediately after. A `get_extension` returning
null (plugin without `clap.state`) falls through to `None`/`false` → default
preset, preserving pre-4c behaviour.

## Real-time impact

None on the audio thread. `getState`/`setState` run on the command thread
(project save / open). No allocation, lock, or syscall is added to
`process_block`.

## Threading caveat (residual risk, documented)

VST3 mandates single-threaded plugin access, but these FFI calls are **not**
sequenced against the audio thread's `process()` on the same instance — the
command thread and audio thread both reach the shared `IComponent`. The
operating invariant is that save/load happen while the **transport is stopped**
(explicit project save, project open), where no `process` is in flight for the
instance. A proper between-blocks barrier (or applying `setState` at instantiate
time, before the node enters the snapshot) is deferred — it would require
threading the blob map through the compiler signature, which is out of this
sub-phase's scope (C-7: compiler extended, not rewritten).

## Compatibility

- **Bit-identical offline (C-2):** no DSP change.
- **Backward-compatible projects (C-4):** `#[serde(default)]` → pre-4b saves load
  unchanged; absent blobs mean default presets, as before.
- **Single-component model only:** controller-only state (`IEditController`
  get/setState) is not captured — consistent with the GUI path's existing
  single-component assumption. Processor state (`IComponent`) covers the
  meaningful state for the supported plugins.

## Alternatives rejected

- **JSON byte array instead of base64.** Dependency-free but ~4× the size of a
  multi-MB preset; rejected for file bloat.
- **Adding the `base64` crate.** A 40-line codec avoids a new dependency for a
  trivial need.
- **Applying `setState` at instantiate time** (the race-free option). Correct,
  but requires changing the compiler/`instantiate` signature to carry the blob
  map — deferred to a focused follow-up to keep this sub-phase contained.

## Technical debt added

- **AU / WASM state unimplemented**: they return `None`/`false` and fall back to
  the empty cache. (CLAP closed in 4c.)
- **State save/load not barrier-synchronised** against `process` (see caveat) —
  applies to CLAP identically: `clap.state.save/load` are `[main-thread]` and are
  called on the command thread while the transport is stopped.
- **Controller-only state not persisted** (single-component assumption, VST3).
- **CLAP state not verified against a real plugin in CI** — the round-trip test
  exercises the host stream callbacks only; smoke-testing Surge XT / a CLAP synth
  preset is a manual release-checklist item (V5 of the migration plan).

## Next step

Phase 4 (state) is complete for VST3 + CLAP. The remaining Phase 4 surface is
already shipped (4a single instance). Subsequent work moves to Phase 5
(PortKind + MIDI + automation); see [`docs/active/migration/migration_plan.md`](../migration/migration_plan.md) §5.
A focused follow-up may apply `setState`/`clap.state.load` at instantiate time
(the race-free option deferred in *Alternatives rejected*) once the compiler can
carry the blob map.
