# ADR-038 — Transpose, scale and chord as stateless note stages ahead of the arpeggiator

## Status

**Accepted — 2026-11-26** (option A, the fixed order, the V1 shape / scale lists and
the UI placement confirmed by the user). Implementation status at the end.
Builds on ADR-030 (arpeggiator note stage, whose "second note effect" trigger this
ADR answers), ADR-001 (graph-first), ADR-004 (immutable snapshots), ADR-011 /
ADR-012 (`PortKind::Midi`, beat-space MIDI), ADR-028 (typed per-track device
settings) and ADR-029 (event budget, `ControlState`).

> **Governed surface touched:** invariant 1 (graph-first: new DSP = new node) is
> stretched a second time, and ADR-030's recorded follow-up ("second note effect ⇒
> revisit option B") is deliberately not followed. Hence this ADR and the user's
> approval before any code.

## Context

The roadmap lists "Note FX other than the arpeggiator (transpose, chord, scale)".
ADR-030 put the arpeggiator in `InstrumentNode::process` as the node's single
note-effect slot and said that the second note effect is the trigger to build a
real MIDI node (its option B), because two real users would then exist to design
the MIDI port against.

The three effects asked for are not like the arpeggiator:

- They are **stateless per note**: a pitch goes in, up to a few pitches come out.
  No clock, no grid, no random generator, no dependence on the transport. The
  arpeggiator is the only stage that needs the tempo.
- Their only state is the _mapping of each sounding note_, so that a `NoteOff`
  releases what its `NoteOn` started even if a setting changed in between.
- None of them needs MIDI to travel between nodes. Nobody has asked for
  track-to-track MIDI routing or a user-reorderable chain.

So the "two real users of the MIDI port" would still be speculative: the second
user (a stateless pitch mapper) tells us nothing about routing.

## Options

### A — Stateless stages in `InstrumentNode`, fixed order (recommended)

A pure `NoteStage` (own file `audio_graph/note_stage.rs`, events in → events out,
no node or device knowledge, same shape as `Arpeggiator`) applying, in this
fixed order: **transpose → scale → chord**, then the existing arpeggiator.

```
sources → event_buf → sort_by_offset → NoteStage → stage_events → Arpeggiator → device
```

- **Cost:** small. One new file, a second pre-allocated `MidiEventList`, one
  project field, one command pair, one panel. No compiler or scheduler change
  (C-7); the compiler only learns that note-effect settings make an instrument
  node appear (like ADR-030 §2).
- **Price:** the second scoped stretch of invariant 1, in the same node, on the
  same precedent. `NoteStage` knows nothing about the node, so it can be lifted
  into a MIDI node later without changing its logic (as ADR-030 said of the
  arpeggiator).
- **What it cannot do:** reorder the effects, put two of the same kind, or route
  MIDI elsewhere.

### B — A real MIDI node with `PortKind::Midi` edges (ADR-030 option B)

`InstrumentNode` split into a MIDI source and an instrument; one node per note
effect; the compiler, execution plan and scheduler learn MIDI buffers.

- **Cost:** large, and it touches the **scheduler (RT-critical)**, plugin-instance
  keying (`instrument:{track_id}`), the recorder and queue wiring, and every
  instrument track's path, so all MIDI golden hashes are at risk (C-2).
- **Verdict:** still the right long-term target, still the wrong size for three
  pure pitch mappers. **New trigger** (replaces "the second note effect"): MIDI has
  to cross nodes (track-to-track routing, a side-chained note source) or the user
  needs an orderable / repeatable chain. Then its own migration step and ADR.

### C — A generic `note_effects: Vec<NoteEffect>` list from the start

Ordered list in the project, ids, add / remove / reorder commands.

- **Verdict:** this is the user-facing shape of B. Without B's engine it is an
  empty promise (the engine still runs a fixed array), and it costs a command set
  for four effects. Rejected for now; a `serde` migration can replace the struct
  below by a list when B lands.

## Decision (if A is approved)

### 1. Settings

One typed struct on the track, next to `arpeggiator`, which stays as it is
(no migration, C-4):

`AppTrack.note_effects: Option<NoteEffectSettings>`, `serde(default,
skip_serializing_if = "Option::is_none")`:

```
NoteEffectSettings {
  transpose: { enabled, semitones },          // -24..=+24, default 0
  scale:     { enabled, root, scale },        // root 0..=11 (C..B)
  chord:     { enabled, shape },
}
```

