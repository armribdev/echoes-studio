# ADR-035 — The CLAP preset in use, remembered with the project

## Status

**Accepted and implemented — 2026-11-07** (the user approved P1 the same day). Not
yet hand-tested in the app. Reopens one alternative ADR-031 rejected ("preset name stored in the project") and
builds on ADR-010 (plugin state as an opaque blob), ADR-015 (sandbox), ADR-017
(addressing by `instance_key`) and ADR-002 (event-sourcing scope).

> **Governed surface touched:** state-management boundary (project file format,
> ADR-002). It needs an ADR because the project file gains a field that is not a
> plugin blob and not an event.

## Context

The preset picker (ADR-031) shows the preset a plugin is using. For VST3 the host
reads it from the controller, so it survives anything. For CLAP the host cannot ask,
so (2026-11-07) it keeps what the plugin reports through `clap.preset-load.loaded`
(`ClapHostContext.loaded_preset`, fetched from the bridge by `Op::LoadedPreset` for a
sandboxed plugin) and marks that preset in the listing. That is live state only: after
a project is reopened the plugin restores its sound from its blob and **does not
report a preset**, so the select is empty until the next load.

What the CLAP spec offers (headers of `free-audio/clap`, read 2026-11-07):

- `clap_host_preset_load.loaded(host, location_kind, location, load_key)`: "Informs
  the host that the following preset has been loaded. This contributes to keep in
  sync the host preset browser and plugin preset browser." Nothing says the host
  keeps it, saves it, or that a plugin calls it again when its state is restored.
- No call asks a plugin which preset it is using.
- `clap.state-context` only tags a save or load as preset / duplicate / project. The
  plugin may store whatever it wants in its own state, but the host cannot read it:
  the blob is opaque (ADR-010).

So the preset's identity cannot be recovered from the plugin after a reload. I could
not find out how other hosts (Bitwig, Reaper) handle it; the spec gives no hint.

What already exists: `plugin_states` in the project file, a base64 blob per
`instance_key` (ADR-017), collected from the live instances at save
(`collect_plugin_states`) and staged at load (`set_pending_state_blobs`) so it is
applied at instantiation, before the node enters a snapshot.

## Decision

**P1 — keep the last reported preset per placement in the project file, next to
`plugin_states`.** A display hint, not a second source of truth: the blob stays the
only thing that defines the sound.

1. **File.** An optional `plugin_presets` map in `ProjectFile`, keyed by
   `instance_key`:
   ```jsonc
   "plugin_presets": {
     "<effect.id>": { "device_id": "<type>", "location_kind": 1,
                      "location": null, "load_key": "warm-pad" }
   }
   ```
   `#[serde(default, skip_serializing_if = "…is_empty")]`, so a project without
   it is written and read exactly as before. `PROJECT_FILE_VERSION` stays 2 (an
   additive optional field; `ProjectFile` does not deny unknown fields, so an older
   build ignores it).
2. **Save.** `PluginHost::collect_loaded_presets()`, beside `collect_plugin_states`:
   one entry per placement whose `PluginInstance::clap_loaded_preset()` is `Some`.
   A placement that is not instantiated (missing plugin) keeps its entry, as its
   state blob is kept (ADR-010, R-4).
3. **Load.** Staged with the state blobs and given to the instance at
   instantiation as `restored_preset`, on the host side of the instance, so the
   same code serves an in-process and a sandboxed plugin (no new bridge op). The
   entry is dropped when its `device_id` is not the placement's plugin (a slot that
   now holds another plugin, as in ADR-017).
4. **Reading.** `clap_loaded_preset()` = what the plugin reported live, else
   `restored_preset`. A later report always wins.
5. **Echoes' own loads count too.** When `load_plugin_preset` succeeds, the host
   records the request it made, even if the plugin never calls `loaded`. This also
   covers plugins that stay silent when the user picks from Echoes' own picker; a
   preset picked in such a plugin's own window is still unknown.
6. **Nothing else changes.** No command, no event, no UI element, no frontend
   state: `PluginPreset.active` already carries it to `PresetPicker.svelte`.

Boundaries stated plainly:

- **Not event-sourced, not undoable.** Same as the blob and the preset load itself
  (ADR-002, ADR-031). Marking the project unsaved already happens on `loaded`.
- **Never used to load anything.** The stored location and key are only compared
  with the plugin's current listing; the host still loads only what the last
  listing offered (ADR-031). A project file is user data, so its strings are
  length-bounded on read (the bridge wire already caps a text at 64 KiB; the file
  caps at 4 096 characters and drops the entry beyond that).
- **Can be stale.** If the user moves a knob after loading, the label stays, as the
  VST3 program label does. If the preset file is deleted or renamed, nothing is
  marked. Both are harmless.

## Real-time impact

None. Collection and staging run on the command thread beside the state blobs; the
instance field is read on the command thread. Nothing is added to the audio
callback or to the bridge's block message (`rt_audio_rules.md`).

## Compatibility and invariants

- **C-2.** No engine change; the offline render never reads it. Golden hashes stay.
- **C-4.** Additive optional field; old projects load as before. A project saved by
  the new build and opened by an old one loses only the label.
