# ADR-033 — Format-neutral sandbox bridge (protocol v3), shared VST3 crates, out-of-process scan

## Status

**Proposed — 2026-11-03; plan approved by the user the same day, steps 0 to 7
and 8a done (steps 4 to 7 delivered 2026-11-04, 8a on 2026-11-05, awaiting the user's review).** Step 0 (this ADR and the header check in Appendix A) changed no code. Step 1
moved `vst3/ffi.rs` and `param_queue.rs` into the `vst3_ffi` and `plugin_param_queue`
crates (see "Step 1 — what shipped"). Step 2 added the test plugin, the harness
baseline and the loader fix ("Step 2 — what shipped"). Step 3 moved the VST3 host objects into
`vst3_host` ("Step 3 — what shipped"). Step 4 froze protocol v3 and moved CLAP onto it
("Step 4 — what shipped"). Step 5 put the VST3 effect in the bridge ("Step 5 — what shipped"). Step 6 gave it its parameters ("Step 6 — what shipped"). Step 7 gave it its editor ("Step 7 — what shipped"). Step 10 moved the scan out of the process ("Step 10 — what shipped"). Step 8 was split: 8a put the VST3 instrument in the bridge ("Step 8a — what shipped"), 8b (several ports, protocol v4) is done except `IComponentHandlerBusActivation`: auxiliary outputs ("Step 8b, first half — what shipped") and the sidechain ("Step 8b, second half — what shipped"); the bus-activation request is not supported in process either, so it waits for a decision. Decisions 1–9 below were taken by the user
before this ADR was written (decision 10 only restates ADR-015). The status stays
_Proposed_ until the user has reviewed step 4, the first step that crossed the
real-time boundary (and hand-tested a sandboxed CLAP plugin).

Extends **ADR-015** (out-of-process sandbox, opt-in), which listed "VST3 behind the
same handle" under _Still open_. Builds on ADR-003 (plugin boundary), ADR-010 (state
and the FFI gate), ADR-013 / ADR-014 (instruments, ports), ADR-016 (instance reuse),
ADR-021 (parameters), ADR-031 (presets). Changes the cross-process ABI
(`BRIDGE_PROTOCOL_VERSION` 2 → 3), which is why it needs an ADR and an approval.

---

## Context

The goal is that **a VST3 plugin that crashes does not take Echoes down**. The bullets
below describe the state **before step 0** (bridge protocol v2, CLAP only); the steps
below say what each of them changed.

Today (before step 0):

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

### Shape of v3 (frozen in step 4, 2026-11-04 — exact layout in "Step 4 — what shipped")

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

| Risk                                                                                                                            | Where               | Answer                                                                                                                                                                                                                   |
| ------------------------------------------------------------------------------------------------------------------------------- | ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| The per-block message changes on **both** audio threads (engine and bridge)                                                     | Step 4              | Same rings, same wake-up, same bounded wait (3/4 of the block, ADR-015 amendment). Sections have a fixed capacity; no allocation, no lock, no new syscall. Round-trip time measured before and after on the test plugin. |
| More sandboxed plugins means more cores spinning in the live wait ("the live wait still spins a core", ADR-015 _Still open_)    | Step 5, mode _Tous_ | Measure with 8 sandboxed VST3 effects before declaring step 5 done. If it does not fit, this ADR is amended (budget, or a smarter wait) before the mode is offered for VST3.                                             |
| The engine's one allowed kernel call on the audio thread (`SetEvent`, non-blocking) now also happens for VST3                   | Step 5              | Already ratified in ADR-015 for the sandbox path; unchanged.                                                                                                                                                             |
| Concurrency rule differs: CLAP lets main-thread and audio-thread calls overlap; for VST3 the engine uses the FFI gate (ADR-010) | Steps 5, 6, 9       | The VST3 adapter inside the bridge carries the same gate (an off-RT call such as `getState` or a preset load makes the bridge's audio thread skip the block). The engine side needs no gate for a bridged instance.      |
| `vst3_host` moves code that runs on the audio thread (`param_delivery`, `event_list`, `buses`, `param_mirror`) and `ParamQueue` | Steps 1, 3          | Moved verbatim first, decoupled second, each in its own commit; reviewed with the `rt-validation` checklist; a golden hash recorded **before** the move (step 2).                                                        |
| The VST3 parameter mirror (audio thread → editor window thread, atomic table) moves with the plugin                             | Step 6              | It stays entirely inside the bridge process: no cross-process atomics are added.                                                                                                                                         |
| A hung or dead bridge during an **offline** render                                                                              | Steps 4, 5          | Unchanged: the offline wait is un-timed up to 10 s per block (ADR-015); a timeout there is an error, not silence.                                                                                                        |

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

| Step | Content                                                                                                                                                                                                                                                                                                                                                                                                     | RT                                                               | C-2                                                                                                  |
| ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| 0    | **Foundations** (done 2026-11-03): official headers fetched outside the repository, bindings checked (Appendix A), new interfaces listed (Appendix B), this ADR.                                                                                                                                                                                                                                            | none                                                             | none (no code)                                                                                       |
| 1    | **`vst3_ffi` and `plugin_param_queue` crates**: `plugin_formats/vst3/ffi.rs` and `plugin_system/param_queue.rs` moved as they are, re-export shims in the engine. The IID test moves with the bindings.                                                                                                                                                                                                     | none in logic; `ParamQueue` is audio-thread code, moved verbatim | harness green, all hashes unchanged                                                                  |
| 2    | **`test_vst3_plugin` DLL and baseline** (done). cdylib built on `vst3_ffi`, written after `fake_plugin.rs` (effect with gain, state, latency, "hang", "crash", COM diagnostics; and an instrument; "touch" comes with step 6). Golden project `vst3_plugins` in the harness. The loader calls `InitDll` / `ExitDll`; the COM question is answered.                                                          | none (the loader is off the audio thread)                        | new golden `vst3_plugins`, recorded in-process before any host code moves; others unchanged          |
| 3    | **`vst3_host` crate** (done). Host COM objects, buses, event list, component handler, parameter mirror, units, channel context, MIDI mapping, transport context. The engine types they use today (see Context) are replaced by neutral ones owned by the crates; `wrapper.rs` and `instrument.rs` stay in the engine. Sub-steps by file group, each green.                                                  | **high**: audio-thread code is moved and decoupled               | `vst3_plugins` and every other hash must not move; TDR Nova render identical before / after (manual) |
| 4    | **Protocol v3 and CLAP migration** (done 2026-11-04). First commit: the "sandbox against in-process" test on v2 for the CLAP test plugin. Then typed sections, port table, `ClapAdapter` in the bridge, neutral control records, `BRIDGE_PROTOCOL_VERSION` = 3.                                                                                                                                             | **high**: message format on both audio threads                   | the new test stays identical; `clap_plugins` unchanged                                               |
| 5    | **VST3 audio in the bridge** (done 2026-11-04). `Vst3Adapter` (effects): load, `InitDll`, activate, process, latency, tail, state, crash and restart, ADR-010 gate inside the bridge. `runs_sandboxed` accepts VST3, the modes and the "Isoler" button cover it.                                                                                                                                            | **high**: first VST3 on the ring; cores spinning                 | sandbox-against-in-process test on the VST3 test DLL (effect)                                        |
| 6    | **VST3 parameters** (done 2026-11-04). Parameter list and text, automation and modulation with a sample offset, the plugin's edits and gestures back (engine gestures ignored, decision b), rescan, controller follows processor (inside the bridge), modulated bases before a state save.                                                                                                                  | medium: bounded queues (128 × 128)                               | same test with automated and modulated parameters                                                    |
| 7    | **VST3 editor in the bridge** (done 2026-11-04). `IPlugView` / `IPlugFrame` / content scale in the bridge's Win32 window (same layout as CLAP), resize in the same call stack, close order.                                                                                                                                                                                                                 | none (bridge UI thread)                                          | none                                                                                                 |
| 8a   | **VST3 instrument** (done 2026-11-05). Notes and raw MIDI, `IMidiMapping`, a bridged VST3 instrument on the main stereo output; the bridge is told its role at launch.                                                                                                                                                                                                                                      | medium: bounded note list, no new call                           | sandbox-against-in-process test for the instrument, with and without MIDI controls                   |
| 8b   | **Several ports.** Aux outputs and sidechain through the port table (several active ports); `IComponentHandlerBusActivation`. **Changes the block layout: protocol v4, needs the user's approval of the design first.** Aux outputs and sidechain done 2026-11-05; `IComponentHandlerBusActivation` deliberately not supported (decision of 2026-11-06, see "Step 8b, second half"). **Step 8b is closed.** | medium-high: multi-port block layout                             | same test for multi-out and sidechain                                                                |
| 9    | **VST3 presets.** `IUnitInfo` programs and `.vstpreset` files through the ADR-031 commands; parsing stays in the app (untrusted file), loading happens in the bridge; `IUnitHandler`.                                                                                                                                                                                                                       | none (control path); gate in the bridge                          | none                                                                                                 |
| 10   | **Out-of-process scan and blocklist.** `plugin_host_bridge --scan`, timeout and crash contained, blocklist kept per machine, a way to re-enable. Replaces the in-process throwaway component of the VST3 sidechain probe. **Done 2026-11-05** (awaiting review; no screen for the blocklist yet).                                                                                                           | none                                                             | none                                                                                                 |

### Tests and what stays manual

| Step | Automated                                                                                                                                                                                                                                                                                                                                                                     | Still to test by hand                                                                                               |
| ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| 0    | The IID comparison script (kept out of the repository, in the scratch folder).                                                                                                                                                                                                                                                                                                | —                                                                                                                   |
| 1    | Moved unit tests (IIDs, layout asserts, `ParamQueue`); the new crates have an empty `[dependencies]` section (no engine dependency possible); IID test against the copied headers; full workspace suite.                                                                                                                                                                      | Open a real VST3 (TDR Nova) once in the app; start the app once (a parameter edit reaches a plugin).                |
| 2    | Test DLL loads through the existing path with the same assertions as `fake_plugin.rs`; `InitDll` / `ExitDll` order; golden `vst3_plugins` and block-size independence.                                                                                                                                                                                                        | TDR Nova and Surge XT: open, close, reload, save and reopen (the load sequence changes for every real VST3).        |
| 3    | Every moved test; real-FFI tests on TDR Nova / Surge XT with `ECHOES_TEST_VST3`; hashes unchanged.                                                                                                                                                                                                                                                                            | Audio, editor, presets, multi-out and sidechain in the app, in-process, as before.                                  |
| 4    | Protocol ABI asserts; bridge integration tests ported; `sandbox_tests.rs` unchanged and green; sandbox-against-in-process; round-trip timing.                                                                                                                                                                                                                                 | A real CLAP plugin sandboxed: play, automate, open the editor, kill the bridge.                                     |
| 5    | `sandbox_tests.rs` mirrored for VST3: processing, latency, state, toggle keeps state, reuse across recompiles, crash contained / reported / recovered, hang contained.                                                                                                                                                                                                        | TDR Nova sandboxed; end the bridge from the Task Manager during playback; mode _Tous_ with several instances (CPU). |
| 6    | Parameters, automation and modulation across the process with the test DLL; editor-follow table; state save with modulated bases.                                                                                                                                                                                                                                             | Knobs in a sandboxed window follow automation; save and reopen.                                                     |
| 7    | Bridge tests with the test DLL's view (`vst3_bridge.rs`): attach, resize inside `resizeView`, content scale, close order, one window and reopen, the controller follows automation only while open, a window-thread edit, a crash in the editor; the engine's view of an edit at a stop; the frame tests in `vst3_host`; TDR Nova's editor opens, runs with audio and closes. | Sandboxed editor on TDR Nova: open, resize, HiDPI screen, close; a crash in the editor kills only the bridge.       |
| 8a   | Instrument notes, raw MIDI and parameters with the test DLL, sandboxed; Surge XT (when installed) as loud in the bridge as in process.                                                                                                                                                                                                                                        | Surge XT sandboxed: play, automate, editor, keyboard controls, kill the bridge.                                     |
| 8b   | Multi-out and sidechain with the test DLL, sandboxed.                                                                                                                                                                                                                                                                                                                         | Surge XT's "Scene A" bus sandboxed; TDR Nova sidechain sandboxed.                                                   |
| 9    | Program change and `.vstpreset` load through the bridge with the test DLL.                                                                                                                                                                                                                                                                                                    | A program of TDR Nova; a real `.vstpreset` (none is known yet).                                                     |
| 10   | Scan of a DLL that crashes on load / in `GetPluginFactory`: the app continues, the plugin is blocklisted, the scan of the others finishes.                                                                                                                                                                                                                                    | The installed plugin set; re-enabling a blocklisted plugin.                                                         |

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
   `the_plugin_finds_the_com_apartment_of_the_thread_that_loads_it`). _Settled in step 5_: the bridge calls `OleInitialize` on its main
   thread, as `tao` does, and leaves the audio thread without COM, as the engine
   does; checked with TDR Nova running in the bridge (Surge XT is an instrument,
   step 8). See "Step 5 — what shipped". _Closed in step 6:_ the test plugin's
   diagnostics are readable through `ParamValue` now, and the bridge's audio thread was
   measured: `InitApartment` = 4 (the main single-threaded apartment `OleInitialize`
   made), `ProcessApartment` = 0 (no COM on the audio thread), exactly as in the engine
   (`the_plugin_loads_in_the_main_apartment_and_processes_without_com`).
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
   value). _Step 4 froze the units_: the handshake declares them, the CLAP adapter
   declares `ParamUnit::Plain`, and the engine refuses a bridge that declares anything
   else (`Normalised` is defined, no adapter sends it yet). **Settled 2026-11-04 (user): keep it.** Recommendation: keep `PARAM_MODULATION` in the ABI. CLAP
   needs it today (a modulatable CLAP parameter is modulated by an offset on top of its
   value, `CLAP_EVENT_PARAM_MOD`; without the kind the CLAP migration could not stay
   bit-identical). The engine would never send it to a VST3 plugin, which ADR-021
   modulates by value around a base, and the VST3 adapter of step 6 would ignore it
   (it does: `gestures_and_modulation_offsets_from_the_engine_are_ignored`).
   The alternative — send every modulation as a value — would change what CLAP plugins
   receive and is not recommended. Implemented as recommended so that CLAP stays
   identical; it costs one enumerator. The VST3 adapter of step 5 ignores it.
