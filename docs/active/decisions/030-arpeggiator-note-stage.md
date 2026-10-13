# ADR-030 — Arpeggiator as a note stage inside `InstrumentNode` (V1)

## Status

**Accepted — 2026-10-13** (option A1 and the left-panel placement approved by the
user; amended the same day after the "Bitwig-style" discussion: one atomic per
setting, a single note-effect slot, option B named as the follow-up).
Builds on ADR-001 (graph-first), ADR-004 (immutable snapshots), ADR-012
(beat-space MIDI), ADR-013 (`InstrumentDevice` seam), ADR-022 (live input),
ADR-028 (sampler, typed per-track device settings) and ADR-029 (`ControlState`
inside `InstrumentNode`).

## Context

An arpeggiator turns the chords a track receives (clip notes, hardware keyboard,
piano-roll audition) into single notes on a tempo grid, **before** the
instrument. It must work for the built-in synth, the sampler and CLAP
instruments, so it can only produce `NoteOn` / `NoteOff`.

The natural place is `InstrumentNode::process`, between `event_buf.sort_by_offset()`
and `device.process_midi`. Two things make that an architectural question:

- Invariant 1 says new DSP is a new `AudioNode`, and `GraphCompiler::validate_port_kinds`
  still rejects `PortKind::Midi` edges (ADR-011, Phase 5c never landed).
- Settings need a home in the project. Invariant 1 also forbids special fields
  on `AppTrack` for DSP.

## Options

### A — Note stage inside `InstrumentNode` (recommended)

A pure `Arpeggiator` struct (own file `audio_graph/arpeggiator.rs`, no graph or
device knowledge: events in → events out) owned by `InstrumentNode`, always
present but dormant. Settings are a typed `AppTrack.arpeggiator` (like
`AppTrack.sampler`, ADR-028 §2) mirrored into lock-free atomics.

- **Cost:** small. One new file, a few lines in `process`, one project field, one
  command pair, one UI block. No compiler/scheduler change (C-7). Existing
  projects have no arpeggiator → the stage never runs → bit-identical (C-2).
- **Price:** a deliberate, scoped stretch of invariant 1 — a MIDI transformer
  lives in an existing node instead of being its own node. Same precedent as
  `ControlState` (ADR-029) and the sampler device (ADR-028). The arpeggiator
  produces no audio and the `Arpeggiator` struct knows nothing about the node, so
  it can be lifted into a real MIDI node later without changing its logic.
- **Settings home, sub-choice.** A1 (recommended): `Option<ArpeggiatorSettings>`
  on `AppTrack`, `serde(default, skip_serializing_if = "Option::is_none")`.
  A2: a generic `note_effects: Vec<NoteEffect>` (enum, one variant today) —
  ready for chord/scale/humanize but needs ids, ordering, add/remove commands for
  one effect. Rejected for V1 (unneeded abstraction); a list can replace the
  single field with a `serde` migration when a second note effect exists. The
  real cost of a note-effect chain is in the engine (option B), not the file
  format.

### B — A real MIDI node with `PortKind::Midi` edges

Split `InstrumentNode` into a MIDI source (clips, audition, live input, controls)
and an instrument node with a MIDI input; add `ArpeggiatorNode` between them; let
the compiler accept `Midi` edges and give the execution plan / scheduler MIDI
buffers.

- **Cost:** large — compiler, execution plan and **scheduler (RT-critical)**
  change; plugin-instance keying (`instrument:{track_id}`), recorder and queue
  wiring move with the split; every instrument track takes the new path, so all
  MIDI golden hashes are at risk (C-2). It is the long-term target (ADR-011/012,
  migration Phase 5c) and would also enable MIDI routing between tracks.
- **Verdict:** right direction (Bitwig-style note-FX chains, MIDI routing), wrong
  size for a single effect. **Trigger:** the second note effect (note echo,
  chord, transpose) — two real users to design the MIDI port against. Then its own
  migration step with its own ADR; `Arpeggiator` moves into a node unchanged.

### C — Decorator device (`ArpeggiatedDevice` wrapping the `InstrumentDevice`)

Wrap the device at compile time; it rewrites `events` inside `process_midi`.

- **Cost:** small, node untouched. But the wrapper exists only if the track has
  settings at compile time (first enable = recompile, cuts tails), is invisible to
  the node's bookkeeping, and splits "what is played" from "what sounds" across two
  places. Harder to test through `InstrumentNode`.
- **Verdict:** same stretch of invariant 1 as A with worse ergonomics.

## Decision (if A is approved)

### 1. Settings

`ArpeggiatorSettings { enabled, mode, rate, triplet, gate_percent, octaves }`:

