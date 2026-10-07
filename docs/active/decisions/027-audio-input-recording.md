# ADR-027 — Audio input capture, track arming, monitoring, and recording

## Status

Accepted — 2026-09-30. Implemented, not yet hand-tested.

## Context

MIDI recording (ADR-022) and count-in (ADR-026) exist. Audio recording did not:
Settings already listed an `input_device`, and `ChannelLevels.input` was reserved,
but CPAL opened only an output stream. The product needs arm → ● → capture into
an audio clip, with software monitoring while armed.

## Decision

### Separate CPAL input stream

`AudioEngine::start_stream_with_settings` opens a second stream via
`build_input_stream` on the selected `input_device` (same sample-rate preference
ladder as output). Failure to open input does **not** fail output: the engine
stays playable; recording/monitoring are unavailable until a restart succeeds.
Held as `_input_stream: Option<SendStream>` next to the output stream.

### Lock-free rings; no graph node

`AudioInputHub` owns two SPSC float rings (always stereo interleaved after
channel conversion):

- **record ring** — drained off the input callback by a dedicated non-RT thread
  into `AudioRecorder`.
- **monitor ring** — drained in the **output** CPAL callback and mixed after the
  graph (and with the metronome click), same exception class as ADR-026.

The input callback: convert samples → peak atomics → push both rings. No alloc,
no lock, no I/O, no Tauri emit.

### Session state (not project)

Like MIDI arming: `armed_track_id` stays the single global arm. Recording mode
is chosen by track shape: instrument track → MIDI (ADR-022); otherwise → audio.
`input_monitor_enabled` is an atomic session flag (default off: software monitoring adds latency, so it echoes without headphones). Nothing is added
to `AppTrack` / `CommandEnvelope` (ADR-002 scope).

### Recording lifecycle

Mirrors `MidiRecorder`:

1. `start_audio_recording(count_in_bars)` — requires armed non-instrument track;
   sets `SharedTransport::recording`; optional count-in when stopped.
2. Drain thread appends stereo PCM only when `count_in_remaining() == 0`.
3. Stop / ● / PAUSE / STOP → write WAV under app-data `recordings/`, emit
   `audio-recording-finished` → frontend `ADD_CLIP` (one undo step).
4. Growing arrange preview (`pending` clip) until the real clip lands. The drain
   thread also folds the take into min/max peaks (512 frames per chunk, mono);
   the UI polls `get_audio_recording_peaks(from)` every ~80 ms and feeds them to
   the preview clip, so the waveform draws live (2026-10-08). Polled, never
   emitted from the audio side.

Latency: clip start uses `LiveInputClock::transport_position_at` on the first
kept sample (output-side `record_latency` already applied). Per-device input
latency refinement is a follow-up.

### C-2 / offline

Input, monitor mix, and recorder never touch the offline renderer or golden
hashes.

## Invariant check

- **Graph-first (ADR-001).** Monitoring is not project DSP — same class as the
  metronome click.
- **RT safety.** Input/output callbacks only touch atomics and preallocated
  rings. WAV write and emit happen off those callbacks.
- **C-2 / C-4.** No render change; no project schema change.

## Alternatives considered

- **Full-duplex single stream.** Rejected: CPAL duplex support is uneven across
  hosts; two streams match the existing device prefs model.
- **Monitor as an `AudioNode` on the armed track.** Rejected: would need
  recompile-free ephemeral graph state and offline exclusion; click-style mix is
  enough for V1.
- **Record into progressive PCM (ADR-024) without a WAV file.** Deferred: WAV +
  existing `ADD_CLIP` / cache path is the smallest shippable finalize.

## Follow-ups

Input-latency calibration; per-track arm/monitor; punch-in / loop takes;
channel selection beyond stereo downmix.