6. **Behaviour change of mode _Tous_.** Users who chose _Tous_ will see their VST3
   effects isolated after step 5 (extra processes, possible CPU cost). To be
   announced; the 8-instance measurement in step 5 decides whether it is acceptable.
   _Step 5 measured it_ (8 test effects: about 23 % of a core against 4 %, no late
   block; "Step 5 — what shipped"): in effect from step 5 for VST3 **effects**, and from step 8a (2026-11-05)
   for VST3 **instruments** (8 test instruments: about 10 % of a core on the audio thread and 8 % in
   the bridges against 2 %, no late block). The announcement is still to do.
7. **`.vstpreset` and `module-info.json` are not in the pluginterfaces repository.**
   Their formats live in the VST3 public SDK (a different repository). The existing
   `preset_file.rs` was therefore **not** verified in step 0; it is checked in step 9
   against that source, fetched the same way (outside the repository first).
8. **Documents to update with the steps that deliver the change** (not in step 0):
   `plugins/plugin-wrapper-system.md` (sandbox, Host sections), ADR-015 (a pointer
   here and, in its historical sub-phase text, the block header is now 40 bytes, not
   32), ADR-003 (consequence "in-process isolation" no longer true for VST3),
   ADR-014 / ADR-031 (amendments), `state_index.md` (Plugin sandbox, Plugin wrappers)
   _(ADR-015 pointer and amendment, wrapper doc, `state_index.md` and
   `change_impact_mapping.md` done in step 4)_,
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

## Step 4 — what shipped (2026-11-04)

**The proof came first.** Before any change to the protocol, two tests were written
and passed on protocol v2: `render_harness::clap_plugins::a_sandboxed_plugin_renders_like_an_in_process_one`
and `…_with_automation_and_modulation`. Each renders the `clap_plugins` project
(`test_clap_plugin` as an effect **and** as an instrument playing two notes; the second
test also automates and modulates the gain of both, so parameter events with sample
offsets cross the process) at the five block sizes of the harness, once with the
plugins in the Echoes process (isolation _Aucun_) and once with every plugin in the
bridge (isolation _Tous_, offline wait), and requires **identical samples**
(`first_difference == None`). They assert that the instances really are sandboxed and
that the automation changes the sound. They passed on v2 without touching the bridge —
no bug in the v2 bridge was found — and pass unchanged on v3. They run in the default
suite (C-10). They are staged in the git index, apart from the rest of the step, so they
can be committed first as the plan asks (the user asked for no commit).

**Round-trip time, before and after.** `sandbox_tests::measure_the_block_round_trip`
(ignored by default; `cargo test --bin echoes-studio measure_the_block_round_trip --
--ignored --nocapture`) times `process` of a sandboxed device on the test plugin, 3000
blocks after 200 of warm-up, debug build, offline wait. Three runs each.

| Device, block                   | v2 mean (p50 / p99), µs       | v3 mean (p50 / p99), µs  |
| ------------------------------- | ----------------------------- | ------------------------ |
| effect, 64 frames               | 20.5–21.2 (19.6–19.9 / 37–40) | 20.9 (19.8 / 38)         |
| effect, 512 frames              | 117–119 (114–115 / 172–177)   | 120.7 (114.7 / 187)      |
| instrument + 2 notes, 64 frames | 14.0–14.8 (13.0–14.1 / 28–34) | 14.7 (13.7–14.0 / 25–30) |
| instrument + 2 notes, 512       | 70–72 (67–69 / 118–119)       | 70–75 (68 / 109–129)     |

The v3 figures are within the run-to-run spread of the v2 ones: no measurable cost (debug
build; one v3 run had a 3 ms outlier on the 512-frame effect, the kind of scheduling
hiccup the p99 already shows on v2). The v2 numbers were taken before the first protocol
edit.

**Layout of the block message** (`plugin_bridge::protocol`, all `#[repr(C)]`, no
implicit padding, sizes and offsets asserted by tests):

| Part                         | Size                    | Content                                                                                                                    |
| ---------------------------- | ----------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `BlockHeader`                | 48 B (was 40)           | magic, version 3, frames, channel count, flags, note count, parameter-event count, reserved, sequence, steady time         |
| `WireTransport` (if flagged) | 64 B                    | playing, bar number, sample rate, position in samples / seconds / beats, bar start, tempo, time signature                  |
| `WireNote` × n               | 12 B each               | offset, kind (note on, note off, raw MIDI), channel, 3 data bytes — one ordered list, so same-offset order is the engine's |
| `WireParam` × m              | 24 B each               | offset, native id, value, kind (value, modulation offset, gesture begin, gesture end)                                      |
| samples                      | channels × frames × 4 B | as before                                                                                                                  |

Reply: the same header; the notes list is the MIDI the plugin produced (the engine does
not use it yet) and the parameter list is what the plugin did to its own parameters
(edits and gestures). Flags: input present, reset, transport present (engine → bridge);
state edited, passed through, restart requested, parameter-list bits (`PARAMS_VALUES`,
`_TEXT`, `_INFO`, `_LIST`), note names changed (bridge → engine). Total section bytes are
bounded by `event_capacity` (now 64 KiB, was 80 KiB); the engine fills at most 1024
parameter events and 256 notes, the bridge answers with at most half of the capacity
each way.

**Control channel.** The CLAP wire forms are gone. `plugin_bridge::records` defines
`ParamRecord`, `TrackRecord`, `NoteNameRecord`, `PresetRecord` and `Handshake`, with
`encode` / `decode`; a count read from the wire never sizes an allocation. New
`Op::Handshake = 12`, asked once after the start: the parameter **unit** and the audio
**port table** (direction, main / auxiliary, channels, layout, active). The engine
refuses a bridge whose unit is not plain or with two active ports on one side — the
plugin is then reported unavailable, never loaded in-process. The CLAP adapter lists the
plugin's real ports and marks the first main one of each direction active (the bus the
audio loop binds, as in v2); sidechain and auxiliary outputs are declared but not fed
yet. The bridge never converts a parameter value.

**CLAP migrated.** Engine: `ClapParamEvents::fill` was split into one body
(`emit_param_events`) over a `ParamEventSink`, so the in-process path (CLAP event
buffer) and the bridged path (wire records) run the same logic; `observe_edit` is the
neutral twin of the output-event observer. `BridgeRoundTrip` writes sections; no CLAP
type is left in `bridged_node.rs` or `bridge.rs`. Bridge: `plugin.rs` became
`clap_adapter.rs` — `ClapAdapter` (the former `LoadedPlugin`) and `ClapBlockProcessor`
keep sleep, tail, rescan, track info, note names, preset load, timers and the editor;
the audio side builds one CLAP event buffer from the sections (parameter events, then
notes, one stable sort — the in-process order) and the transport event through
`clap_ffi::transport_event`, now the single formula for both paths. CLAP in the Echoes
process is untouched except that its transport function calls that shared one.

**Real-time checklist** (`rt-validation`), engine audio thread and bridge audio thread:
no allocation (every vector is sized before the first block; the two that are pushed to
are guarded by `len < capacity`), no lock, no new system call (still one non-blocking
`SetEvent`), no `drop`, no log, no Tauri call; the bounded wait (3/4 of the block, 10 s
offline) and the watchdog are untouched. Indexing is bounded by the layout check done
before the ring write (`Skipped` otherwise). Written in `threading/realtime-safety.md`.

**C-2.** `clap_plugins` (0x928da7dcd260c4e1), `vst3_plugins` (0x5c0f0710b6813439) and
every other golden hash are unchanged; no hash was regenerated. The new tests are
identical on v2 and v3.

**Tests.** Whole workspace, `cargo test --workspace --no-fail-fast -- --test-threads=1`: green. Engine 743
(+2 ignored: the timing measure and the older `automation_cost_report`), `plugin_bridge` 44, `plugin_host_bridge`
4 + 34 integration, `clap_ffi` 87, `vst3_host` 63, `audio_buffer` 11, `vst3_ffi` 8,
`test_vst3_plugin` 8, `plugin_param_queue` 4. `sandbox_tests.rs` is unchanged apart from the
appended measure; `cargo clippy -D warnings` is clean on `plugin_bridge`,
`plugin_host_bridge` and `clap_ffi` (the workspace-wide clippy failures are the known
ones, not these files).

**Not done, on purpose / still open.**

- Hand test with a real sandboxed CLAP plugin: play, automate, open the editor, kill the
  bridge during playback. Not run.
- Gestures from the engine have no CLAP equivalent; the CLAP adapter ignores them (the
  kind exists for VST3, step 6). The MIDI a plugin produces crosses the process but the
  engine drops it, as it always did.
- A sandboxed CLAP plugin still processes one input and one output bus; the port table
  describes the rest. The editor is still the CLAP one in `editor.rs`.
- Open question 5 (the modulation-offset kind) is the user's decision, see above.
- The context bullets of this ADR describe protocol v2 as it was before this step.

## Step 5 — what shipped (2026-11-04)

**The proof came first.** `render_harness::vst3_plugins::a_sandboxed_effect_renders_like_an_in_process_one`
was written and **failed** (the effect was not isolated) before any bridge code existed. It renders the
`vst3_plugins` project with the test effect loaded at half gain (its default gain is 1, so a pass-through
would otherwise look identical), once in the Echoes process and once in the bridge (isolation _Tous_,
offline wait), at the five block sizes, and requires identical samples. It also asserts that the loaded
state changes the sound, that the effect really runs in the bridge, and that the instrument is **not**
sandboxed. It passes in the default suite (C-10). `vst3_plugins` (0x5c0f0710b6813439), `clap_plugins`
(0x928da7dcd260c4e1) and every other golden hash are unchanged; none was regenerated. The protocol did
not change: no format change of the block message, version still 3.

