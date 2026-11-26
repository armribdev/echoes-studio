# State Management

## Two Distinct State Domains

Echoes Studio has two fundamentally different kinds of state that must not be mixed:

| Domain            | Type                                              | Owner                   | Persistence             |
| ----------------- | ------------------------------------------------- | ----------------------- | ----------------------- |
| **Project state** | Tracks, clips, tempo, routing, automation         | Rust `EventStore`       | Saved to `.echoes` file |
| **Runtime state** | Transport position, meter levels, playback cursor | Atomics / Svelte stores | Ephemeral               |

Mixing these is one of the most common architectural mistakes. See
`anti-patterns.md` for concrete examples.

---

## Event Sourcing — Scope and Limits

Event sourcing is applied **only to project structure**. The event log records
what the user _decided_ about their project, not what the audio engine is doing.

### What IS event-sourced

| Event                                                                                                                       | Why                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| --------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `TRACK_CREATED` / `TRACK_DELETED`                                                                                           | Structural, undoable. `TRACK_CREATED` may carry the track's `instrument`, so "new track + pick instrument" is one event (absent = built-in PolySynth, as in older projects); when it carries the sampler, the stored event also carries the starting kit (see `TRACK_INSTRUMENT_SET`)                                                                                                                                                                                                                                     |
| `CLIP_ADDED` / `CLIP_MOVED` / `CLIP_REMOVED`                                                                                | Structural, undoable (`CLIP_ADDED` and `CLIP_MOVED` carry the trim `offset` — ADR-018)                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `MIDI_CLIP_ADDED`                                                                                                           | Structural, undoable. Carries the clip (`content_id`, left-trim `offset`) and the new `content` (notes + `channel_controls`, ADR-029), so pasting or recording a MIDI clip is one event and replay recreates the content. `ADD_MIDI_CLIP` by `content_id` (repeat) sends no `content`: the clip plays an existing one, and the command fails if the content is unknown or if a seed comes with it (ADR-032)                                                                                                               |
| `MIDI_CONTENT_CONTROLS_SET`                                                                                                 | Content, undoable. Replaces a content's `channel_controls` (pitch bend, CC, aftertouch in beats, ADR-029), addressed by `content_id`; the command stays addressed by clip (`SET_CLIP_MIDI_CONTROLS`) and the gateway resolves the content. Published into the live instrument of every track whose clips play it, without a recompile, like `MIDI_CONTENT_NOTES_SET`                                                                                                                                                      |
| `MIDI_CONTENT_NOTES_SET`                                                                                                    | Content, undoable. Replaces a content's notes, addressed by `content_id` (`SET_CLIP_MIDI_NOTES` is addressed by clip); published without a recompile (ADR-032, replaces `MIDI_CLIP_NOTES_SET`)                                                                                                                                                                                                                                                                                                                            |
| `MIDI_CLIP_UNLINKED`                                                                                                        | Structural, undoable, no recompile (the copy equals what was played). `UNLINK_MIDI_CLIP { track_id, clip_id }` clones the clip's shared content into a new content; the event carries the new `content_id` and the cloned `content`, so replay does not regenerate ids. A clip nobody shares produces no event (ADR-032)                                                                                                                                                                                                  |
| `MIDI_CLIP_REMOVED` / `MIDI_CLIP_MOVED` / `MIDI_CLIP_MOVED_TO_TRACK`                                                        | Structural, undoable. A clip is a window (`start`, `end`, `offset`) onto a content and keeps its `content_id` when moved; removing a clip (or deleting its track) drops every content no clip references any more (ADR-032)                                                                                                                                                                                                                                                                                               |
| `TRACK_UPDATED` (name, volume, pan)                                                                                         | Structural, undoable                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `TRACK_MUTED` / `TRACK_SOLOED`                                                                                              | Structural, undoable                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `TRACK_INSTRUMENT_SET`                                                                                                      | Structural, undoable, recompiles. Sets `AppTrack.instrument` (plugin id, `builtin:sampler`, or `null` = PolySynth). Choosing the sampler on a track without a kit gives it the factory kit (16 generated sounds, hi-hats in one choke group) in the same event, and the stored event carries that kit (`sampler`) so replay restores it — older events without it replay to an empty 16-pad kit; the kit is kept when another instrument is chosen. The engine payload also carries the resulting kit for the UI. ADR-028 |
| `SAMPLER_PAD_SET`                                                                                                           | Structural, undoable, recompiles. Carries the whole resulting pad (`SET_SAMPLER_PAD` patches one pad); replay re-resolves the sample file through the shared audio path cache, and a missing file leaves the pad silent with `missing: true`. ADR-028                                                                                                                                                                                                                                                                     |
| `NOTE_EFFECTS_SET`                                                                                                          | Content, undoable. Carries the whole resulting `NoteEffectSettings` (`SET_NOTE_EFFECTS` patches them field by field, ADR-038). Same contract as `ARPEGGIATOR_SET`: no recompile once the track has an instrument node (the command writes the track's `NoteEffectCells`), every compile rewrites the cells from the project. `AppTrack.note_effects` is `serde(default)`.                                                                                                                                                 |
| `ARPEGGIATOR_SET`                                                                                                           | Content, undoable. Carries the whole resulting `ArpeggiatorSettings` (`SET_ARPEGGIATOR` patches them, ADR-030). No recompile once the track has an instrument node: the command writes the track's `ArpeggiatorCells` atomics, and every compile rewrites them from the project so undo / redo reach the audio thread. The first settings of a track with no MIDI clip recompile so its node exists. `AppTrack.arpeggiator` is `serde(default)`.                                                                          |
| `TRACK_INPUT_SET`                                                                                                           | Structural, undoable, recompiles. Sets `AppTrack.input_source` — an aux output of a multi-out plugin fed into the track (`null` clears it; absent in older projects = none). ADR-014                                                                                                                                                                                                                                                                                                                                      |
| `TEMPO_UPDATED`                                                                                                             | Structural, affects rendering                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `PROJECT_NAME_UPDATED`                                                                                                      | Structural                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `MARKER_ADDED` / `MARKER_UPDATED` / `MARKER_REMOVED`                                                                        | Structural, undoable, no recompile. Locators in beats on the arrangement timeline                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `REGION_ADDED` / `REGION_UPDATED` / `REGION_REMOVED`                                                                        | Structural, undoable, no recompile. Named arrangement spans in beats; may drive the ephemeral `SET_LOOP` brace from the UI                                                                                                                                                                                                                                                                                                                                                                                                |
| `AUTOMATION_LANE_ADDED` / `AUTOMATION_LANE_REMOVED` / `AUTOMATION_LANE_UPDATED` (interpolation) / `AUTOMATION_CLIP_UPDATED` | Structural, undoable, recompile (ADR-021). `AUTOMATION_CLIP_UPDATED` replaces the clip with the same id or adds it. Mirrored in the frontend reducer (`TrackState.automation_lanes`) since 2026-09-29; no editing UI yet                                                                                                                                                                                                                                                                                                  |
| FX routing (future)                                                                                                         | Structural, undoable                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |

### What is NOT event-sourced

| State                            | Why NOT                                                         |
| -------------------------------- | --------------------------------------------------------------- |
| Transport position               | 60fps real-time, never undone                                   |
| Playback loop brace (`SET_LOOP`) | Session runtime; arrangement regions may _drive_ it from the UI |
| Audio meter levels               | Real-time feedback, ephemeral                                   |
| UI selection / panels            | Ephemeral UI-only state                                         |
| Drag gesture state               | Temporary, resolved on drop                                     |
| Window scroll / zoom             | UI preference, not project data                                 |

Attempting to event-source real-time state creates an unbounded event log,
breaks the undo/redo model, and adds synchronization overhead to the hot path.

See `decisions/002-event-sourcing-scope.md` for the rationale.

---

## Rust EventStore

```rust
pub struct EventStore {
    events: Vec<ProjectEvent>,           // immutable append-only log
    head: usize,                         // current position
    snapshot: Option<(usize, AppProject)>, // periodic checkpoint
}
```

`head` is a pointer into the events array. Undo moves it back; redo moves it
forward. New events discard everything after `head` (redo-future).

**Snapshot optimization:** Rather than replaying from event 0 every time,
the store periodically saves a full `AppProject` snapshot. On undo/redo, it
replays from the nearest snapshot before `head`.

`AppProject` is a cache — it can be reconstructed from the event log at any
time. The event log is the source of truth.

---

## Frontend Event Store

The frontend mirrors the Rust event log locally:

```typescript
ProjectEventStore {
    _events: EngineEvent[]    // session log
    _state: ProjectState      // derived cache (from reducer)
    _subscribers: Set         // Svelte store contract
}
```

On startup, it seeds from `get_project` (full DTO). It then listens to the
`engine-event` Tauri channel and applies each event via `applyEvent`.

On `PROJECT_RESET` (undo/redo), it receives a full project snapshot from the
Rust side and resets the derived cache wholesale.

---

## Reducer Philosophy

`applyEvent(state, event) → ProjectState` is a pure function:

```typescript
// Pure: no side effects, no mutation, no external calls
export function applyEvent(state: ProjectState, event: EngineEvent): ProjectState {
    switch (event.type) {
        case ev.TRACK_CREATED:  return { ...state, tracks: [...state.tracks, newTrack] };
        case ev.TRACK_DELETED:  return { ...state, tracks: state.tracks.filter(...) };
        // ...
        default:                return state;  // forward-compat: unknown events are ignored
    }
}
```

**Rules:**

- Never mutate `state`. Always return a new object or structurally-shared copy.
- Unknown event types must be silently ignored (forward compatibility with future events).
- The reducer has no knowledge of the UI, the audio engine, or Tauri IPC.

---

## Undo / Redo Model

```
events: [A, B, C, D, E]
                  ↑ head = 3  → current state is A+B+C

undo() → head = 2 → current state is A+B
redo() → head = 3 → current state is A+B+C
new F  → head = 3, future [D, E] discarded → events: [A, B, C, F]
```

The audio graph is recompiled after every undo/redo because the `AppProject`
changes. The frontend receives a `PROJECT_RESET` event with the new full
project state and resets its derived cache.

**Audio data is preserved across undo/redo.** `AudioDataCache` retains loaded
WAV samples indexed by clip ID. A `CLIP_ADDED` event replay does not re-read
from disk.

---

## Derived UI State

UI-only state (selection, hover, scroll, open panels) lives in Svelte stores
in `src/stores/`. These are independent of the `ProjectEventStore` and are
never persisted to the event log.

```
engineStore.ts  ← derived from ProjectEventStore (project truth)
project.ts      ← selected track / clip (UI only)
selection.ts    ← multi-select state (UI only)
transport.ts    ← play/pause/position (real-time, not event-sourced)
preferences.ts  ← user preferences (persisted separately, not events)
```

**Rule:** Svelte stores derived from `engineProject` are read-only computed
views. They must never be written to directly from UI components — mutations
go through `dispatcher.ts` only.

---

## Command Flow Summary

```
User action (e.g. drag clip)
  ─► dispatcher.ts: moveClip(trackId, clipId, newStart, newEnd)
  ─► invoke('send_command', CommandEnvelope{ type: MOVE_CLIP, payload: ... })
  ─► engine_gateway/api.rs: validate → mutate AppProject → append ProjectEvent
  ─► recompile audio graph
  ─► app.emit('engine-event', EngineEvent{ type: CLIP_MOVED, payload: ... })
  ─► eventStore.ts: receive → applyEvent → update _state
  ─► Svelte store subscribers notified → UI re-renders
```

The round-trip is always Rust → frontend, never frontend → Rust for state.

**Unsaved changes.** `AppState.dirty` (drives the Save menu item) is set by
every command, and — since edits inside a plugin window never become commands —
by the plugins' own edit reports (`main.rs::watch_plugin_edits`, see
`plugins/plugin-wrapper-system.md`). Load, new and save clear it (`mark_saved`).
Plugin edits are not events: they are not undoable (ADR-002 scope).

