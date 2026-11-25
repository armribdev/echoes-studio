# ADR-023 — Library Browser UX Contract

## Status

Accepted — 2026-09-28.

Canonical product detail: [`docs/library-browser-ux.md`](../../library-browser-ux.md).  
Delivery plan: [`docs/library-browser-roadmap.md`](../../library-browser-roadmap.md).

---

## Context

Echoes needs a content library (one-shots, loops, later chord progressions and
MIDI patterns). Reference DAWs diverge:

- **Ableton Live 12** — unified docked browser, search + tags, Collections.
- **Bitwig 5** — contextual pop-up browser, Sources, per-context defaults.
- **Logic** — fragmented Loop Browser / Sound Library / Browser.

Echoes UI principles (`docs/UI_GUIDELINES.md`) require zero entry friction,
controlled density, and progressive disclosure. Tracks are already **unified**
(audio clips + MIDI clips on one track). ADR-005 forbids the UI from owning
authoritative project state.

We need a frozen UX + boundary decision before implementation so the library
does not silently become a second plugin browser or a full-screen destination.

---

## Decision

1. **Dual surface, one engine**  
   Docked **Browser panel** (explore) + **Pop-up browser** (insert in context).
   Same index, search, sources, and drop rules.

2. **Library = musical content first**  
   Phases 1–2 ship audio assets only (samples/loops). Chords/MIDI in Phase 3.
   **Plugin / device browsing stays out of the library** (no-go confirmed
   2026-11-26, see _Devices decision_ below). Instrument/FX pickers keep their
   current flows.

3. **Lane focus, not track types**  
   Drop targets are `audio` | `midi` | `neutral` lane focus derived from pointer
   region and selection — not a stored track kind. Incompatible drops reject
   visually (`not-allowed`).

4. **Drops are commands only**  
   Insertions go through `ADD_CLIP` / `ADD_MIDI_CLIP` / `ADD_TRACK` (and later
   instrument commands). No direct engine mutation from the library UI (ADR-005).

5. **Preview is isolated (Phase 1 = UI WebAudio)**  
   Audition must not move the arrange playhead, create clips, or touch the
   audio-callback hot path. **Phase 1** implements preview in the **frontend**
   (decode + Web Audio / equivalent UI-side player). Engine-faithful / BPM-synced
   preview is a later upgrade, not a Phase 1 dependency.

6. **Defaults over configuration**  
   Every open context has factory source + filters that work without setup.
   Optional “remember for this context” is opt-in (shipped in Phase 4,
   2026-11-26).

7. **Shortcuts: focus-scoped Space**  
   `B` toggles the library panel. When keyboard focus is in library results,
   `Space` previews; otherwise `Space` remains transport play/pause.
   (`L` also toggled the panel as an alias; amended 2026-11-26: `B` is the
   only library toggle and `L` is the playback loop again.)

8. **Non-blocking index**  
   Folder indexing is async and incremental. Startup must not wait on a full
   library scan.

9. **Dock: right panel**  
   The library lives in the **right dock** (same chrome pattern as the left
   track inspector). Phase 1 **evolves** the existing `RightPanel.svelte`
   stub rather than inventing a third rail. Left panel stays inspector-only.

10. **Phase 1 source model: single Audio source**  
    Phase 1 ships one content source for files (`audio` / “Audio”), plus
    `recents` and `favorites` as organisational views — **not** separate
    Samples vs Loops taxonomies. Splitting `one_shot` / `loop` kinds is
    Phase 2+ (metadata or heuristics).

---

## Consequences

**Positive:**

- Clear Phase 1 MVP (panel + audio drop) without waiting on chords/devices.
- Bitwig-like speed of insertion without Bitwig’s empty “Everything” chaos.
- Compatible with unified tracks and existing command protocol.
- Preview isolation protects arrange state and RT rules.

**Negative / costs:**

- Two UI surfaces to maintain (panel + pop-up) sharing one data layer —
  requires a single library service module.
- Focus-scoped `Space` must be implemented carefully to avoid transport bugs.
- Users coming from Logic may look for a separate Loop Browser — we document
  Sources instead.
- Warp-on-drop and rich chord voicing are deferred; early loop placement is
  raw file length (may feel “wrong” vs Ableton until Phase 3+).

**Invariants this ADR adds (do not change silently):**