**Decision taken first (open question 5, user, 2026-11-04):** `PARAM_MODULATION` stays in the ABI. The
VST3 adapter ignores it.

**Bridge** (`crates/plugin_host_bridge`).

- `vst3_adapter.rs`: `Vst3Adapter` (main thread: load through `vst3_ffi::Vst3Library`, so `InitDll` /
  `ExitDll` run once per file; create component and processor; activation in the spec order — bus
  arrangements, `activateBus`, `setupProcessing`, `setActive`, `setProcessing`; latency; component
  state save / load; teardown) and `Vst3BlockProcessor` (audio thread). One stereo main bus each way;
  `vst3_host::buses` binds the buffers, exactly the code the in-process wrapper uses.
- `ProcessContext` is built from `WireTransport` with `vst3_host::transport::process_context`, the same
  function as the in-process path. A `PARAM_VALUE` event becomes an `IParameterChanges` point (the value is
  already normalised: **the bridge converts nothing**); modulation and gesture kinds are ignored.
- **Gate (ADR-010) inside the bridge.** `process` and every off-RT call (`setActive`, `getState`,
  `setState`, teardown) go through one atomic gate. The audio thread never waits: if the main thread holds
  the gate it passes the block through (`BLOCK_PASSED_THROUGH`); the main thread waits up to 2 s. The
  activation (buffers, bus tables) lives behind the same gate, so a reactivation cannot race a block.
- `main.rs`: a `HostedPlugin` trait (the control ops of one format) and `--plugin-format clap|vst3`
  (default `clap`, so a v3 launch line of step 4 still works). The audio thread gets a
  `Box<dyn BridgeProcessor + Send>`; the CLAP path is the same code as before behind that box.
- **Handshake**: `ParamUnit::Normalised` and the plugin's real port table (main bus active each way,
  auxiliary buses listed and inactive).
- **COM (open question 2, settled).** `OleInitialize` on the bridge's main thread, the thread that loads
  the plugin, as `tao` does for the app's main thread; the audio thread has no COM initialisation, as in
  the engine. TDR Nova renders identically in the bridge (test below). The bridge's audio-thread apartment
  was not read from a plugin (the test DLL records it only through parameters, which are step 6): "no COM
  on the audio thread" is the same convention as the engine, verified only by TDR Nova working.
- **Crash / hang.** No SEH guard is needed: a plugin that faults kills the bridge, which is the point; the
  engine's watchdog (unchanged) condemns it after 8 late blocks.

**Not in this step, on purpose** (each answers an explicit error or an empty list, never a silent fallback;
_the parameter part was delivered by step 6_):
parameter lists, text, automation and modulation of a sandboxed VST3 effect (the node writes **no parameter
event** for VST3; `Params` is empty, `ParamValue` / `PresetLoad` / `OpenEditor` are refused) — step 6, 7, 9;
no edit controller in the bridge, so no `kLatencyChanged` / restart request and no controller sync after a
state load; no VST3 instrument in the sandbox (`instrument_runs_sandboxed` is CLAP only) — step 8; no
sidechain or auxiliary output (the port table lists them, inactive); no per-plugin tail or processing mode
(not used in-process either). A parameter pushed to a sandboxed VST3 effect is queued and never delivered
until step 6.

**Engine.** `runs_sandboxed` accepts VST3 (effects); `PluginInstance::new_bridged` takes the format;
`BridgedPlugin::spawn` takes `(format, native id)`, passes `--plugin-format`, and `check_handshake` now
requires the unit of the **format** (plain for CLAP, normalised for VST3) — a bridge that declares anything
else is refused and the plugin reported unavailable, never loaded in-process. `host.rs` builds a
`BridgedNode` for a sandboxed VST3 effect (new and reused). `bridged_node.rs` skips the parameter events for
VST3. Frontend: the "Isoler" button, its context menu and `isSandboxed` cover VST3 effects
(`EffectChain.svelte`, no new element); the settings hint now reads "CLAP and VST3 effects, and CLAP
instruments, for now." (`isolationSupportedFormats`).

**Mode _Tous_ (open question 6).** After this step a user who chose _Tous_ gets their VST3 **effects** in
bridges (instruments stay in-process until step 8). Measured below; not announced anywhere in the app yet.

**Measures** (debug build, the test effect, offline wait, 3000 blocks after 200 of warm-up,
`measure_the_vst3_block_round_trip`, ignored by default):

| Block      | VST3 in-process mean (p50 / p99), µs | VST3 sandboxed mean (p50 / p99), µs | CLAP sandboxed, step 4 |
| ---------- | ------------------------------------ | ----------------------------------- | ---------------------- |
| 64 frames  | 4.1 (4.0 / 5.2)                      | 24.4 (22.8 / 46.3)                  | 20.9 (19.8 / 38)       |
| 512 frames | 28.9 (28.6 / 38.7)                   | 145.1 (138.0 / 220.3)               | 120.7 (114.7 / 187)    |

There is no "before" for a sandboxed VST3 block; the sandbox adds about 20 µs (64 frames) to 115 µs (512) to the
test effect, the same order as CLAP (the VST3 adapter copies the block in and out of its own buffers).

**Eight sandboxed effects** (`measure_eight_vst3_effects`, ignored; 512-frame blocks at the live pace for
5.3 s, the engine thread's CPU and the bridges' CPU as a share of one core, two runs):

| 8 test effects | audio thread  | bridges     | late blocks |
| -------------- | ------------- | ----------- | ----------- |
| in-process     | 3.8 – 4.7 %   | —           | 0 of 500    |
| sandboxed      | 13.2 – 14.1 % | 9.1 – 9.4 % | 0 of 500    |

About 23 % of one core in total against 4 %, on a 24-thread machine, with a plugin that does almost no work:
the engine's wait spins for as long as the bridge works, so the audio-thread figure grows with the plugin's
own cost, and the bridges' figure with it. **The budget was not amended**: the wait is bounded and no block
was late. This is a measure with a trivial plugin; the _Tous_ mode with several real plugins on a smaller
machine is on the hand-test list.

**Real plugin.** `vst3_sandbox_tests::a_real_vst3_effect_renders_in_the_bridge_like_in_the_echoes_process`
(skipped when no VST3 is installed; `ECHOES_TEST_VST3` selects one): TDR Nova renders 32 768 samples in the
bridge **identical** to the Echoes process. Surge XT is an instrument and waits for step 8.

**Open question of step 3 (scan on one thread, instantiation on another).** Looked at: `scan_plugins` and
every instantiation are `&mut PluginHost` methods of the engine, which is behind the `audio_engine` mutex
in the app (startup scan: the engine's constructor, before any command). They cannot overlap, by
construction. What crashed in the tests was true parallelism, which the app does not have. A scan on one
thread followed by an instantiation on another is possible and not shown to be a problem.

**Tests.** `plugin_host_bridge/tests/vst3_bridge.rs` (8: handshake, audio and normalised parameter values,
state, re-activation, explicit refusals, crash, hang, unknown class), `plugin_system/vst3_sandbox_tests.rs`
(9 + 2 ignored measures: processing and latency, state, toggle keeps state, reuse across recompiles,
isolation preference, **bridge killed from outside** (contained, reported _Crashed_, recovered with its last
state), **bridge frozen** (contained, no block waits more than 3/4 of its period), cannot start → unavailable
and never in-process, TDR Nova). The crash tests kill or freeze the bridge instead of using the plugin's crash
and hang parameters, which a sandboxed VST3 effect cannot be sent before step 6. Whole workspace,
`cargo test --workspace --no-fail-fast -- --test-threads=1`: green — engine 753 (+4 ignored: the two VST3 measures and two older ones), plugin_bridge 44, plugin_host_bridge 4 + 34 + 8 integration, clap_ffi 87, vst3_host 63, audio_buffer 11, vst3_ffi 8, test_vst3_plugin 8, plugin_param_queue 4.

**Real-time checklist** (`rt-validation`). Engine audio thread: the message format and the wait are those of
step 4, untouched; the VST3 path only skips the parameter section (fewer events). Bridge audio thread
(`Vst3BlockProcessor::process`): no allocation (the activation sizes the input, the output, the bus tables and
the parameter-change list; `HostParameterChanges::reset` / `push` fill in place), no lock (an atomic
compare-exchange, never waited on), no I/O, no log, no `drop` (the `Activation` is replaced only under the
main-thread gate, never by the audio thread), no new system call. `process_context` builds on the stack.
Indexing is bounded by the check `frames <= max_frames` before the copy; a block outside the activation passes
the input through. `HostParameterChanges::push` is bounded and drops what does not fit, as in-process.

**Not hand-tested** (yours): TDR Nova sandboxed in the app (play, kill the bridge from the Task Manager
during playback); mode _Tous_ with several real instances (CPU); the "Isoler" button on a VST3 effect.

## Step 6 — what shipped (2026-11-04)

**The proof came first.** Before any change to the bridge or the engine, three tests were added to
`render_harness::vst3_plugins` and **failed**: `a_sandboxed_effect_renders_like_an_in_process_one_with_automation`,
`…_with_modulation` and `…_with_automation_and_modulation`. Each renders a **new** project (the
`vst3_plugins` project is untouched) where the test effect's gain is driven by a lane, by an LFO route, or by both,
once in the Echoes process and once in the bridge (isolation _Tous_, offline wait), at the five block sizes, and
requires identical samples. They also require the driven render to sound unlike the plain one. They failed at the
first sample the effect processes (frame 96 000, 1.0 s: the audio clip). They are split in three because VST3
automation takes precedence over modulation on the same parameter, and the test effect has one parameter that sounds.
They run in the default suite (C-10) and pass. `vst3_plugins` (0x5c0f0710b6813439), `clap_plugins`
(0x928da7dcd260c4e1) and every other golden hash are unchanged; none was regenerated. The protocol did not change:
no format change of the block message, no new `Op`, no new record field, version still 3.

**Decisions taken first (user, 2026-11-04, the recommendations):**

- **a) The base of a value-modulated parameter lives in the engine.** The bridge receives the effective normalised
  value of each point and computes nothing.
- **b) Gestures from the engine are ignored** by the bridge, with a test that says so
  (`gestures_and_modulation_offsets_from_the_engine_are_ignored`, together with the modulation offset kind). The engine
  does not send them either: no editor produces any before step 7.
- **c) A loaded state is handed to the controller** (`sync_controller_to_state`, the in-process behaviour). That helper
  only acts on a separate controller; the test DLL is one object, so no test exercises the call. It is the same code as
  `load_state_blob`, not a new path.

**Bridge** (`crates/plugin_host_bridge/src/vst3_adapter.rs`).

- `Vst3Adapter::load` creates the edit controller as `create_vst3_instance` does (single component or separate, the
  host context, the component handler); without one the plugin still plays and lists no parameter. The handler is the
  `vst3_host::component_handler` one, wired to a `BridgeSignals` (`HostSignals`): the unsaved, restart and
  parameter-list signals are atomics the audio thread turns into reply flags (`BLOCK_PLUGIN_EDITED`,
  `BLOCK_RESTART_REQUESTED`, `PARAMS_TEXT` / `PARAMS_INFO` in the rescan bits), once.
- Control ops: `Params` (`vst3_host::params::read_vst3_params`: normalised `0..1`, default, automatable, hidden, never
  modulatable, no module — as in process), `ParamValue`, `ParamText` (empty when the plugin has no text),
  `FlushValues`. All of them take the ADR-010 gate (`enter_main`): the audio thread skips its block meanwhile, it never
  waits. `Activate` reactivates once when the plugin asked for it (`restartComponent` with a latency, I/O or reload flag:
  `take_reactivation_request`), which reads the latency again; otherwise it stays a no-op at the same size.
- Audio thread (`Vst3BlockProcessor::process`): the engine's `PARAM_VALUE` points become `IParameterChanges` points,
  in the order received, with their sample offsets (`HostParameterChanges`, bounded 128 × 128, what does not fit is
  dropped). The plugin window's edits (`EditState::drain_edits`) are appended **last**, so the user's value wins within
  the block, forwarded to the processor and reported in the reply as `PARAM_VALUE` records. A new
  `EditState::active_gestures` (no allocation) gives the gestures; their begin and end are reported as
  `PARAM_GESTURE_BEGIN` / `_END` records by comparing with the last block.
