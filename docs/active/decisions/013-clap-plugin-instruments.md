# ADR-013 — CLAP plugin instruments via an `InstrumentDevice` adapter (Phase 5c)

## Status

Accepted (CLAP slice; the VST3 slice is the amendment of 2026-10-21). Implements the **plugin-instrument** half of **Phase 5c**
of [`docs/active/migration/migration_plan.md`](../migration/migration_plan.md) — audit **P0-2**
(_"Aucun support des plugins instruments"_) and target **BP-4**
(`InstrumentNode` inner accepts a plugin or PolySynth).

Builds on [`007-plugin-instance-arc.md`](007-plugin-instance-arc.md) (shared
`Arc<PluginInstance>`), [`011-portkind-typed-edges.md`](011-portkind-typed-edges.md)
and [`012-midi-snapshot-beats.md`](012-midi-snapshot-beats.md) (beat-accurate,
tempo-live per-block note scheduling).

Scope: **CLAP only** (C-3 variant-by-variant). VST3i (`IEventList`) is a
follow-up. Live MIDI input (`MidiInputNode` + SPSC) is out of scope — this slice
covers the **clip-driven** instrument path.

---

## Context

`InstrumentNode` already converts timeline MIDI clips into a per-block
`MidiEventList` with correct sample offsets (ADR-012), then calls
`device.process_midi` on a `Box<dyn InstrumentDevice>`. The only inner device
was the built-in `PolySynth`: there was no way for a VST3i/CLAPi to generate a
track's audio. Meanwhile the CLAP note-event FFI (`FixedEventList`,
`push_note_on/off`) already existed in
[`plugin_formats/clap/ffi.rs`](../../../src-tauri/src/plugin_formats/clap/ffi.rs)
but was unused, and `PluginHost` could already build a shared
`Arc<PluginInstance>` for a CLAP plugin.

## Decision

Reuse the existing `InstrumentNode` + `InstrumentDevice` seam (the
node-internal MIDI path target ADR-008 keeps for clip playback) and add a
CLAP-backed `InstrumentDevice`. A track names its instrument by `device_id`; the
compiler instantiates it as the node's generator, falling back to `PolySynth`
on any failure.

### Module map

1. **[`project.rs`](../../../src-tauri/src/project.rs)**
   - `AppTrack.instrument: Option<String>` (device_id). `TrackFile.instrument`
     is `#[serde(default, skip_serializing_if = "Option::is_none")]`; `TrackDto`
     carries it for the frontend. Propagated through `to_file`/`from_file`,
     `to_dto`, and all five `AppTrack` construction sites. Tests: legacy file
     (no field) → `None`; save→load round-trip preserves the id.

2. **New
   [`audio_graph/nodes/clap_instrument.rs`](../../../src-tauri/src/audio_graph/nodes/clap_instrument.rs)**
   - `ClapInstrument` — `InstrumentDevice` over an `Arc<PluginInstance>`.
     `prepare` runs `init`/`activate`/`start_processing`; `process_midi` fills a
     pre-allocated `FixedEventList` from the `MidiEventList`, builds a
     `ClapProcess` with `audio_inputs_count = 0` (pure generator) and
     `in_events` = the note list, and calls `ClapPlugin::process`. Mirrors the
     proven `ClapWrapperNode` RT discipline; `Drop` does `stop_processing` +
     `deactivate`.
   - `fill_clap_notes(&MidiEventList, &mut FixedEventList)` — the pure note
     translation (velocity `0..127` → CLAP `0..1`), unit-tested without a live
     plugin.

3. **[`plugin_system/host.rs`](../../../src-tauri/src/plugin_system/host.rs)**
   - The CLAP FFI create sequence is extracted into `create_clap_instance`,
     shared by `instantiate_clap` (effect wrapper) and the new
     `instantiate_instrument(plugin_id) -> Box<dyn InstrumentDevice>`. The
     instrument path registers the shared instance in `records` (same as
     `instantiate`) so params / state / lifecycle still address it. Non-CLAP
     formats return `Err` → caller falls back.

4. **[`audio_graph/compiler.rs`](../../../src-tauri/src/audio_graph/compiler.rs)**
   - `build_node(NodeKind::Instrument)` reads `track.instrument`; if `Some`,
     calls `plugin_host.instantiate_instrument` and uses the result as the inner
     device; on `Err` (or `None`) builds `PolySynth`. The graph topology is
     unchanged — only the inner generator differs.

## Real-time impact