| Invariant                                             | Notes                       |
| ----------------------------------------------------- | --------------------------- |
| Library drops → commands only                         | Extends ADR-005 to library  |
| Preview ≠ arrange mutation                            | Product + engine boundary   |
| Phase 1 preview = UI-side                             | Not on audio callback       |
| Library dock = right panel                            | Left stays inspector        |
| Phase 1 = single `audio` source (+ recents/favorites) | No fake Samples/Loops split |
| Devices ∉ library (no-go, 2026-11-26)                 | See _Devices decision_      |
| No blocking startup scan                              | UX + performance            |

---

## Devices decision (Phase 4 go/no-go, 2026-11-26)

**No-go: plugins and built-in devices do not become a library source.**
Verified against the code, not assumed:

- **Different target.** A library drop places content at a timeline position
  (`ADD_CLIP` / `ADD_MIDI_CLIP`, snap, lane focus, reject matrix). A device
  goes into a track **slot** (instrument, or a place in the effect chain). The
  `instrument_slot` / `fx_slot` contexts have stayed `deferred` in the UX
  contract since Phase 0 because none of the library's drop rules apply to them.
- **A browser already exists where it is needed.** `PluginPicker` is opened from
  the instrument panel, the effect chain and the arrange (new instrument track).
  It already filters by mode (instrument / effect), lists the built-in devices,
  searches, rescans, and manages the blocklist (ADR-033, ADR-034). Presets have
  their own picker bound to the plugin instance (ADR-031).
- **Different lifecycle.** The plugin registry is owned by `stores/plugins`
  (scan, blocklist, sandbox crash markers, format support per role). The library
  index is local, path-keyed and format-agnostic. Merging them would couple the
  library to plugin scanning and break « the library never blocks on a scan »
  in both directions.
- **No user value without a drop target.** Search and tags already work inside
  the pickers; a second entry point to the same list adds a surface to maintain
  (the Phase 2 cost: two UI shells over one data layer) and no new capability.

**Revisit only if** a `preset` asset kind is built (set the instrument **and**
load its state in one drop, UX §2 `preset`, « Phase 3+ »). That would be a new
ADR scoped to presets, not to the plugin list, and it would reuse the ADR-031
loading path.

---

## Alternatives considered

| Alternative                           | Why rejected                                               |
| ------------------------------------- | ---------------------------------------------------------- |
| Ableton-only docked browser           | Slower insertion; misses Echoes “10 s to idea”             |
| Bitwig-only pop-up, no panel          | Weak for browsing/organising large libraries               |
| Full-screen Library page              | Breaks arrange flow; fights UI guidelines                  |
| Unified Devices + Sounds from day one | Recreates “Everything” overload; blocks MVP                |
| Stored AudioTrack vs MidiTrack kinds  | Conflicts with current unified `AppTrack` model            |
| Preview via temporary arrange clip    | Pollutes undo stack and transport                          |
| Dock library in left inspector        | Left panel is track inspector (`I`); would destroy density |
| Engine preview stream in Phase 1      | Blocks MVP; UI WebAudio is enough for audition             |
| Samples + Loops sources in Phase 1    | No reliable kind metadata yet; fake taxonomy               |

---

## Amendments

| Date       | Change                                                                                                                                                                                                                                                                                 |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 2026-09-28 | Initial acceptance                                                                                                                                                                                                                                                                     |
| 2026-09-28 | Lock Phase 1: right-dock panel, UI WebAudio preview, single Audio source                                                                                                                                                                                                               |
| 2026-09-28 | Lock deferred product calls: raw warp-until-ADR, factory MIDI then rules for chords, compact pop-up, no context memory until P4 opt-in, `B`+`L` alias, empty-project auto-track in P2, preview BPM native then optional UI stretch                                                     |
| 2026-11-26 | **Phase 4 closed the open calls**: devices are **not** a library source (no-go, _Devices decision_); `B` is the only library toggle and `L` is the loop again; context memory shipped as opt-in only (source per pop-up context, restorable); packs and smart collections follow UX §3 |
| 2026-09-30 | **Pop-up = Bitwig dual surface**: same browser UI as the right dock (full list/search/sources/preview), opened from arrange **lane** `+` slot — not a truncated compact list; `+` not beside mute/solo                                                                                 |