**Plugin state beside the project (ADR-017, ADR-035).** `AppProject.plugin_states` (the opaque blob per placement) and `AppProject.plugin_presets` (the CLAP preset a placement was last known to use, `instance_key` → plugin type, location kind, location, load key) are harvested from the live `PluginHost` at save and handed back to it at load; neither is an event, neither is in undo snapshots (`snapshot_project` leaves them empty). `plugin_presets` is an optional field of the project file (absent when empty, so format 2 is unchanged); it is a display hint compared with the plugin's listing, never used to load anything.

**MIDI content (ADR-032).** `AppProject.midi_contents` holds the notes and controls (`MidiContent`); an `AppMidiClip` only carries `content_id`. The project file is format 2 (`PROJECT_FILE_VERSION`): contents are written once in `midi_contents`, clips name them, and a format-1 clip (inline notes) is read into a content of its own named after the clip. A newer format is refused on load instead of silently losing fields; the Open menu shows any failed open in a native error dialog (`show_open_error` in `main.rs`) and keeps the current project. The event log is never written to disk, so events are free to change shape.

The frontend mirrors the pool as is (step 3, 2026-11-03): `ProjectState.midi_contents` is seeded from `ProjectDto.midi_contents`, `MidiClip` carries only `content_id`, and readers resolve a clip through `midiContentOf(state, clip)` (`src/frontend/selectors/`; an empty content when the pool has no entry). The reducer applies `MIDI_CLIP_ADDED` (clip + `content`), `MIDI_CONTENT_NOTES_SET` / `MIDI_CONTENT_CONTROLS_SET` (ignored for an unknown content, like `apply_*`) and prunes unreferenced contents on `MIDI_CLIP_REMOVED` and `TRACK_DELETED`; `MIDI_CLIP_MOVED_TO_TRACK` moves the clip only. The commands are unchanged, still addressed by `(track_id, clip_id)`. Sharing (step 5, 2026-11-03): the reducer applies `MIDI_CLIP_UNLINKED` like `assign_midi_clip_content` (repoint the clip, insert the content, prune; an unknown clip leaves the state unchanged and inserts nothing), and `MIDI_CLIP_ADDED` without `content` adds a clip on an existing content. `linkedMidiClipIds(state)` (`src/frontend/selectors/`) lists the clips whose content is referenced at least twice, across tracks; `MainPanel` derives the `linked` prop of `MidiClip` from it. The repeat handle sends `ADD_MIDI_CLIP` with the source clip's `content_id`; Alt+Shift+drag also sends `content_id` (a linked copy); Alt+drag and Alt+Shift+drag create the clip on drop, in one command, so one undo removes it; paste, plain Alt+drag, library drops and recording still send a seed. "Rendre indépendant" (`unlinkMidiClip` → `UNLINK_MIDI_CLIP`) is offered in `ClipContextMenu` when at least one selected clip is linked, and sends one command per selected linked clip, in order.

---

## Further Reading

- `../../ARCHITECTURE.md` — data pipeline diagram, key rules, undo/redo sequence
- `../snapshot-rebuild-flow.md` — full command → compile → swap sequence
- `decisions/002-event-sourcing-scope.md` — why event sourcing is bounded
- `decisions/005-ui-engine-separation.md` — why UI and DSP share no state
- `decisions/018-clip-source-offset-trim.md` — the clip `offset` (trim) field
