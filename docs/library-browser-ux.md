# Library Browser UX — Echoes Studio

> Product contract for the content library (sounds, loops, chord progressions,
> MIDI patterns). Complements [UI_GUIDELINES.md](UI_GUIDELINES.md).
>
> **Status:** Accepted via [ADR-023](active/decisions/023-library-browser-ux.md).
> Phase 0 product calls are fully locked (dock, preview, sources, warp, chords,
> pop-up, context memory, shortcuts, empty project, preview BPM).
> Implementation follows [library-browser-roadmap.md](library-browser-roadmap.md).
> Change only via ADR amendment — no silent drift.

---

## 1. Principle

The library is an **insertion tool**, not a destination page.

| Surface                        | Trigger                          | Role                                          |
| ------------------------------ | -------------------------------- | --------------------------------------------- |
| **Browser panel (right dock)** | `B` / toolbar                    | Explore, search, organise, import             |
| **Pop-up browser**             | `+` on a drop target _(Phase 2)_ | Insert in context without leaving the arrange |

Same search engine, same assets, same filters. Only the **open context** changes
default source + filters + drop behaviour.

**Docking (locked):** library = **right panel**, same interaction model as the
left track inspector (resize, open/close, width persisted). Evolve
`RightPanel.svelte` — do not replace the left inspector.

Aligns with UI principles: zero entry friction, controlled density, progressive
disclosure, absolute consistency (`UI_GUIDELINES.md` §1.2).

**Out of scope:** plugin / VST-CLAP device browsing. Devices stay in the
existing plugin picker (instrument and effect slots); a unified “Devices” source
was evaluated for Phase 4 and rejected on 2026-11-26 (ADR-023, _Devices
decision_).

---

## 2. Asset types

| Asset kind          | Payload               | Preview                                      | Typical drop result                            |
| ------------------- | --------------------- | -------------------------------------------- | ---------------------------------------------- |
| `one_shot`          | Audio file (short)    | Audition one-shot                            | Audio clip on track                            |
| `loop`              | Audio file (loopable) | Audition synced to project BPM when possible | Audio clip (raw length Phase 1; warp Phase 3+) |
| `midi_pattern`      | MIDI notes + length   | Piano-roll miniature / soft synth            | MIDI clip on track                             |
| `chord_progression` | Chord list + rhythm   | Piano-roll / block chords                    | MIDI clip (notes derived from chords)          |
| `preset`            | Instrument id + state | Optional dry audition                        | Set track instrument + load state _(Phase 3+)_ |

Factory Echoes packs and user folders share the same kinds. Tags and folders are
metadata, not separate asset types.

**User tags (Phase 4, 2026-11-26).** The detail strip under the results lists the
tags the user gave to the selected asset (click one to remove it) and a field to
add one (Enter). A tag is lowercase, at most 24 characters, never mandatory. Search
matches the name **or** any tag (catalogue tags such as `drums` included). Tags
are stored by asset id with the favorites (`echoes.library.v1`, `userTags`): local
to the machine, not part of the project, no engine command.

---

## 3. Sources (entry windows)

Sources are **views into the catalogue**, not a second folder tree.

| Source id       | Shows                                                      | Default sort                        |
| --------------- | ---------------------------------------------------------- | ----------------------------------- |
| `recents`       | Last used / previewed / dropped assets                     | Recency                             |
| `favorites`     | User-starred assets                                        | Name                                |
| `audio`         | All indexed audio files (Phase 1 sole content source)      | Name                                |
| `all`           | Every indexed asset _(Phase 2+ when multiple kinds exist)_ | Relevance / name                    |
| `samples`       | `one_shot` _(Phase 2+)_                                    | Name                                |
| `loops`         | `loop` _(Phase 2+)_                                        | BPM proximity to project, then name |
| `chords`        | `chord_progression`                                        | Name / key match                    |
| `midi`          | `midi_pattern`                                             | Name                                |
| `drums`         | Hybrid: drum-tagged one-shots + drum MIDI _(Phase 3)_      | Name                                |
| `collection:*`  | Smart collection: a saved search (text + type) _(Phase 4)_ | Name                                |
| `user_folder:*` | One indexed user folder                                    | Path / name                         |
| `pack:<folder>` | One indexed folder, browsed on its own _(Phase 4)_         | Name                                |

**Phase 1 ships:** `audio` (single file source), plus `recents` and `favorites`.  
No Samples/Loops split until kinds exist (Phase 2+).  
**Phase 2+ may add:** `samples`, `loops` (split from `audio`), user folders.  
**Phase 3 adds:** `chords`, `midi`, `drums` (factory content first; drums = 16 generated factory one-shots + factory drum MIDI).  
**Phase 4 adds:** packs + smart collections.

