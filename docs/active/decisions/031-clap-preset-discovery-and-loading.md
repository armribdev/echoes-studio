# ADR-031 — CLAP preset discovery and loading

## Status

**Accepted — 2026-10-15.** The user approved the recommended options D1, L1 and
U1. Implementation is in progress, step by step (see the plan); L2 stays the
fallback if the audible bypass of L1 is not acceptable on real plugins. Builds on
ADR-010 (plugin state as an opaque blob, the FFI gate), ADR-013 (CLAP host
extensions), ADR-015 (sandbox), ADR-021 (value-modulated parameters) and ADR-023
(library browser scope). The host side of `clap.preset-load` (`loaded` / `on_error`)
already shipped on 2026-10-14 without an ADR; this one covers the rest.

## Context

A CLAP plugin can tell the host which presets it ships and where its users keep
theirs (`clap.preset-discovery-factory/2`), and can load one on request
(`clap.preset-load/2`, plugin side `from_location`). Today the user can only pick a
preset inside the plugin's own window. Checked against the spec headers
(`clap/factory/preset-discovery.h`, `clap/ext/preset-load.h`, CLAP main):

- The factory comes from the **library entry**
  (`clap_entry.get_factory("clap.preset-discovery-factory/2")`, compat id
  `…/draft-2`, which the header says may be removed in 2026), not from a plugin
  instance. `count` / `get_descriptor` / `create(factory, indexer, provider_id)`
  are thread-safe; the provider, the indexer and the metadata receiver are **not**.
- Flow: `create` → `provider.init` (the provider calls the host's `declare_filetype`
  / `declare_location` / `declare_soundpack`) → the host crawls the declared
  locations and calls `provider.get_metadata(kind, location, receiver)` per file.
  A location is `FILE` (a file or a directory) or `PLUGIN` (presets bundled in the
  DLL; the location is null). `get_metadata` "must be fast", indexing is
  non-interactive, and background threads are encouraged.
- The receiver gets, per preset: `begin_preset(name, load_key)`, `add_plugin_id`
  (which plugins can load it), `set_flags` (factory / user / demo / favorite),
  `add_creator`, `set_description`, `add_feature`, `set_soundpack_id`, timestamps.
  `load_key` is opaque and means something only to the plugin.
- `from_location(plugin, kind, location, load_key)` is `[main-thread]`. The plugin
  answers through the host's `loaded` / `on_error`.
- The header mentions `set_invalidation_watch_file` and a
  `clap_plugin_invalidation_factory` for change detection, but the indexer struct
  in the header has no such function. Not used here.

What Echoes already has and this must respect:

- Plugin state is an opaque blob, saved with the project and restored at
  instantiation (ADR-010). It is **not event-sourced**: a change made inside a
  plugin window is not undoable, only flagged as unsaved. A preset load is the same
  kind of change.