- `outputParameterChanges` is read after `process` and kept for the controller (the atomic table of
  `vst3_host::param_mirror`, **only while an editor watches**), as in process. **Deviation from the brief, to confirm:**
  they are _not_ reported to the engine as edits. In process they neither mark the project unsaved nor move a
  modulation base (a meter or a program change would otherwise mark every block), so the bridge does the same.
- The controller follows the processor inside the bridge: the table is filled by the audio thread, applied by
  `Vst3Adapter::mirror_to_controller` on the window thread without the gate, and `watch_params` gives the controller
  the processor's state when a window opens. No window exists before step 7, so this has no visible effect yet; the
  code and its tests (`the_controller_follows_the_processor_only_while_a_window_watches`,
  `a_value_the_plugin_writes_to_its_output_reaches_the_controller_of_an_open_window`) are in.
- **Flush and save are one step in the bridge.** `FlushValues` puts the parameters modulated by value on their base
  with a zero-sample `process` (the parameter flush `ProcessData` allows) and remembers them; the next `SaveState`
  applies them again inside its own gate hold, just before `getState`, and forgets them. Without that, a block with
  modulation could land between the two requests and the state would hold a point of the modulation (CLAP in the
  bridge has that window; it is not closed here). `LoadState` forgets them too.

**Engine.**

- `plugin_formats/vst3/param_delivery.rs`: the logic of `fill_param_changes` is split behind a `Vst3ParamSink`
  (`HostParameterChanges` in process, a wire section for the bridge). `emit_control_changes` — modulation around the
  base, automation first, a drag pauses the modulation — and `settle_followed` — a newly modulated parameter starts from
  the plugin's value, one no longer modulated goes back to its base — are shared by both paths. The in-process render is
  bit for bit the same (`vst3_plugins` golden hash).
- `Vst3ParamEvents` is the bridged twin of `ClapParamEvents` (`set_controls`, `fill_into`, `observe_edit`); an edit moves
  the base of a followed parameter, a gesture pauses its modulation. `bridged_node.rs` picks CLAP or VST3 logic by
  format (`BridgedParams`); the `format == Clap` conditions are gone. The two formats never share a unit: CLAP events
  carry plain values and `PARAM_MODULATION`, VST3 events normalised `PARAM_VALUE` only.
- The bases of the value-modulated parameters are kept in `PluginInstance::value_modulated_params` with the
  normalised range `0..1`, so `save_state_blob` (restore the bases, then save) and the preset rebase work for VST3
  as they do for CLAP; no change to `instance.rs`. The parameter list, the text and the values of the UI come from
  the bridge (`BridgedPlugin::params`, `param_text`, `param_value`), already format-neutral since step 4.
- **One block of lag, by design.** The engine learns of a drag one block after it started, so a modulation point can
  reach the plugin in that block; the user's edit is last in the block and wins, then the modulation pauses. Releasing
  is symmetric.

**Test plugin** (`crates/test_vst3_plugin`; the `vst3_plugins` hash does not move). New parameters: 7 `Self value`
(the controller and the processor each keep their own copy, outside the state, so a host that does not tell the
controller what the processor got shows it), 12 `Processor self` (reads the processor's copy), and four things the
plugin does by itself when the processor receives a value: 8 `Write output` (puts a new `Self value` in
`outputParameterChanges`), 9 `Hold edit` (`beginEdit`, `performEdit`, `endEdit` when released, through the host's
handler), 10 `Lookahead` (latency 64 ↔ 128 and `restartComponent(kLatencyChanged)`, applied at the next
`setActive`), 11 `Rename` (new title of `Self value`, `restartComponent(kParamTitlesChanged)`). A zero-sample
`process` now applies its points (a flush). The edits are made from the processor's thread, which the handler allows
(lock-free); a real editor thread comes with step 7.

**Real-time checklist** (`rt-validation`). The block message, the rings, the wake-up, the bounded wait (3/4 of the
period, 10 s offline) and the one non-blocking `SetEvent` are untouched. Engine audio thread:
`Vst3ParamEvents::fill_into` allocates nothing (the wire section is sized before the first block and full means
dropped; `editing` has a fixed capacity, `retain` and `push` stay within it), no lock, no new system call.
`observe_edit` runs after the reply, bounded by the 256 edits the reply keeps. Bridge audio thread: no allocation
(`HostParameterChanges`, the output list and the reply slice are preallocated, `active_gestures` returns an array),
no lock (atomics only: `BridgeSignals`, the gate, the edit queue is the SPSC `ParamQueue`), no I/O, no log, no `drop`,
no new system call; the plugin's `restartComponent` / `performEdit` callbacks only store atomics. The gate (ADR-010)
stays the rule: every control op that touches the plugin (`Params`, `ParamValue`, `ParamText`, `FlushValues`,
`SaveState`, `LoadState`, `Activate`) holds it, the audio thread skips its block instead of waiting. The mirror
table is filled only while a window watches, so there is no cost before step 7.

**C-2.** The new tests pass, `vst3_plugins`, `clap_plugins` and every hash are unchanged, none was regenerated. A real
plugin, TDR Nova, lists the same 76 parameters, renders the same text and the same samples with its parameter 50
automated, in the bridge and in the Echoes process (`a_real_vst3_effect_lists_and_follows_its_parameters_in_the_bridge_like_in_the_echoes_process`,
skipped when no VST3 is installed; `ECHOES_TEST_VST3` selects one).

**Tests.** Whole workspace, `cargo test --workspace --no-fail-fast -- --test-threads=1`: green, no failure — engine 779 (+4 ignored: the two VST3 measures and two older ones), `plugin_bridge` 44, `plugin_host_bridge` 7 + 34 + 20 integration, `clap_ffi` 87, `vst3_host` 64, `audio_buffer` 11, `vst3_ffi` 8, `test_vst3_plugin` 12, `plugin_param_queue` 4. New: the three sandbox-against-in-process renders; in `vst3_bridge.rs` the parameter list, value, text, several points, flush before save, the plugin's edit and gesture, output changes, restart and latency, new titles, ignored engine gestures, refused unknown ids, the COM apartments; in `vst3_adapter.rs` the table (3); in `param_delivery.rs` the shared logic (6); in `vst3_sandbox_tests.rs` the list, text, push, automation and modulation, base before save, latency, titles, window edit and drag, each in process and in the bridge, and TDR Nova's list and an automated parameter; in the test plugin 5. `cargo clippy -D warnings` is clean on `plugin_host_bridge`, `test_vst3_plugin` and `plugin_bridge`; `vst3_host` and the engine crate keep their older warnings (none in the files of this step).

**Not done, on purpose / still open.**

- **Hand tests (yours):** TDR Nova isolated — the parameter buttons appear and follow the automation, an automation
  curve and an LFO on a parameter, save and reopen (the base is the user's, not a point of the modulation), and the
  step-5 list (kill the bridge during playback, the "Isoler" button, mode _Tous_ with several instances).
- `Op::TrackInfo` still answers empty for a sandboxed VST3 (the controller's `IInfoListener` is not wired in the
  bridge; wired 2026-11-24, see "Amendment 2026-11-24"); `Op::NoteNames`, `PresetLoad` and `OpenEditor` are as in
  step 5.
- `outputParameterChanges` are not edits (see above); no real plugin is known to write any, so this is tested with the
  test plugin only.
- The call that hands a loaded state to a **separate** controller is the in-process helper; the test DLL cannot
  exercise it. TDR Nova's controller was not checked for being separate.
- `take_crash_suspect` in `main.rs` still offers "Isoler" for CLAP plugins only (`can_isolate`): a VST3 effect that
  crashed in process is not offered the button, although the sandbox now takes it. A step-5 leftover, not changed
  here.
- Gestures from the engine (`IEditControllerHostEditing`, Appendix B) are not implemented (decision b).

## Step 7 — what shipped (2026-11-04)

**The proof came first.** Before any change to the bridge, the engine or `vst3_host`, the test plugin got
a view (`crates/test_vst3_plugin`, parameters 13 to 19) and
`the_editor_of_a_sandboxed_effect_opens_in_a_bridge_window_and_closes_in_order` was put in the git index. It
**failed** with the explicit error step 6 left (`the editor of a sandboxed VST3 plugin is not available yet`)
and passes now. It runs in the default suite (C-10): it opens the bridge's real window and looks at it from the
test process (the owner of the window is the bridge, the plugin area has the size the view asked for), then posts
`WM_CLOSE` like the × button and reads the order of the calls the view recorded. The window is shown, like the one
the existing CLAP editor test opens; there is no hidden mode. No audio is rendered, so C-2 is not concerned:
`vst3_plugins` (0x5c0f0710b6813439), `clap_plugins` (0x928da7dcd260c4e1) and every other hash are unchanged, none
was regenerated. The protocol did not change: no format change of the block message, no new `Op`, no new record
field, version still 3.

**Questions settled first (user, 2026-11-04, the recommendations):**

- **a) Where the window and the frame live.** The neutral part moved into `vst3_host::editor_view`
  (`EditorView`, the `IPlugFrame` object, `MAX_RESIZE_DEPTH`); the engine (`plugin_system/gui.rs`) and the bridge
  (`plugin_host_bridge/src/editor.rs`) keep their own Win32 window and each give the frame a function that sizes
  it. No duplicate of the frame, and the crate still depends on the bindings only.
- **b) The thread of the view.** The bridge's main thread, the one that loaded the plugin, called `OleInitialize`
  and already pumps the Win32 messages. The bridge's audio thread never touches the view or waits for the window.
- **c) Optional interfaces of appendix B** (`IComponentHandler3` / `IContextMenu`, `IDataExchange`): **not
  added.** Plugins fall back without them; TDR Nova, the real effect used by the tests, opens and closes without them
  (the other real VST3 here, Surge XT, is an instrument: step 8). If a real plugin turns out to need one it is
  added by an amendment.
- **d) `can_isolate`** in `take_crash_suspect` (`src/main.rs`): fixed. The flag now comes from
  `plugin_system::host::can_run_sandboxed(format, is_instrument)`, which asks the two rules that already decide
  (`runs_sandboxed`, `instrument_runs_sandboxed`), so a VST3 **effect** that crashed in process is offered
  "Isoler" and a VST3 instrument is not (its sandbox is step 8).

**`vst3_host::editor_view`** (new, no dependency on the engine). `EditorView::create` makes the view
(`createView("editor")`, `isPlatformTypeSupported("HWND")`, `setFrame`), `size` reads `getSize`, `attach` stores the
window in the frame and calls `attached`, `apply_content_scale` hands the monitor scale to
`IPlugViewContentScaleSupport` (when the view has it) and returns the size at that scale. `Drop` is the close order:
**`setFrame(null)`, then `removed()`, then `release`**. The frame is the code that was in `gui.rs` (`resizeView`
answers in the same call stack: `getSize` before, window resized, `getSize` after, `onSize` only for a view that did
not follow, nesting capped at 3), parameterised by a `ResizeWindow` function instead of the engine's
`resize_to_client`. Its two unit tests moved with it.

**Bridge** (`crates/plugin_host_bridge`).

- `editor::open_vst3` (main thread): `watch_params()` first, so the controller takes the processor's state and the
  window opens on current values; the view is made by `Vst3Adapter::open_view` under the ADR-010 gate (the audio
  thread skips its block meanwhile, it never waits); the window is the same borderless window as CLAP's (drag
  strip, rounded corners, plugin area as its only child); `attached`, then the content scale and the size at that
  scale, centred, a 10 ms `WM_TIMER`, shown. One window per bridge: opening again brings it to the front.
- The window's `WM_TIMER` calls `Vst3Adapter::mirror_to_controller` (no gate, the controller is called from its own
  window thread), so the knobs follow automation and modulation. `WM_DPICHANGED` repeats scale → size. Closing
  (`editor::close`) kills the timer, drops the session (view, then the parameter watch, then the adapter handle) and
  only then destroys the window. The `allow(dead_code)` marks of step 6 are gone.
