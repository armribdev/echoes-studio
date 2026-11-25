# Library Browser — Roadmap

> Delivery plan for the Echoes content library.  
> Contract: [`library-browser-ux.md`](library-browser-ux.md)  
> Decision: [ADR-023](active/decisions/023-library-browser-ux.md)

Status legend: `todo` · `doing` · `done` · `blocked`

---

## Overview

```
Phase 0 ──► Phase 1 ──► Phase 2 ──► Phase 3 ──► Phase 4
 docs       audio MVP    pop-up      musical     scale
 accepted   panel+drop   context     chords/MIDI packs+memory
```

**North-star metric (all phases):** from app open with an indexed folder, place
an audible loop on the timeline in **&lt; 10 seconds** (UI guideline: zero entry
friction).

---

## Phase 0 — Freeze contract

**Status:** `done` (2026-09-28) — including deferred product-call freeze

| Deliverable                               | Done when                          |
| ----------------------------------------- | ---------------------------------- |
| UX contract + wireframes + states         | `library-browser-ux.md`            |
| ADR-023 accepted + Phase 1/deferred locks | indexed in `000-index.md`          |
| This roadmap                              | linked from UX doc + UI guidelines |

**Exit:** Product agrees §§ asset types, contexts, drop matrix, Space rule, Phase 1 dock/preview/sources, and deferred calls in UX §12. **Met.**

---

## Phase 1 — Audio MVP (panel)

**Status:** `done` (2026-09-30) — code-verified exit; product closed  
**Goal:** Docked library; find and drop audio; preview without polluting arrange.

### Scope

| In                                                            | Out                                    |
| ------------------------------------------------------------- | -------------------------------------- |
| **Right-dock** library (evolve `RightPanel`)                  | Pop-up browser                         |
| Sources: `recents`, `favorites`, **`audio`** (single)         | Samples/Loops split, chords/MIDI/packs |
| Async index of ≥1 user folder                                 | Factory packs, cloud                   |
| **UI WebAudio** preview (hover / Space)                       | Engine preview, BPM/key sync           |
| Drag → `ADD_CLIP` on track                                    | MIDI drops, create-track-on-empty      |
| Shortcuts: `B` (+ keep `L` alias), `/`, ↑↓, Space, Enter, Esc | `Ctrl+1…4`, context memory             |

### Engineering slices (suggested order)

1. **Library service (TS)** — asset model, in-memory index, search stub
2. **Indexer (Rust or Node via Tauri)** — scan folder, emit paths + basic metadata
3. **RightPanel library UI** — wireframe §7.2 / empty §7.3 (replace stub list)
4. **UI preview player (WebAudio)** — isolated; stop on drag/Esc
5. **DnD → `ADD_CLIP`** — snap; reuse/extend existing OS-drop path
6. **Recents / favorites** — local persistence (not project undo)
7. **Shortcuts + focus scopes** — `B`/`L`, Space rule from ADR-023

### Progress (2026-09-29, from commit history — slices not individually re-verified)

Landed: right-dock panel and source management, drag-and-drop to arrange with
keyboard shortcuts, UI preview player, plus the audio import pipeline it relies
on (progressive peaks/PCM, cancellation, focus of imported clips — ADR-024).
2026-09-29 — scan-scalability pass (code-verified, not yet hand-tested on a
large folder): the indexer emits batches of 1000 (was 64), the store copies its
index once per batch instead of once per entry, and the results list is
windowed (fixed 24 px rows, ~viewport + overscan in the DOM) so a 50 000-file
folder no longer mounts 50 000 rows. Keyboard navigation scrolls the selection
into view.

Also 2026-09-29: a finished folder scan now drops the assets it no longer
found under that folder (files deleted or moved). A failed scan — e.g. an
unplugged drive — keeps the existing entries. Scans are tracked per folder, so
several indexed folders now all load at startup (previously a newer scan made
the store ignore the batches of the one before).

2026-09-30 — exit closed (code-verified): shortcut `B`/`L` → dock → search →
drag → `ADD_CLIP`; WebAudio preview isolated from arrange; lane focus reject
via content-derived focus (Phase 3); indexer on a dedicated thread with batched
store updates + windowed results; undo is a single `ADD_CLIP` on an existing
track. The &lt; 10 s cold path remains a soft product metric (path wired; not
bench-timed on a large folder in CI).

### Exit criteria

- [x] Cold open → `B` → search “loop” → drag to track → hear on play, **&lt; 10 s** _(path code-verified; soft timing metric)_
- [x] Preview does not create clips or move playhead
- [x] Drop on MIDI-focused lane rejects audio-incompatible… _(content-derived lane focus, Phase 3)_
- [x] Full folder scan never blocks first paint / transport _(dedicated thread + batches + windowed list)_
- [x] Undo reverts the `ADD_CLIP` only _(existing track; empty-project create-track is a separate undo step)_

### Risks

| Risk                           | Mitigation                          |
| ------------------------------ | ----------------------------------- |
| Large folders freeze UI        | Incremental index + debounce search |
| Space steals transport         | Focus-scope tests                   |
| Duplicate of OS file drop path | Reuse import/`ADD_CLIP` path        |

---

## Phase 2 — Contextual pop-up

**Status:** `done` (2026-09-30)  
**Depends on:** Phase 1 exit

### Scope

| In                                                                  | Out                                      |
| ------------------------------------------------------------------- | ---------------------------------------- |
| Pop-up on lane `+` — **same browser as dock** (Bitwig dual surface) | Truncated mini-list clone                |
| Contexts: `track_empty`, `track_audio_lane`, `playhead_insert`      | `drum_pad`, context memory               |
| Create track + clip when dropping on empty timeline                 | Warp algorithm                           |
| Optional UI-only preview BPM stretch if metadata present            | Engine warp                              |
| Full reject matrix for audio vs MIDI lanes (UI lanes ready)         | Chord factory packs                      |
| Favorite shortcut; optional Quick Sources                           | Smart collections / silent context learn |