**Packs (Phase 4, 2026-11-26).** A pack is an indexed folder: each one gets its
own source after the collections, labelled with the folder name (full path in the
tooltip), in the order the folders were added. Sample packs ship as folders, so
this is the « installed pack » without a separate install mechanism. There is
deliberately **no factory pack**: the factory content is already split by type
(Chords / MIDI / Drums) and a pack would only repeat it. When a pack is open,
« Stop indexing this folder » forgets the folder and the sounds only it brought
in (files on disk are untouched; favorites, tags and recents keep pointing at
their path). A folder that is still being scanned stops being scanned. Contexts
that remembered the pack fall back to their factory source.

**Grid results (Phase 4, 2026-11-26).** A button next to the type filter
switches the results between the list (one 24 px row per sound, default) and a
grid of tiles (about 104 px wide, as many columns as fit). The choice is stored
with the favorites (`resultsView`). A tile shows the same things as a row —
format, name, favorite, preview, insert — and keeps every gesture (hover preview,
drag to arrange, double-click, Enter). In the grid, `←` `→` move by one tile and
`↑` `↓` by one row. Both views are windowed (`virtualWindow`), so a large folder
stays cheap in either.

**Smart collections (Phase 4, 2026-11-26).** Next to the search field, a type
filter (all / audio / MIDI / chords) narrows any source. As soon as the search
or the type filter is set, a bar under the sources offers « Save as collection »
(name optional: it defaults to the search text or the type). A collection stores
the search text — matched against the name **or** a tag, like any search — and
the type; it is listed after the built-in sources, searches every content source
(audio, chords, MIDI, drums) and stays live: a sound tagged later joins it. The
search and the type filter still narrow a collection further. « Delete
collection » removes it (the sounds are untouched). Collections are stored with
the favorites (`echoes.library.v1`, `collections`): local to the machine, not
part of the project, no engine command.

Sidebar order Phase 1: Recents → Favorites → **Audio**.  
Later: … → Samples → Loops → Chords → MIDI → Drums → User folders → Packs.

---

## 4. Track model & lane focus

Engine tracks are **unified** (`clips` + `midi_clips` + optional `instrument`).
The UI still needs a drop target notion. Define **lane focus**:

| Lane focus | How it is determined                                                                                          | Accepts                                                    |
| ---------- | ------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| `audio`    | Pointer over audio clip region, or last selected item was an audio clip, or empty track opened from audio `+` | `one_shot`, `loop`                                         |
| `midi`     | Pointer over MIDI clip region, or last selected item was a MIDI clip, or empty track opened from MIDI `+`     | `midi_pattern`, `chord_progression`                        |
| `neutral`  | Empty track, no last selection                                                                                | Both — prefer asset under drag; pop-up defaults to `loops` |

**Track “type” for this UX** = current lane focus, not a stored track kind.

| Track situation      | Default lane focus on `+`                            |
| -------------------- | ---------------------------------------------------- |
| Empty track          | `neutral` → pop-up source `audio`                    |
| Has audio clips only | `audio`                                              |
| Has MIDI clips only  | `midi`                                               |
| Mixed audio + MIDI   | Last selected clip’s kind; if none, region under `+` |
| No tracks in project | Phase 1: hint; Phase 2: create track on Enter/drop   |

---

## 5. Open contexts

| Context id         | UI trigger                | Default source                  | Default filters                          | Primary intent    |
| ------------------ | ------------------------- | ------------------------------- | ---------------------------------------- | ----------------- |
| `panel`            | Right-dock browser        | `recents` (cold start: `audio`) | none                                     | Explore           |
| `track_empty`      | `+` on empty track        | `audio`                         | —                                        | First content     |
| `track_audio_lane` | `+` / drop on audio focus | `audio`                         | audio files only                         | Add audio clip    |
| `track_midi_lane`  | `+` / drop on MIDI focus  | `chords` (fallback `midi`)      | kind ∈ {chord_progression, midi_pattern} | Add MIDI clip     |
| `playhead_insert`  | Insert at playhead        | inherits selected lane          | same as lane                             | Place at playhead |
| `instrument_slot`  | Instrument picker         | _(deferred)_                    | —                                        | Load instrument   |
| `fx_slot`          | Effect `+`                | _(deferred)_                    | —                                        | Load effect       |
| `drum_pad`         | Drum pad `+` _(Phase 3)_  | `drums`                         | drums tag                                | Assign hit        |

