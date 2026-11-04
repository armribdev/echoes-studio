# ADR-033 — Format-neutral sandbox bridge (protocol v3), shared VST3 crates, out-of-process scan

## Status

**Proposed — 2026-11-03; plan approved by the user the same day, steps 0 to 3
done.** Step 0 (this ADR and the header check in Appendix A) changed no code. Step 1
moved `vst3/ffi.rs` and `param_queue.rs` into the `vst3_ffi` and `plugin_param_queue`
crates (see "Step 1 — what shipped"). Step 2 added the test plugin, the harness
baseline and the loader fix ("Step 2 — what shipped"). Step 3 moved the VST3 host objects into
`vst3_host` ("Step 3 — what shipped"). Decisions 1–9 below were taken by the user
before this ADR was written (decision 10 only restates ADR-015). The status stays
_Proposed_ until the protocol change of step 4, the first step that crosses the
real-time boundary, is approved.

Extends **ADR-015** (out-of-process sandbox, opt-in), which listed "VST3 behind the
same handle" under _Still open_. Builds on ADR-003 (plugin boundary), ADR-010 (state
and the FFI gate), ADR-013 / ADR-014 (instruments, ports), ADR-016 (instance reuse),
ADR-021 (parameters), ADR-031 (presets). Changes the cross-process ABI
(`BRIDGE_PROTOCOL_VERSION` 2 → 3), which is why it needs an ADR and an approval.

---

## Context

The goal is that **a VST3 plugin that crashes does not take Echoes down**. Today:

- The sandbox exists for **CLAP only** (effects, and track instruments through the
  preference). `runs_sandboxed(format, sandboxed)` in `plugin_system/host.rs` returns
  `sandboxed && format == DeviceFormat::Clap`; a VST3 plugin always loads in the
  Echoes process, on the audio thread.
- The bridge protocol (v2) is **CLAP-shaped**: the engine writes CLAP event structs
  into the block (`ClapParamEvents`) and the bridge forwards them verbatim
  (`plugin_bridge/src/protocol.rs`: "opaque CLAP bytes"). The control channel has
  CLAP-only ops (`TrackInfo`, `NoteNames`, `PresetLoad` carry `clap_ffi` wire forms).
  `plugin_host_bridge` links `clap_ffi` and nothing for VST3.
- The VST3 host code (≈ 20 files in `plugin_formats/vst3/`) lives inside the engine
  crate and **imports engine modules**: `audio_graph::{buffer, midi, node,
  plugin_params}` and `plugin_system::{instance, lifecycle, crash_marker, param_queue,
  presets, …}` (measured with a grep on 2026-11-03). The bridge cannot use it as is.
- The render harness (C-2) contains **no VST3 plugin** and **no sandboxed plugin**
  (`render_harness.rs` builds its plugin effects with `sandboxed: false`).
  `plugin_system/sandbox_tests.rs` proves behaviour (state, crash, latency, …); a
  search of its test names and assertions found no test that compares a sandboxed
  render with an in-process render bit for bit, even for CLAP. ADR-015's statement "a sandboxed plugin renders the same
  whatever the OS scheduling" is therefore argued, not tested.
- The VST3 bindings were checked against the official headers for the first time in
  step 0 (Appendix A): they are correct, but the loader skips two documented host
  steps (`InitDll` / `ExitDll`) and the repository never initialises COM.

## Decision

1. **Neutral bridge protocol v3.** The engine sends events that do not depend on a
   plugin format: transport, notes, raw MIDI, parameter changes with a sample
   offset, start / end of a gesture. A **format adapter** in the bridge turns them
   into CLAP events or VST3 `ProcessContext` / `IEventList` / `IParameterChanges`.
   CLAP migrates onto v3 (no CLAP struct crosses the boundary any more). The v3
   handshake describes an **audio port table** (direction, main / aux, channel count,
   speaker layout, active flag) from the start; **one port is active** to begin with
   (the main stereo bus), so the multi-port layout (ADR-014) needs no second ABI break.
2. **One process per plugin instance.** A crash, a hang or a leak affects one
   plugin. This is the current CLAP model; it also keeps invariant 4 (one
   `Arc<PluginInstance>` per device, the proxy is `NativeHandle::Bridged`).
3. **CLAP parity, delivered by steps** (plan below): audio, parameters, editor,
   instruments, presets — in that order.
4. **Two crates for the VST3 host code, plus a small shared one.** `vst3_ffi`
   (bindings) and `vst3_host` (host COM objects, buses, event list, parameter
   delivery, component handler, parameter mirror, presets). `ParamQueue` moves to a
   small crate shared by the engine and the bridge. **These crates never depend on
   `audio_graph`** (ADR-003). In Cargo this is enforced by construction: they are
   siblings of the engine crate, which depends on them, so a back-edge would be a
   cycle. Precedent: `clap_ffi` (ADR-015 6d-iii-b-2), re-exported by a shim so every
   in-engine import path stays valid.
5. **The isolation modes apply to every format.** _Aucun_ / _Choisis_ / _Tous_, the
   per-plugin "always isolate" list and the "Isoler" button now cover VST3. The
   default stays **Choisis** (C-9): nothing is isolated unless the user asks.
6. **Out-of-process scan and a blocklist** (`plugin_host_bridge --scan`), as the
   last step. A plugin that crashes during the scan is recorded and skipped.
7. **A real VST3 test plugin** — a `test_vst3_plugin` DLL ported from
   `vst3/fake_plugin.rs`, the counterpart of `test_clap_plugin`.
8. **Order of delivery:** audio, parameters, editor, instruments, presets.
9. **COM is initialised explicitly on the bridge's main thread before any VST3 is
   loaded.** _To be verified_ (see "Open questions": the headers say nothing about
   it; the check is part of step 2).
10. **No silent fallback** (unchanged from ADR-015): a VST3 plugin that cannot start
    sandboxed is reported unavailable, never loaded in-process instead.