| Change                         | RT cost                                | RT risk                                          |
| ------------------------------ | -------------------------------------- | ------------------------------------------------ |
| `fill_clap_notes` per block    | bounded copy into a pre-allocated list | none                                             |
| `ClapInstrument::process_midi` | pointer fill + one FFI `process` call  | none — no alloc/lock, mirrors the effect wrapper |
| param ring drain-and-discard   | N atomic loads (N pending)             | none — bounded                                   |
| instrument instantiation       | compile-time (off-RT)                  | none                                             |

C-1 preserved: all buffers/lists allocated in `prepare`; the audio thread only
fills pre-allocated scratch and calls the plugin. The `Arc<PluginInstance>` is
cloned at compile and dropped via the snapshot drop ring, never on the audio
thread.

## Compatibility

- **C-2 (bit-identique offline).** Preserved. `instrument == None` (every
  existing project) builds `PolySynth` exactly as before; the new branch is
  never taken.
- **C-3 (variant-by-variant).** CLAP only; VST3i returns `Err` → PolySynth
  fallback, no regression.
- **C-4 (saved projects).** `instrument` defaults to `None`; legacy files load
  unchanged (tested).
- **C-7 (compiler extended, not rewritten).** One conditional in the existing
  `Instrument` arm.

## Consequences

- **Positive.** A CLAP synth can generate a MIDI track's audio (audit P0-2,
  quick-win "Vital/Diva jouables") through the existing beat-accurate scheduler
  — no new MIDI timing code.
- **Limits / debt.**
  - **Acceptance pending real-plugin smoke test.** The FFI `process` path cannot
    run in CI (no embedded plugin / the V4 mock harness is not built). Unit
    tests cover note translation, the project round-trip, and the PolySynth
    fallback; final validation is a manual smoke test (plan V5) or the Rust mock
    plugin (audit B4).
  - **No instrument parameter automation yet.** _Resolved 2026-09-24 by ADR-021._ The param ring is
    drained-and-discarded; a merged note+param input list (ADR-008 follow-up)
    is needed to apply automation to instruments.
  - **VST3i not wired** (returns `Err` → PolySynth). _Resolved 2026-10-21, see the
    last amendment._
  - A track needs MIDI clips to get an `Instrument` node (graph.rs unchanged);
    an instrument with no clips produces silence.
- **Reversibility.** Additive. Clearing `instrument` (or an instantiation
  failure) returns the track to `PolySynth`.

## Known risk — surfaced 2026-06-03 (architecture review), not yet closed

1. **Silent `PolySynth` fallback is a fidelity hazard.** _Resolved 2026-09-23:_
   a chosen instrument that fails to load now becomes a `SilentInstrument`
   (the track is muted, never re-voiced). The host reports it through
   `PluginHost::unavailable_plugins` and the `plugin-availability` event, and
   the instrument panel shows the reason with a retry button. `PolySynth` is
   used only when no instrument is chosen. The instrument picker offers CLAP
   instruments only until VST3i lands (_VST3 instruments are offered since
   2026-10-21_). Original text: A saved project whose
   CLAP synth fails to instantiate (missing DLL, version mismatch) plays a
   `PolySynth` **silently** — the render is wrong with no signal to the user.
   Robust against crashing, but "your instrument was substituted without telling
   you" is worse than a visible error. The fallback should be **surfaced** (UI
   warning / project-load report), not muted.
2. **C-2 does not hold once a third-party synth is in the project.** Bit-identical
   offline is guaranteed only for `instrument == None`. Most synths are not
   bit-deterministic (internal state, denormals, SIMD paths), so a project using
   a CLAP instrument is not reproducible cross-machine. This is structural, not a
   bug — but it means the determinism invariant applies only to the built-in
   signal path; document that boundary where C-2 is claimed.

## Alternatives considered

- **`InstrumentNode.inner: Box<dyn AudioNode>` + `PortKind::Midi` input edges
  now.** Rejected for this slice — a larger refactor (per-port kinds on every
  node, scheduler MIDI routing) for no extra clip-playback capability. ADR-008
  explicitly keeps the node-internal snapshot path for clip MIDI; the typed-edge
  path is for NoteFx / live input later.
- **Duplicate the CLAP create code in the instrument path.** Rejected —
  extracted `create_clap_instance` so the effect and instrument paths share one
  FFI sequence.
- **Build the effect wrapper and reuse it as an instrument.** Rejected — the
  wrapper has 1 audio input and its `Drop` deactivates the shared plugin;
  discarding it would tear down the instance.

## Follow-ups

- ~~**Host transport**~~ — done 2026-09-23: instruments and CLAP effects receive
  a `clap_event_transport` (tempo, beats, bars, time signature, playing state).
  The musical math is shared in `TransportInfo::musical_position` for VST3i.
