# ADR-018 — Clip source offset & trim (audio + MIDI window)

## Status

**Accepted — implemented 2026-06-04.** Audio and MIDI clips gain resize handles
(left/right edges) for true trimming. Backed by a new `offset` field on the clip
model, the `TrackNode` source-skip, and a MIDI playable-window cull in
`InstrumentNode`.

> **Governed surfaces touched:** State-management boundary (project file format +
> clip model, ADR-002) **and** MIDI scheduling semantics (ADR-012). This ADR
> exists because both are "never change silently" constraints.

---

## Context

Clips could be moved but not resized. The runtime anchored audio playback to the
clip's left edge — the source file always played from sample 0 at `start`
([`nodes/track.rs`](../../../src-tauri/src/audio_graph/nodes/track.rs)) — and the
MIDI scheduler ignored `clip.end` entirely, anchoring notes to `clip.start`
([`audio_graph/compiler.rs`](../../../src-tauri/src/audio_graph/compiler.rs),
[`nodes/instrument.rs`](../../../src-tauri/src/audio_graph/nodes/instrument.rs)).

A right-edge trim (change `end`) was already expressible, but a **left-edge trim**
— hide the start of the content while the rest stays anchored in time — was
impossible: there was no place to record "how much of the source is hidden".

## Decision

Add `offset: f64` (seconds) to the clip model. The **content origin** sits at
`start - offset`; the **playable window** is `[start, end]`.

- **Audio.** `offset` = seconds skipped into the source file before playback.
  `TrackNode::ClipData` carries `offset_sample` (= `offset · engine_sr`); the
  per-frame source index adds it: `local_offset = (pos − start) + offset_sample`.
  The content origin (`start − offset`) is invariant under a left-trim, so the
  source-end clamp (`end ≤ origin + source_duration`) is unaffected.
- **MIDI.** Notes anchor to the content origin: the compiler sets
  `MidiNoteSnapshot.clip_start_sec = start − offset` and bakes
  `window_start_sample` / `window_end_sample` (= `start|end · engine_sr`,
  tempo-independent). `InstrumentNode` clamps each note to
  `[window_start, window_end)` — notes before the window are dropped (left-trim),
  notes past it are silenced (right-trim).

Resizing reuses the existing `MOVE_CLIP` / `MOVE_MIDI_CLIP` command and
`CLIP_MOVED` / `MIDI_CLIP_MOVED` events, extended with an optional `offset`
(payload already fully describes the new clip state; undo/redo via the existing
event-sourcing snapshots). The frontend drag math keeps the content origin fixed
on a left-trim (`offset = newStart − origin`) and clamps the audio tail to the
source length.

### Scope of edit

- [`project.rs`](../../../src-tauri/src/project.rs): `offset` on `AppClip`,
  `AppMidiClip`, their DTOs, and the persisted `ClipFile` / `MidiClipFile`
  (`#[serde(default)]`).
- [`audio_graph/nodes/track.rs`](../../../src-tauri/src/audio_graph/nodes/track.rs)
  - [`compiler.rs`](../../../src-tauri/src/audio_graph/compiler.rs): source-skip.
- [`audio_graph/midi.rs`](../../../src-tauri/src/audio_graph/midi.rs) +
  [`nodes/instrument.rs`](../../../src-tauri/src/audio_graph/nodes/instrument.rs):
  window cull.
- [`engine_gateway/{api,commands}.rs`](../../../src-tauri/src/engine_gateway):
  optional `offset` on the move command + event.
- Frontend: resize handles in `Clip.svelte` / `MidiClip.svelte`, drag handlers in
  `MainPanel.svelte`, waveform offset in `lib/waveform.worker.ts`.

---

## Real-time impact

Audio: one extra `u64` add per frame — RT-safe (no alloc/lock/branch growth).
MIDI: two `max`/`min` clamps on pre-baked `u64` values per note per block —
RT-safe. Validated with the `rt-validation` checklist.

## Compatibility (migration constraints)

- **C-4 (saved projects).** `offset` is `#[serde(default)]` → legacy files load
  with `offset = 0`.
- **C-2 (bit-identical offline).** Audio with `offset = 0` is unchanged. The MIDI
  window cull is **new behaviour**: a legacy MIDI clip whose notes extended past
  `clip.end` now stops them at the edge. The offline C-2 harness
  ([`audio_engine/offline.rs`](../../../src-tauri/src/audio_engine/offline.rs)) is
  audio-only, so it stays green; this is a deliberate semantic change, not a
  regression of a covered guarantee. Default MIDI clips author their notes inside
  the clip bounds, so practical impact is nil.
- **ADR-012.** Extended, not contradicted: notes remain in beat space with
  per-block tempo resolution; the window bounds are in tempo-independent seconds.

---

## Alternatives considered

- **A dedicated `RESIZE_CLIP` command/event.** Rejected: mechanically identical to
  a move that also sets `offset`; a second event widens the event model for no
  semantic gain (ADR-002 keeps the model bounded).
- **Compile-time MIDI culling.** Rejected: whether a note falls in the window is
  tempo-dependent (beats × BPM), so culling must be evaluated per block to keep
  the "tempo change without recompile" property (ADR-012).
- **Time-stretch instead of trim.** Out of scope — a large DSP effort
  (resampling / stretch algorithm); the request was edge-trim resizing.

## Consequences

- **Positive.** Standard DAW edge-trim on both clip types; the content-origin
  model makes left-trim non-destructive and reversible.
- **Risk.** The MIDI window cull changes playback for legacy clips with
  out-of-bounds notes. Mitigation: documented here; covered by `midi.rs`
  determinism tests staying green.
- **Reversibility.** `offset = 0` everywhere restores prior behaviour exactly.