- `Vst3Adapter::load` now returns an `Arc` built with `Arc::new_cyclic`, because the open editor keeps the adapter
  alive (`Op::OpenEditor` is answered by `Vst3Adapter::open_editor`).

**Engine.** `open_vst3_editor` in `plugin_system/gui.rs` uses `EditorView`; its behaviour is the one step 7 found:
the three `vst3_resize_tests` (call order with the `FakeView`) pass (only their imports changed). `BridgedPlugin::open_editor` returns
the window handle (the reply already carried it) so a test can look at the window; `open_plugin_editor` ignores it.
A sandboxed VST3 plugin already took the `instance.bridge()` branch of `open_plugin_editor`, so nothing else changed.

**Test plugin** (`crates/test_vst3_plugin`; the `vst3_plugins` hash does not move, ids after 12 only). The view is a
window-less object recording what the host does to it in readable parameters: 13 `View calls` (the last six calls as
decimal digits: 1 `setFrame` with a frame, 2 without, 3 `attached`, 4 `onSize`, 5 `removed`, 6 / 7 `resizeView`
accepted / refused, 8 content scale, 9 last release), 14 and 15 `View width` / `View height`, 16 `View scale` (× 100),
and three the host sets beforehand: 17 `View ask` (`width * 1000 + height`, asked from inside `attached`),
18 `View crash` (the next `attached` aborts), 19 `View edit` (the next `attached` edits `Self value` from the window
thread). It holds a reference on the plugin, so a host that released the controller first would not free it under
the view. The parameter count in the two lists tests went from 13 to 20.

**D — edits made in the window.** They take the step 6 path (`EditState`, the SPSC edit queue, `BridgeSignals`), which
is lock-free and does not care which thread the plugin calls from, so they mark the project modified and move the
base of a value-modulated parameter as they do in process (`an_edit_made_in_the_window_of_the_bridge_reaches_the_engine_while_stopped`,
a window-thread edit with the transport stopped). **The engine sends blocks while the transport is stopped:** the
CPAL callback calls `Scheduler::process_block` for every callback and `is_playing` is only a field of
`TransportInfo` (`audio_engine/engine.rs`), and a VST3 effect has no sleep logic (that is CLAP). So the reply flags
come back at a stop as well; nothing in the message changes. They do not come back when no audio device is open.

**E — a crash or a hang in the editor.** The editor is on the bridge's main thread, so a crash ends the process: the
engine's ordinary path reports it (late blocks, then `UnavailableReason::Crashed`, `note_crash`) and `open_editor`
returns an error; the instance stays the bridge's, never loaded in process
(`a_crash_in_the_sandboxed_editor_is_reported_and_never_falls_back_in_process`, and `a_crash_in_the_editor_only_kills_its_bridge`
at the protocol level). A **hang** in the editor freezes the bridge's main thread, which the bridge's audio thread does not use (the gate
is held only for the controller calls, and `createView` is one of them). The control request then times out as for
any frozen bridge. That case has no test of its own; the frozen-bridge test of step 5 covers the engine's side.

**Real-time checklist** (`rt-validation`). The audio thread of neither process changes: same block message, rings,
wake-up, bounded wait and single non-blocking `SetEvent`; no new system call on either audio thread. All of the
view (`createView`, `attached`, `onSize`, `removed`, the frame) runs on the bridge's main thread; the plugin's
`resizeView` callback only resizes the window on that thread, `performEdit` / `restartComponent` only store atomics.
The ADR-010 gate is still the rule: the one controller call added (`createView`) holds it, the audio thread skips a
block instead of waiting; the view calls that follow are not controller calls and, as in process, are not gated. The
mirror table is written by the audio thread only while a window watches, as in step 6.

**Tests.** The new tests and the unchanged ones are listed in the table above. New: in `vst3_bridge.rs` the staged
proof, a size asked while attaching (window and `onSize` inside one `resizeView`), the content scale equal to the
window's DPI, one window and a reopen, the controller following automation only while the window is open (checked to
fail without the timer), a window-thread edit, a crash in the editor; in `vst3_sandbox_tests.rs` the engine's view of
a window-thread edit with the transport stopped, a crash in the editor, and TDR Nova's editor opening in the bridge,
running with audio, and closing (skipped when none is installed; `ECHOES_TEST_VST3` selects another); in `vst3_host`
the two frame tests; in the test plugin four view tests.

**Not done, on purpose / still open.**

- **Hand tests (yours):** TDR Nova isolated — open, resize, a HiDPI screen, close, reopen; the knobs move with the
  automation while the window is open; a crash in the editor kills only the bridge. And the lists of steps 2 to 6.
- `canResize` / `checkSizeConstraint` are still not used (the windows are borderless, as in process).
- The `WM_DPICHANGED` path in the bridge has no automated test (it needs a second monitor); the scale at open does.
- A hang inside `attached` is covered by reasoning and by the step 5 frozen-bridge test, not by a test of its own.
- `Op::TrackInfo` still answers empty for a sandboxed VST3 (wired 2026-11-24, see "Amendment 2026-11-24"),
  `Op::NoteNames` and `PresetLoad` are as in step 6.
- Gestures from the engine stay ignored (decision 9): the editor did not need them.
- The "Editor Windows — layout" section of the wrapper doc said 8 px and "no native close button"; the code has an
  18 px strip and a × button. Corrected in this change.

## Step 8a — what shipped (2026-11-05)

**Step 8 was split in two (user, 2026-11-05).** Before any code, the block message was read: it carries one
`channel_count` and one input and one output buffer (`BlockHeader`, `BridgeLayout.channels`), and
`Handshake::active_port` refuses a second active port on one side. Auxiliary outputs and a sidechain need
several audio buffers per block, a layout change, hence protocol v4: the work stopped and the user chose to
split. **8a (this section)** is the VST3 instrument with one stereo output: notes, raw MIDI, `IMidiMapping`, the
bridged instrument. **8b** (auxiliary outputs, sidechain through the port table, `IComponentHandlerBusActivation`,
protocol v4, `BRIDGE_PROTOCOL_VERSION` 4) waits for the user's approval of its design. The block message, the
rings, the wake-up, the bounded wait, the control ops and the records are untouched: version still 3.

**The proof came first.** `render_harness::vst3_plugins::a_sandboxed_instrument_renders_like_an_in_process_one` was
put in the git index (that file only) **before any other change** and failed with "the instrument must really run in
the bridge". It renders the `vst3_plugins` project (the test instrument plays two notes) at the five block sizes,
in the Echoes process and with every plugin in the bridge, and requires identical samples. It runs in the default
suite (C-10). Second proof, written once the test plugin could show it:
`…_with_midi_controls` (the mod wheel and a pitch bend on the first note, which change the sound in process, and the
bridge must match). The former assertion "a VST3 instrument is not sandboxed before step 8" of the effect test was
removed.

**Questions settled first (user, 2026-11-05, the recommendations):**

- **a) Skipped blocks.** The bridge keeps the NoteOffs of a block it skipped (the main thread held the gate) and
  sends them first in the next block, as `VstInstrument` does; a reset releases the sounding notes.
- **b) Where the note code lives.** `vst3_host::notes` (`SoundingNotes`, `NoteMessage`, `push_events`), moved out of
  `plugin_formats/vst3/instrument.rs`: the engine's `VstInstrument` converts its `MidiEventList` to `NoteMessage`s
  and the bridge converts its `WireNote`s, so one function builds the events for both. Its unit tests moved with it.
- **c) Test plugin.** `IMidiMapping` on the instrument's controller (CC 1 → parameter 20 `Mod wheel`, pitch bend →
  21 `Bend`), the instrument plays `velocity × gain × (1 + wheel) × (1 + bend − 0.5)` (exactly `velocity × gain` at
  the defaults 0 and 0.5), and 22 `Last pitch` (diagnostic). Ids 0 to 19 untouched; the parameter count of the lists
  went from 20 to 23. `vst3_plugins` (0x5c0f0710b6813439) did not move.
- **d) CPU of eight instruments**, measured (below).

**Decided while coding (to confirm).** The bridge needs to know whether it hosts an instrument (events in, no audio
input, reset only releases the notes). It first inferred it from the buses ("event input and no audio input"); the
real-plugin test showed that wrong — **Surge XT has an audio input** and stayed silent. The engine now says it at
launch: `--plugin-role instrument` (absent = effect; `BridgedPlugin::spawn` takes a `PluginRole`). It is a launch
argument, not part of the block message, the handshake or the version check.

**Engine** (`src-tauri/src`). `runs_sandboxed` covers instruments and effects (`instrument_runs_sandboxed` removed);
`can_run_sandboxed(format)` no longer asks whether the plugin is an instrument; `instantiate_instrument_placement`
builds the bridged instance for CLAP and VST3 and ADR-016 reuse compares `is_sandboxed()` with the same rule;
`instrument_device` gives any sandboxed instance a `BridgedInstrument`. `BridgedInstrument::latency_samples` reports
the plugin's latency for VST3 (a VST3 instrument does in process, `InstrumentNode` feeds it to the PDC) and 0 for CLAP
(as `ClapInstrument`); without it the sandboxed render was 64 samples early. The crash dialog's `can_isolate` is
true for a VST3 instrument.

**Bridge** (`crates/plugin_host_bridge/src/vst3_adapter.rs`). With the instrument role: `HostEventList` and the
`IMidiMapping` table (`MidiControllerMap`, read at each activation, under the gate) belong to the activation;
the block's notes become events (`push_events`), its raw MIDI controls become points (`push_mapped_controls`),
after the engine's parameter points and before the window's own edits; `numInputs` is 0; a reset calls
`release_all` instead of `setProcessing(false/true)`. `SoundingNotes` belongs to the processor, not to the
activation, because the gate that guards the activation is what makes a block skip.

**Real-time checklist** (`rt-validation`). No new system call, no wait, no allocation on either audio thread (the
event list and the table are made at activation; `SoundingNotes` is two 128-entry arrays); the bounded wait, the
watchdog, the rings and the single non-blocking `SetEvent` are unchanged. Written in `threading/realtime-safety.md`.

**C-2.** `vst3_plugins` (0x5c0f0710b6813439), `clap_plugins` (0x928da7dcd260c4e1) and every other golden hash are
unchanged; none was regenerated. The sandbox renders of the instrument are identical to the in-process ones at the
five block sizes, with and without mod wheel and pitch bend.

**CPU of eight instruments** (`measure_eight_vst3_instruments`, debug build, 500 blocks of 512 frames at the live
pace, three runs; the figures of the eight effects measured in the same sessions are beside them):

| 8 plugins               | audio thread (% of a core) | bridges (% of a core) | late blocks |
| ----------------------- | -------------------------- | --------------------- | ----------- |
| instruments, in process | 1.5 – 2.9                  | —                     | 0 of 500    |
| instruments, sandboxed  | 9.4 – 10.3                 | 7.0 – 8.8             | 0 of 500    |
| effects, in process     | 3.5 – 4.4                  | —                     | 0 of 500    |
| effects, sandboxed      | 15.2 – 15.8                | 11.1 – 12.3           | 0 of 500    |

Mode _Tous_ now isolates VST3 instruments too; the cost is below that of the effects and no block was late. The
announcement of that change is still to do (open question 6).

**Surge XT** (the first real VST3 instrument in the bridge; `ECHOES_TEST_VST3` must name the file inside the bundle,
`…\Surge XT.vst3\Contents\x86_64-win\Surge XT.vst3`, the search is too shallow for the bundle): it plays in the
bridge at the same loudness as in the Echoes process (RMS 0.0578 against 0.0576). **It is not deterministic** — two
runs in the same process differ — so no bit-for-bit claim is made for it; the test instrument carries that proof.

