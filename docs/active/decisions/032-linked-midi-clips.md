# ADR-032 — Shared MIDI content and linked clip repeats (Logic-style aliases)

## Status

**Accepted — 2026-10-26 (approved by the user after one revision the same day).
Implementation in progress, one plan step at a time with approval between steps.
Step 1 (harness `repeated_midi`), step 2 (backend content pool, no sharing yet:
every clip has its own content, all 14 golden hashes unchanged) and step 3
(frontend mirror, 2026-11-03: types, reducer, `midiContentOf`, every reader; no
sharing, no new UI, reducer and selector unit-tested, not hand-tested in the app)
and step 4 (sharing, backend, 2026-11-03: `ADD_MIDI_CLIP` by `content_id`,
`UNLINK_MIDI_CLIP` / `MIDI_CLIP_UNLINKED`, `repeated_midi_linked` renders
bit-identical to `repeated_midi`, no UI can send either yet, not hand-tested)
are done; step 5 (sharing, frontend) is next.** Touches two hard constraints of
`architecture_rules.md`: invariant 5 (Rust owns the state, the frontend mirrors
it) and invariant 6 (event-sourcing scope, ADR-002), plus the project file format
(C-4). Builds on ADR-002, ADR-005, ADR-012 (beat-space MIDI), ADR-018 (clip
window) and ADR-029 (channel controls).

_Revision note._ The first draft recommended "link groups" (a `link_id` on clips
that each keep a copy of the notes). It was chosen mainly because an older build
could still open the file. The user waived compatibility with older builds and
asked for the most logical, efficient model for a professional DAW, which is the
shared content entity. The link-group design is kept below as a rejected
alternative.

## Context

The repeat handle at the top-right corner of a MIDI clip (`MidiClip.svelte` →
`onMidiClipRepeatStart` / `onRepeatMove` / `onRepeatEnd` in `MainPanel.svelte`)
creates **independent copies**: each repeat is an `ADD_MIDI_CLIP` seeded with the
source clip's notes, controls and offset (`midiClipSeed`). Every `AppMidiClip`
owns its `notes` and `channel_controls` (`project.rs`). The wanted behaviour is
Logic Pro's: repeats share one content and change together, with an action to
make one repeat independent. Reaper ("pooled MIDI") and Cubase ("shared copies")
work the same way: **the content is an object, a clip is a window onto it.**

What the code does today (verified, not taken from the docs):

- **Single writers.** Clip content is written only by `ADD_MIDI_CLIP`,
  `SET_CLIP_MIDI_NOTES`, `SET_CLIP_MIDI_CONTROLS` (`engine_gateway/api.rs`) and
  their replay mirrors `apply_midi_clip_added`, `apply_midi_clip_notes_set`,
  `apply_midi_clip_controls_set`. `midi_recording.rs` never edits an existing
  clip: a finished take becomes a normal `ADD_MIDI_CLIP` sent by the frontend
  (`midiRecording.ts`).
- **Single reader on the engine side.** `compiler::bake_track_midi(track, sr)`
  turns `track.midi_clips` into `MidiNoteSnapshot` / `MidiControlClip` per clip
  with the clip's own window (ADR-018). `publish_track_midi(track_id)` rebakes one
  track into the lock-free `MidiTrackPublisher`; a recompile bakes the same way, so
  undo/redo (`finish_history_step` → `recompile_if_needed`) needs no special path.
- **Events are replayed, the file is a snapshot.** Undo/redo rebuilds the project
  from a periodic `AppProject` snapshot plus `apply_*` replays
  (`EventStore::replay_plan`). Saving writes `AppProject::to_file()`; opening
  re-seeds the store (`init_from_project`). The event log is **never written to
  disk**: events can be reshaped without a persisted-event migration.
- **File version is decorative.** `ProjectFile.version` is always written as `1`
  and never read; there is no `deny_unknown_fields` in `src-tauri`.
- **Frontend.** The mirrored clip carries `notes` and `channel_controls`. Content
  is read in `Track.svelte` (arrange preview, pitch range), `PianoRoll.svelte`
  (controls), `pianoRoll/clipEditor.ts` (notes), `midiClipSeed.ts` and
  `reducer.ts`. The piano roll edits a local working copy while dragging and
  commits one `SET_CLIP_MIDI_NOTES` on mouse-up, and re-syncs from engine state
  when not dragging.

