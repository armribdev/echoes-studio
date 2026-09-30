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
branch were added (see _Module map_ and _CLAP addendum_ below). The host API,
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
   home: the command-thread host and the audio-thread wrapper reach the _same_
   `IComponent`.

3. **Host API does the real thing.** `PluginHost::save_device_state` returns a
   `DeviceState::Binary` from FFI (falling back to the `saved_state` cache for
   formats without an FFI path). `device_ids()` lets the save path enumerate live
   devices. **State restore is no longer a post-compile call** (see _Restore is
   race-free_ below): the project-load path stages blobs via
   `set_pending_state_blobs`, and `instantiate` applies them to each instance
   before it enters a snapshot.

4. **Project file carries the blobs.** `ProjectFile.plugin_state_blobs:
HashMap<device_id, base64>` with `#[serde(default)]` (backward compatible — a
   pre-4b save loads with an empty map → previous behaviour). On save the live
   state is harvested from the host before serialisation; on load it is **staged
   on the host and applied during instantiation** (before any node is published
   to the audio thread). _Amended 2026-09-23:_ a placement whose plugin fails to
   load keeps its blob in the host (`unavailable_states`) and is saved
   unchanged; see `plugins/plugin-system.md` §Per-instance persistence. Base64 (a ~40-line self-contained RFC 4648 codec in
   `state.rs`, round-trip tested) keeps large synth chunks compact and adds no
   dependency.

### Module map

| File                         | Change                                                                                                                                                                        |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `plugin_formats/vst3/ffi.rs` | `HostStream` (`IBStream`) + `IID_IBSTREAM` + `read/write_component_state`                                                                                                     |
| `plugin_formats/clap/ffi.rs` | **(4c)** `CLAP_EXT_STATE` + `ClapPluginState`/`ClapIStream`/`ClapOStream` vtables + `ClapStateStream` (`Vec<u8>`-backed) + `read/write_plugin_state` + stream round-trip test |
| `plugin_system/instance.rs`  | `save_state_blob` / `load_state_blob` (VST3 dispatch; **4c** adds the CLAP branch)                                                                                            |
| `plugin_system/host.rs`      | `save/load_device_state` via FFI; `device_ids()` (format-agnostic — unchanged by 4c)                                                                                          |
| `plugin_system/state.rs`     | `encode_base64` / `decode_base64` + tests                                                                                                                                     |
| `project.rs`                 | `AppProject.plugin_state_blobs` + `ProjectFile.plugin_state_blobs` (serde default); `to_file` / `from_file`                                                                   |
| `engine_gateway/api.rs`      | `snapshot_project` carries the (empty) field                                                                                                                                  |
| `main.rs`                    | harvest on save; restore after recompile on load                                                                                                                              |

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

## Threading caveat — restore race-free; save fenced on the project-save path

VST3 mandates single-threaded plugin access. The two FFI directions are no
longer equivalent in their exposure:

**Restore (`setState`) — race-free (implemented 2026-06-03).** State is no longer
pushed onto a live, snapshotted instance after the compile. The project-load path
stages the blobs (`PluginHost::set_pending_state_blobs`) and `instantiate` applies
each blob to its freshly-created instance **before the node enters any snapshot**
— so no `process()` can be in flight for that instance when `setState` runs. This
closes the load-direction race for **all** triggers, not just transport-stopped
ones.

