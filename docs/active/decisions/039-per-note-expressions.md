# ADR-039 — Per-note expressions (pressure, timbre, pitch) drawn on the notes of a clip

## Status

**Accepted — 2026-11-26** (spec, plan and the three open choices confirmed by the
user: expressions aimed at the key rather than at a note id, the arpeggiator
ignores them, edits go through the existing `MIDI_CONTENT_NOTES_SET`).
Builds on ADR-029 (channel controls, held values, event budget), ADR-032 (shared
MIDI content), ADR-038 and ADR-030 (note stages), ADR-012 (beat-space MIDI) and
ADR-002 (event sourcing). Implementation status at the end.

> **Governed surface touched:** the project state shape (`MidiNote` grows a field,
> C-4) and a new event on the audio path (C-2). The built-in instruments ignore it
> and the render harness proves it.

## Context

ADR-029 stores values that belong to the whole channel (pitch bend, CC, channel
pressure). A sustain pedal belongs to no note, so it was needed first. What
Bitwig calls note expressions is different: a curve drawn on **one note** (how
hard it is pressed, its timbre, a micro-pitch glide), played by the instrument to
that note's voice only. ADR-029 left it out on purpose and listed it as
"Future": data on `MidiNote`, CLAP note-expression events, capture that keeps the
MIDI channel.

This ADR is the first cycle: **the model, the playback and the editing**. MPE
capture (cycle 2) needs a controller to be validated and is not decided here.

## Decision

### 1. Data model — `MidiNote.expressions`

`NoteExpressionPoint { start, kind, value }` (`project.rs`), a flat list per note:

- `start` in **beats, relative to the start of its note** (not to the clip), so
  moving or resizing the note moves its curves with it and a tempo change needs no
  recompile (ADR-012). A point at or beyond the note's duration cannot play and is
  dropped.
- `kind`: `pressure` and `timbre` in `0..1`, `pitch` in semitones, clamped to
  ±24 (`PITCH_EXPRESSION_SEMITONES`).
- **Held steps**, like ADR-029: a value holds until the next point of the same
  kind; smooth curves are baked into steps at edit time. Before its first point a
  kind sends nothing, so the plugin keeps its own default.
- Persisted with `serde(default, skip_serializing_if = "Vec::is_empty")`: a note
  without curves is written exactly as before and an old file loads unchanged
  (C-4). `sanitize_note_expressions` (bake) drops non-finite or out-of-note points,
  clamps values and sorts, so the engine never trusts the file.
- Lives in `MidiContent.notes`, hence in shared contents (ADR-032): every clip
  that plays the content plays the curves.

### 2. Aimed at the key, not at a note id

The engine identifies a sounding note by its pitch everywhere (`NoteOn { pitch }`,
`NoteOff { pitch }`, `active_pitch_count`). Adding a note id to every stage, every
device and the bridge protocol was the expensive option. The new event is

`MidiEventKind::NoteExpression { pitch, kind, value }`

and a CLAP instrument receives it as `CLAP_EVENT_NOTE_EXPRESSION` with `key =
pitch`, `note_id = -1`, `channel = -1`, which CLAP defines as "every note on that
key". Mapping: `pressure` → `PRESSURE`, `timbre` → `BRIGHTNESS`, `pitch` →
`TUNING`.

**Known limit:** two notes of the same pitch that overlap in time share their
expression (the last value wins for both). Rare in practice; per-note ids can be
added later without changing the file format.

### 3. Playback and transport jumps — `audio_graph/note_expressions.rs`

`MidiNoteSnapshot.expressions` holds the baked points (note start + point start,
in beats from the clip origin). `InstrumentNode` asks `push_block_expressions` for
each note overlapping the block:

- A point plays at its own sample if it lies inside the block **and** inside the
  note's playable window (trim, ADR-018).
- On a **resync block** (play start, seek, loop wrap, or a content swap while
  playing) a note that was already sounding first receives, at offset 0, the value
  each kind holds at the block start; its points then play from the next sample.
  This keeps the event stream identical for every block size (unit-tested at 4096,
  512, 441, 64 and 37), exactly like `ControlState`.
- `NoteExpression` sorts **after** `NoteOn` at the same offset (it needs the voice
  it shapes). Nothing is sent when a note ends: the voice is gone.

### 4. Note stages and the arpeggiator

- **ADR-038 stage:** an expression of a played pitch is repeated on every voice that
  pitch currently sounds (a chord gets the curve on each of its notes; a transposed
  note gets it on the transposed pitch). An expression for a pitch that is not held
  is dropped. A dormant stage leaves the list untouched.
- **Arpeggiator (ADR-030):** while it is engaged it **ignores** expressions: the
  notes it plays are its own, not the held ones the curve was drawn for. Choosing
  which arpeggiated step inherits a curve would be a design of its own.

### 5. Event budget

`MAX_EVENTS_PER_BLOCK` stays 256 (ADR-029 §5). Expressions use `push_control`, so
the last 32 places stay for notes and live input: a dense curve can never drop a
`NoteOff`. The editor keeps curves sparse (one point per 1/32 beat at most, like
the control lanes). A chord multiplies the events by its voices; the budget rule
of ADR-038 §4 already protects the `NoteOff`s.

### 6. Who receives it