## Decision (recommended)

### 1. State model — a content pool; every clip references exactly one content

```text
AppProject.midi_contents : { content_id → MidiContent { notes, channel_controls } }
AppMidiClip              : { id, start, end, offset, content_id }   // no notes, no controls
```

- **One representation, no special case.** Every clip has a `content_id`. A clip
  is "linked" when at least one other clip (any track) references the same
  content. There is no "inline vs pooled" duality and no distinguished "original".
- A content exists **iff** at least one clip references it (see §4). It has no
  name, no UI of its own; it is a model object, not a user-visible one.
- Clip windows stay per clip: `start`, `end`, `offset` (ADR-018) are not shared, so
  repeats keep their own trim (a cut-off last repeat stays cut off).
- Note ids are unique **within a content**; a clone keeps them.
- **Frontend mirror (normalised):** `ProjectState.midi_contents` is mirrored as is;
  `MidiClip` loses `notes` / `channel_controls` and gains `content_id`. A selector
  `midiContentOf(state, clip)` resolves it. TypeScript strictness lists every
  reader that must change. Content is held once in memory and sent once in
  `PROJECT_RESET`.

### 2. Editing — events addressed by content id

- Commands stay addressed by clip, because the editor is open on a clip:
  `SET_CLIP_MIDI_NOTES` / `SET_CLIP_MIDI_CONTROLS` keep `(track_id, clip_id)`. The
  gateway resolves the clip's `content_id`.
- Events are addressed by **content**: `MIDI_CONTENT_NOTES_SET { content_id,
  notes }` and `MIDI_CONTENT_CONTROLS_SET { content_id, channel_controls }`
  **replace** `MIDI_CLIP_NOTES_SET` / `MIDI_CLIP_CONTROLS_SET`. One write, one
  event, one undo step, whatever the number of clips. No fan-out anywhere: the
  Rust apply, the replay and the TypeScript reducer each write one entry.
- **Publication:** the gateway collects the distinct tracks holding a clip that
  references the content and calls `publish_track_midi` for each (up to N tracks);
  if one track has no slot yet, `recompile_if_needed` runs as today.
- **Undo/redo:** replay from the snapshot restores the content and every
  reference together; the existing recompile after undo rebakes all tracks.
- **Optimistic UI:** unchanged in scope (ADR-005). The open piano roll edits its
  local working copy and commits on mouse-up. Other clips on the same content
  update when the event returns (the same moment the edited clip's own preview
  does today); they do not follow a drag live. The piano roll's existing "sync from
  engine when not dragging" now reads the content.

### 3. Creating a link and "Rendre indépendant"

- `ADD_MIDI_CLIP` has two modes: **seed** (notes/controls, as today: a new content
  is created) or **`content_id`** (the clip references an existing content, no
  notes sent). `MIDI_CLIP_ADDED` carries the clip and, in seed mode, the new
  `content`, so replay recreates it. One event, one undo step.
- The repeat handle sends `content_id` of the source clip: no notes cross the IPC
  boundary and there is no "assign an id to the source" special case.
- Paste, Alt+drag duplicate, library drops and recording stay independent (seed
  mode).
- `UNLINK_MIDI_CLIP { track_id, clip_id }` → `MIDI_CLIP_UNLINKED { track_id,
  clip_id, content_id (new), content }`: clones the clip's content into a **new
  content** and repoints the clip. The new id and the cloned content are in the
  event payload (ids are not regenerated on replay). Undoable. Offered in the
  existing clip context menu, shown only on linked clips. If the clip was the only
  reference there is nothing to do and no event.

### 4. Deletion and lifetime

A content lives while a clip references it. `MIDI_CLIP_REMOVED`, track deletion
and `MIDI_CLIP_UNLINKED` end with `prune_unreferenced_midi_contents()` in the same
apply, so the pool never holds orphans. There is no "original": deleting the clip
the repeats were made from leaves the others untouched. Pruning is safe for undo
because undo rebuilds from a snapshot and replays events: the deleted clip and its
content come back through replay of the earlier `MIDI_CLIP_ADDED`.

