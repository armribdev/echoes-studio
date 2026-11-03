# ADR-029 — Channel controls in MIDI clips (pitch bend, CC, aftertouch)

## Status

Accepted — 2026-10-13 (approved by the user after the plan and the piano-roll
UI were discussed). Builds on ADR-012 (beat-space MIDI), ADR-018 (clip window),
ADR-022 (live hardware input, 2026-10-13 amendment) and ADR-002 (event
sourcing). Not hand-tested yet with a real keyboard.

## Context

Since the ADR-022 amendment of 2026-10-13, a keyboard's pitch bend, control
changes and aftertouch reach CLAP instruments live, but `MidiRecorder::capture`
dropped them: clips held notes only, so a take lost its sustain pedal and its
bends on playback. Recording them needs a project-format change (C-4), and a
control is a **state**, not an event: after a seek, a loop wrap or the end of a
clip, the instrument must get the right value, or notes ring forever under a
pedal that was never released.

## Decision

### 1. Data model — `AppMidiClip.channel_controls`

`MidiControlPoint { start, kind, number, value }` (`project.rs`):

- `start` in **beats**, relative to the clip content origin — the same space as
  `MidiNote.start` (ADR-012), so a tempo change moves controls with the notes
  without a recompile, and the clip window cuts them like notes (ADR-018).
- `kind`: `pitch_bend`, `control_change`, `channel_pressure`, `poly_pressure`.
  `number` is the CC number or the key of a polyphonic pressure, `0` otherwise.
  `value` is 14-bit for a bend, 7-bit for the others. Typed rather than raw
  bytes so the file stays readable and the editor needs no MIDI parsing.
- One flat list per clip, sorted by `start`, no per-point id (a bend sweep is
  hundreds of points; the editor replaces a span, never one point by id).
- Persisted in `MidiClipFile` with `#[serde(default, skip_serializing_if =
"Vec::is_empty")]`: older `.echoes` load unchanged and a clip without controls
  is written exactly as before. `sanitize_control_points` (load, `ADD_MIDI_CLIP`,
  `SET_CLIP_MIDI_CONTROLS`, bake) drops invalid positions, clamps values and
  sorts.
- Named `channel_controls` on purpose: per-note expressions (MPE, Bitwig-style
  note curves) are a different model that would live on `MidiNote`. See
  "Future" below.

### 2. Recording

`MidiRecorder::capture` keeps the `Control` messages of the armed track, placed
with the same latency-compensated `transport_position_at` as notes.

- Controls played during the count-in only set the value the clip starts with
  (written at beat 0 unless neutral): a pedal held before the first beat is down
  when the clip starts. Polyphonic pressure is not carried over (it follows a
  note).
- Thinning: per control, the last value of each 5 ms bucket, then repeated
  values dropped. At most ~200 points per second per control, final value of
  every gesture kept.
- A recording with controls but no note still makes no clip (unchanged rule,
  confirmed by the user).
- The clip goes through the existing `ADD_MIDI_CLIP` (new optional
  `channel_controls` field): still one undo step.

### 3. Publication — one content per track, no recompile

`MidiNotePublisher` becomes `MidiTrackPublisher` and publishes a
`MidiTrackContent { notes, control_clips }` instead of a note `Vec`. Notes and
controls swap together, so an undo restores both in the same block. The bake
(`compiler::bake_track_midi`, ex-`bake_track_midi_notes`) groups the points per
clip with the clip's content origin and window (`MidiControlClip`).
`SET_CLIP_MIDI_CONTROLS` (event `MIDI_CLIP_CONTROLS_SET`, now `MIDI_CONTENT_CONTROLS_SET`,
ADR-032) publishes without a
recompile, exactly like `SET_CLIP_MIDI_NOTES`; both share
`api::publish_track_midi`. They stay separate commands so a note edit never
re-sends thousands of control points.

### 4. Playback and transport jumps — `audio_graph/midi_controls.rs`

Held controls are the 128 CCs, the bend and the channel pressure (130 slots).
The **timeline value** of a slot at a position is the last point at or before
it in a clip whose window covers it — points hidden by a left-trim count, so a
trimmed or split clip starts with the right pedal state. Elsewhere the slot is
**neutral**, as after a "reset all controllers": bend centre, pressure 0, CC 1 /
64–67 at 0, CC 11 (expression) at 127. Other CCs (volume, pan, filters…) have
no neutral: they keep their level outside a clip.