### Progress (2026-09-30)

Reworked toward Bitwig dual-surface: shared `LibraryBrowser` powers the right
dock and a large floating pop-up (~440×560). Trigger is arrange **lane** `+`
(visible on empty / hover) — not mute/solo strip. Context menu and empty-project
CTA still open the same pop-up. Esc / click-outside dismiss; Enter inserts at
playhead; empty project creates a track on insert. Preview BPM stretch still
deferred; audio↔MIDI reject landed with Phase 3 lane focus.

### Exit criteria

- [x] Insert a loop via pop-up **without** opening the docked panel
- [x] `Esc` closes pop-up; arrange click-outside cancels
- [x] Empty project: drop/Enter can create track + clip
- [x] Defaults match UX §5 without user configuration
- [x] Lane `+` / dock–popup parity / Esc–outside _(code-verified 2026-09-30)_

---

## Phase 3 — Musical content

**Status:** `done` (2026-09-30) — hybrid drums one-shots delivered 2026-10-13  
**Depends on:** Phase 2

### Scope

| In                                                                        | Out                                     |
| ------------------------------------------------------------------------- | --------------------------------------- |
| Sources `chords`, `midi`, `drums`                                         | Full Devices browser                    |
| `ADD_MIDI_CLIP` from **factory MIDI** first, then rule-based voicing      | Drop-time chord editor; sampler product |
| Lane focus `midi` + reject audio↔midi                                     | —                                       |
| Preview piano-roll peek; key transpose toward project key                 | Perfect voice-leading                   |
| Loop BPM sync in preview when metadata allows                             | —                                       |
| **Warp-on-drop** only behind a **dedicated ADR** (no silent UI re-encode) | Shipping warp without ADR               |

### Progress (2026-09-30)

Factory catalogue (chord progressions + MIDI patterns + drum MIDI), sources
`chords` / `midi` / `drums` in the shared browser, insert via `ADD_MIDI_CLIP`
(block-chord expand for progressions), WebAudio MIDI preview with project-key
transpose (preview only), lane focus for pop-up defaults + audio↔MIDI reject on
drag/insert. Warp deferred — detail tip “longueur d'origine”. Drums source: 16
factory one-shots generated in code (ADR-028 amendment, 2026-10-13; preview,
audio-lane drop and pad drop work, not yet hand-tested by ear) listed before the
factory drum MIDI. Piano-roll miniature peek deferred (audition via
oscillators).

### Exit criteria

- [x] Drop a chord progression on MIDI lane → notes audible via PolySynth _(code-verified)_
- [x] Project key change + re-preview reflects transpose (preview only) _(code-verified)_
- [x] Hybrid `drums` source returns both one-shots and drum MIDI _(2026-10-13: 16 generated factory one-shots + factory drum MIDI)_
- [x] Warp-on-drop explicitly deferred with UX copy (“placed at original length”)

### Follow-up ADR likely

Warp / tempo-stretch on loop drop (engine + determinism impact) — **not**
silently folded into Phase 3 without its own ADR if it touches the audio graph
or offline render.

---

## Phase 4 — Scale & power-user

**Status:** `todo`  
**Depends on:** Phase 3

| In                                                                      | Out                                  |
| ----------------------------------------------------------------------- | ------------------------------------ |
| Factory / installed **packs**                                           | Silent auto-learn of browser context |
| **Smart collections**                                                   | —                                    |
| **Opt-in** “Use current selections for this context” + restore defaults | —                                    |
| Tag editor; richer filters                                              | —                                    |
| Evaluate **Devices** source merge — go/no-go ADR                        | Merging devices without decision     |
| Optional grid results; shortcuts cleanup (Loop key restored)            | —                                    |

**Progress (2026-11-26).** Slice 1 done: user tags (detail strip add / remove,
search by tag, persisted with the favorites; `frontend/library/tags.ts`, 10 unit
tests; code-verified, not hand-tested). Left, in order: smart collections (a saved
tag + kind filter shown as a source), opt-in context memory with « restore
defaults », the Devices go/no-go ADR, pack sources, optional grid results.

Prerequisite landed 2026-10-09: the built-in drum sampler (ADR-028) — a
`one_shot` dragged from the library onto a pad loads it. The hybrid `drums`
source landed 2026-10-13 (factory one-shots listed next to drum MIDI).

### Exit criteria

- [ ] Power user can rebuild a personal “Drums for this genre” source in &lt; 1 min
- [ ] Context memory is opt-in and restorable to factory defaults
- [ ] Devices-in-library decision recorded (amend ADR-023 or new ADR)

---

## Cross-cutting checklist (every phase)

- [ ] Doc sync: UX contract still matches shipped behaviour
- [ ] ADR-005 respected (commands / events only)
- [ ] No library work on the audio callback hot path
- [ ] Shortcuts listed in UI guidelines when shipped
- [ ] Windows + macOS smoke for panel layout and DnD

---

## Milestone sketch (relative)

| Milestone | Outcome                                             |
| --------- | --------------------------------------------------- |
| M1        | Phase 1 usable internally with one sample folder    |
| M2        | Phase 2 pop-up is the default insert path for audio |
| M3        | First factory chord + MIDI pack demo-able           |
| M4        | Packs + collections; devices go/no-go decided       |

Exact calendar dates intentionally omitted — bind dates when scheduling a sprint.

---

## Links

- [`library-browser-ux.md`](library-browser-ux.md)
- [ADR-023](active/decisions/023-library-browser-ux.md)
- [`UI_GUIDELINES.md`](UI_GUIDELINES.md)