### 5. File format and compatibility (C-4)

- New: `ProjectFile.midi_contents: Vec<MidiContentFile>` (`serde(default)`);
  `MidiClipFile.content_id: Option<String>`. New saves write `content_id` and no
  inline notes.
- **Reading old projects, forever:** a clip with inline `notes` /
  `channel_controls` and no `content_id` gets a content whose id **is the clip id**
  (deterministic, unique) holding its inline data. Old projects load and render
  identically.
- Reconciliation on load: a `content_id` missing from the pool gets an empty
  content (logged; the clip stays); contents no clip references are dropped.
- **`ProjectFile.version` becomes real:** written as `2`, and load **refuses** a
  version greater than the build knows ("project created by a newer version")
  instead of silently dropping fields. Older builds do not check this and will lose
  the notes of a v2 project; that cost was accepted by the user.
- Ordering: format and load migration (step 2 of the plan) ship **before** any
  sharing exists, so every clip first gets its own content and all behaviour and
  hashes are unchanged.

### 6. Audio engine and C-2

The engine still receives **resolved content**: `bake_track_midi(track, &project
contents, sr)` looks up each clip's content and emits the same
`MidiNoteSnapshot` / `MidiControlClip` as today. `MidiTrackContent`,
`MidiTrackPublisher`, the compiler's node construction, snapshots and the audio
thread are untouched. `rt_audio_rules.md` is not affected: the new work (content
lookup, publishing to up to N tracks) is on the command thread.

C-2 gate, in order:

1. **Before any product change**, add harness project `repeated_midi` (the
   reference MIDI clip repeated three times as independent copies, with a
   cut-off last repeat) with a golden hash and a block-size-independence test.
2. The pool refactor (plan step 2) must leave **every existing hash and
   `repeated_midi` unchanged**. This is the real regression gate of the change:
   every MIDI project now goes through content resolution.
3. A `repeated_midi_linked` variant (three clips, one content) must have the
   **same hash** as `repeated_midi`.

Control events remain unreachable from the harness (the built-in synth ignores
them, ADR-029); the control path is covered by unit tests on `bake_track_midi`.

### 7. Recording and arpeggiator

- **Recording** always produces a new clip with a **new** content (seed mode); it
  never writes into a shared content, whichever clip it overlaps in time. No change
  in `midi_recording.rs`.
- **Arpeggiator** (ADR-030) is a note stage in `InstrumentNode`; it reads no clip
  data and the recorder captures the played notes before it. No interaction.
- **Piano roll / control lanes** edit through the same two commands, so opening a
  linked clip edits every repeat. No new widget.

### 8. UI (constraints from the request)

No new widget. `MidiClip.svelte` gets one small marker on linked clips (prop
`linked`, computed in `MainPanel` from the mirrored state, reference count ≥ 2,
across tracks). One conditional "Rendre indépendant" entry in the existing clip
context menu. Nothing else without approval.

## Alternatives considered

- **Link groups: a `link_id` on clips that each keep a copy of the notes.** (The
  first draft.) Smaller diff and a graceful downgrade for old builds, but:
  - "identical content" is enforced by convention (one writer helper, a load
    check, tests), not by construction; any future feature writing `clip.notes`
    directly (quantize all, per-clip transpose, MIDI effects) desyncs silently;
  - N copies stored, sent in every `PROJECT_RESET` and `MIDI_CLIP_MOVED_TO_TRACK`,
    and written N times per edit;
  - needs a fan-out in three places (Rust apply, replay, reducer) and a special
    "give the source an id" step when linking;
  - it is a stepping stone to the pool anyway. Its only real advantage, opening a
    linked project in an older build, was waived.
- **Source clip + alias clips pointing to it.** Rejected: deleting or moving the
  source needs a promotion rule or orphans the aliases (the Logic annoyance), and
  the engine still has to resolve aliases. The pool is the same idea with a
  neutral owner.
- **Optional `content_id` (clips without it keep inline notes).** Rejected: two
  representations in every reader (bake, DTO, reducer, piano roll, tests) forever.
  Legacy files are migrated once, at load.