**Tests.** `vst3_host::notes` (the moved tests); `vst3_adapter.rs` (notes at their offset, a NoteOff of a skipped
block, a reset, the mod wheel through `IMidiMapping`, an effect is given no notes); `test_vst3_plugin` (the mapping,
the shaping, the last pitch); `vst3_instrument_sandbox_tests.rs` (preference, latency, a killed bridge reported and
recovered, no in-process fallback, a reset in both places, Surge XT); the two harness renders;
`measure_eight_vst3_instruments` (ignored). Whole workspace, `cargo test --workspace --no-fail-fast -- --test-threads=1`: green. Engine 780 (+5 ignored: the timing measures), `plugin_bridge` 44, `plugin_host_bridge` 12 + 34 + 27 integration, `clap_ffi` 87, `vst3_host` 74, `audio_buffer` 11, `vst3_ffi` 8, `test_vst3_plugin` 18, `plugin_param_queue` 4. The parameter count of the test plugin (20 → 23) was updated in `vst3_bridge.rs` and `vst3_sandbox_tests.rs`. `cargo clippy` shows nothing new (the warnings left are in `vst3_host` files this step did not touch).

**Not done, on purpose / still open.**

- **Step 8b**: auxiliary outputs and sidechain through the port table, `IComponentHandlerBusActivation`, protocol v4.
  A track still takes only the main stereo output of a sandboxed instrument.
- **Hand tests (yours):** Surge XT isolated in the app — play, automate a parameter, open the editor, mod wheel and
  pitch bend from a keyboard, kill the bridge during playback, mode _Tous_ with several instruments.
- A block the **engine** cannot send (`Skipped`: too many events, port busy) still drops its notes, a NoteOff
  included, as the CLAP bridge always did; only the block the bridge skips keeps its NoteOffs.
- `Op::TrackInfo` still answers empty for a sandboxed VST3 plugin (wired 2026-11-24, see "Amendment 2026-11-24"),
  `Op::NoteNames` too; presets are step 9; the
  plugin's own MIDI output (`outputEvents`) is not read, in process or in the bridge.
- An instrument with audio inputs (Surge XT) is given none, in process and in the bridge alike.

## Step 8b, first half — what shipped (2026-11-05): auxiliary outputs, protocol v4

**Design approved by the user (2026-11-05)** after the block message, the rings and the records were read. Two
answers given before any code: the shared region is sized for a fixed ceiling (the main port and 8 auxiliary
ones, 18 channels, about 2.4 MB per sandboxed VST3 plugin against 0.26 MB; CLAP keeps 2 channels), and the
engine tells the bridge which auxiliary buses to open in the `Activate` request, VST3 side.

**The proof came first.** `render_harness::vst3_plugins::a_sandboxed_instrument_renders_its_aux_output_like_an_in_process_one`
was put in the git index before any other change to the engine or the bridge. A second track takes port 1 of the
test instrument (`AppTrack.input_source`); the test requires the in-process render to differ from the render
without that track (so the bus is heard) and the sandboxed render to equal the in-process one at the five block
sizes. It failed with a difference at the first sample (the compiler cut the port of a one-port node).

**The protocol (v4).** `BlockHeader.channel_count` is the channels of the samples of that message (a request:
every input port; a reply: every output port), `PORT_CHANNELS` = 2 per active port, in the order of the
handshake's table. The header layout and size are unchanged; what changed is the meaning of `channel_count` and
of the region's `channels` (a capacity, no longer the block width), so `BRIDGE_PROTOCOL_VERSION` is 4 and a v3
bridge is refused at the handshake. `BlockRequest` gained `input_channels` / `output_channels`;
`BlockContext.channels` became `input_channels` / `output_channels`; `BridgeProcessor` gained
`output_channels()`. `Handshake::active_port` (which refused two active ports on one side) became
`active_port_count` / `block_channels`; the engine now refuses a table wider than the region instead. `Activate`
carries a third field, `wanted_outputs` (bit `n` = output bus `n`); the CLAP bridge ignores it.

**Decided while coding (to confirm).** The approved design said "active ports in table order". The VST3 bridge
marks **every** output bus up to the cap as active, whether or not the plugin has it open: the block shape then
depends only on the table, never on the wish, and a closed bus plays silence — which is what a graph port the
plugin does not feed already is in process. The wish only decides which buses the plugin opens. The input side
is unchanged (the main bus; the sidechain is the second half).

**Bridge** (`vst3_adapter.rs`). `Vst3Adapter::load` lost its `channels` argument (a stereo pair per port).
`activate(sample_rate, max_frames, wanted_outputs)` keeps `wanted_outputs` in its configuration, so a different
set reactivates the plugin; `opened_outputs` remembers which buses are open so that `open_output_buses` closes
the ones no longer wanted. The activation holds one `AudioBuffer` per port; after `process` the engine's rule is
applied (`Vst3OutputBuses::finish`: a mono bus is spread) and each port is copied into its place in the reply. A
skipped block passes only the main input through.

**Engine** (`bridge.rs`, `bridged_node.rs`). `BridgedPlugin` reads the handshake once (`read_handshake`) and keeps
the channel counts; `BridgeRoundTrip::prepare` sizes its vectors from them and sends the instance's wanted
buses (`PluginInstance::vst3_wanted_outputs`) in `Activate`; `BridgedNode` and `BridgedInstrument` expose
`output_ports()` as `num_output_ports()`, final once `prepare` has run, like the in-process wrappers.

**Real-time checklist** (`rt-validation`). No allocation, lock or system call added on either audio thread; every
vector is sized at `prepare` / at the activation; the bridge's per-block `output_channels()` is a read of a field
fixed at load. Written in `threading/realtime-safety.md`.

**C-2.** `vst3_plugins` (0x5c0f0710b6813439), `clap_plugins` (0x928da7dcd260c4e1) and every other golden hash are
unchanged; none was regenerated. The test plugin gained an `Aux Out` bus on both classes (closed unless opened):
the hash did not move.

**Tests.** `plugin_bridge` (a block carrying 4 channels in and 6 out, a block wider than the region is not sent,
`block_channels`); `vst3_adapter.rs` (every output bus is an active port, a closed bus is silent, an open one
plays half of the main sound, a new wish reactivates and a same wish does not); `vst3_bridge.rs` (the handshake,
`an_auxiliary_output_plays_only_once_the_engine_wants_it`); `test_vst3_plugin` (the second bus); the harness
render. Whole workspace, `cargo test --workspace --no-fail-fast -- --test-threads=1`: 1108 passed, 0 failed, 5
ignored (the timing measures).

**Not done, on purpose / still open.**

- **Second half of 8b**: the sidechain input through the port table (`Vst3InputBuses` in the bridge, the
  engine's second input port, a mono mix as in process), then `IComponentHandlerBusActivation` (a bus the plugin
  asks to open or close reuses the restart flag).
- A plugin with more than 9 output buses shows 9 ports when sandboxed and all of them in process.
- **Hand tests (yours):** Surge XT with a track on its _Scene A_ bus, isolated; a project saved before this step.
  Nothing was run against a real plugin for this half; only the test plugin proves it.
- The CPU cost of the wider block was not measured; the extra ports are zeros copied through shared memory.

## Step 8b, second half — what shipped (2026-11-05): the sidechain

**The proof came first.** `render_harness::vst3_plugins::a_sandboxed_effect_hears_its_sidechain_like_an_in_process_one`
was put in the git index before any change to the engine or the bridge. The instrument track feeds the test
effect's sidechain (`PluginParams.sidechain_source`); the test requires the in-process render to differ from the
render without the sidechain (it is heard) and the sandboxed render to equal the in-process one at the five
block sizes. It failed at the first sample.

**No format change.** Protocol v4 already carries any number of input ports: a request has two channels per
input port in table order, `channel_count` says how many. This half only uses it.

**Bridge** (`vst3_adapter.rs`). The handshake marks the auxiliary inputs of an **effect** (up to `MAX_PORTS`) as
active; an instrument carries none, as it is given no audio input. The activation opens every auxiliary input bus
(as `vst3_ensure_active` does), keeps one stereo `AudioBuffer` per input port and builds `Vst3InputBuses` from the
declared buses, so the mono mix of a bus the plugin declares mono and its silence flags are the in-process code.
A port the block did not carry is zeroed, so a stale block never leaks into a sidechain. A skipped block passes
only the main input through.

**Engine** (`bridged_node.rs`). `BridgeRoundTrip::process` takes the graph inputs and copies each into its port
(zeros for a port with no source); `BridgedNode::num_input_ports()` is the number of active input ports, final
once `prepare` has run; `BridgedInstrument` passes no input.

**Test plugin.** The effect has a mono auxiliary input, `Sidechain`; fed, a quarter of its level is added to the
output, unfed nothing changes. `vst3_plugins` (0x5c0f0710b6813439), `clap_plugins` and every other golden hash are
unchanged; none was regenerated.

**Real-time checklist** (`rt-validation`). No allocation, lock or system call added on either audio thread; the
per-port buffers and the engine's input vector are made at `prepare` / at the activation; the bridge's per-block
list of input references is a fixed array on the stack. Written in `threading/realtime-safety.md`.

**Tests.** The harness render above; `vst3_bridge.rs` (a sidechain port is mixed to mono and added, a block with
no sidechain signal leaves the effect alone, the handshake carries four input channels); `vst3_adapter.rs` (an
effect carries its sidechain, an instrument none); `test_vst3_plugin` (the sidechain bus). Whole workspace,
`cargo test --workspace --no-fail-fast -- --test-threads=1`: 1113 passed, 0 failed, 5 ignored (the timing measures).

**Not done, on purpose / still open.**

- **`IComponentHandlerBusActivation`** is not coded, and will not be (closed by the user, 2026-11-06: the buses follow what the graph reads, so a request from the plugin would have no effect; creating the track that reads a requested output would be a project-model change and needs its own ADR). It is supported **neither in process nor in the bridge**
  (ADR-014 lists it as not supported): it would be a new COM interface on the component handler
  (`vst3_ffi` header and IID, `vst3_host::component_handler`) with its own semantics (what the host does when a
  plugin asks to open or close a bus, how that meets the buses the graph wants), for both paths. It is a new
  feature, not parity, so it waits for the user's decision.
- **Hand tests (yours):** TDR Nova isolated with a track on its sidechain (the detector reacts as in process);
  a project saved before this step.
- A plugin with more than 8 auxiliary inputs carries 8 when sandboxed.
- No CPU measure of the wider request (the extra ports are zeros or a stereo pair copied through shared memory).

## Step 9 — what shipped (2026-11-05): presets of a sandboxed VST3 plugin

**Found first.** A sandboxed VST3 plugin offered **no preset at all**: `PluginHost::preset_listing` needed
`PluginInstance::vst3_controller()`, which exists only in the Echoes process, so the listing was empty and
`instance_for_preset` answered "no presets". The "not available yet" error of `Op::PresetLoad` in the bridge was
never reached from the interface.

**The proof came first.** `plugin_system::vst3_preset_sandbox_tests::a_sandboxed_plugin_offers_and_loads_its_presets_like_an_in_process_one`
was put in the git index (that file only) before any change to the engine or the bridge. It runs the same script
in both places — list the presets (three programs and a `.vstpreset` file), choose the program `Loud` while a
thread plays blocks, load the file — and requires the same list, the same "in use" mark, the same parameter
values, the same saved state, the same samples and the same count of "unsaved" marks. The in-process half passed;
the sandboxed half failed with "a VST3 plugin offers presets (sandboxed: true)". The test plugin had to learn
programs first (below); that is test material, not engine code.

**Questions settled first (user, 2026-11-05, the recommendations):**

- **a) Where each piece lives.** The programs are listed and chosen **in the bridge** (the controller is there).
  The `.vstpreset` files are listed, read and checked **by the engine** (`Vst3PresetFiles`, `read_preset_chunks`,
  unchanged: same bounds, same class-id check, same whitelist of the last listing); the engine sends the two
  states the file holds and the bridge applies them behind its gate.