- `mode`: `up`, `down`, `up_down`, `played`, `random`.
- `rate`: `quarter`, `eighth`, `sixteenth`, `thirty_second`; `triplet` ×2/3.
  A step is `rate` beats (1/4 = 1 beat … 1/32 = 0.125) × 60 / bpm.
- `gate_percent` 5–100 (note length = step × gate); `octaves` 1–4.
- `Default`: disabled, `up`, `sixteenth`, no triplet, 80 %, 1 octave.
- Persisted in `TrackFile` with `#[serde(default, skip_serializing_if)]`:
  older `.echoes` load unchanged (C-4). Values are clamped at load and on command.
- Not in V1: latch, swing, velocity options, chord memory.

### 2. State and hot path

- Command `SET_ARPEGGIATOR { track_id, patch }` → event `ARPEGGIATOR_SET`
  carrying the **whole resulting settings** (replay-safe, like `SAMPLER_PAD_SET`).
  One user action = one undo step. The frontend stays a reactive mirror (ADR-005).
- Settings live in `ArpeggiatorCells` in `LiveParams`: **one atomic per setting**
  (mode, rate, triplet, gate, octaves, enabled), keyed by track id and created at
  compile for every instrument node. One atomic per setting rather than a packed
  word keeps the cells shaped like `ParameterBlock` (ADR-009), so rate and gate
  can become modulation / automation targets later (`InstrumentParam`-style
  target, amending ADR-021 / 025 at that time — not in V1). The command writes
  them; the node reads them once per block, so a knob drag or an undo changes the
  arpeggiator without a recompile. A block may see the old mode with the new rate;
  that is inaudible and harmless.
- A recompile happens only when the instrument node must **appear**: a plugin or
  synth track with no MIDI clips gets a node when it has arpeggiator settings
  (as the sampler does, ADR-028 §7). Tracks without settings keep their graph.

### 3. Position in `process`

```
sources → event_buf → sort_by_offset → Arpeggiator::apply(event_buf → arp_buf) → device
```

- This is the node's single **note-effect slot** (one call, not a chain). When a
  second note effect exists, the slot becomes a loop and option B is revisited.
- Disabled and idle → the device receives `event_buf` untouched (no copy).
- Enabled → `NoteOn` / `NoteOff` are **consumed** (they feed the held-note set)
  and replaced by the arpeggiator's own notes; `Control` events pass through at
  their original offsets. `arp_buf` is a second pre-allocated `MidiEventList`.
- `active_pitch_count`, `preview_count`, `live_input_count` keep counting the
  **played** notes, as today. This is what guarantees a balanced stream into the
  arpeggiator: stop / pause / seek / loop already turn every played note off at
  offset 0, so the held set empties and the arpeggiator releases its own note in
  the same block. No parallel release logic.
- Envelope gate (`envelope_cells`): unchanged — high while any **played** note is
  held, so envelope modulators do not chatter at 1/32. Per-step retrigger is a V2
  question.
- The ● recorder captures from the MIDI input callback, upstream of every queue
  and of this stage, so it records what was **played** (non-destructive) without
  any change.

### 4. Clock — two modes, both integer-anchored

- **Transport playing — grid mode.** Step _k_ starts at the sample of beat
  _k_ × step, computed with the same beats→samples function as clip notes
  (ADR-012), so a step and a note on the same beat land on the same sample. The
  grid is a function of `position_samples` and `bpm` only: no wall clock, and
  offline = live. A chord pressed between two steps waits for the next step
  (≤ one step); a chord starting on a grid line sounds at once.
- **Transport stopped — free mode (live keyboard, audition).** There is no
  position to lock to. The grid is anchored on the sample where the chord starts:
  the first note sounds at that `NoteOn`'s offset, then every step length after
  it, advanced from a sample counter owned by the node (+`block_size` per block).
  Block-size independent; tempo changes apply from the next step.