**Memory (Phase 4, 2026-11-26).** Opt-in, per context, on the pop-up only (the
dock has no context). When the source shown differs from the one the context
opens on, the pop-up header offers « Use for this context »: next time that
context opens on this source (a built-in source or a collection). « Restore
default » appears once a context has a remembered source and puts the factory
source back. Nothing is learned silently: only that click stores a choice, and
only the source is remembered (the search text is kept across opens as before).
Deleting a collection forgets the contexts that pointed to it; a missing
collection falls back to the factory source. Stored with the favorites
(`echoes.library.v1`, `contextSources`). Factory defaults above always remain
restorable. Never require configuration before use.

---

## 6. Drop rules (authoritative)

### 6.1 Compatibility matrix

| Asset ↓ \ Target →  | Lane `audio`       | Lane `midi`                | Lane `neutral` (empty track) | Timeline (no track)        | FX / instrument slot |
| ------------------- | ------------------ | -------------------------- | ---------------------------- | -------------------------- | -------------------- |
| `one_shot` / `loop` | **Add audio clip** | Reject                     | **Add audio clip**           | Create track + clip _(P2)_ | Reject               |
| `midi_pattern`      | Reject             | **Add MIDI clip**          | **Add MIDI clip**            | Create track + MIDI _(P2)_ | Reject               |
| `chord_progression` | Reject             | **Add MIDI clip** (expand) | Same                         | Same _(P2)_                | Reject               |
| `preset`            | —                  | —                          | Set instrument _(P3+)_       | Create + set _(P3+)_       | Load slot _(P3+)_    |

Reject = `not-allowed` cursor, no drop line (`UI_GUIDELINES.md` §4.2).

### 6.2 Placement

| Rule                     | Behaviour                                         |
| ------------------------ | ------------------------------------------------- |
| Horizontal position      | Snap to grid (same as clip move)                  |
| Vertical target          | Track under pointer; else selected track          |
| Duration (audio)         | File length in project time (Phase 1)             |
| Duration (MIDI / chords) | Pattern length in beats → seconds via project BPM |
| Key adaptation           | Phase 3: transpose when asset declares a key      |
| Undo                     | One undo step wrapping the command group          |

### 6.3 Commands (engine)

UI never mutates engine state directly (ADR-005 / ADR-023).

| Drop outcome                          | Command(s)                                            |
| ------------------------------------- | ----------------------------------------------------- |
| Audio on track                        | `ADD_CLIP`                                            |
| MIDI / chords on track                | `ADD_MIDI_CLIP` (notes expanded before or in gateway) |
| New track + content                   | `ADD_TRACK` then add clip _(Phase 2)_                 |
| MIDI on track with `instrument: null` | Keep built-in PolySynth — no extra command            |

### 6.4 Double-click / Enter

Same as drop onto **selected track** at **playhead**, using current lane focus.
No selection: Phase 1 = no-op + hint; Phase 2 = create track.

---

## 7. Wireframes

### 7.1 Shell — panel closed (default arrange)

```
┌─ Echoes ──────────────────────────────────────────── [▶][■] BPM Key ─┐
│ [B Library]  [+] Track                                    meters     │
├──────────────┬───────────────────────────────────────────────────────┤
│ Track 1      │ ████ audio clip ████                                  │
│  vol pan     │     ░░ midi clip ░░                                   │
│ Track 2      │                                                       │
│              │         ▶ playhead                                    │
├──────────────┴───────────────────────────────────────────────────────┤
│ Piano roll / inspector                                               │
└──────────────────────────────────────────────────────────────────────┘
```

`B` toggles the library panel. Arrange remains the primary composition.

### 7.2 Panel open — idle (Phase 1)

```
┌─ Echoes ─────────────────────────────────────────────────────────────┐
│ [B] …                                                                │
├──────────────────────────────────┬───────────────────────────────────┤
│ LIBRARY                          │ Track 1 │ ████                    │
│ ┌──────────────────────────────┐ │ Track 2 │                         │
│ │ Search_____              [+] │ │                                   │
│ │ Recents  ★ Fav  Audio  Folder│ │                                   │
│ │ kick…                        │◄── selected row (kbd focus)         │
│ │ loop_A                       │ │                                   │
│ │ hat…                         │ │                                   │
│ │ ▁▂▃▅ waveform                │ │                                   │
│ │ file · duration              │ │                                   │
│ └──────────────────────────────┘ │                                   │
└──────────────────────────────────┴───────────────────────────────────┘
```