- **b) Records.** No change to the block message, the rings or the handshake: `BRIDGE_PROTOCOL_VERSION` stays 4.
  Two control ops were added, as ADR-015 added ops before: `Op::PresetList` (→ `ProgramListRecord`s: list id and
  name, selector parameter, program in use, program names) and `Op::PresetLoadState` (`PresetStateRecord`: the
  processor's state and, if the file has one, the controller's). `Op::PresetLoad` and `PresetRecord` are reused
  for a program (source `Plugin`, key `<parameter id>:<index>`). `PayloadWriter::bytes` / `PayloadReader::bytes`
  carry the states (`length:u32`, the frame limit of 256 MiB is the file limit). The CLAP bridge answers an error
  to the two new ops.
- **c) `IUnitHandler`.** Not coded, in process or in the bridge (ADR-031 had decided not to: the list is read again
  each time the picker is shown). A plugin that changes its program by itself moves the selector parameter
  through `IComponentHandler`, which the bridge already reports (step 6); only the "in use" mark of an open picker
  is stale until it is shown again.
- **d) Test plugin.** `IUnitInfo` on both classes: unit 1 uses the list `Factory` — `Quiet` (gain 0.25), `Unity`
  (1.0, the default), `Loud` (1.75) — chosen by the parameter 23 `Program` (`kIsProgramChange`, 3 steps, default
  0.5). Only the **processor** applies a program (it takes the selector as a parameter point); the controller
  only remembers the selector, so a host that never gives the processor the choice hears nothing. The
  controller's `setState` (a file's `Cont` chunk) is one `f64`, its `Self value`. Ids 0 to 22 are untouched; the
  parameter count went from 23 to 24 (updated in `vst3_bridge.rs` and `vst3_sandbox_tests.rs`). `vst3_plugins`
  (0x5c0f0710b6813439) and `clap_plugins` did not move.

**Bridge** (`vst3_adapter.rs`). `preset_list` reads the program lists under the gate (`vst3_host::units`, the
engine's own code and bounds). `load_program` does what `PluginInstance::load_vst3_program` does in process:
`select_program` on the controller, then a **zero-sample `process`** (`flush_bases`, the flush `ProcessData`
allows) gives the processor the selector's value on the spot, then the controller takes the processor's state
(`setComponentState`) when the plugin is active. In process the value goes through the parameter queue and the
host waits up to 200 ms for a block to take it; in the bridge the whole step is one control request on the main
thread, so there is no wait and no queue. `load_preset_state` loads the processor's state through the ADR-010
path (`load_state`, which also hands the controller the state), then the controller's own state; a controller
that refuses it does not fail the load. `program_key` / `parse_program_key` moved to `vst3_host::units` (both ends
need them).

**Engine.** `PluginInstance::has_vst3_presets` (controller in process, or a sandboxed VST3) replaces the
controller test in `PluginHost::preset_listing` and `load_preset`; `vst3_program_presets`, `load_vst3_program`
and `load_vst3_preset_file` have a bridged branch (`BridgedPlugin::program_lists`, `load_preset`,
`load_preset_state`). The file is read and checked **before** the bridge is touched, a load marks the project
unsaved and re-reads the bases of the parameters modulated by value, as in process.

**Real-time checklist** (`rt-validation`). Nothing on either audio thread changed: no new system call, no
allocation, no lock. The new requests run on the bridge's main thread under the ADR-010 gate, which the audio
thread never waits on: while a preset loads, the bridge skips its block and passes the input through, as for a
state load. The zero-sample `process` is called on the main thread with the gate held, like the existing flush
before a state save.

**C-2.** `vst3_plugins` (0x5c0f0710b6813439), `clap_plugins` (0x928da7dcd260c4e1) and every other golden hash
are unchanged; none was regenerated.

**Tests.** The proof above, plus `a_sandboxed_plugin_loads_only_what_its_last_listing_offered` and
`a_file_the_engine_cannot_use_is_refused_before_the_bridge_is_touched`; `vst3_bridge.rs` (the lists, a program
reaches the processor and the controller follows, a program not of the listing is refused, the states of a file,
a refused state, a controller that refuses its own state); `plugin_bridge` records and ops; `test_vst3_plugin`
(the list, the selector, only the processor applies a program, the controller state). The old test "what a VST3
plugin cannot do yet is refused" was replaced by these.

**Not done, on purpose / still open.**

- Open question 7 is **not** closed: `preset_file.rs` was not checked against the SDK's `vstpresetfile.cpp` in
  this step (it needs that repository, fetched outside the project first). The file reader is unchanged.
- **Hand tests (yours):** TDR Nova isolated, choose one of its 73 programs; a real `.vstpreset` isolated (none
  is known on the development machine).
- `IUnitHandler` (see c), `IProgramListData`, the `Prog` / `Info` chunks, presets bundled in the plugin.
- A load waits at most the bridge's gate budget for a block in flight and answers "the plugin is busy processing"
  past it (in process: "the plugin is busy, try again").

## Step 10 — what shipped (2026-11-05): out-of-process scan and blocklist

**Found first.** The scan loaded every plugin file in the Echoes process. `scan_plugins` ran inside the engine's
constructor, `vst3/host.rs` and `clap/host.rs` guarded each load with `crash_guard` (SEH), and the VST3
sidechain probe (`probe_aux_input_count`) created a throwaway component per effect. A plugin that crashed in a
way SEH does not catch, or hung in `GetPluginFactory`, ended or froze the application at startup, and the next
start met it again.

**The proof came first.** `plugin_system::scan_sandbox_tests` runs the application's own scan over a machine whose
only VST3 folder holds the good test plugin plus the examples of the new `test_crashing_plugin` crate
(`crash_on_load`, `crash_in_factory`, `hang_in_factory`; each appends its name to a log the test counts). They
require: the crash and the hang are blocked and the good plugin is still listed; a blocked file is not loaded
again at the next start; it is listed again once it changed or the user re-enabled it; a file that is not a plugin
is skipped, not blocked; a file listed before and unchanged is not listed again (the second start runs with no
bridge at all); a forced rescan lists everything again.

**Design.**

- **One process per file.** `plugin_host_bridge --scan <vst3|clap> <file>` (`crates/plugin_host_bridge/src/scan.rs`)
  loads the file, lists its plugins (class id, name, vendor, features, ports) and, for an effect, probes the
  auxiliary inputs with the same `read_aux_inputs` as before. It prints one `ScannedPluginRecord` text block
  (`plugin_bridge::records`) and exits. Nothing in it is guarded: the process boundary is the guard.
- **The engine side** is `plugin_system/scan.rs`: `scan_file` starts the process, polls it, and kills it after
  `SCAN_TIMEOUT` (15 s). Exit 0 with a readable block = `Plugins`; the exit code `SCAN_EXIT_REFUSED` (wrong
  architecture, missing dependency) = `Refused`, skipped but **not** blocked; any other exit, a missing or
  unreadable block = `Blocked(Crashed)`; the time limit = `Blocked(TimedOut)`.
- **The blocklist and the scan cache** (`ScanStore`, `plugin-blocklist.json` and `plugin-scan-cache.json`) live in
  the application data directory, next to the crash marker (ADR-015). A file is identified by path, size and
  modification time (`FileStamp`): it stays blocked until it changes or the user re-enables it; a listed file
  that has not changed is not listed again. Files that disappeared are dropped from both.
- **`PluginHost`** gained `scan_plugins` (cache and blocklist aware), `rescan_plugins` (forgets the cache, keeps
  the blocklist), `blocked_plugins`, `take_newly_blocked` and `reenable_plugin`. `PluginHost::new` no longer scans:
  `main.rs::scan_installed_plugins` points the host at the data directory, then scans, in Tauri's `setup`. _(Since
  2026-11-06 the scan belongs to `PluginScanner`, not to `PluginHost`, and starts in a background thread: ADR-034.)_
- **Interface.** Three commands: `list_blocked_plugins`, `take_newly_blocked_plugins` (each file is announced
  once) and `reenable_blocked_plugin` (recompiles like a rescan). The commands exist; since 2026-11-06 the interface announces a blocked file in the status bar and lists the blocked
  files, with a "Re-enable" button, in the plugin picker (see `plugin-wrapper-system.md`, "Out-of-process scan and blocklist").
- **Removed.** `probe_aux_input_count` and the in-process CLAP `audio-ports` probe, with their `crash_guard`
  calls: the scan process does both. `PluginManifest` is built by `manifest_of` from the record.

**Real-time checklist** (`rt-validation`). Nothing on an audio thread changed. The scan runs once at startup on
the setup thread and on a rescan, before the engine runs a project; it holds the engine mutex while it runs, as
the old in-constructor scan did. _Amended 2026-11-06 (ADR-034): the scan no longer holds the engine mutex and no
longer runs on the UI thread or in `setup`; this paragraph describes step 10 as first delivered._

**Proof.** Workspace: 1139 passed, 0 failed. The golden hashes (`vst3_plugins`, `clap_plugins`) did not move.
The tests that change the search-folder variables share `sandbox_tests::environment_lock` with the tests that
read them (installed-plugin discovery, VST3 presets); without it they failed when run in parallel.

**Not done.** Not hand-tested with the installed plugin set (TDR Nova, Surge XT, Vital): the first start scans
every file in a process each, which is expected to be slower than before (not measured); the next starts use the cache. No interface to show or
lift a block. Whether the user-facing re-enable belongs in the plugin browser is left for a UX decision.

## Amendment 2026-11-24 — the track of a sandboxed VST3 plugin

A VST3 plugin in the Echoes process hears the name and color of its track through the `IInfoListener` of its
controller (ADR-013 amendment 2026-10-21). In the bridge it did not: `Op::TrackInfo` answered an empty reply for
VST3 and `--track-info` was read for CLAP only. The sandbox was therefore not equivalent for a plugin that shows the
track name.

- **Same delivery as in the process.** `Vst3Adapter::load` takes the launch track (`--track-info`, already sent by
  `BridgedPlugin::spawn` for both formats) and tells the controller right after creating it. `Op::TrackInfo` decodes
  the `TrackRecord` and calls `Vst3Adapter::set_track_info`: the same `vst3_host::channel_context::notify_channel_context`
  as the engine, from the bridge's main thread, under the ADR-010 gate (`enter_main`).
- **An empty track is never announced** and the same track is not told twice (the adapter keeps the last track told,
  empty at the start), as `PluginInstance::set_vst3_track_info`. A controller without `IInfoListener`, or no
  controller, is left alone.
- **A busy plugin is an error, not a loss.** If the gate is not free within its budget the request fails, the adapter
  keeps the previous track and `BridgedPlugin::set_track_info` (which records a track only after a successful
  request) sends it again at the next sync.
- **Nothing else moves.** No new `Op`, no new field, protocol still v4; no audio-thread work (the call is a main-thread
  control request). The golden hashes of the render harness do not move: the test DLL only gained parameters.
- **Test plugin.** `test_vst3_plugin` implements `IInfoListener` on both classes and shows what it heard in three
  read-only parameters (24 `Track calls`, 25 `Track name units`, 26 `Track color`, RGB without alpha, 0 for none);
  27 parameters now.
- **Tests.** `plugin_host_bridge/tests/vst3_bridge.rs`: told at launch, never told an empty track, a new track once, a
  garbled record refused with the channel still open. `plugin_system/vst3_sandbox_tests.rs`: the same scenario in
  process and sandboxed, born knowing the track then renamed then repeated, and the no-track case, each answering
  the same.
- **Still open.** `Op::NoteNames` is still empty for a sandboxed VST3 (VST3 has no host-facing equivalent worked out
  yet); not hand-tested in the app (a plugin that displays its track name, rename and recolor a track, mode _Tous_).

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
(Windows x86*64, `#pragma pack(16)` of `falignpush.h`, which never raises the natural
alignment of any structure below). COM byte order was checked against
`INLINE_UID` of `funknown.h` (`COM_COMPATIBLE`): the first long is little-endian, the
second and third are split into little-endian halves, the fourth is big-endian — which
is exactly what `interface_iid()` does. All 18 `IID*\*`constants match. The existing
test`interface_iids_match_the_sdk`covers 17 of them (not`IID_FUNKNOWN`) and its
expected values equal the headers' as well.