- `scale`: `major`, `natural_minor`, `harmonic_minor`, `dorian`, `phrygian`,
  `lydian`, `mixolydian`, `pentatonic_major`, `pentatonic_minor`, `blues`.
  A pitch outside the scale moves to the **nearest scale pitch, down on a tie**
  (fixed rule: deterministic, no per-note state).
- `chord.shape`: semitone offsets above the played pitch, a closed list of fixed
  shapes — `octave` (0, 12), `fifth` (0, 7), `major` (0, 4, 7), `minor` (0, 3, 7),
  `sus2`, `sus4`, `diminished`, `augmented`, `major7`, `minor7`, `dominant7`.
  At most **4 voices** per played note.
- Values are clamped at load and on command; output pitches above 127 or below 0
  are dropped (the chord keeps its other voices).
- **Not in V1:** diatonic chords (degrees of the scale — the shapes above are
  exact semitones, so a chord on a scale-quantized note can leave the scale),
  user-defined intervals, strum, inversions, velocity spread.

### 2. State and hot path

- Command `SET_NOTE_EFFECTS { track_id, patch }` → event `NOTE_EFFECTS_SET`
  carrying the **whole resulting settings** (replay-safe, like `ARPEGGIATOR_SET`);
  one user action = one undo step. The frontend stays a reactive mirror (ADR-005).
- `NoteEffectCells` in `LiveParams`: **one atomic per setting**, keyed by track id
  and created at compile for every instrument node (same pattern and same reason
  as `ArpeggiatorCells`: a knob drag or an undo needs no recompile, and each
  value can become a modulation / automation target later — not in V1).
- A recompile happens only when the instrument node must **appear**
  (`plays_instrument` includes `note_effects.is_some()`, like the arpeggiator).

### 3. Mapping and note bookkeeping

For every played `NoteOn`, the stage computes its output pitches (transpose, then
scale, then chord shape) and **records them against the played pitch**
(`voices[pitch]`: up to 4 pitches). The matching `NoteOff` releases exactly the
recorded pitches, whatever the settings are by then. Consequences:

- Changing or disabling an effect while notes are held never leaves a note stuck:
  held notes keep their old mapping until released; new notes use the new one.
- Two played notes can map to the same output pitch (a chord on C and on E share
  G). `out_count[pitch]` counts them: the instrument gets a `NoteOn` on 0 → 1 and
  a `NoteOff` on 1 → 0, so a shared pitch is never cut by the first release.
- `Control` events pass through at their original offsets. Events keep their order
  (the output of an ordered input is ordered), so the stage never calls
  `sort_by_offset` on its output.
- Dormant stage: every effect disabled **and** no recorded note → the arpeggiator
  receives `event_buf` untouched (no copy); existing projects are bit-identical
  (C-2).
- `active_pitch_count`, `preview_count`, `live_input_count`, the envelope gate and
  the ● recorder keep seeing the **played** notes, upstream of the stage — exactly
  the ADR-030 §3 position, so the recorder is non-destructive.
- The arpeggiator now receives the stage's output, so it arpeggiates the chord the
  chord effect built, and its held-set accounting stays balanced because the stage
  emits balanced `NoteOn` / `NoteOff` pairs. Stop, pause, seek and loop wrap
  already turn every played note off at offset 0; the stage turns off what it
  recorded in the same block.

### 4. Event budget (`MAX_EVENTS_PER_BLOCK` = 256, unchanged)

A chord multiplies events by up to 4. Policy, in this order of priority:

1. A `NoteOn` is emitted only if the output list can hold **its voices and the
   releases of the same voices** (`2 × voices` free slots); otherwise it is skipped
   whole and **not recorded** (its later `NoteOff` is then ignored). A chord is
   never half-played.
2. A `NoteOff` group is always written when it fits. If the list is full, the
   releases stay **owed** in `voices[pitch]` and are written first at offset 0 of
   the next block: late by one block, never stuck.

Worst realistic case: a stop with 30 held chord notes of 4 voices = 120 events,
inside the budget. The limit is not raised (ADR-029 §5).

### 5. Real-time footprint

All state is fixed arrays inside the boxed node: `voices: [[u8; 4]; 128]`,
`voice_count: [u8; 128]`, `out_count: [u8; 128]` (~0.7 KB) and a second
`MidiEventList` (~2 KB, allocated at compile). Per block: one `Relaxed` atomic
load per setting and O(events) work. No allocation, lock, `drop`, I/O or
`HashMap` traversal on the audio thread (`rt_audio_rules.md`).

## Compatibility