- **Mode switch** (play ↔ stop, seek, loop wrap — detected like
  `InstrumentNode`'s existing discontinuity): the sounding note is released at
  offset 0 and the grid is re-established in the new mode. The held set is kept.
- Step counter and random generator reset whenever the held set goes from empty
  to non-empty, so a chord always starts its pattern from the same place.

### 5. Patterns

Held notes live in a fixed `[Held; 16]` (pitch, velocity, press order); a 17th
note is ignored. Sequence length is `held × octaves`, indexed arithmetically
(nothing materialised): `up`, `down`; `up_down` bounces without repeating the end
notes (C E G → C E G E C E G …; one note repeats); `played` keeps press order;
`random` uses xorshift32 with a fixed seed, reset as above. Velocity is the
played velocity. Pitches above 127 are skipped.

### 6. Event budget (`MAX_EVENTS_PER_BLOCK` = 256, unchanged)

The arpeggiator emits at most two events per step and never raises the limit
(ADR-029 §5: a larger list would make the stable sort allocate). Worst case in the
harness block sizes (≤ 4096): 1/32 triplet at 300 BPM ≈ 5 steps per block.
Policy if the list is nearly full: **steps are skipped, a due `NoteOff` never is**
(it is written first), so a note can be lost but never stuck. One note sounds at
a time (gate ≤ 100 %), so only one release is ever pending.

## Real-time impact

| Change                  | RT cost                                        | RT risk                                 |
| ----------------------- | ---------------------------------------------- | --------------------------------------- |
| `Arpeggiator::apply`    | ≤ 16-entry sort on change, O(events) per block | none — fixed arrays, no alloc/lock/drop |
| `ArpeggiatorCells` read | one `Relaxed` atomic load per block            | none                                    |
| Second `MidiEventList`  | ~2 KB inside the boxed node                    | none — allocated at compile             |
| Dormant stage           | one branch per block                           | none                                    |

Validated against `rt_audio_rules.md`: no allocation, lock, `drop`, I/O, `unwrap`
that can panic, `HashMap` traversal or Tauri call on the audio thread.

## Compatibility

- **C-2.** No settings ⇒ no stage runs ⇒ all 12 existing golden hashes
  unchanged. New harness project `arpeggiator` (own golden hash, block-size
  independence at 4096 / 512 / 441 / 64 / 37) is added **before** the engine code.
  Known inherited risk: ADR-012's f64 beats→samples truncation across operating
  systems.
- **C-4.** `serde(default)`; event logs without `ARPEGGIATOR_SET` replay to no
  arpeggiator.
- **C-7.** Compiler touched only at the `plays_instrument` rule and the cell
  creation; scheduler untouched.
- **Invariant 5.** Rust owns the settings; the UI sends `SET_ARPEGGIATOR` and
  mirrors `ARPEGGIATOR_SET`.

## UI (placement to confirm before integration)

In `InstrumentPanel.svelte` (left panel of an instrument track; the panel is
always `compact` there), a block `ArpeggiatorPanel.svelte` under the instrument
slot, shown for every instrument kind (synth, sampler, CLAP): a power dot in the
style of the effect bypass, `CustomSelect` pickers for mode and rate, a triplet
button, and `ParamKnob`s for gate and octaves. No badge, no status text, no
placeholder. `cursor: default` on disabled controls; focus ring only in
`:focus-visible`. A mock-up is shown to the user before integration.

## Tests

- Pure `Arpeggiator`: each mode, each rate (+ triplet), gate, 1–4 octaves, chords
  of 1 / 3 / 4 notes, tempo change mid-run, 17th note ignored, budget policy.
- `InstrumentNode`: NoteOff guaranteed on stop, seek, loop wrap, and when the
  arpeggiator is switched off while sounding; disabling mid-chord hands the held
  notes to the instrument, enabling releases them first; controls untouched;
  played-note counters and envelope gate unchanged.
- `render_harness.rs`: project `arpeggiator` as above; existing hashes unchanged.

## Alternatives considered

- **Option B / C** — see above.
- **Arpeggiator in the frontend** — rejected: invariant 5, and the grid would
  depend on UI timing instead of the transport.
- **Raising `MAX_EVENTS_PER_BLOCK`** — rejected (ADR-029 §5).
- **Wall-clock anchoring when stopped** — rejected: non-deterministic (invariant 7).

## Consequences

- Arpeggiator on any instrument track, deterministic, undoable, persisted.
- Mono output (one arp note at a time); a chord of CLAP voices releasing under
  the arpeggiator is the instrument's own release time.
- Live keys while the song plays wait for the next grid step (≤ one step).

## Implementation status (2026-10-13)

Done: `arpeggiator_settings.rs`, `audio_graph/arpeggiator.rs`, `ArpeggiatorCells`
(`live_params.rs`), the note-effect slot in `InstrumentNode`, `SET_ARPEGGIATOR` /
`ARPEGGIATOR_SET`, `AppTrack.arpeggiator`, harness project `arpeggiator`
(golden `0x94de_c710_d2b7_1fe1`; the 12 earlier hashes unchanged), the left-panel
block. Not hand-tested by ear. Shipped refinements to the text above: while the
arpeggiator is disabled the stage also tracks the notes it passed through
(`through` counters), so enabling it releases exactly what the instrument holds
and disabling it hands the held chord back; `Arpeggiator` is only called with
sorted input.

## Future (not decided here) — recommended for V2

1. **Latch** (high value: lets a hardware keyboard player take their hands off).
2. **Retrigger / free-start option** for live keys while playing (removes the
   wait for the grid).
3. **Swing** (cheap: delay odd grid steps).
4. Velocity: fixed or as played; per-step envelope retrigger.
5. Lift `Arpeggiator` into a MIDI node when Phase 5c routing exists (option B).