Widths: right-dock library; persist width via existing `rightPanel` layout
(default ~360px). Sources are a top tab strip under search. Detail strip under
results is always visible when a row is selected. Left inspector unchanged.

### 7.3 Panel — empty index / first run

```
┌─ LIBRARY ─────────────────────┐
│ Search                        │
│                               │
│  No sounds yet                │
│  [Add folder…]  [Import file] │
│                               │
│  Tip: Drop audio on a track   │
│  or press B anytime           │
└───────────────────────────────┘
```

### 7.4 Panel — searching

```
│ Search: ambient pad________ × │
│ Loops · 12 results            │
│  ▸ Soft Pad Loop    90 · Dm   │  ← highlight match
│    Wide Pad         95 · Am   │
│    …                          │
```

Query filters within the **current source**. Switching source keeps the query
(Bitwig-like). Clearing `×` or first `Esc` clears the query only.

### 7.5 Panel — previewing

```
│  ▸ Soft Pad Loop   ▶ preview  │  ← row shows speaker / pulse
│ Detail: waveform scrubber     │
│ [synced to 128 BPM]           │  ← Phase 3+ badge; Phase 1 optional
```

Transport playhead does **not** move. Arrange does not gain a clip until drop
or Enter.

### 7.6 Drag from panel → arrange

```
│ Results          │ Track 1 audio lane
│  Soft Pad  ──drag──●━━━━━━━━ drop line (snap) ━━━
│                  │ Track 2 midi lane
│                  │     (dimmed if asset is audio — reject zone)
```

Valid target: accent drop line. Invalid: `not-allowed`, lane visually rejects.

### 7.7 Pop-up browser (Phase 2) — opened from lane `+`

```
  Track lane (empty / hover)  [+]
                                 │
         ┌───────────────────────▼────────────────────────┐
         │ LIBRARY                         (same as dock) │
         │ [Search…………]  [+]                              │
         │ Recents  Favorites  Audio  Add folder…         │
         │  Soft Pad Loop                                 │
         │  Dusty Kick                                    │
         │  …full scrollable list…                        │
         │ Detail strip                                   │
         └────────────────────────────────────────────────┘
```

Pop-up is the **same browser engine and UI** as the right dock (Bitwig dual
surface). It is large enough to search and preview — not a short-list clone.
Modal to pointer/keyboard for the browser only — transport continues;
clicking arrange outside cancels (closes without insert). Trigger = `+` on the
arrange **lane** (slot), not beside mute/solo.

### 7.8 Reject state (incompatible lane)

```
│ dragging midi_pattern
│   Track audio lane  ✕  not-allowed
│   Track midi lane   ●  drop line OK
```

---

## 8. UI states

### 8.1 Panel state machine

```
                    ┌──────────┐
           B / UI   │  closed  │
          ┌────────►│          │◄──────── Esc (if focus in panel)*
          │         └────┬─────┘
          │              │ B / open
          │              ▼
          │         ┌──────────┐
          │         │  open    │◄───┐
          │         │  idle    │    │ Esc clears search → idle
          │         └────┬─────┘    │
          │     type /   │ select   │
          │     source   ▼          │
          │         ┌──────────┐    │
          │         │ browsing │────┘
          │         │ (+query) │
          │         └────┬─────┘
          │    Space /   │ hover dwell
          │    focus     ▼
          │         ┌──────────┐
          │         │previewing│── Space/Esc/blur/drag start → browsing
          │         └────┬─────┘
          │              │ pointer drag
          │              ▼
          │         ┌──────────┐
          └─────────│ dragging │── drop → command → idle/browsing
                    │ (ghost)  │── cancel Esc → browsing
                    └──────────┘
```

\* `Esc` does not close the docked panel by default (avoid accidental layout
thrash). Panel closes via `B` or chrome toggle. Pop-up: `Esc` always closes.

### 8.2 Focus scopes (for shortcuts)

| Scope             | When                                 | `Space` means                                    |
| ----------------- | ------------------------------------ | ------------------------------------------------ |
| `arrange`         | Default                              | Transport play/pause                             |
| `library_results` | Panel or pop-up results list focused | Preview toggle                                   |
| `library_search`  | Search field focused                 | Type space character                             |
| `popup`           | Pop-up open                          | Same as results/search inside pop-up; Esc closes |

### 8.3 Result row states

`default` → `hovered` → `selected` → `previewing` → `dragging`  
Favorite star toggles independently (`★` / `☆`) without changing selection.

### 8.4 Indexer states (background)