- **C-6 / invariant 4.** One instance per placement; no second hosting.
- **Invariant 5 (UI/engine).** The state is owned by Rust; the frontend mirrors the
  listing it already fetches.
- **ADR-010.** The blob is untouched; the hint lives beside it, not inside it.
- **ADR-015.** `Op::LoadedPreset` (2026-11-07) already exists; no protocol change.

## Alternatives considered

- **P0 — do nothing.** Cheapest, and exactly today's behaviour: right while the
  session lasts, empty after a reopen. Kept as the fallback if the user prefers no
  file change.
- **P2 — put the identity inside the plugin blob.** Rejected: the blob is opaque and
  handed to the plugin's `setState`; wrapping it breaks ADR-010 and every plugin
  would have to ignore our header.
- **P3 — recognise the preset by comparing states.** Rejected: it means loading each
  preset into a throwaway instance to compare, a second hosted instance (invariant
  4, C-6), and plugin states are not guaranteed byte-stable.
- **P4 — ask the plugin through `state-context`.** Not possible: the host cannot
  read what a plugin stores.
- **P5 — keep it in the frontend (`ui_layout`) or in `localStorage`.** Rejected:
  invariant 5 and ADR-005; a layout field is UI state, not engine state.
- **P6 — read a plugin-specific parameter that names the preset.** Rejected: no
  convention exists; it would be one-plugin code.

## Consequences

- The select shows the right preset after a reopen for CLAP plugins that reported
  one (or that were loaded from Echoes), in process or sandboxed.
- The project file grows by a few lines per CLAP placement that has a known preset.
- This is the first project field that describes a plugin's state without being the
  state; the rule to keep is "a hint, compared, never trusted".
- A plugin that never calls `loaded`, and whose preset the user picks only in its own
  window, stays unknown. No host-side fix exists.

## Implementation plan (if approved)

1. `ProjectFile.plugin_presets` + `AppProject.plugin_presets` + round-trip tests: a
   project without the field loads unchanged; with it, it survives save, load, save;
   an over-long entry is dropped; a placement of another `device_id` is dropped.
2. `PluginHost::{collect_loaded_presets, set_pending_loaded_presets}`,
   `PluginInstance::restored_preset`, the record in `PluginInstance::load_preset`;
   host tests: restored then reported (the report wins), a missing plugin keeps its
   entry, sandbox and in-process alike (`sandbox_tests`).
3. `main.rs` save / load wiring beside the state blobs.
4. Docs: `architecture/state-management.md` (project file), `plugins/plugin-wrapper-system.md`
   (Preset picker), ADR-031 (note), `000-index.md`, `.claude/state_index.md`.

## Implementation status

Done 2026-11-07, in one pass. One difference from the text above: the remembered
preset is held by `PluginHost` (`stored_presets`, keyed by `instance_key`), not by a
`restored_preset` field of the instance. It serves the same purpose, in process and
sandboxed, and is simpler: a live report wins (`loaded_preset_of`), the entry is
kept only for the plugin type it was saved for, `insert_record` carries a replaced
instance's live preset into it (so a re-instantiation does not bring back an older
label), and `release_inactive` prunes it with the other per-placement maps.

- File: `ProjectFile.plugin_presets` / `AppProject.plugin_presets`
  (`BTreeMap<instance_key, StoredPreset>`), `StoredPreset` in
  `plugin_system/presets.rs`.
- Save: `PluginHost::collect_loaded_presets`, called beside `collect_plugin_states`
  in `main.rs`. Load: `set_stored_presets` in `install_loaded_project` (texts past
  4 096 characters are dropped).
- Echoes' own loads: `ClapHostContext::note_loaded_preset` after a successful
  `PluginInstance::load_preset` and in the bridge's `ClapAdapter::load_preset`.
- Proof: `project::tests::plugin_presets_round_trip_and_an_older_file_has_none`;
  `sandbox_tests` (a loaded preset is the one in use, in process and sandboxed, also
  when the plugin does not report a file preset; a remembered preset is shown until
  the plugin reports another; another plugin type or an over-long entry is
  ignored). Golden hashes unchanged (no engine change).
- Not covered by a test: the whole save-then-reopen path through `main.rs` (the
  pieces are tested; the wiring is not).
- Hand test 2026-11-07 (Surge XT and Zebralette 3, CLAP, in process): both ask the
  host for `clap.preset-load/2`, which Echoes serves, yet neither calls `loaded` when
  the preset is changed in the plugin's own window. The picker therefore follows
  only the presets loaded from Echoes for them; the live path is proven by the tests
  with the test plugin, not yet by a real plugin. A preset picked in such a window
  leaves the last label in place (see the open points).

## Open points

- Should the label be cleared as soon as the plugin's state no longer matches the
  preset? Recommended **no**: it needs a state comparison, which is P3 again.
- Whether the picker should say "modified" after a knob moves. Not proposed
  (minimal UI).

Sources: [`clap/ext/preset-load.h`](https://github.com/free-audio/clap/blob/main/include/clap/ext/preset-load.h),
[`clap/ext/state-context.h`](https://github.com/free-audio/clap/blob/main/include/clap/ext/state-context.h).