CLAP instruments, in the Echoes process and in the sandbox. The bridge cannot carry
the raw CLAP event bytes (a `WireNote` holds three data bytes), so step B added a
wire kind: `NOTE_EXPRESSION` with `data = [pitch, EXPRESSION_*, 0]` and the value as
an exact `f32`, which grew `WireNote` from 12 to 16 bytes and moved
`BRIDGE_PROTOCOL_VERSION` to **5**. The engine fills it in `push_wire_notes`, the
bridge's `ClapAdapter` turns it into the same `CLAP_EVENT_NOTE_EXPRESSION` as the
in-process path (same key target, same ids); a quantised value would have broken the
"sandbox renders like in process" proof. **Ignored**: the
built-in synth and sampler, VST3 instruments in the sandbox as in process (VST3 note
expression is a different interface, not decided here; the VST3 adapter skips the new
kind), and the recorder (cycle 2).

### 7. Editing

Edits replace the notes of the content with the existing `SET_CLIP_MIDI_NOTES`
(event `MIDI_CONTENT_NOTES_SET`): one undo step per gesture, no new command or
event. The price is that a curve edit re-sends the notes of the content; a content
is a clip's worth of notes, and a dedicated command can come if that is measured
to matter.

The piano roll draws the curves as three more **lanes** of the control-lane stack
(`note_pressure`, `note_timbre`, `note_pitch`, picked like any controller, same
height, same gestures: click, drag, Ctrl + drag free-hand, Shift + drag ramp). A
lane edits the curve of the **one selected note** (nothing selected or several: the
lane is empty and inert); it draws in clip beats, dims what lies outside the note and
keeps every point inside it. Values are lane integers (0..127 for pressure and timbre,
tenths of a semitone for micro-pitch, 240 = no bend) converted at the edge
(`frontend/pianoRoll/noteExpressionLanes.ts`). A lane holding a curve on some note
is shown when the clip opens. Copying and pasting notes carries their curves.

## Real-time impact

| Change                                    | RT cost                                                     | RT risk                                          |
| ----------------------------------------- | ----------------------------------------------------------- | ------------------------------------------------ |
| `expressions: Vec` in `MidiNoteSnapshot`  | read-only; published off-thread with the notes (ADR-029 §3) | none — freed on the command thread               |
| Per-block scan                            | only notes overlapping the block, points sorted, early exit | none — no alloc/lock/drop                        |
| `NoteStage` repeat per voice              | one loop over ≤ 4 voices                                    | none — `push_control`, fixed arrays              |
| `ClapEventBuffer::push_note_expression`   | one slot of the existing fixed buffer                       | none — dropped if full, like every event         |

Validated against `rt_audio_rules.md`: no allocation, lock, `drop`, I/O or
`HashMap` traversal in `process`.

## Compatibility

- **C-4.** `expressions` is `serde(default)` and skipped when empty; an old note
  loads with none and is written as before (unit-tested).
- **C-2.** All earlier golden hashes unchanged. New reference project
  `note_expressions` (the reference MIDI clip with four curves on every note): its
  hash equals `static_mix`, proof that curves cannot disturb a render the built-in
  synth plays, and it is block-size independent. The event stream itself is covered
  at the event level (`note_expressions.rs`, `instrument.rs`, `note_stage.rs`).
- **ADR-032.** Every track playing a shared content bakes the same curves
  (unit-tested).

## Alternatives considered

- **A note id end to end.** Rejected for now: it touches every stage, every device
  and the bridge block layout for a case (overlapping same-pitch notes) that is
  rare. The key target is standard CLAP.
- **Curves in clip-relative beats.** Rejected: moving a note would leave its curves
  behind and every drag would rewrite them.
- **Interpolation in the engine** (a curve shape per point). Rejected, as in
  ADR-029: it needs engine interpolation, a bigger event budget and a new harness
  contract; held steps baked at edit time keep the file and the engine simple.
- **A new command and event for curves.** Rejected for now: see §7.
- **Expressions through MPE channels.** Rejected for playback: CLAP note
  expressions are the native form and every MPE-aware CLAP plugin reads them; MPE
  is an input format for cycle 2, converted to these points.

## Consequences

- A CLAP synth that supports note expressions plays per-note pressure, timbre and
  micro-pitch from the clip, and seeks, loops and edits never leave a stale value.
- Not hand-tested: nothing was heard on a real synth (Vital, Surge XT).
- Step B and step C are tested (the test plugin, in process and sandboxed) but neither was tried by hand.

## Future (not decided here)

Cycle 2: MPE live input (the MIDI channel kept, per-channel bend / pressure / CC 74
mapped to these points, the `MIDI_MPE` dialect) and its capture in the recorder.
A note id if overlapping same-pitch notes ever matter. VST3 note expression.

## Implementation status

2026-11-26 — step A done (model, engine, note stage, CLAP output, harness project
`note_expressions`, a real CLAP plugin test). Step B (sandbox) pending.

2026-11-26 — step C done (piano-roll lanes, see §7; `noteExpressionLanes.test.ts`,
`controlLanes.test.ts`; not tried in the app).

2026-11-27 — step B done (sandbox, §6): protocol v5, `WireNote` 16 B,
`NOTE_EXPRESSION`; tests `a_note_pressure_scales_a_held_{in_process,sandboxed}_note`
(`plugin_system/sandbox_tests.rs`), `a_note_expression_travels_…`
(`bridged_node.rs`), the ABI tests of `protocol.rs`. Left: cycle 2 (MPE).
