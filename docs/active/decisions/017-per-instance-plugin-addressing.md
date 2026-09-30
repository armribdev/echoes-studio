# ADR-017 — Per-instance plugin addressing & state persistence

## Status

**Accepted — implemented 2026-06-03.** Param routing (`api.rs` →
`push_plugin_param` by `instance_key`), GUI-open (`instance_by_key`,
`open_*_editor(effect_id)`), and the project file-format migration
(`plugin_states` keyed by `instance_key`, legacy `plugin_state_blobs` read on
load) are in place. Fully closes **point 2** of
[`audits/engine-architecture-audit-2026-06-03.md`](../audits/engine-architecture-audit-2026-06-03.md).
Builds on the per-instance records keying landed in the
[ADR-007 update (2026-06-03)](007-plugin-instance-arc.md#update--2026-06-03-records-keyed-by-instance_key)
and complements [ADR-016](016-plugin-instance-reuse.md) (instance reuse, accepted).

> **Governed surfaces touched:** State-management boundary (project file format,
> ADR-002) **and** UI/engine separation (IPC parameter protocol, ADR-005). This
> ADR exists because both are "never change silently" constraints.

---

## Context

Step 2 (ADR-007 update) re-keyed `PluginHost.records` by the per-placement
`instance_key`, so the host map no longer collapses two instances of the same
plugin type. But the **addressing that reaches the host is still by `device_id`
(plugin type)** on three surfaces, so the user-visible R-2 bug persists:

| Surface             | Today                                        | File                                                                                           |
| ------------------- | -------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Param push (UI→DSP) | `HotEffectUpdate::Plugin(device_id, params)` | [`engine_gateway/api.rs`](../../../src-tauri/src/engine_gateway/api.rs)                        |
| GUI editor open     | `host.instance(plugin_id)`                   | [`main.rs`](../../../src-tauri/src/main.rs)                                                    |
| State save/restore  | `plugin_state_blobs: HashMap<device_id, …>`  | [`main.rs`](../../../src-tauri/src/main.rs), [`project.rs`](../../../src-tauri/src/project.rs) |

The serialized format is the hard part: `ProjectFile.plugin_state_blobs:
HashMap<String /*device_id*/, String>` ([`project.rs`](../../../src-tauri/src/project.rs))
means **two instances of the same plugin type cannot persist distinct state** —
their blobs collide in the file. The runtime
`PluginHost::record_by_device_id` bridge (ADR-007 update) keeps the common
one-instance-per-type case correct but is explicitly ambiguous for duplicates.

The stable, persistent per-instance identity already exists: `TrackEffect.id`
(the project `effect.id`), which is what `NodeKind::Plugin.instance_key` already
carries.

---

## Decision

Adopt `instance_key` (= project `effect.id`; `instrument:{track_id}` for a track
instrument) as the **addressing and persistence unit** for plugins.

1. **IPC parameter protocol.** `HotEffectUpdate::Plugin` carries the
   `instance_key` (the effect id the frontend already owns). `push_plugin_param`
   addresses by `instance_key` via a direct `records.get`, retiring the ambiguous
   `record_by_device_id` path for params.
2. **GUI editor open.** The editor target is addressed by `instance_key` (which
   effect slot), not plugin type.
3. **Project file format (the C-4 migration).** Re-key persisted state by
   instance, in a self-describing list:
   ```jsonc
   "plugin_states": [
     { "instance_key": "<effect.id>", "device_id": "<type>", "data": "<base64>" }
   ]
   ```
   carrying `device_id` so a load can validate the blob still targets the same
   plugin type. The legacy `plugin_state_blobs: { <device_id>: <b64> }` field is
   **kept readable** (never deleted): on load, legacy entries map to the single
   matching instance of that type; on save, only the new `plugin_states` list is
   written.
4. **Restore staging.** `PluginHost::set_pending_state_blobs` keys by
   `instance_key`; `instantiate` applies the blob for its own `instance_key`
   (still at instantiation, before the node enters a snapshot — ADR-010 race-free
   restore preserved).

`record_by_device_id` remains only for genuinely type-scoped queries and as the
legacy-load mapper.

### Scope of edit (cross-boundary — the reason this needs an ADR)

- **Frontend:** include the effect id in the plugin param IPC payload and the
  GUI-open call (small, additive — the id is already in hand).
- [`engine_gateway/api.rs`](../../../src-tauri/src/engine_gateway/api.rs),
  [`main.rs`](../../../src-tauri/src/main.rs): address by `instance_key`.
- [`plugin_system/host.rs`](../../../src-tauri/src/plugin_system/host.rs):
  `push_plugin_param` / staging keyed by `instance_key`; `save_device_state` and
  the GUI accessor take `instance_key`.
- [`project.rs`](../../../src-tauri/src/project.rs): new `plugin_states` list +
  backward-compatible read of legacy `plugin_state_blobs`.

---

## Real-time impact

None. Every changed path runs on the command / main thread (param push, save,
load, GUI open). The audio `process` path, the SPSC param transport (ADR-008),
and determinism are untouched.

---

## Compatibility (migration constraints)

- **C-4 (saved projects) — central concern.** Old projects **must** load: the
  legacy `plugin_state_blobs` field is retained as a read path and mapped to the
  matching instance. New saves use the per-instance list. A project saved by the
  new build and opened by an old build loses per-instance state (old build reads
  only the legacy field) — acceptable forward-incompat, documented in the PR.
- **C-2 (bit-identical offline).** Unaffected — no DSP change.
- **C-5 (lock-free cmd↔audio).** Unaffected — still the SPSC `ParamChange` queue;
  only the host-side _addressing_ of which queue changes.
- **ADR-002 / ADR-005.** Honoured: the project remains the source of truth; the
  frontend sends an identifier it already owns; no mutable engine state is
  exposed.

---

## Alternatives considered

- **Use the runtime `PluginInstance::instance_id` (u64).** Rejected: not stable
  across sessions and not meaningfully serializable; `effect.id` is the
  persistent project identity.
- **Keep `device_id` keying + a per-type disambiguation index.** Rejected: does
  not solve the **file-format** collision — two instances still can't persist
  distinct state.
- **Map `HashMap<instance_key, bytes>` instead of a self-describing list.**
  Viable and smaller, but loses the `device_id` cross-check that catches a stale
  blob whose effect slot now hosts a different plugin type.
- **Do this inside ADR-016.** Rejected: ADR-016 is engine-internal and
  independently shippable; this ADR crosses the file-format + IPC boundary and
  carries the C-4 migration risk.

---

## Consequences

- **Positive.** Same plugin on two tracks gets independent parameters, editor,
  and persisted state — the full closure of audit point 2. Removes the
  `record_by_device_id` ambiguity from the param hot path.
- **Risk.** The file-format migration (C-4). Mitigation: keep the legacy read
  path; add round-trip tests (legacy file → load → save → new shape; duplicate
  instances → distinct blobs survive a save/load cycle).
- **Reversibility.** The IPC/host changes are local. The format change is the
  sticky part — hence the retained legacy read path and the explicit
  forward-incompat note.

## Follow-ups

- Sequenced after **ADR-016** (reuse, accepted) so the stable instance identity
  is already load-bearing in the host.
- Frontend coordination: the param IPC payload and GUI-open call gain the effect
  id.