- **C-2.** No `note_effects` ⇒ the stage is dormant ⇒ all existing golden hashes
  unchanged. A new harness project `note_effects` (own golden hash; block-size
  independence at 4096 / 512 / 441 / 64 / 37; one project mixing chord, scale and
  the arpeggiator) is added **before** the engine code. Stateless stages have no
  f64 beat arithmetic, so no new cross-OS risk beyond ADR-012's.
- **C-4.** `serde(default)`; an event log without `NOTE_EFFECTS_SET` replays to no
  note effects.
- **C-7.** Compiler touched only at the `plays_instrument` rule and the cell
  creation; scheduler untouched.
- **Invariant 5.** Rust owns the settings; the UI sends `SET_NOTE_EFFECTS` and
  mirrors `NOTE_EFFECTS_SET`.

## UI (placement to confirm; mock-up shown before integration)

In `InstrumentPanel.svelte` (always `compact` in the left panel), a block
`NoteEffectsPanel.svelte` above the arpeggiator, for every instrument kind: three
rows (Transpose, Scale, Chord), each with a power dot in the style of the effect
bypass. Transpose: `NumberInput` (semitones). Scale: two `CustomSelect` (root,
scale). Chord: one `CustomSelect` (shape). Shared widgets only (`ui_controls_rules.md`),
no badge, no status text, `cursor: default` on disabled controls.

## Tests

- Pure `NoteStage`: each transform alone and combined (order matters: transpose
  then scale then chord); every scale quantizing out-of-scale pitches down on a
  tie; every chord shape; pitches clamped to 0–127; a setting changed or disabled
  while a note is held releases the old mapping; two played notes sharing an
  output pitch; budget policy (chord skipped whole, owed releases written next
  block); controls untouched.
- `InstrumentNode`: NoteOff guaranteed on stop, seek, loop wrap; played-note
  counters and envelope gate unchanged; stage + arpeggiator together (the arp
  receives the built chord); dormant stage sends the same events as no stage.
- `render_harness.rs`: project `note_effects`; existing hashes unchanged.

## Alternatives considered

- **Option B / C** — above.
- **Putting the effects inside `Arpeggiator`** — rejected: it needs the clock and
  the held set; the new stages need neither, and a chord must work with the
  arpeggiator off.
- **Chord / transpose in the frontend** — rejected: invariant 5, and live and
  clip notes would not go through the same path.
- **Raising `MAX_EVENTS_PER_BLOCK`** — rejected (ADR-029 §5).

## Consequences

- Transpose, scale and chord on any instrument track (synth, sampler, CLAP, VST3),
  deterministic, undoable, persisted, working for clips, live keys and audition.
- Fixed order and one instance of each; reordering or repeating an effect needs
  option B.
- ADR-030's trigger is amended on acceptance: option B is revisited when MIDI must
  cross nodes or a chain must be orderable, not at the second note effect.

## To confirm with the user

1. Option A (fixed stages in `InstrumentNode`) rather than B (real MIDI node).
2. The fixed order transpose → scale → chord → arpeggiator.
3. Shape and scale lists as V1 (no free intervals, no diatonic chords).
4. UI placement above the arpeggiator, one block with three rows.

## Implementation plan

1. Harness project `note_effects` + `NoteEffectSettings` (clamping, serde, tests).
2. Pure `NoteStage` + its unit tests.
3. `NoteEffectCells`, the slot in `InstrumentNode`, compiler rule; harness golden.
4. `SET_NOTE_EFFECTS` / `NOTE_EFFECTS_SET`, reducer, undo.
5. `NoteEffectsPanel.svelte` after a mock-up.
6. Docs: `audio-graph.md`, `state-management.md`, `realtime-safety.md` (event
   budget), `change_impact_mapping.md` row, `state_index.md`, amendment note in
   ADR-030.

## Implementation status (2026-11-26)

All six steps done. `note_effect_settings.rs`, `audio_graph/note_stage.rs`,
`NoteEffectCells` (`live_params.rs`), the stages and `DeviceFeed` in
`InstrumentNode`, `SET_NOTE_EFFECTS` / `NOTE_EFFECTS_SET`, `AppTrack.note_effects`,
`NoteEffectsPanel.svelte` (the "one block, three rows" mock-up the user chose),
harness project `note_effects` (golden `0xf98b_cd02_1dc9_c040`; the 14 earlier
hashes unchanged; block-size independence at 4096 / 512 / 441 / 64 / 37; an
enabled effect changes the render, a disabled one renders exactly like none).
Not hand-tested by ear or in the app. Two refinements to the text above: a
`NoteOn` on a pitch whose release is still owed is skipped (extreme load only),
and a pitch held by two sources (clip and keyboard) is counted, so only the last
`NoteOff` releases it.
