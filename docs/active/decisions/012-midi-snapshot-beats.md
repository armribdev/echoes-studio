# ADR-012 — `MidiNoteSnapshot` in beats, per-block tempo resolution (Phase 5b)

## Status

Accepted. Implements **Phase 5b** of
[`docs/active/migration/migration_plan.md`](../migration/migration_plan.md) — the `ADR-012` of
[`docs/active/migration/target_architecture.md`](../migration/target_architecture.md) §9.6. File
numbering continues from the existing `decisions/` series.

The runtime mechanism landed with the instrument/MIDI work; this ADR records the
decision and adds the **BP-3 bit-identical test lock** the migration plan
requires before later phases build on the MIDI path.

---

## Context

Audit / target (critical_issues **2.1**, target §6.5) — *"`UPDATE_TEMPO`
désync"*:

- A naive bake stores MIDI note positions in **samples** computed from the BPM
  at compile time. Changing the tempo then either desyncs the timeline (notes
  keep their old sample positions) or forces a full graph recompile per tempo
  edit.
- The project model already stores notes in beats: `MidiNote.start` /
  `.duration` are beats relative to the clip, while `AppMidiClip.start` / `.end`
  are seconds on the timeline (same coordinate space as audio clips) —
  [`project.rs`](../../../src-tauri/src/project.rs). The bake must respect this
  mixed model rather than collapse it to samples early.

## Decision

`MidiNoteSnapshot` stores notes in **beat space** (`start_beats`, `end_beats`)
anchored by a tempo-independent `clip_start_sec`. Sample positions are resolved
**per block** from the live transport BPM, so a tempo change reaches the
scheduler without a rebake or recompile.

### Module map

1. **[`audio_graph/midi.rs`](../../../src-tauri/src/audio_graph/midi.rs)**
   - `MidiNoteSnapshot { pitch, velocity, clip_start_sec, start_beats,
     end_beats }`.
   - `beats_to_sample(clip_start_sec, beats, bpm, sample_rate)` —
     `(clip_start_sec + beats·60/bpm)·sample_rate`. The clip anchor is in
     seconds (tempo-independent); only the beat offset scales with tempo.
   - `start_sample(bpm, sr)` / `end_sample(bpm, sr)` resolve a note for a given
     tempo.

2. **[`audio_graph/compiler.rs`](../../../src-tauri/src/audio_graph/compiler.rs)**
   - `build_node(NodeKind::Instrument)` bakes each project note into a
     `MidiNoteSnapshot` — beats from `note.start` / `note.duration`, anchor from
     `clip.start`. No BPM is captured at bake time.

3. **[`audio_graph/nodes/instrument.rs`](../../../src-tauri/src/audio_graph/nodes/instrument.rs)**
   - `InstrumentNode::process` resolves `note.start_sample(transport.bpm,
     transport.sample_rate)` **every block**, emits `NoteOn`/`NoteOff` at the
     intra-block offset, and handles seek/resume discontinuity with a catch-up
     `NoteOn`. On a play→pause transition it emits a `NoteOff` for notes still
     sounding at the pause point, so voices release instead of ringing until the
     transport later crosses their end.
     _Fixed 2026-09-23:_ the release now uses the set of pitches actually turned
     on (`active_pitch_count`), not the notes found at the current position. Stop
     and end-of-song auto-stop reset the playhead to 0 before the stop block, so
     the old position-based check missed the sounding notes: stuck notes after
     Stop, and an intermittent stuck last note when a note ended exactly on the
     last block boundary before the ~30 Hz auto-stop fired.

4. **[`audio_engine/transport.rs`](../../../src-tauri/src/audio_engine/transport.rs)**
   - `SharedTransport.bpm_bits: AtomicU32`; `snapshot(block_size)` reads it into
     `TransportInfo.bpm`. The audio callback sees the live BPM each block.
     _Updated 2026-09-23:_ the setter is now `set_tempo(bpm, time_signature)`,
     and `AudioEngine::update_from_project` also calls it on every recompile.
     Before that, project load and undo/redo left the live BPM stale until the
     next `UPDATE_TEMPO`, so MIDI played at the wrong tempo after opening a
     project.

