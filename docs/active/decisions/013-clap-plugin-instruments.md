# ADR-013 — CLAP plugin instruments via an `InstrumentDevice` adapter (Phase 5c)

## Status

Accepted (CLAP slice). Implements the **plugin-instrument** half of **Phase 5c**
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
  - **VST3i not wired** (returns `Err` → PolySynth).
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
   instruments only until VST3i lands. Original text: A saved project whose
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
- **VST3i** — `IEventList` note events, same `InstrumentDevice` adapter shape.
- **Live MIDI input** — `MidiInputNode` + per-instance MIDI SPSC queue.