- **Any new live-FFI caller must hold the instance FFI gate** (ADR-010, "Note —
  future callers"). While an off-RT caller holds it, the audio thread skips that
  plugin's block with a transparent bypass (an instrument is silent). Exceptions so
  far (`clap.gui`, `on_main_thread`, `on_timer`) were decided with the user.
- A hosted plugin is loaded once per DLL (`ClapLibrary::load`, `init` once); the
  scan already crawls every DLL in-process.
- Value-modulated, automatable-only parameters (ADR-021 amendment) keep a **base**
  on the host that follows plugin-window drags and is written back to the plugin
  before every state save. A preset load changes those values behind the host's
  back.
- ADR-023 freezes "Devices ∉ library": the library browser holds musical content,
  not plugin browsing.

## Decision

Two features, shipped one after the other: **listing** (read-only), then
**loading** (changes a plugin's state), then a **picker** in the plugin's panel.

### D — Discovery: on demand, per plugin, in the background

**Recommended — D1.** When the user shows a CLAP plugin's panel, a Tauri command
`list_plugin_presets(plugin_id)` runs the whole cycle (`create`, `init`, crawl,
`get_metadata`, `destroy`) on **one short-lived worker thread** and returns a
list. Nothing happens at app start or at scan time.

- Entries: `{ name, load_key, location_kind, location, provider_id, flags,
creator, description, features, soundpack }`. Only entries that declare the
  plugin's own id (`add_plugin_id`, abi `clap`) are returned; an entry without a
  plugin id is dropped (the spec leaves its meaning open).
- Crawl: `FILE` locations are walked for the declared extensions (bounded depth
  and a bounded number of files); `PLUGIN` locations are read once with a null
  location. A provider that fails, returns false or exceeds the bounds yields a
  partial list, never an error for the whole plugin; `on_error` from the receiver
  is logged.
- The result is an in-memory cache per plugin id, replaced by the next listing. No
  file watcher, nothing persisted, no `.echoes` change.
- Crash safety: the provider is third-party code run in the app process, like the
  scan's probe. The cycle is wrapped in the same SEH guard (`crash_guard`) and the
  worker never touches the audio engine lock; the registry path is read, the lock
  released, then the cycle runs. A crashed plugin is reported as "no presets".
- Not on the audio thread, no allocation concern there: nothing in
  `audio_graph/` changes (ADR-003). New FFI lives in `crates/clap_ffi`
  (`preset_discovery.rs`), orchestration in `plugin_system/presets.rs`.

_D2 — index every plugin at scan time and keep it in the registry._ Rejected for
now: one crawl per plugin multiplies scan time (a synth can ship thousands of
files), and the index is stale as soon as the user saves a preset.
_D3 — persisted index with a file watcher._ The right shape if presets later join
the library browser; far larger than what a picker needs.

### L — Loading: a command that calls `from_location` under the FFI gate

**Recommended — L1.** Tauri command `load_plugin_preset(instance_key, location_kind,
location, load_key)`:

1. The request must match an entry of that plugin's **last listing** (the host does
   not load an arbitrary path sent by the UI). Otherwise it is refused.
2. The command claims the instance FFI gate (`offrt_try_acquire`, a budget longer
   than the 3 ms state harvest, e.g. 50 ms; "plugin busy" on a timeout), calls
   `from_location`, and releases the gate.
3. On success the host marks the project unsaved itself (a plugin that forgets to
   call `loaded` must not leave the project "saved"), wakes a sleeping plugin
   (`request_process`), re-reads the actual values of its value-modulated
   parameters into their bases (see below), and emits the existing
   `plugin-params-changed` event so the knobs and automation texts follow.
4. A refusal (`from_location` returns false, or `on_error` was called during the
   call) is reported to the user as an error notification; nothing else changes.

Consequences stated plainly:

- **Not undoable**, like any edit in the plugin window. The loaded sound is part of
  the plugin state and is saved with the project (ADR-010). No project field stores
  the preset's name: the state blob is the truth.
- **Audible cost.** While the gate is held the plugin is bypassed (silent for an
  instrument). A preset that loads samples can hold it for a noticeable time. This
  is the price of following ADR-010 and has to be checked by ear.
- **Value-modulated bases.** Without a refresh, a base that predates the preset
  would be written back over the preset's value at the next save (ADR-021
  amendment). The load therefore re-reads the plugin's values for those parameters
  while it still holds the gate. This is a requirement with its own test.
- **Automation lanes** are absolute (ADR-025) and override a preset's values for
  the parameters they target at the next block, as they do for any value change.

_L2 — call `from_location` without the gate_, as the plugin's own window does when
the user picks a preset there (CLAP allows main-thread calls beside `process`, and
`clap.gui` / `on_timer` already run without the gate). No bypass while loading, but
it breaks the ADR-010 rule and needs the user's explicit agreement, as the timer
decision did. Hung or racy plugins are then not fenced.
_L3 — load the preset into a throwaway instance, save its state, restore the blob
through a recompile._ Rejected: a second hosted instance (invariant 4, C-6), a
heavy plugin loads twice, and the recompile cuts tails.

### U — Where the user sees it

**Recommended — U1.** A labelled "Preset" select in the plugin's own panel, built
like `SidechainPicker.svelte`: in the instrument panel for a CLAP instrument and in
the expanded plugin effect for an effect. It is an **action menu**, not a state
display: choosing an entry loads it, then the select returns to its empty first
option (the current preset is not known to the host and is not stored). Entries are
grouped Factory / User by `flags`, then listed by name. **The row is absent when
the plugin has no presets** — no badge, no placeholder, no status text. It is
listed when the panel is shown and again when the select gets focus (a native
select cannot refresh while open). `cursor: default` on disabled, `:focus-visible`
only, per the UI conventions. A mock-up is shown before integration.

_U2 — a "Presets" source in the library browser._ Needs an ADR-023 amendment
("Devices ∉ library"), a per-plugin filter and a persisted index (D3). Larger and
changes a frozen scope; kept as the later direction.
_U3 — no UI._ Leaves the feature unusable.