- **Edit events addressed by clip with fan-out.** Rejected: with a pool the event
  names what changed (the content).
- **Making copy/paste and Alt+drag linked too.** Out of scope, not requested;
  Logic keeps those independent.
- **Sharing the baked `MidiNoteSnapshot` between clips** (one `Arc` per content).
  Not now: the snapshot embeds the per-clip window, so it would need a split; the
  bake is cheap and off the audio thread.

## Consequences

- **Positive.** Single source of truth by construction; one write and one event
  per edit; smaller IPC (a repeat or a move sends no notes); the model that future
  features (content library, named patterns, per-content tools) need; deletion is
  trivially safe.
- **Risks.**
  - **Bigger refactor.** About a dozen Rust sites build an `AppMidiClip` (3 in
    `render_harness.rs`, `compiler.rs` and `graph.rs` tests, `host.rs` test, 2 in
    `api.rs`, 3 in `project.rs`) and about six frontend readers change. Mitigation:
    steps 1 and 2 below are a pure refactor gated by unchanged hashes.
  - Replay must create, edit and prune contents exactly as the live path does;
    covered by replay-equals-live tests.
  - A content shared across tracks republishes several instruments on one edit.
  - Repeat drags send N parallel `ADD_MIDI_CLIP`, N undo steps, as today.
  - Pruning relies on undo rebuilding from a snapshot; true for the current
    `EventStore`, to be re-checked if undo ever becomes inverse-based.
- **Not hand-tested** (nothing is implemented).

## Implementation plan (one step at a time, backend first, approval between steps)

1. **Harness first** (test code only): `repeated_midi`, golden hash,
   block-size independence. Gate: existing hashes unchanged.
2. **Pool, no sharing yet** (pure refactor): `midi_contents`, `content_id`,
   `bake_track_midi` resolving, `ADD_MIDI_CLIP` seed mode creating a content,
   `SET_CLIP_MIDI_*` writing the content, `MIDI_CONTENT_*_SET` events and replay,
   pruning, file format v2 with legacy load migration. Every clip has its own
   content, so behaviour is unchanged. Gate: all hashes identical, replay-equals-
   live, legacy and v2 round-trip tests.
3. **Frontend mirror** of step 2 (done 2026-11-03): types, reducer, `midiContentOf`,
   `Track.svelte`, `PianoRoll`, `clipEditor`, `midiClipSeed`, `MainPanel`, with
   `reducer.test.ts` cases. App still behaves as before.
4. **Sharing, backend** (done 2026-11-03): `ADD_MIDI_CLIP` by `content_id`, multi-track publish,
   `UNLINK_MIDI_CLIP` / `MIDI_CLIP_UNLINKED`; `repeated_midi_linked` hash equality;
   tests for edit-through-one-clip, cross-track, delete-the-first, undo/redo.
5. **Sharing, frontend:** repeat handle sends `content_id`, `linked` marker,
   "Rendre indépendant" in the clip context menu.
6. **Docs sync** (`doc_sync_rules.md`): `state-management.md`, `audio-graph.md`,
   `state_index.md`, this ADR's status, `change_impact_mapping.md`, a note in
   `engine_gateway/CLAUDE.md` ("content is written only through the pool").

## Open questions

- Should Alt+drag duplicate (or a modifier on it) also create linked clips?
- May one content be shared by tracks with different instruments? (Proposed: yes,
  like Logic.)
- Multi-selection: "Rendre indépendant" on several clips at once?
- Refusing a newer `version` on load: acceptable message wording and behaviour
  (modal vs status bar)?

## Documentation divergences found while reading

- `state-management.md` lists `MIDI_CLIP_ADDED` and `MIDI_CLIP_CONTROLS_SET` but no
  row for `MIDI_CLIP_NOTES_SET`, `MIDI_CLIP_REMOVED`, `MIDI_CLIP_MOVED` or
  `MIDI_CLIP_MOVED_TO_TRACK`, which exist in the code (`events.rs`, `api.rs`, the
  reducer). To be fixed at step 6.
- The project file "version" does not work as a schema version today (see
  Context); this ADR makes it real.