**Restore is not load-only (R-4, 2026-06-04).** The restore trigger was widened
from "project load only" to "every valid instantiation". `instantiate` now routes
through `PluginHost::restore_state_for`, which after the staged-blob case carries
the _previous_ instance's state into the rebuilt one when the placement still
hosts the same plugin type (`PluginInstance::restore_from`). ADR-016 instance
reuse already preserves state on the common knob-turn recompile; this covers the
rebuild paths reuse declines (terminal instance, non-reuse-safe format) so state
survives a controlled re-instantiation, not only a reload — and it also makes the
CLAP-**instrument** path restore staged state at load (previously dropped). The
applied blob is to the brand-new instance (pre-snapshot — race-free); reading the
previous instance's live state is fenced by its FFI gate (see save caveat below,
2026-06-05). Locked by
`host::tests::state_survives_controlled_reinstantiation` and
`reinstantiation_does_not_carry_state_across_types`. (It also turned out **not** to require threading the blob map through the
compiler signature, as this section previously assumed: the staging map lives on
the host, which the compiler already holds `&mut`. C-7 untouched — no compiler
signature change.) Locked by `host::tests::staged_state_is_applied_at_instantiation`
and `cleared_staged_state_is_not_applied`.

**Save (`getState`) — live reads now fenced (2026-06-05).** The
between-blocks barrier foreshadowed below has landed as a per-instance,
non-blocking **FFI gate** (`PluginInstance::{rt_try_enter, offrt_try_acquire}`).
The audio thread claims the gate around its `process()` FFI (skipping that block
with a transparent bypass if an off-RT caller holds it — it never waits, so no
priority inversion); off-RT harvests claim the same gate before each live
`getState`, falling back to the cached blob if a block is in flight and the gate
cannot be claimed within a short budget. (2026-09-24: the CLAP instrument
adapter now claims it too; before, a save could race a playing CLAP synth.)
**Every** current live-FFI reader is
routed through it: the project-save harvest (`collect_plugin_states`), the R-4
re-instantiation read of the previous instance (`PluginInstance::restore_from`),
`save_device_state`, and the wrappers' `prepare`-time introspection (VST3
`getLatencySamples`; CLAP `clap.latency` + param enumeration), which on a reuse
recompile would otherwise run while the outgoing graph still processes the
instance. This makes the save direction safe **even during playback** — closing
the data race for autosave-during-playback and for re-instantiation while playing.

> **Note — future callers.** The save direction is fully fenced today. Any new
> live-FFI caller (e.g. a GUI `getState`/`setState`) must hold the gate via
> `offrt_try_acquire` — it is the reusable primitive for this.

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
- **Applying `setState` at instantiate time** (the race-free option). Was
  deferred here on the assumption it needed a compiler/`instantiate` signature
  change. **Implemented 2026-06-03** via a host-side staging map instead (no
  signature change) — see the threading caveat. Restore is now race-free.

## Technical debt added

- **AU / WASM state unimplemented**: they return `None`/`false` and fall back to
  the empty cache. (CLAP closed in 4c.)
- **State _save_** live reads are all fenced by the per-instance FFI gate
  (`collect_plugin_states`, `restore_from`'s previous-instance read,
  `save_device_state`, and the wrappers' `prepare` introspection), VST3 and CLAP
  alike (`clap.state.save` is `[main-thread]`) — safe during playback. _Restore_
  is race-free (applied at instantiation). No save-direction residual remains;
  future live-FFI callers (GUI) must reuse the gate.
- **Controller-only state not persisted** (single-component assumption, VST3).
- **CLAP state not verified against a real plugin in CI** — the round-trip test
  exercises the host stream callbacks only; smoke-testing Surge XT / a CLAP synth
  preset is a manual release-checklist item (V5 of the migration plan).

## Next step

Phase 4 (state) is complete for VST3 + CLAP. The remaining Phase 4 surface is
already shipped (4a single instance). Subsequent work moves to Phase 5
(PortKind + MIDI + automation); see [`docs/active/migration/migration_plan.md`](../migration/migration_plan.md) §5.
State **restore** at instantiate (the race-free option) is now implemented, and
the **save** direction's live reads are all fenced via the per-instance FFI gate
(2026-06-05) — `collect_plugin_states`, `restore_from`, `save_device_state`, and
the wrappers' `prepare` introspection — safe during playback. No save-direction
follow-up remains; future live-FFI callers (GUI editor get/setState) must hold
the gate.