### Sandbox

- **Listing stays in the app process**, like the scan: the DLL is already loaded
  there (`ClapLibrary`), and the provider is not tied to an instance. A user who
  isolates a plugin still has it scanned in-process today; this does not change
  that. Isolating discovery too (a `--discover` mode of the bridge) is deferred.
- **Loading** goes through one new control op, `Op::PresetLoad = 11`
  (`kind:u32`, then `location` and `load_key` as length-prefixed UTF-8; reply empty
  or an error text). The bridge's main thread calls `from_location`, as it does for
  `LoadState`. Shared memory and `BRIDGE_PROTOCOL_VERSION` unchanged, like
  `Op::ParamText` / `Op::NoteNames`.

## Real-time impact

None on the audio thread. Discovery runs on a worker thread with no engine lock;
loading holds the FFI gate, which only ever makes the audio thread skip a block
(ADR-010). The host's `loaded` / `on_error` stay one atomic store / one log line.
No allocation, lock, `drop`, I/O or Tauri call is added to any audio-callback
path (`rt_audio_rules.md`).

## Compatibility and invariants

- **C-2.** No engine, compiler or scheduler change. Existing golden hashes stay
  unchanged; the offline render never lists or loads presets. A new `clap_plugins`
  harness check proves that a project whose state came from a preset load renders
  like one restored from the same blob.
- **C-4.** No project format change. Older `.echoes` load as before.
- **C-6 / invariant 4.** One `Arc<PluginInstance>` per device; no second instance.
- **Invariant 5 (UI/engine).** The UI calls two commands and mirrors the result; it
  keeps no authoritative state. Listing is a read-only query (like
  `get_plugin_note_names`); loading changes plugin state, which ADR-002/010 keep
  outside the event log.
- **Threading.** The worker thread that runs discovery is new in kind but not in
  role: Tauri commands already call plugin FFI (`get_plugin_note_names`, state
  harvest). It owns no engine state and exits when the cycle ends.
- **Plugin boundary (invariant 3).** `audio_graph/` imports nothing new.
- **Hard constraints** (RT, threading, graph, plugin boundary, state boundaries):
  none is changed. The ADR exists because of the new FFI surface, the new bridge
  op, the use of the ADR-010 gate by a long-running call, and the ADR-023 scope
  note.

## Implementation plan (each step shown to the user before the next)

1. **Listing.** `crates/clap_ffi/src/preset_discovery.rs` (indexer, receiver,
   factory structs; layouts checked against the headers, including
   `clap_universal_plugin_id` and `clap_timestamp`, which this ADR has not
   re-read), `plugin_system/presets.rs`, the `list_plugin_presets` command.
   Test plugin: a `clap.preset-discovery-factory/2` with one provider, one `PLUGIN`
   location (two bundled presets) and one `FILE` location in a directory given by
   an environment variable (the test writes the file). Real FFI tests in
   `clap_ffi`, `sandbox_tests.rs` (in-process and sandboxed plugin) and a crash
   test (provider that faults).
2. **Loading.** Plugin-side `clap_plugin_preset_load` in `clap_ffi`, the command,
   `Op::PresetLoad`, the base refresh, the harness check. Test plugin: the preset
   sets `Gain`; assertions on the parameter value, the saved blob, the unsaved flag
   (in `bridge.rs`, whose process owns the flag), a sleeping plugin woken, and the
   base of a value-modulated parameter not reverting.
3. **Picker.** The select row, a frontend store/test, `npm run check`.

Each step ends with the docs of `change_impact_mapping.md`
(`plugins/plugin-wrapper-system.md`, `threading/realtime-safety.md`), ADR-013 /
ADR-015 amendments, and `state_index.md`.

## Implementation status

- **Step 1 — listing (2026-10-15): done**, as decided; see
  `../plugins/plugin-wrapper-system.md` (Host preset discovery). Two details the
  text above left open: a declared `FILE` path that does not exist is skipped (a
  missing user directory is normal), and a preset that names no plugin is dropped.
  Not tried with real plugins.
- **Step 2 — loading (2026-10-15): done**, L1 as decided; see
  `../plugins/plugin-wrapper-system.md` (Host preset loading). Details the text
  left open: the gate budget is 50 ms; the load request is checked against the
  plugin's last listing, kept per CLAP plugin id in `PluginHost`; the plugin's own
  `on_error` message is returned to the caller; the host marks the project
  unsaved itself; the C-2 check is a sandbox test (state saved after a preset
  renders like the preset) rather than a harness project, since no engine code
  changed. Not tried with real plugins: the audible bypass of L1 is still to be
  judged by ear (L2 remains the fallback).