- **Real-plugin acceptance** — smoke test a CLAP synth (Surge XT) end-to-end;
  add the Rust mock plugin (B4) for CI coverage.
- ~~**Instrument param automation**~~ — done 2026-09-24 (ADR-021): notes and
  parameter events share one time-sorted `ClapEventBuffer`.
- ~~**VST3i**~~ — done 2026-10-21: `IEventList` note events behind the same
  `InstrumentDevice` adapter shape (last amendment).
- **Live MIDI input** — `MidiInputNode` + per-instance MIDI SPSC queue.

## Amendment 2026-10-14 — a silent instrument is put to sleep

`ClapInstrument` used to discard the status returned by `process()`. It now
follows `ProcessSleep` (`crates/clap_ffi/src/process_sleep.rs`): once the plugin
answers `SLEEP` — or `CONTINUE_IF_NOT_QUIET` / `TAIL` with a `clap.tail` that has
elapsed with no event — and its output is silent, `process()` is not called and
the output is zero until a note, a parameter event or a `request_process`. A
release longer than the declared tail is never cut; without `clap.tail` only
`SLEEP` sleeps. No invariant changes (no new ADR). Details:
`../plugins/plugin-wrapper-system.md` (Plugin sleep). Tests:
`plugin_system/sandbox_tests.rs`, `audio_engine/render_harness.rs::clap_plugins`.

## Amendment 2026-10-14 — the host answers `clap.params`

The host now exposes `clap.params` (`rescan`, `clear`, `request_flush`) and uses
the plugin's `value_to_text`. `request_flush` wakes a sleeping plugin like
`request_process`; `rescan` makes the UI refetch parameters and texts (and
`ALL` recompile); `clear` is accepted but ignored, because dropping automation
or modulation is a project change a plugin callback must not make (ADR-005) —
undecided, needs the user. No invariant changes (no new ADR). Details:
`../plugins/plugin-wrapper-system.md` (Host `clap.params`).

## Amendment 2026-10-14 — the host answers `clap.track-info`

A plugin instrument (like an effect) reads the name and color of its track and
is told through `changed()` when either changes. The placement key is
`instrument:{track_id}`; the info comes from the project and adds no state,
command or file-format change. Only the name and color are reported — no audio
channels, no return / bus / master flags (the project has none). No invariant
changes (no new ADR). Details: `../plugins/plugin-wrapper-system.md` (Host
`clap.track-info`); sandbox: ADR-015 amendment 2026-10-14.

## Amendment 2026-10-14 — the host answers `clap.note-ports` and `clap.note-name`

The host announces the dialects it sends to an instrument (CLAP notes + raw MIDI)
through `clap.note-ports`; a rescan of all ports is a restart request. It still
does not read the plugin's note ports nor filter by dialect (ADR-022 unchanged).
It reads an instrument's note names (`clap.note-name`, e.g. drum pads) on demand
and reports `changed` as the `plugin-note-names-changed` event; no UI uses them
yet. No invariant changes (no new ADR). Details: `../plugins/plugin-wrapper-system.md`
(Host `clap.note-ports` and `clap.note-name`); sandbox: ADR-015 amendment 2026-10-14.

## Amendment 2026-10-14 — the host answers `clap.timer-support`

A plugin (effect or instrument) registers periodic timers and the host calls its
`on_timer` on the main thread — the Win32 thread of its open editor, or the
bridge's main loop when sandboxed — from a message loop only, never from a host
callback or the audio thread. Registration is a lock-free table in the plugin's
`ClapHostContext` (no state, command, event or `.echoes` change). Decided with the
user: timers run only while the plugin's editor window exists; `on_timer` does
not take the instance FFI gate (a gate held by a periodic call would make the
audio thread bypass blocks) and only waits while another off-RT caller holds it;
a crashed plugin is not called; periods under 16 ms are raised, 0 is refused;
ids are unique per plugin and never reused. The offline render opens no editor
and is unchanged (C-2). No invariant changes (no new ADR). Details:
`../plugins/plugin-wrapper-system.md` (Host `clap.timer-support`); sandbox:
ADR-015 amendment 2026-10-14.

## Amendment 2026-10-14 — the host answers `clap.preset-load`

A plugin (effect or instrument) tells the host that it loaded a preset (`loaded`)
or failed to (`on_error`). `loaded` raises the unsaved-changes flag already used
by `clap.state.mark_dirty`; `on_error` is only logged. No state, command, event or
`.echoes` change: the plugin's state is saved as a blob, as before (ADR-010
unchanged). The host does not yet list or load presets (`from_location`,
preset discovery): that would be a separate decision. No invariant changes (no new
ADR). Details: `../plugins/plugin-wrapper-system.md` (Host `clap.preset-load`);
sandbox: ADR-015 amendment 2026-10-14.