| Element in `ffi.rs`                                                                                                              | Header                                              | Verdict                                                                                               |
| -------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `IID_FUNKNOWN`                                                                                                                   | `base/funknown.h`                                   | OK (`0,0,0xC0000000,0x46`, identical with or without the COM swap)                                    |
| `IID_ICOMPONENT`                                                                                                                 | `vst/ivstcomponent.h`                               | OK                                                                                                    |
| `IID_IAUDIO_PROCESSOR`                                                                                                           | `vst/ivstaudioprocessor.h`                          | OK                                                                                                    |
| `IID_IEDIT_CONTROLLER`                                                                                                           | `vst/ivsteditcontroller.h`                          | OK                                                                                                    |
| `IID_IMIDI_MAPPING`                                                                                                              | `vst/ivsteditcontroller.h`                          | OK                                                                                                    |
| `IID_ICOMPONENT_HANDLER`, `IID_ICOMPONENT_HANDLER2`                                                                              | `vst/ivsteditcontroller.h`                          | OK, OK                                                                                                |
| `IID_IEVENT_LIST`                                                                                                                | `vst/ivstevents.h`                                  | OK                                                                                                    |
| `IID_IPLUGIN_FACTORY2`                                                                                                           | `base/ipluginbase.h`                                | OK                                                                                                    |
| `IID_IMESSAGE`, `IID_ICONNECTION_POINT`                                                                                          | `vst/ivstmessage.h`                                 | OK, OK                                                                                                |
| `IID_IATTRIBUTE_LIST`                                                                                                            | `vst/ivstattributes.h`                              | OK                                                                                                    |
| `IID_IINFO_LISTENER`                                                                                                             | `vst/ivstchannelcontextinfo.h`                      | OK                                                                                                    |
| `IID_IUNIT_INFO`                                                                                                                 | `vst/ivstunits.h`                                   | OK                                                                                                    |
| `IID_IHOST_APPLICATION`                                                                                                          | `vst/ivsthostapplication.h`                         | OK                                                                                                    |
| `IID_IPLUG_FRAME`                                                                                                                | `gui/iplugview.h`                                   | OK                                                                                                    |
| `IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT`                                                                                           | `gui/iplugviewcontentscalesupport.h`                | OK                                                                                                    |
| `IID_IBSTREAM`                                                                                                                   | `base/ibstream.h`                                   | OK                                                                                                    |
| `Tuid` (16 bytes), `TResult` (`int32`)                                                                                           | `base/funknown.h`, `base/ftypes.h`                  | OK                                                                                                    |
| `K_RESULT_OK` 0, `K_RESULT_FALSE` 1                                                                                              | `base/funknown.h`                                   | OK                                                                                                    |
| `K_NO_INTERFACE`, `K_INVALID_ARGUMENT`, `K_OUT_OF_MEMORY` (Windows)                                                              | `base/funknown.h`                                   | OK (`0x80004002`, `0x80070057`, `0x8007000E`)                                                         |
| same three, other platforms (`-1`, `2`, `6`)                                                                                     | `base/funknown.h` (`COM_COMPATIBLE` = 0)            | OK                                                                                                    |
| `PClassInfo` (116 bytes: cid 16, cardinality, category 32, name 64)                                                              | `base/ipluginbase.h`                                | OK                                                                                                    |
| `PClassInfo2` (440 bytes, asserted)                                                                                              | `base/ipluginbase.h`                                | OK (field order and sizes 32 / 64 / 128 / 64 / 64 / 64)                                               |
| `IPluginFactory` vtable (4 own methods)                                                                                          | `base/ipluginbase.h`                                | OK; `getFactoryInfo` is typed `*mut c_void` (the `PFactoryInfo` struct is not defined) — fine today   |
| `IPluginFactory2` vtable (`getClassInfo2` after `createInstance`)                                                                | `base/ipluginbase.h`                                | OK                                                                                                    |
| `IComponent` vtable (14 slots, `setState` before `getState`)                                                                     | `vst/ivstcomponent.h`                               | OK                                                                                                    |
| `IAudioProcessor` vtable (11 slots; latency and tail return `uint32`)                                                            | `vst/ivstaudioprocessor.h`                          | OK                                                                                                    |
| `AudioBusBuffers` (`int32`, `uint64`, pointer; 24 bytes)                                                                         | `vst/ivstaudioprocessor.h`                          | OK                                                                                                    |
| `ProcessSetup`                                                                                                                   | `vst/ivstaudioprocessor.h`                          | OK                                                                                                    |
| `ProcessData` (order of the 12 fields)                                                                                           | `vst/ivstaudioprocessor.h`                          | OK                                                                                                    |
| `ProcessModes` (0 realtime, 1 prefetch, 2 offline), `SymbolicSampleSizes` (0 = 32)                                               | `vst/ivstaudioprocessor.h`                          | OK (only 0 is used; offline is deliberately not used, see the wrapper doc)                            |
| `ProcessContext` (112 bytes, asserted; offsets by hand)                                                                          | `vst/ivstprocesscontext.h`                          | OK                                                                                                    |
| `CONTEXT_PLAYING` 1<<1, `PROJECT_TIME_MUSIC_VALID` 1<<9, `TEMPO_VALID` 1<<10, `BAR_POSITION_VALID` 1<<11, `TIME_SIG_VALID` 1<<13 | `vst/ivstprocesscontext.h`                          | OK                                                                                                    |
| `K_AUDIO` 0, `K_EVENT` 1, `K_INPUT` 0, `K_OUTPUT` 1                                                                              | `vst/ivstcomponent.h`                               | OK                                                                                                    |
| `SPEAKER_STEREO` 0x3                                                                                                             | `vst/vstspeaker.h`                                  | OK (L = 1<<0, R = 1<<1)                                                                               |
| `CATEGORY_AUDIO_MODULE` `"Audio Module Class"`                                                                                   | `vst/ivstaudioprocessor.h` (`kVstAudioEffectClass`) | OK                                                                                                    |
| `is_instrument_subcategory` (first token `Instrument`)                                                                           | `vst/ivstaudioprocessor.h` (`kInstrument`)          | OK                                                                                                    |
| `K_NOTE_ON_EVENT` 0, `K_NOTE_OFF_EVENT` 1, `K_POLY_PRESSURE_EVENT` 3                                                             | `vst/ivstevents.h`                                  | OK                                                                                                    |
| `NoteOnEvent`, `NoteOffEvent`, `PolyPressureEvent`                                                                               | `vst/ivstevents.h`                                  | OK (field order and types)                                                                            |
| `Event` (48 bytes, payload at offset 24) and its union `raw: [u64; 3]`                                                           | `vst/ivstevents.h`, `vst/ivstnoteexpression.h`      | OK: widest member is `NoteExpressionTextEvent`, 24 bytes; the new `NoteExpressionIntValueEvent` is 16 |
| `IParamValueQueue`, `IParameterChanges` vtables (host-implemented)                                                               | `vst/ivstparameterchanges.h`                        | OK                                                                                                    |
| `IAttributeList`, `IMessage`, `IInfoListener`, `IHostApplication` vtables                                                        | `vst/ivst*.h`                                       | OK (`AttrID` is `const char*`, `TChar` is `char16_t`, sizes are in bytes)                             |
| `IComponentHandler`, `IComponentHandler2` vtables                                                                                | `vst/ivsteditcontroller.h`                          | OK                                                                                                    |
| `IBStream` vtable and seek modes 0 / 1 / 2                                                                                       | `base/ibstream.h`                                   | OK                                                                                                    |
| `IEditController` vtable (18 slots, asserted), `ParameterInfo` (792 bytes, asserted), `String128`                                | `vst/ivsteditcontroller.h`, `vst/vsttypes.h`        | OK                                                                                                    |
| `PARAM_CAN_AUTOMATE` 1<<0, `IS_READ_ONLY` 1<<1, `IS_HIDDEN` 1<<4, `IS_PROGRAM_CHANGE` 1<<15                                      | `vst/ivsteditcontroller.h`                          | OK                                                                                                    |
| `ViewRect` (4 × `int32`)                                                                                                         | `gui/iplugview.h`                                   | OK                                                                                                    |
| `IPlugView` vtable (15 slots, asserted), `IPlugFrame`                                                                            | `gui/iplugview.h`                                   | OK                                                                                                    |
| `IPlugViewContentScaleSupport` vtable (`float`)                                                                                  | `gui/iplugviewcontentscalesupport.h`                | OK                                                                                                    |
| `IConnectionPoint` vtable                                                                                                        | `vst/ivstmessage.h`                                 | OK                                                                                                    |

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

| Interface (header)                                                                   | IID (4 longs)                                                                                                       | Why                                                                                                                                                                        | Step         |
| ------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| `IProcessContextRequirements` (`vst/ivstaudioprocessor.h`)                           | `2A654303 EF764E3D 95B5FE83 730EF6D0`                                                                               | the plugin says which `ProcessContext` fields it needs; fills only those in v3                                                                                             | 5            |
| `IAudioPresentationLatency` (`vst/ivstaudioprocessor.h`)                             | `309ECE78 EB7D4fae 8B2225D9 09FD08B6`                                                                               | tell the plugin the output latency of the device (optional)                                                                                                                | 5            |
| `IPlugInterfaceSupport` (`vst/ivstpluginterfacesupport.h`)                           | `4FB58B9E 9EAA4E0F AB361C1C CCB56FEA`                                                                               | plugins ask the host which interfaces it supports; host-implemented                                                                                                        | 5            |
| `IPluginFactory3` (`base/ipluginbase.h`)                                             | `4555A2AB C1234E57 9B122910 36878931`                                                                               | `getClassInfoW` (Unicode names) and `setHostContext` on the factory; scan                                                                                                  | 5, 10        |
| `PFactoryInfo`, `PClassInfoW` (`base/ipluginbase.h`; structs)                        | —                                                                                                                   | typed factory info and Unicode class info for the scan (sizes asserted when added)                                                                                         | 10           |
| `IEditControllerHostEditing` (`vst/ivsteditcontroller.h`)                            | `C1271208 70594098 B9DD34B3 6BB0195E`                                                                               | host-side begin / end of an edit, for the gesture events sent by the engine                                                                                                | 6            |
| `IComponentHandlerBusActivation` (`vst/ivsteditcontroller.h`)                        | `067D02C1 5B4E274D A92D90FD 6EAF7240`                                                                               | a plugin asks the host to activate a bus (aux outputs); host-implemented                                                                                                   | 8            |
| `INoteExpressionController` (`vst/ivstnoteexpression.h`)                             | `B7F8F859 41234872 91169581 4F3721A3`                                                                               | instruments: note expression types; events `kNoteExpressionValueEvent` (4), `…TextEvent` (5), `…IntValueEvent` (8), `DataEvent` (2, SysEx), `LegacyMIDICCOutEvent` (65535) | 8            |
| `IUnitHandler`, `IUnitHandler2` (`vst/ivstunits.h`)                                  | `4B5147F8 4654486B 8DAB30BA 163A3C56`, `F89F8CDF 699E4BA5 96AAC9A4 81452B01`                                        | presets: the plugin reports a program-list or unit change; host-implemented                                                                                                | 9            |
| `IStreamAttributes` (`vst/ivstattributes.h`)                                         | `D6CE2FFC EFAF4B8C 9E74F1BB 12DA44B4`                                                                               | file name / metadata on the preset stream (optional)                                                                                                                       | 9            |
| `IComponentHandler3`, `IContextMenu`, `IContextMenuTarget` (`vst/ivstcontextmenu.h`) | `69F11617 D26B400D A4B6B964 7B6EBBAB`, `2E93C863 0C9C4588 97DBECF5 AD17817D`, `3CDF2E75 85D34144 BF86D36B D7C4894D` | context menus of the editor (optional; plugins fall back when absent; **not added in step 7**)                                                                             | 7 (optional) |
| `IDataExchangeHandler`, `IDataExchangeReceiver` (`vst/ivstdataexchange.h`)           | `36D551BD 6FF54F08 B48E830D 8BD5A03B`, `45A759DC 84FA4907 ABCB6175 2FC786B6`                                        | audio-thread to editor data channel of newer plugins (optional; usage to be checked; **not added in step 7**)                                                              | 7 (optional) |

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