- **Step 3 — picker (2026-10-15): done** as the mock-up shown to the user
  (`PresetPicker.svelte`; see `../plugins/plugin-wrapper-system.md`, Preset
  picker). Not hand-tested; nothing about the preset is stored or displayed beyond
  the menu itself.

## Alternatives considered

See D2/D3, L2/L3 and U2/U3 above. Also rejected:

- **VST3 / AU presets** (program lists, `IUnitInfo`): other formats, other
  mechanisms; out of scope. _VST3 was taken in by the amendment of 2026-10-22
  below; AU stays out of scope._
- **Preset name stored in the project**: a second source of truth next to the
  state blob; the host cannot know the current preset after the user edits it.
- **Letting the UI load any file**: the host loads only what a provider listed.

## Consequences

- The user picks a preset of a CLAP plugin without opening its window, and the
  project is saved with the sound.
- Loading is not undoable and may briefly silence the plugin.
- The listing is only as good as each plugin's provider; many plugins ship none,
  and the row is then absent.
- Presets in the library browser remain possible later (U2/D3) without redoing the
  FFI work.

## Open points

- Whether L1's audible bypass is acceptable on real plugins (Vital, Surge XT) —
  to be checked by ear after step 2; L2 is the fallback if not.
- Whether real providers meet the spec's "fast `get_metadata`" on large libraries;
  the bounds in D1 are a guess until measured.

## Amendment — 2026-10-22: VST3 presets

VST3 plugins use the same two commands (`list_plugin_presets` /
`load_plugin_preset`), the same last-listing whitelist, the same FFI gate (50 ms)
and the same `PresetPicker.svelte` as CLAP: D1, L1 and U1 are unchanged. Windows
only, in the app process (the sandbox stays CLAP-only). Two sources:

1. **The plugin's own programs** (`IUnitInfo`): the programs of every list the host
   can select, offered as factory presets in the plugin's order. A program is chosen
   by setting the parameter flagged `kIsProgramChange` (the plugin's own
   `plainParamToNormalized` gives the value), sending the same value to the
   processor, and — when the plugin is active — giving the processor's state to the
   controller (`setComponentState`) so the other knobs follow.
2. **`.vstpreset` files** in Steinberg's standard locations (`Documents` = user,
   `%APPDATA%` and `%PROGRAMDATA%` = factory, under `VST3 Presets\<vendor>\<plugin>`),
   listed on demand on the worker thread. A file must carry the plugin's class id;
   loading it goes through the ADR-010 state path (`setState` of the component,
   `setComponentState` / `setState` of the controller).

The files come from the user's disk and are untrusted: every size and offset is
checked against the real file length before use, the walk is bounded (depth,
number of entries and presets, no links), the listing reads 48 bytes per file, and
a file is read and checked again just before it is loaded.

Consequences are those of CLAP: not undoable, the plugin is bypassed (silent for an
instrument) while the gate is held, nothing in the project (the state blob carries
the sound; no preset name is stored). No project, command or event change, no audio
thread change, no VST3 in the render harness (C-2 unchanged). **Decided not to do
now:** `IUnitHandler` (the list is read again each time the picker is shown, so a
stale list corrects itself and no load waits for the callback),
`IProgramListData` / `IUnitData`, the `Prog` / `Info` chunks, presets bundled in
the plugin, a file watcher. No invariant is touched, hence this amendment instead
of a new ADR. Details and bounds: `../plugins/plugin-wrapper-system.md` ("Host
VST3 presets"). Unverified: the class-id byte order against a `.vstpreset` written
by another host (none on the development machine), and the whole feature by hand.

**Amendment 2026-11-05 (ADR-033 step 9): sandboxed VST3 plugins.** A sandboxed VST3
plugin offers and loads the same presets through the same two commands and
whitelist. The programs are listed and chosen in the bridge (two control ops,
`PresetList` and `PresetLoadState`; protocol v4 unchanged), the `.vstpreset` files
are read and checked by the engine as before and their states sent to the bridge.
`IUnitHandler` is still not coded. See `033-format-neutral-sandbox-bridge.md`
("Step 9") and `../plugins/plugin-wrapper-system.md` ("VST3 presets in the sandbox").
