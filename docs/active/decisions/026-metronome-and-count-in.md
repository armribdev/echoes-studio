# ADR-026 — Metronome click and recording count-in, outside the mix graph

## Status

Accepted — 2026-09-30. Implemented, not yet hand-tested. Raised for review
because it adds audio-thread code that is **not** an `AudioNode` (see
*Invariant check*).

## Context

Recording MIDI (ADR-022 amendments) had no way to play in time: no click, and no
lead-in before the take. Both are monitoring aids for the person playing.

## Decision

### The click is generated in the CPAL callback, after the graph

`audio_engine/metronome.rs::Metronome` renders a short decaying sine burst
(accent 1.6 kHz on the first beat of the bar, 1 kHz otherwise, 40 ms) into a
pre-allocated mono `click_buf`. The callback adds it to both output channels when
writing to the device — after the scheduler has produced the mix and after the
meters were measured. Consequences: the click is never in the project mix, the
track/master meters, or an export.

Beats come from an integer sample grid: a click starts at the first frame where
`floor(sample × bpm / 60 / sr)` changes, so it is identical whatever the block
size and follows loop wraps. Starting playback mid-beat does not click at once.

### Count-in is a transport state, not a position trick

`SharedTransport::count_in_samples` holds the samples of lead-in left. While it
is non-zero and the transport runs, the callback renders **stopped** slices
(`plan_slice`: the position does not move, clips do not play, live instruments
still sound) that end exactly where the count-in ends, so the music starts on the
sample. The click runs during the count-in even when the metronome is off. It
works at position 0 (no negative positions needed) and needs no extra clock in
the graph.

`start_midi_recording(count_in_bars)` arms it (bars × time-signature numerator
beats at the current tempo) only when the transport is stopped; the frontend then
calls `play`. Notes played during the count-in are not recorded. Stop, pause and
the end of a recording cancel it.

### State

Both settings are session/preferences state, outside `AppProject` and outside
`CommandEnvelope` (like arming): `set_metronome_enabled` sets an atomic on
`SharedTransport`; the frontend keeps `metronomeEnabled` / `countInEnabled` in
`localStorage` and pushes the metronome flag at startup and on change. Buttons
next to ● in the transport bar.

## Invariant check

- **Graph-first (ADR-001).** Every *project* DSP element is an `AudioNode`. The
  click is not project DSP: it is not in the mix, has no track, and is never
  rendered offline — the same kind of exception as the live-input queue
  (ADR-022), which also bypasses the graph. Nothing was added to `AppTrack`.
- **RT safety.** The callback gains: two atomic reads per slice
  (`count_in_remaining`, `metronome_enabled`), `Metronome::render` (arithmetic,
  one `sin` per audible sample, no allocation, no lock), a buffer add per frame.
  `click_buf` and the voice are allocated before the stream starts.
- **C-2.** The offline renderer does not use `plan_slice`, the metronome or the
  count-in: harness green, golden hashes unchanged.
- **C-4.** No project schema change.

## Alternatives considered

- **A metronome `AudioNode` on the master bus.** Rejected: it would have to be
  excluded from offline compiles by a special case, would show in the meters, and
  needs a recompile-free control path anyway.
- **Count-in by seeking to `start − N beats`.** Rejected: impossible at position 0
  (the most common case) and it would play the clips of the previous bars.
- **Web Audio click in the frontend.** Rejected: not sample-locked to the engine
  and subject to UI jitter.

## Follow-ups

Count-in length and click volume/sound as preferences; count-in also when
playing without recording; metronome to a separate output; loop take handling.