### Shape of v3 (to be frozen in step 4, not before)

- A block message stays `[header][typed sections][samples]`, in preallocated rings of
  fixed capacity (`event_capacity`, as today). Sections: transport (one POD per
  block), note events, raw MIDI, parameter events (native id, value, sample offset,
  kind), gesture begin / end. Reverse direction: the plugin's own parameter edits,
  gestures, dirty flag, restart request (latency / I/O / parameters), output MIDI.
- The bridge never interprets a parameter value: the handshake declares the unit of
  the format (CLAP plain value, VST3 normalised value) and the engine keeps sending
  what it sends today (ADR-021).
- Control ops (`Op::*`) lose their CLAP wire forms: parameter lists, note names,
  track info and preset requests become format-neutral records.
- `BRIDGE_PROTOCOL_VERSION` becomes 3. Engine and bridge ship together; the
  existing handshake (`--print-protocol-version`, control block, per-block header)
  already rejects a mismatch.

## Real-time risks

| Risk                                                                                                                                 | Where                      | Answer                                                                                                                                                                                                              |
| ------------------------------------------------------------------------------------------------------------------------------------ | -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| The per-block message changes on **both** audio threads (engine and bridge)                                                          | Step 4                     | Same rings, same wake-up, same bounded wait (3/4 of the block, ADR-015 amendment). Sections have a fixed capacity; no allocation, no lock, no new syscall. Round-trip time measured before and after on the test plugin. |
| More sandboxed plugins means more cores spinning in the live wait ("the live wait still spins a core", ADR-015 _Still open_)         | Step 5, mode _Tous_        | Measure with 8 sandboxed VST3 effects before declaring step 5 done. If it does not fit, this ADR is amended (budget, or a smarter wait) before the mode is offered for VST3.                                      |
| The engine's one allowed kernel call on the audio thread (`SetEvent`, non-blocking) now also happens for VST3                        | Step 5                     | Already ratified in ADR-015 for the sandbox path; unchanged.                                                                                                                                                        |
| Concurrency rule differs: CLAP lets main-thread and audio-thread calls overlap; for VST3 the engine uses the FFI gate (ADR-010)       | Steps 5, 6, 9              | The VST3 adapter inside the bridge carries the same gate (an off-RT call such as `getState` or a preset load makes the bridge's audio thread skip the block). The engine side needs no gate for a bridged instance. |
| `vst3_host` moves code that runs on the audio thread (`param_delivery`, `event_list`, `buses`, `param_mirror`) and `ParamQueue`       | Steps 1, 3                 | Moved verbatim first, decoupled second, each in its own commit; reviewed with the `rt-validation` checklist; a golden hash recorded **before** the move (step 2).                                                  |
| The VST3 parameter mirror (audio thread → editor window thread, atomic table) moves with the plugin                                  | Step 6                     | It stays entirely inside the bridge process: no cross-process atomics are added.                                                                                                                                    |
| A hung or dead bridge during an **offline** render                                                                                    | Steps 4, 5                 | Unchanged: the offline wait is un-timed up to 10 s per block (ADR-015); a timeout there is an error, not silence.                                                                                                 |

## Impact on C-2 (bit-identical offline render)

- **Non-sandboxed projects:** no render path changes in steps 0–3 other than moved
  code. Every existing golden hash must stay unchanged.
- **The harness has no VST3 plugin.** Step 2 adds a golden project `vst3_plugins`
  (the test DLL as an effect and as an instrument, in-process), recorded **before**
  `vst3_host` is extracted, on the model of `repeated_midi` (recorded before the
  ADR-032 refactor so it must not move) and of `clap_plugins`.
- **New test "sandbox against in-process".** Render the same project twice — plugin in
  the Echoes process, plugin in the bridge, offline wait — and require **identical
  samples** on every block size of the harness. It is written for **CLAP first, on
  protocol v2, as the first commit of step 4**, so the v2 → v3 migration is held to
  a baseline that already passes; it is then repeated for the VST3 test DLL (effect,
  instrument, automated and modulated parameters, several ports). A sandboxed render
  that differs from the in-process render is a bug in the bridge, not a tolerance.
- **No feature flag may gate this** (C-10): the test runs in the default suite; only
  the test with a real third-party VST3 (TDR Nova, Surge XT) is opt-in through the
  existing `ECHOES_TEST_VST3` override.
- **C-4:** projects saved before this change have `sandboxed: false` on every VST3
  effect (the "Isoler" button is shown for CLAP only), so they load and render as
  before. A project edited by hand with `sandboxed: true` on a VST3 effect will start
  honouring the flag in step 5.

## Alternatives rejected

- **CLAP as the common language** (VST3 events translated to CLAP in the engine, the
  bridge stays CLAP-only). Rejected: the engine would carry two translations (VST3 →
  CLAP → VST3), VST3 concepts with no CLAP equivalent (units and programs, parameter
  gestures with `restartComponent` flags, aux bus activation, `IMidiMapping`) would
  be lost or squeezed through opaque bytes, and CLAP stays the reference format for
  every future one. It also repeats the "format leaks into the boundary" problem
  ADR-003 was written to avoid.
- **A bridge linked to the whole engine** (the bridge depends on the Echoes crate to
  reuse `plugin_formats/vst3` as is). Rejected: the bridge would pull in Tauri,
  `audio_graph` and the whole project model; the sandbox would stop being a small
  process that can only host a plugin; and nothing would prevent audio-graph types
  from reaching plugin code. It also breaks the existing split (`plugin_host_bridge`
  is a standalone binary that links only `plugin_bridge` and `clap_ffi`).
- **Duplicating the VST3 code in the bridge.** Rejected: two copies of
  `param_delivery`, `event_list`, `buses` and the component handler would drift, and
  the fixes of the last weeks (IID byte order, `kNoInterface` codes, bus activation
  order) would have to be made twice. The precedent of `clap_ffi` shows that moving
  once is cheaper.
- **One bridge process hosting several plugins.** Rejected by the user (decision 2):
  a crash would take the neighbours with it.
- **Keeping VST3 in-process behind an SEH guard only.** Already in place
  (`crash_guard`), already rejected as the final answer by ADR-015.

## Plan

Each step is shippable on its own, leaves the engine bit-identical for projects
without a sandboxed VST3, and waits for the user's approval before the next one.
"RT" is the real-time risk of the step, "C-2" its effect on the offline render.

| Step | Content                                                                                                                                                                                                                                                                                                 | RT                                   | C-2                                                                                                  |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------ | ---------------------------------------------------------------------------------------------------- |
| 0    | **Foundations** (done 2026-11-03): official headers fetched outside the repository, bindings checked (Appendix A), new interfaces listed (Appendix B), this ADR.                                                                                                                                         | none                                 | none (no code)                                                                                       |
| 1    | **`vst3_ffi` and `plugin_param_queue` crates**: `plugin_formats/vst3/ffi.rs` and `plugin_system/param_queue.rs` moved as they are, re-export shims in the engine. The IID test moves with the bindings.                                                                                               | none in logic; `ParamQueue` is audio-thread code, moved verbatim | harness green, all hashes unchanged                                                                  |
| 2    | **`test_vst3_plugin` DLL and baseline** (done). cdylib built on `vst3_ffi`, written after `fake_plugin.rs` (effect with gain, state, latency, "hang", "crash", COM diagnostics; and an instrument; "touch" comes with step 6). Golden project `vst3_plugins` in the harness. The loader calls `InitDll` / `ExitDll`; the COM question is answered. | none (the loader is off the audio thread) | new golden `vst3_plugins`, recorded in-process before any host code moves; others unchanged        |
| 3    | **`vst3_host` crate** (done). Host COM objects, buses, event list, component handler, parameter mirror, units, channel context, MIDI mapping, transport context. The engine types they use today (see Context) are replaced by neutral ones owned by the crates; `wrapper.rs` and `instrument.rs` stay in the engine. Sub-steps by file group, each green. | **high**: audio-thread code is moved and decoupled | `vst3_plugins` and every other hash must not move; TDR Nova render identical before / after (manual) |
| 4    | **Protocol v3 and CLAP migration.** First commit: the "sandbox against in-process" test on v2 for the CLAP test plugin. Then typed sections, port table, `ClapAdapter` in the bridge, neutral control records, `BRIDGE_PROTOCOL_VERSION` = 3.                                                          | **high**: message format on both audio threads | the new test stays identical; `clap_plugins` unchanged                                              |
| 5    | **VST3 audio in the bridge.** `Vst3Adapter` (effects): load, `InitDll`, activate, process, latency, tail, state, crash and restart, ADR-010 gate inside the bridge. `runs_sandboxed` accepts VST3, the modes and the "Isoler" button cover it.                                                      | **high**: first VST3 on the ring; cores spinning | sandbox-against-in-process test on the VST3 test DLL (effect)                                       |
| 6    | **VST3 parameters.** Parameter list and text, automation and modulation with a sample offset, gestures both ways, rescan, controller follows processor (inside the bridge), modulated bases before a state save.                                                                                    | medium: bounded queues (128 × 128)   | same test with automated and modulated parameters                                                    |
| 7    | **VST3 editor in the bridge.** `IPlugView` / `IPlugFrame` / content scale in the bridge's Win32 window (same layout as CLAP), resize in the same call stack, close order.                                                                                                                              | none (bridge UI thread)              | none                                                                                                 |
| 8    | **VST3 instruments and several ports.** Notes and raw MIDI, `IMidiMapping`, a bridged VST3 instrument; aux outputs and sidechain through the port table (several active ports); `IComponentHandlerBusActivation`.                                                                                   | medium-high: multi-port block layout | same test for instrument, multi-out and sidechain                                                    |
| 9    | **VST3 presets.** `IUnitInfo` programs and `.vstpreset` files through the ADR-031 commands; parsing stays in the app (untrusted file), loading happens in the bridge; `IUnitHandler`.                                                                                                                | none (control path); gate in the bridge | none                                                                                                 |
| 10   | **Out-of-process scan and blocklist.** `plugin_host_bridge --scan`, timeout and crash contained, blocklist kept per machine, a way to re-enable. Replaces the in-process throwaway component of the VST3 sidechain probe.                                                                              | none                                 | none                                                                                                 |

### Tests and what stays manual

| Step | Automated                                                                                                                                                                 | Still to test by hand                                                                                                        |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| 0    | The IID comparison script (kept out of the repository, in the scratch folder).                                                                                            | —                                                                                                                            |
| 1    | Moved unit tests (IIDs, layout asserts, `ParamQueue`); the new crates have an empty `[dependencies]` section (no engine dependency possible); IID test against the copied headers; full workspace suite.                         | Open a real VST3 (TDR Nova) once in the app; start the app once (a parameter edit reaches a plugin).                                                                                 |
| 2    | Test DLL loads through the existing path with the same assertions as `fake_plugin.rs`; `InitDll` / `ExitDll` order; golden `vst3_plugins` and block-size independence.     | TDR Nova and Surge XT: open, close, reload, save and reopen (the load sequence changes for every real VST3).                 |
| 3    | Every moved test; real-FFI tests on TDR Nova / Surge XT with `ECHOES_TEST_VST3`; hashes unchanged.                                                                        | Audio, editor, presets, multi-out and sidechain in the app, in-process, as before.                                           |
| 4    | Protocol ABI asserts; bridge integration tests ported; `sandbox_tests.rs` unchanged and green; sandbox-against-in-process; round-trip timing.                             | A real CLAP plugin sandboxed: play, automate, open the editor, kill the bridge.                                              |
| 5    | `sandbox_tests.rs` mirrored for VST3: processing, latency, state, toggle keeps state, reuse across recompiles, crash contained / reported / recovered, hang contained.    | TDR Nova sandboxed; end the bridge from the Task Manager during playback; mode _Tous_ with several instances (CPU).          |
| 6    | Parameters, automation and modulation across the process with the test DLL; editor-follow table; state save with modulated bases.                                         | Knobs in a sandboxed window follow automation; save and reopen.                                                              |
| 7    | Bridge tests with the test DLL's view (`FakeView` ported): attach, resize, close order.                                                                                    | Sandboxed editor on TDR Nova: open, resize, HiDPI screen, close; a crash in the editor kills only the bridge.                |
| 8    | Instrument notes and parameters, multi-out, sidechain with the test DLL, sandboxed.                                                                                        | Surge XT sandboxed (its "Scene A" bus); TDR Nova sidechain sandboxed.                                                        |
| 9    | Program change and `.vstpreset` load through the bridge with the test DLL.                                                                                                 | A program of TDR Nova; a real `.vstpreset` (none is known yet).                                                              |
| 10   | Scan of a DLL that crashes on load / in `GetPluginFactory`: the app continues, the plugin is blocklisted, the scan of the others finishes.                                | The installed plugin set; re-enabling a blocklisted plugin.                                                                  |

## Open questions (to settle, not decided here)

1. ~~**Headers in the repository.**~~ _Settled 2026-11-03 (user: yes):_ the 14
   header files that declare the IDs `vst3_ffi` uses are copied unmodified under
   `crates/vst3_ffi/sdk_headers/` with the MIT `LICENSE.txt` and a `SOURCE.md` giving
   the commit; a test compares every `IID_*` constant with them. IDs of the
   interfaces added later (Appendix B) need their header file added to that folder
   in the same change.
2. **COM initialisation (decision 9).** _Partly answered in step 2._ The headers are
   silent. `src-tauri` never calls `CoInitializeEx` / `OleInitialize`; `tao` does
   (`OleInitialize`, window creation, main thread), so the app's main thread is an
   OLE single-threaded apartment, but the thread that instantiates plugins and the
   editor window thread have no COM unless they inherit it. A plugin finds the
   apartment of the thread that calls `initialize` and nothing else (test
   `the_plugin_finds_the_com_apartment_of_the_thread_that_loads_it`). Still open for
   step 5: which call the bridge makes (`OleInitialize`, as `tao` does, or
   `CoInitializeEx` single-threaded) on its main thread, and the apartment of the
   bridge's audio thread — decided with TDR Nova and Surge XT running in the bridge.
3. ~~**`InitDll` / `ExitDll`.**~~ _Fixed in step 2, corrected in step 3:_ `Vst3Library` calls
   `InitDll` after the first `LoadLibrary` of a file and `ExitDll` before the `FreeLibrary` of
   its last user, as `base/doc.h` says (see "Step 3" for why once per file). TDR Nova and
   Surge XT export both and pass the real-plugin tests; **still to hand-test in the
   app** (open, close, reload, save and reopen a project with each). An `InitDll`
   that returns `false` (module refuses to load) is handled but has no test, because
   the test plugin has no way to refuse for one copy only.
4. **Decoupling cost in step 3.** `vst3_host` cannot import the engine types it uses
   today (`audio_graph::plugin_params::ValueModulatedParam`, `midi::MidiEventList`,
   `buffer::AudioBuffer`, `node::TransportInfo`, `plugin_system::{instance,
   lifecycle, crash_marker, presets}`). Step 3 is therefore not a `git mv`: it
   defines small neutral types or traits in the crates and adapts the engine to
   them. If the diff grows past what can be reviewed, step 3 is split by file group.
5. **Units of a parameter value in v3** (plain for CLAP, normalised for VST3) and
   whether a "modulation offset" kind is needed for VST3 (ADR-021 emulates it by
   value). Frozen in step 4 with the ABI.
6. **Behaviour change of mode _Tous_.** Users who chose _Tous_ will see their VST3
   effects isolated after step 5 (extra processes, possible CPU cost). To be
   announced; the 8-instance measurement in step 5 decides whether it is acceptable.
7. **`.vstpreset` and `module-info.json` are not in the pluginterfaces repository.**
   Their formats live in the VST3 public SDK (a different repository). The existing
   `preset_file.rs` was therefore **not** verified in step 0; it is checked in step 9
   against that source, fetched the same way (outside the repository first).
8. **Documents to update with the steps that deliver the change** (not in step 0):
   `plugins/plugin-wrapper-system.md` (sandbox, Host sections), ADR-015 (a pointer
   here and, in its historical sub-phase text, the block header is now 40 bytes, not
   32), ADR-003 (consequence "in-process isolation" no longer true for VST3),
   ADR-014 / ADR-031 (amendments), `state_index.md` (Plugin sandbox, Plugin wrappers),
   `change_impact_mapping.md` (new rows for `vst3_ffi`, `vst3_host`,
   `plugin_param_queue`, `test_vst3_plugin`, the bridge's adapters), `threading/` docs.

## Step 1 — what shipped (2026-11-03)

- `crates/vst3_ffi` — `plugin_formats/vst3/ffi.rs` moved (`git mv`) as `src/lib.rs`.
  The only edit: `Vst3Library::placeholder()`, used by the hand-made plugin tests of
  the engine, was `#[cfg(test)] pub(crate)`; it is now `pub` behind the cargo feature
  `test-support`, which the engine enables in its `[dev-dependencies]`.
- `crates/plugin_param_queue` — `plugin_system/param_queue.rs` moved unchanged
  (std-only; its four tests moved with it).
- The old paths are one-line re-export shims (`pub use vst3_ffi::*;`,
  `pub use plugin_param_queue::*;`): no other engine file changed.
- Both crates have an empty `[dependencies]` section: they cannot reach
  `audio_graph` or the engine.
- New test `interface_iids_match_the_sdk_headers` (next to the older
  `interface_iids_match_the_sdk`): reads `DECLARE_CLASS_IID` from the copied headers
  and compares all 18 `IID_*` constants, `IID_FUNKNOWN` included. It fails if one
  byte of a constant changes (checked by hand).

## Step 2 — what shipped (2026-11-03)

- `crates/test_vst3_plugin` (cdylib + rlib): a real single-component VST3 with two
  classes, `Echoes Test Gain` (`Fx`: input × gain) and `Echoes Test Synth`
  (`Instrument|Synth`: held velocity × gain). Parameters: gain, hang, crash, and four
  read-only diagnostics (COM apartment seen by `initialize` and by the first
  `process`, process calls, `InitDll` calls). 64 samples of latency reported, state =
  the gain, parameter points and notes applied at their sample offset. It exports
  `InitDll`, `ExitDll` and three counters a test reads even after the host unloaded
  the module. Eight tests drive it through the real `vst3_ffi` calls.
  `IID_IPLUGIN_FACTORY` was added to `vst3_ffi` for it (value read in the headers,
  in the header test).
- **Loader fix** in `vst3_ffi::Vst3Library` (see open question 3; step 3 made it once
  per file). The test `the_loader_runs_init_dll_after_loading_and_exit_dll_before_unloading`
  loads a copy of the DLL (its own counters) and reads them through a second reference.
- **Harness baseline.** Project `vst3_plugins` in `audio_engine/render_harness.rs`:
  an audio clip through the effect and an instrument playing two notes, in the
  Echoes process. Three tests: audible with silent gaps, identical at the five block
  sizes of the harness, and `VST3_PLUGINS_GOLDEN_HASH = 0x5c0f0710b6813439`
  (stable over repeated runs). Recorded **before** any `vst3_host` code moves.
  Every other golden hash is unchanged. The context bullet saying the harness has no
  VST3 plugin describes the state before this step.
- `plugin_formats/vst3/test_dll.rs`: builds the DLL once per run into
  `target/vst3-test-plugin`, hands out separate copies for tests that count what the
  host did to a module.
- **Real plugins:** `ffi_lifecycle_tests` run green with the new loader on TDR Nova
  (effect) and, with `ECHOES_TEST_VST3` pointing at its binary, on Surge XT
  (instrument: a note makes sound, controller mapping, aux outputs, programs).

## Step 3 — what shipped (2026-11-04)

Sub-steps, each left green: 3a pure COM objects, 3b objects that touched engine types,
3c the handler, MIDI mapping and transport. Test counts: the 805 engine tests of step 2
are now 736 in the engine, 63 in `vst3_host` and 11 in `audio_buffer` (810), five more: two
loader tests (shared module), one channel-context test, and the transport tests split in two
places. The `vst3_plugins` golden hash and every other hash did not move.

- **`crates/vst3_host`** (depends on `vst3_ffi`, `audio_buffer`, `plugin_param_queue`; never
  on the engine): `attribute_list`, `message`, `host_context`, `com_object`, `event_list`,
  `param_mirror`, `units`, `params`, `channel_context`, `buses`, `midi_mapping`,
  `component_handler`, `transport`. The old paths still resolve through one
  `pub use vst3_host::{…}` line in `plugin_formats/vst3/mod.rs`.
- **What had to become neutral** (the engine types these files used):
  - `AudioBuffer` (buses) is now the `audio_buffer` crate, re-exported at
    `audio_graph::buffer`; its hot accessors were already `#[inline]`, three more got it.
    Chosen over a trait plus a copy of the buffer: one type, no duplicate.
  - `HostTrackInfo` (channel context) → the name and color as plain values; the engine keeps a
    10-line adapter.
  - `PluginParamInfo` (parameter list) → `vst3_host::params::ParamInfo`, same fields; the
    engine maps it.
  - `MidiEventList` (MIDI mapping) → `(offset, [u8; 3])` pairs, built lazily by the engine.
  - `TransportInfo` (transport) → `TransportState` (playing, rate, samples, beats, bar start,
    tempo, time signature): the shape step 4 will carry in the protocol.
  - `ValueModulatedParam` (handler) → `FollowedParam { native_id, base }`.
  - The process-wide signals the handler raised (`mark_plugin_state_edited`, restart, rescan)
    → the `HostSignals` trait; the engine's implementation calls the same functions as before.
    The bridge will give its own.
- **Stayed in the engine, on purpose:** `wrapper.rs` and `instrument.rs` (`AudioNode`,
  `InstrumentDevice`), `host.rs` (the scan fills the engine's registry), `fake_plugin.rs`,
  and `param_delivery.rs`: it is glue between the engine's parameter controls, the instance,
  the FFI gate and the crash guard. What the bridge needs for parameter delivery —
  `HostParameterChanges` — already lives in `vst3_ffi`; the neutral delivery is written with
  step 6, when it has a consumer. `preset_file.rs` and `preset_locations.rs` stay too: the
  `.vstpreset` parsing belongs to the app (step 9). Decision 4's list ("delivery, presets")
  is therefore met by the program lists (`units`) and the pieces above.
- **Tests that need the hand-made plugin** (`fake_plugin.rs` builds an engine `PluginInstance`)
  moved to engine files next to the code they test: `units_tests.rs`, `buses_tests.rs`, and the
  notify tests of `channel_context.rs`. Cargo features `test-support` on `vst3_ffi`,
  `vst3_host` and `audio_buffer` expose the few accessors that were `#[cfg(test)]`.
- **Loader corrected.** Step 2 ran `InitDll` / `ExitDll` at every load and drop. Two instances of
  one plugin share the module's globals, so the first drop could take them from the other.
  `Vst3Library` now keeps one entry per file (path, user count) under a lock: `InitDll` at the
  first user, `ExitDll` and unload at the last. New tests: two users share one pair of calls,
  and eight threads loading and dropping one module keep the counts paired.
- **Real-plugin tests were flaky, before this ADR.** Run in parallel, the tests that drive TDR Nova
  or Surge XT failed 12 times in 60 on the code of steps 0–1 (7 crashes), 18 in 60 with the
  loader doing nothing, 19 in 60 with it; run one at a time, 0 in 90. A lock held for the length
  of any test that looks for a real plugin (`hold_real_plugins` in `ffi_lifecycle_tests.rs`)
  brings it to 0 in 60 parallel runs of that subset and 0 in 15 of the whole engine suite. The
  cause is the plugin not being safe to call from several threads, not the host split; whether
  the app can hit the same thing (the scan on a worker thread while the command thread
  instantiates the same plugin) is **not established** and is worth a look in step 5.
- **Not hand-tested in the app:** the loader sharing, the handler signals (unsaved flag after an
  edit in a plugin window, restart and rescan requests), the MIDI mapping and the transport —
  all covered by tests, none by a hand test.

## Consequences

- **Positive.** A crashing VST3 no longer ends the session; the bridge stops being a
  CLAP special case; the VST3 code gets a second, cleaner home with enforced
  boundaries; C-2 gains its first sandbox-against-in-process proof.
- **Negative.** A second ABI break for the bridge (v2 → v3) and a CLAP migration with
  nothing new for CLAP users; a large extraction (step 3) on audio-thread code; a
  wider dual code path (in-process and bridged) for VST3, inherent to C-9; one more
  process per isolated plugin.
- **Invariants.** No hard invariant is changed: the graph still sees only
  `AudioNode` (1, 3), one instance per device (4), Rust owns the state (5),
  determinism is protected by a new test (7). Real-time rules are unchanged; the
  change of the message on the RT path is what this ADR ratifies.

---

## Appendix A — Check of `vst3/ffi.rs` against the official headers (step 0)

**Source.** `https://github.com/steinbergmedia/vst3_pluginterfaces`, branch `master`,
commit `4f547e8e102b47de4a8b8aaf343c73b700786372` (2026-08-11, "VST3 SDK 3.8.1"), the
same commit as tag `v3.8.1_build_84`. Cloned on 2026-11-03 into a new, empty folder
**outside** the repository; nothing downloaded was executed or copied. **Licence:**
`LICENSE.txt` is the MIT licence, "Copyright (c) 2026, Steinberg Media Technologies
GmbH". The file banners still say "subject to the license terms in the LICENSE file",
which is that MIT file; the README only points to `steinberg.net/sdklicenses_vst3`,
which was not consulted. Older SDK versions were under a different licence, so the
version matters.

**Method.** A script (kept out of the repository) read every
`DECLARE_CLASS_IID (name, l1, l2, l3, l4)` from the headers and compared it with the
literal of each `IID_*` constant of `ffi.rs`, and printed the virtual methods of each
interface in declaration order. Layouts were computed by hand from the field lists
(Windows x86_64, `#pragma pack(16)` of `falignpush.h`, which never raises the natural
alignment of any structure below). COM byte order was checked against
`INLINE_UID` of `funknown.h` (`COM_COMPATIBLE`): the first long is little-endian, the
second and third are split into little-endian halves, the fourth is big-endian — which
is exactly what `interface_iid()` does. All 18 `IID_*` constants match. The existing
test `interface_iids_match_the_sdk` covers 17 of them (not `IID_FUNKNOWN`) and its
expected values equal the headers' as well.

| Element in `ffi.rs`                                                                 | Header                                    | Verdict                                                                                                  |
| ----------------------------------------------------------------------------------- | ----------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `IID_FUNKNOWN`                                                                      | `base/funknown.h`                         | OK (`0,0,0xC0000000,0x46`, identical with or without the COM swap)                                       |
| `IID_ICOMPONENT`                                                                    | `vst/ivstcomponent.h`                     | OK                                                                                                       |
| `IID_IAUDIO_PROCESSOR`                                                              | `vst/ivstaudioprocessor.h`                | OK                                                                                                       |
| `IID_IEDIT_CONTROLLER`                                                              | `vst/ivsteditcontroller.h`                | OK                                                                                                       |
| `IID_IMIDI_MAPPING`                                                                 | `vst/ivsteditcontroller.h`                | OK                                                                                                       |
| `IID_ICOMPONENT_HANDLER`, `IID_ICOMPONENT_HANDLER2`                                 | `vst/ivsteditcontroller.h`                | OK, OK                                                                                                   |
| `IID_IEVENT_LIST`                                                                   | `vst/ivstevents.h`                        | OK                                                                                                       |
| `IID_IPLUGIN_FACTORY2`                                                              | `base/ipluginbase.h`                      | OK                                                                                                       |
| `IID_IMESSAGE`, `IID_ICONNECTION_POINT`                                             | `vst/ivstmessage.h`                       | OK, OK                                                                                                   |
| `IID_IATTRIBUTE_LIST`                                                               | `vst/ivstattributes.h`                    | OK                                                                                                       |
| `IID_IINFO_LISTENER`                                                                | `vst/ivstchannelcontextinfo.h`            | OK                                                                                                       |
| `IID_IUNIT_INFO`                                                                    | `vst/ivstunits.h`                         | OK                                                                                                       |
| `IID_IHOST_APPLICATION`                                                             | `vst/ivsthostapplication.h`               | OK                                                                                                       |
| `IID_IPLUG_FRAME`                                                                   | `gui/iplugview.h`                         | OK                                                                                                       |
| `IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT`                                              | `gui/iplugviewcontentscalesupport.h`      | OK                                                                                                       |
| `IID_IBSTREAM`                                                                      | `base/ibstream.h`                         | OK                                                                                                       |
| `Tuid` (16 bytes), `TResult` (`int32`)                                              | `base/funknown.h`, `base/ftypes.h`        | OK                                                                                                       |
| `K_RESULT_OK` 0, `K_RESULT_FALSE` 1                                                 | `base/funknown.h`                         | OK                                                                                                       |
| `K_NO_INTERFACE`, `K_INVALID_ARGUMENT`, `K_OUT_OF_MEMORY` (Windows)                 | `base/funknown.h`                         | OK (`0x80004002`, `0x80070057`, `0x8007000E`)                                                            |
| same three, other platforms (`-1`, `2`, `6`)                                        | `base/funknown.h` (`COM_COMPATIBLE` = 0)  | OK                                                                                                       |
| `PClassInfo` (116 bytes: cid 16, cardinality, category 32, name 64)                 | `base/ipluginbase.h`                      | OK                                                                                                       |
| `PClassInfo2` (440 bytes, asserted)                                                 | `base/ipluginbase.h`                      | OK (field order and sizes 32 / 64 / 128 / 64 / 64 / 64)                                                  |
| `IPluginFactory` vtable (4 own methods)                                             | `base/ipluginbase.h`                      | OK; `getFactoryInfo` is typed `*mut c_void` (the `PFactoryInfo` struct is not defined) — fine today      |
| `IPluginFactory2` vtable (`getClassInfo2` after `createInstance`)                   | `base/ipluginbase.h`                      | OK                                                                                                       |
| `IComponent` vtable (14 slots, `setState` before `getState`)                        | `vst/ivstcomponent.h`                     | OK                                                                                                       |
| `IAudioProcessor` vtable (11 slots; latency and tail return `uint32`)               | `vst/ivstaudioprocessor.h`                | OK                                                                                                       |
| `AudioBusBuffers` (`int32`, `uint64`, pointer; 24 bytes)                            | `vst/ivstaudioprocessor.h`                | OK                                                                                                       |
| `ProcessSetup`                                                                      | `vst/ivstaudioprocessor.h`                | OK                                                                                                       |
| `ProcessData` (order of the 12 fields)                                              | `vst/ivstaudioprocessor.h`                | OK                                                                                                       |
| `ProcessModes` (0 realtime, 1 prefetch, 2 offline), `SymbolicSampleSizes` (0 = 32)  | `vst/ivstaudioprocessor.h`                | OK (only 0 is used; offline is deliberately not used, see the wrapper doc)                               |
| `ProcessContext` (112 bytes, asserted; offsets by hand)                             | `vst/ivstprocesscontext.h`                | OK                                                                                                       |
| `CONTEXT_PLAYING` 1<<1, `PROJECT_TIME_MUSIC_VALID` 1<<9, `TEMPO_VALID` 1<<10, `BAR_POSITION_VALID` 1<<11, `TIME_SIG_VALID` 1<<13 | `vst/ivstprocesscontext.h` | OK                                                                                                       |
| `K_AUDIO` 0, `K_EVENT` 1, `K_INPUT` 0, `K_OUTPUT` 1                                | `vst/ivstcomponent.h`                     | OK                                                                                                       |
| `SPEAKER_STEREO` 0x3                                                                | `vst/vstspeaker.h`                        | OK (L = 1<<0, R = 1<<1)                                                                                  |
| `CATEGORY_AUDIO_MODULE` `"Audio Module Class"`                                     | `vst/ivstaudioprocessor.h` (`kVstAudioEffectClass`) | OK                                                                                            |
| `is_instrument_subcategory` (first token `Instrument`)                              | `vst/ivstaudioprocessor.h` (`kInstrument`) | OK                                                                                                      |
| `K_NOTE_ON_EVENT` 0, `K_NOTE_OFF_EVENT` 1, `K_POLY_PRESSURE_EVENT` 3               | `vst/ivstevents.h`                        | OK                                                                                                       |
| `NoteOnEvent`, `NoteOffEvent`, `PolyPressureEvent`                                  | `vst/ivstevents.h`                        | OK (field order and types)                                                                               |
| `Event` (48 bytes, payload at offset 24) and its union `raw: [u64; 3]`              | `vst/ivstevents.h`, `vst/ivstnoteexpression.h` | OK: widest member is `NoteExpressionTextEvent`, 24 bytes; the new `NoteExpressionIntValueEvent` is 16 |
| `IParamValueQueue`, `IParameterChanges` vtables (host-implemented)                  | `vst/ivstparameterchanges.h`              | OK                                                                                                       |
| `IAttributeList`, `IMessage`, `IInfoListener`, `IHostApplication` vtables           | `vst/ivst*.h`                             | OK (`AttrID` is `const char*`, `TChar` is `char16_t`, sizes are in bytes)                                |
| `IComponentHandler`, `IComponentHandler2` vtables                                   | `vst/ivsteditcontroller.h`                | OK                                                                                                       |
| `IBStream` vtable and seek modes 0 / 1 / 2                                          | `base/ibstream.h`                         | OK                                                                                                       |
| `IEditController` vtable (18 slots, asserted), `ParameterInfo` (792 bytes, asserted), `String128` | `vst/ivsteditcontroller.h`, `vst/vsttypes.h` | OK                                                                                         |
| `PARAM_CAN_AUTOMATE` 1<<0, `IS_READ_ONLY` 1<<1, `IS_HIDDEN` 1<<4, `IS_PROGRAM_CHANGE` 1<<15 | `vst/ivsteditcontroller.h`        | OK                                                                                                       |
| `ViewRect` (4 × `int32`)                                                            | `gui/iplugview.h`                         | OK                                                                                                       |
| `IPlugView` vtable (15 slots, asserted), `IPlugFrame`                               | `gui/iplugview.h`                         | OK                                                                                                       |
| `IPlugViewContentScaleSupport` vtable (`float`)                                     | `gui/iplugviewcontentscalesupport.h`      | OK                                                                                                       |
| `IConnectionPoint` vtable                                                           | `vst/ivstmessage.h`                       | OK                                                                                                       |

**Deviations in `ffi.rs` itself: none.** Two findings concern the host, not the
bindings: `Vst3Library::load` never calls `InitDll` / `ExitDll` (open question 3), and
no COM initialisation exists anywhere in the repository (open question 2).

**Also read, outside `ffi.rs`** (not part of the requested scope, done because step 3
moves these files): `IEventList` vtable (`event_list.rs`), `IMidiMapping` vtable and
the controller numbers 128 / 129 (`midi_mapping.rs`), `UnitInfo` (268 bytes) and
`ProgramListInfo` (264 bytes) and the first slots of `IUnitInfo` (`units.rs`),
`BusInfo` (276 bytes, `busType` at 268) and its flag (`buses.rs`), the restart flag
constants 1<<0 to 1<<5 (`component_handler.rs`), the channel-context keys `"channel
name"`, `"channel name length"`, `"channel color"` (`channel_context.rs`) — all OK.
**Not read:** the rest of the `IUnitInfo` slots, `message.rs`, `attribute_list.rs`,
`host_context.rs`, `preset_file.rs`, `preset_locations.rs`, `param_mirror.rs`,
`transport.rs` mapping choices; they are verified when their step touches them.

## Appendix B — VST3 interfaces the bridge will need in addition (step 0)

IDs are the four longs of `DECLARE_CLASS_IID` in the same headers (commit
`4f547e8`); **none of them is written in any code yet**, and each must be put in a
test like `interface_iids_match_the_sdk` when it is added.

| Interface (header)                                                       | IID (4 longs)                                          | Why                                                                                    | Step |
| ------------------------------------------------------------------------ | ------------------------------------------------------ | -------------------------------------------------------------------------------------- | ---- |
| `IProcessContextRequirements` (`vst/ivstaudioprocessor.h`)               | `2A654303 EF764E3D 95B5FE83 730EF6D0`                  | the plugin says which `ProcessContext` fields it needs; fills only those in v3         | 5    |
| `IAudioPresentationLatency` (`vst/ivstaudioprocessor.h`)                 | `309ECE78 EB7D4fae 8B2225D9 09FD08B6`                  | tell the plugin the output latency of the device (optional)                            | 5    |
| `IPlugInterfaceSupport` (`vst/ivstpluginterfacesupport.h`)               | `4FB58B9E 9EAA4E0F AB361C1C CCB56FEA`                  | plugins ask the host which interfaces it supports; host-implemented                    | 5    |
| `IPluginFactory3` (`base/ipluginbase.h`)                                 | `4555A2AB C1234E57 9B122910 36878931`                  | `getClassInfoW` (Unicode names) and `setHostContext` on the factory; scan              | 5, 10 |
| `PFactoryInfo`, `PClassInfoW` (`base/ipluginbase.h`; structs)            | —                                                      | typed factory info and Unicode class info for the scan (sizes asserted when added)     | 10   |
| `IEditControllerHostEditing` (`vst/ivsteditcontroller.h`)                | `C1271208 70594098 B9DD34B3 6BB0195E`                  | host-side begin / end of an edit, for the gesture events sent by the engine            | 6    |
| `IComponentHandlerBusActivation` (`vst/ivsteditcontroller.h`)            | `067D02C1 5B4E274D A92D90FD 6EAF7240`                  | a plugin asks the host to activate a bus (aux outputs); host-implemented               | 8    |
| `INoteExpressionController` (`vst/ivstnoteexpression.h`)                 | `B7F8F859 41234872 91169581 4F3721A3`                  | instruments: note expression types; events `kNoteExpressionValueEvent` (4), `…TextEvent` (5), `…IntValueEvent` (8), `DataEvent` (2, SysEx), `LegacyMIDICCOutEvent` (65535) | 8 |
| `IUnitHandler`, `IUnitHandler2` (`vst/ivstunits.h`)                      | `4B5147F8 4654486B 8DAB30BA 163A3C56`, `F89F8CDF 699E4BA5 96AAC9A4 81452B01` | presets: the plugin reports a program-list or unit change; host-implemented       | 9    |
| `IStreamAttributes` (`vst/ivstattributes.h`)                             | `D6CE2FFC EFAF4B8C 9E74F1BB 12DA44B4`                  | file name / metadata on the preset stream (optional)                                   | 9    |
| `IComponentHandler3`, `IContextMenu`, `IContextMenuTarget` (`vst/ivstcontextmenu.h`) | `69F11617 D26B400D A4B6B964 7B6EBBAB`, `2E93C863 0C9C4588 97DBECF5 AD17817D`, `3CDF2E75 85D34144 BF86D36B D7C4894D` | context menus of the editor (optional; plugins fall back when absent) | 7 (optional) |
| `IDataExchangeHandler`, `IDataExchangeReceiver` (`vst/ivstdataexchange.h`) | `36D551BD 6FF54F08 B48E830D 8BD5A03B`, `45A759DC 84FA4907 ABCB6175 2FC786B6` | audio-thread to editor data channel of newer plugins (optional; usage to be checked) | 7 (optional) |

Considered and **not planned**: `IMidiMapping2`, `IMidiLearn`, `IMidiLearn2`
(MIDI 2.0 / learn), `IEditController2`, `IKeyswitchController`,
`INoteExpressionPhysicalUIMapping`, `IParameterFunctionName`, `IParameterFinder`,
`IAutomationState`, `IProgress`, `IPrefetchableSupport`, `IRemapParamID`,
`IXmlRepresentationController`, `ITransportControl`, `IPluginCompatibility`,
`ISizeableStream`, the inter-app-audio and Linux (`IRunLoop`, `IEventHandler`,
`ITimerHandler`, `IWayland*`) interfaces, and the test interfaces. They can be added
by an amendment if a real plugin needs them.

**Outside the pluginterfaces repository** (so not verifiable from step 0): the
`.vstpreset` binary format, `module-info.json`, and the Windows module entry points'
exact exports beyond what `base/doc.h` shows (`InitDll`, `GetPluginFactory`,
`ExitDll`).