5. **[`engine_gateway/api.rs`](../../../src-tauri/src/engine_gateway/api.rs)**
   - `UPDATE_TEMPO` calls `transport.set_tempo` + updates `project.bpm`, and
     **does not recompile**. The new BPM flows to the next block's snapshot.

### Test lock (the Phase 5b deliverable)

- `midi.rs`: `beats_to_sample` is exact and deterministic at constant tempo
  (beat 1 @ 120 BPM / 48 kHz = 24_000 frames) — the bit-identical guard (C-2 /
  BP-3); the clip anchor is verified tempo-independent; a single snapshot
  rescales correctly when the BPM doubles (no rebake).
- `instrument.rs`: a `CaptureDevice` mock records emitted events. One test pins
  the `NoteOn` offset at 120 BPM; another changes `transport.bpm` between two
  blocks on the **same node** (notes unchanged) and asserts the `NoteOn` lands
  at the position dictated by the new tempo — proving per-block resolution
  without recompile.

## Real-time impact

| Change | RT cost | RT risk |
|---|---|---|
| `start_sample` / `end_sample` per note per block | a few f64 mul/add + cast | none — bounded by note count, no alloc |
| `TransportInfo.bpm` read | one `Relaxed` atomic load at snapshot | none |
| `UPDATE_TEMPO` | `set_tempo` atomic stores (off-RT) | none — no recompile |

C-1 preserved: resolution is pure arithmetic over a pre-allocated, `Arc`-shared
note list; the `CaptureDevice` `Mutex` exists only under `#[cfg(test)]`.

## Compatibility

- **C-2 (bit-identique offline).** Preserved. Offline export resolves at a
  constant `project.bpm`, giving the same sample positions as a sample-baked
  snapshot would have. The new tests freeze these exact values.
- **C-4 (saved projects).** No schema change — `MidiNote` is already in beats,
  `AppMidiClip` already in seconds.
- **C-7 (compiler extended, not rewritten).** Only the bake site changed.

## Consequences

- **Positive.** Tempo edits during playback re-place MIDI notes with no
  recompile and no desync (audit 2.1 closed). Regression-locked by BP-3 tests.
- **Limits / debt.**
  - The mixed model (clip anchor in seconds, notes in beats) means a tempo
    change moves notes **within** a clip but not the clip's timeline anchor.
    Intended for the current project model; documented by
    `clip_anchor_is_tempo_independent`.
  - The BP-3 lock is unit-level (resolution + node), not yet an end-to-end
    golden-master `.wav` (harness V1 of the plan, still to land in CI).
- **Reversibility.** The runtime is unchanged by this ADR (it predates it); the
  tests are additive.

## Known risk — surfaced 2026-06-03 (architecture review), not yet closed

**Cross-platform determinism of `beats_to_sample` is asserted, not proven.**
`(clip_start_sec + beats·60/bpm)·sample_rate` is computed in `f64` and cast
(truncated) to a sample index. For a note landing near an integer boundary
(e.g. 23999.9999 vs 24000.0001), the FPU rounding / compiler FMA / libm
differences between Windows, macOS and Linux can flip the truncated result by one
sample, moving the `NoteOn` into a different block. The current test pins
`beat 1 @ 120 BPM / 48 kHz = 24_000` on **one** platform — that does not
establish cross-OS bit-identity (C-2 across machines).

This is governed by the determinism invariant
(`architecture-decision-rules.md`). Close it with the **cross-OS golden-master
`.wav`** for a MIDI project (harness V1), and consider resolving notes via exact
integer/rational arithmetic at the bake anchor rather than an `f64` multiply-then-
truncate, so the result cannot depend on the FPU.

## Alternatives considered

- **Bake to samples at compile using the current BPM.** Rejected — the original
  desync/recompile shape (critical_issues 2.1).
- **Store clip anchor in beats too.** Rejected for now — the project model
  deliberately keeps clips in seconds to share the audio-clip coordinate space;
  changing it is out of 5b scope.

## Follow-ups

- **Phase 5c** — the same per-block note resolution feeds plugin instruments
  (via `PluginHost.midi_queues` and `PortKind::Midi` edges) in addition to the
  built-in `PolySynth`.
- **CI** — promote the unit lock to an end-to-end bit-identical render (V1).