`idle` → `scanning` → `ready` | `error`  
UI: non-blocking progress in panel chrome (“Indexing… 40%”). Assets appear
incrementally. App startup never waits on a full scan (ADR-023).

---

## 9. Shortcuts

| Action                | Shortcut                      | Scope                                     |
| --------------------- | ----------------------------- | ----------------------------------------- |
| Toggle library panel  | `B`                           | Global (unless text field, except search) |
| Focus library search  | `/` or `Ctrl/Cmd+F`           | Panel open → focuses search               |
| Move result selection | `↑` `↓` (`←` `→` in the grid) | `library_results`                         |
| Preview selected      | `Space`                       | `library_results` only                    |
| Insert at playhead    | `Enter`                       | `library_results` / pop-up                |
| Clear search          | `Esc`                         | Search non-empty                          |
| Close pop-up          | `Esc`                         | Pop-up open (search empty or second Esc)  |
| Toggle favorite       | `F` or `Ctrl/Cmd+D`           | `library_results` _(Phase 2)_             |
| Jump sources 1–4      | `Ctrl/Cmd+1…4`                | Panel/pop-up _(Phase 2)_                  |

Transport and edit shortcuts from `UI_GUIDELINES.md` §4.1 remain unchanged
outside `library_*` scopes.

---

## 10. Preview

| Rule           | Phase 1                                             | Later                         |
| -------------- | --------------------------------------------------- | ----------------------------- |
| Implementation | **UI-side** (decode + Web Audio / equivalent)       | Optional engine-faithful path |
| Trigger        | Hover dwell ~300 ms or `Space` in results           | Same                          |
| Stop           | `Space` / Esc / blur / drag start                   | Same                          |
| BPM sync       | None required (play file as-is)                     | Required for loops            |
| Key sync       | —                                                   | MIDI / chords                 |
| Routing        | Must not mutate arrange clips or transport position | Same                          |
| RT             | Never on audio callback alloc/lock/IO path          | Same                          |

Phase 1 preview may differ slightly from arrange playback (sample rate, no
project FX). That is accepted.

---

## 11. Non-goals

- Full-screen Library as primary UX
- Mandatory tagging before use
- Blocking scan on startup
- Devices inside the library (decided no-go, 2026-11-26)
- Required Bitwig-style context configuration before defaults work

---

## 12. Resolved product calls (see ADR-023)

| Topic                | Decision                                                                                                                                                    |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Dock                 | **Right panel** — evolve `RightPanel.svelte`; left = inspector only                                                                                         |
| Preview path         | **UI WebAudio** in Phase 1; engine preview later if needed                                                                                                  |
| Phase 1 sources      | **`audio` + recents + favorites** only (no Samples/Loops split)                                                                                             |
| Warp on loop drop    | **Raw file length** until a dedicated warp ADR (target Phase 3); no UI re-encode hack                                                                       |
| Chord expansion      | **Factory MIDI files first**; canonical rule-based voicing next; no drop-time editor until demanded                                                         |
| Pop-up (Phase 2)     | **Same browser as the dock** (Bitwig-style): full search + sources + scrollable results + preview — opened from a lane `+` slot. Not a truncated mini-list. |
| Devices source       | **No-go** (2026-11-26): slots are not timeline drops, `PluginPicker` already serves them; revisit only for a `preset` asset kind (new ADR)                  |
| Context memory       | **None until Phase 4**, then **opt-in only** (“use current selections…”) — never silent auto-learn                                                          |
| Sampler vs PolySynth | Library does not introduce a sampler; MIDI → PolySynth default                                                                                              |
| Space conflict       | Focus-scope rule (§8.2) — accepted                                                                                                                          |
| `B` vs `L`           | `B` toggles the library panel; `L` is back on the playback loop (shortcuts cleanup, 2026-11-26)                                                             |
| Empty project insert | Phase 1 = hint only; Phase 2 = auto `ADD_TRACK` + clip                                                                                                      |
| Preview BPM sync     | Phase 1 = native file rate; Phase 2 optional UI-only stretch **if** BPM metadata exists                                                                     |

---

## Links

- [library-browser-roadmap.md](library-browser-roadmap.md) — phased delivery
- [ADR-023](active/decisions/023-library-browser-ux.md) — decision record
- [UI_GUIDELINES.md](UI_GUIDELINES.md) — principles, DnD, shortcuts
- [ARCHITECTURE.md](ARCHITECTURE.md) — frontend / engine split
- [ADR-005](active/decisions/005-ui-engine-separation.md) — UI / engine separation