`InstrumentNode` holds a `ControlState` (fixed `[bool; 130]` "moved by the
timeline") and, per playing block:

| When                                                                        | Sent                                                                                                              |
| --------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| Discontinuity (play start, seek, loop wrap) or a content swap while playing | `sync`: the timeline value of every slot at the block start (neutral for a slot the timeline had moved), offset 0 |
| A clip window ends in the block                                             | neutral for every slot that clip uses                                                                             |
| A clip window starts in the block                                           | the clip's held value at its window start                                                                         |
| A point falls in the block and in its window                                | the point                                                                                                         |
| Play → pause / stop                                                         | `release`: neutral for every slot the timeline moved                                                              |

Ends are pushed before starts, starts before points; the stable sort keeps that
order at one offset, so a clip ending where the next begins hands the pedal
over cleanly. When `sync` ran, the block start itself belongs to it (`from =
block_start + 1`), which keeps the event stream identical for every block size
(unit-tested at 4096 / 512 / 441 / 64 / 37). Polyphonic pressure is played but
never held or reset.

Built-in instruments (`PolySynth`, sampler) still ignore controls; CLAP
instruments receive them as `CLAP_EVENT_MIDI` through the existing path.

### 5. Event budget

`MAX_EVENTS_PER_BLOCK` stays 256: the stable sort of 8-byte `MidiEvent`s stays
on its 4 KB stack scratch only up to 512 events, so raising it would allocate
on the audio thread. Timeline controls use `MidiEventList::push_control`, which
leaves the last 32 places to notes, audition and live input: a dense control
block can never drop a NoteOff. With 5 ms thinning a 4096-frame block holds
about 17 points per control.

### 6. Live input fix — channel pressure

`parse_live_message` only accepted 3-byte messages, so channel aftertouch
(`0xD0`, 2 bytes — what most keyboards send) never reached instruments, live or
recorded. It is now parsed as `Control { message: [0xD0, value, 0] }`.

### 7. Editing — piano-roll control lanes

Up to three stacked lanes under the velocity lane, each with a control picker,
a clear button and a hide button; lanes holding data open automatically when a
clip opens; `+ Contrôle` adds one (presets: pitch bend, modulation, sustain,
expression, aftertouch, plus any CC present). Same gestures as arrange
automation lanes: click to add, drag a point, double-click to delete,
Ctrl + drag to draw free-hand (one value per 1/32 beat), and Shift + drag for a
straight ramp (held steps every 1/16 beat). Values are held steps, drawn as a
staircase; the bend lane shows its centre line. Every gesture commits one
`SET_CLIP_MIDI_CONTROLS` on mouse-up (one undo step). Which lanes are shown is
UI state, not project data. Clip copies (copy/paste, Alt-drag duplicate,
repeat) carry the controls with the notes (`frontend/midiClipSeed.ts`).

## Real-time impact

| Change                             | RT cost                                                      | RT risk                                              |
| ---------------------------------- | ------------------------------------------------------------ | ---------------------------------------------------- |
| `ControlState` in `InstrumentNode` | 130-byte array, fixed                                        | none                                                 |
| Per-block control scan             | binary search + points in the block, per clip                | none — no alloc/lock/drop                            |
| `sync` (seek, swap)                | one pass over the track's points into a 130-slot stack array | none                                                 |
| `MidiTrackContent` swap            | same atomic pointer + retire ring as before                  | none — displaced content freed on the command thread |

Validated against `rt_audio_rules.md`: no allocation, no lock, no drop, no I/O
in `process`.

## Compatibility

- **C-4.** `channel_controls` is `serde(default)` and skipped when empty; event
  logs without it replay with no controls.
- **C-2.** All 11 golden hashes unchanged. New reference project
  `midi_controls` (the reference MIDI track plus bend, mod, sustain, channel and
  polyphonic pressure): block-size independent, and its hash equals
  `static_mix` because the built-in synth ignores controls — proof that controls
  cannot disturb a render. The control event stream itself is covered at the
  event level (`midi_controls.rs` tests), since the harness cannot load a CLAP
  instrument.

## Alternatives considered

- **Raw MIDI bytes in the project file.** Rejected: unreadable JSON, and the
  editor would have to parse status bytes.
- **Control points in seconds.** Rejected: they would drift from the notes on a
  tempo change (ADR-012).
- **A separate publisher for controls.** Rejected: an undo could swap notes and
  controls in different blocks.
- **Synthesising window-edge points at bake time.** Rejected: which point is
  before a window edge depends on the tempo (beats vs seconds), so it must be
  resolved per block.
- **Sending "reset all controllers" (CC 121) at stops and clip ends.** Rejected:
  plugins interpret it differently and it also resets controls the clip never
  touched.
- **Raising `MAX_EVENTS_PER_BLOCK`.** Rejected: see §5.
- **Per-note expressions now (Bitwig).** Rejected for this step: a classic
  keyboard sends channel-wide controls (a sustain pedal belongs to no note), so
  they are needed either way; per-note curves need a new model, MPE input and
  CLAP note-expression output.

## Consequences

- A take now plays back with its pedal, bends and wheel moves on CLAP
  instruments, and stops/seeks/clip ends never leave a pedal down.
- Polyphonic pressure is stored and played but has no lane in the editor yet.
- Not hand-tested: recording, playback on a real CLAP synth, and the lanes.

## Future (not decided here)

Per-note expressions (pressure, timbre, micro-pitch curves drawn on the notes,
MPE controllers): data on `MidiNote`, CLAP note-expression events, capture
keeping the MIDI channel. Needs its own ADR. Tracked in `ROADMAP.md`.