## Amendment 2026-10-21 — VST3 instruments

A VST3 instrument generates a track's audio behind the same `InstrumentDevice`
seam as CLAP (C-3, variant by variant). No invariant changes (no new ADR): no new
node, no compiler or scheduler change, no file-format change — `AppTrack.instrument`
already names the device. Step 4 of the VST3 catch-up. Unit-tested; not yet
validated with a real VST3 instrument (none was installed when this was written:
the real-FFI tests `vst3_instrument_plays_a_note` and
`vst3_instrument_reports_its_midi_controller_mapping` skip until one is).

- **Detection.** The scan reads `PClassInfo2.subCategories` through
  `IPluginFactory2` (`vst3/ffi.rs`). A class is an instrument when the first
  `|`-separated token is `Instrument`, so `Fx|Instrument` stays an effect. Only a
  factory without `IPluginFactory2` falls back to the name guess.
- **Adapter.** `VstInstrument` (`plugin_formats/vst3/instrument.rs`) wraps the shared
  `Arc<PluginInstance>`. `prepare` allocates the parameter list and the event list;
  `process_midi` refills them, then calls `IAudioProcessor::process` with
  `numInputs = 0`, the transport `ProcessContext` and `inputEvents`, under the FFI
  gate and `crash_guard` (a crash sets the instance `Error`, so it is not reused).
  The lifecycle stays on the instance (ADR-016): reuse across recompiles works for
  VST3 instruments as for CLAP.
- **Events.** `HostEventList` (`vst3/event_list.rs`) is a fixed 512-event
  `IEventList` allocated in `prepare`. `MidiEventList` notes become `kNoteOnEvent` /
  `kNoteOffEvent` (velocity `0..127` → `0..1`, `noteId = -1`, matched by pitch as in
  the CLAP path) at their sample offsets; polyphonic aftertouch becomes
  `kPolyPressureEvent`. The `Event` layout (48 bytes, payload at offset 24) is
  asserted at compile time against `ivstevents.h` / `ivstnoteexpression.h`.
- **Buses.** Activation (`vst3_ensure_active`) reads `getBusCount`: a plugin with no
  audio input gets `setBusArrangements` without an input and no input bus; one with
  an event input bus gets it activated. An ordinary effect keeps its sequence.
  One stereo output only: extra output buses are not passed (ADR-014 multi-out
  stays CLAP only).
- **MIDI controllers.** VST3 has no raw MIDI: pitch bend, CC (0..127) and channel
  aftertouch reach the plugin as parameter changes through the controller's
  `IMidiMapping` (`vst3/midi_mapping.rs`). The table is read in `prepare` under the
  FFI gate (busy gate: unmapped until the next recompile) and applied each block at
  the control's offset (CC `/127`, bend `/16383`); an unmapped controller is dropped.
  `restartComponent(kMidiCCAssignmentChanged)` requests a recompile, which reads the
  table again, without reactivating the plugin.
- **No lost note-off.** The adapter counts the notes the plugin received. A NoteOff
  of a block the FFI gate skipped, or that a full event list could not take, is sent
  at offset 0 of the next block; `reset` marks every sounding note the same way (no
  FFI call), and each pitch is released once. `InstrumentNode` already ends played
  notes on stop, pause, seek and loop (ADR-012, ADR-022, ADR-030).
- **Sandbox (ADR-015)** stays CLAP only: a VST3 instrument always runs in-process,
  whatever the isolation preference.
- **Parameters (ADR-021)** reach a VST3 instrument as for the VST3 effect: queued
  edits, plugin-window edits, modulation and automation are delivered by the code
  both share (`vst3/param_delivery.rs`).
- **Not done.** Plugin latency is not read (`InstrumentDevice` has no latency hook,
  as for CLAP instruments); no `kNoteExpression*` events, no `IMidiLearn`; no
  `outputEvents`; a single output bus (ADR-014).
- **C-2.** No VST3 plugin is in the render harness; internal and CLAP instruments
  are untouched and the golden hashes are unchanged. Known risk #2 above (third-party
  synths are not bit-deterministic) applies to VST3 too.
- **Boundary (ADR-003).** `VstInstrument` lives in `plugin_formats/`, not in
  `audio_graph/` (unlike `ClapInstrument`, an existing debt left as is).

Details: `../plugins/plugin-wrapper-system.md` (VST3 instruments). Tests:
`vst3/instrument.rs`, `vst3/event_list.rs`, `vst3/midi_mapping.rs`,
`plugin_system/host.rs`, `plugin_system/ffi_lifecycle_tests.rs`.
