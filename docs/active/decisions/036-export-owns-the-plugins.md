# ADR-036 — The export owns the plugins while it renders

## Status

**Accepted and implemented — 2026-11-25** (the user chose the option below on
2026-11-25 over refusing the export during playback and over separate instances for
the export). Not yet hand-tested in the app. Builds on ADR-007 (one plugin instance
per placement), ADR-016 (an instance survives a recompile), ADR-004 (immutable
runtime snapshot) and ADR-015 (sandbox).

> **Governed surface touched:** threading model — which thread may call a plugin
> (`threading/threading-model.md`), and one atomic read of the audio callback
> (`threading/realtime-safety.md`).

## Context

`export_wav` and `export_mp3` render with `render_project_offline`, which compiles
the project like live playback does, on the **same `PluginHost`** (so the export
reflects current knob positions and plugin state, ADR-007). With ADR-016 the compile
reuses the live `Arc<PluginInstance>` of each placement. So during an export the
plugin has two callers:

- the **live stream**: the CPAL callback keeps running `process_block` even with
  the transport stopped (meters, live MIDI, effects tails), and the front end does
  not stop playback before an export;
- the **export**, on the command thread, calling the plugin block after block.

What follows, read in the code (`vst3/wrapper.rs`, `instance.rs`):

1. The wrapper claims the instance with the non-blocking `rt_try_enter`. When the
   live block is inside the plugin, the export's claim fails and the block is
   **copied through dry**: a hole in the file.
2. Even with no collision, two streams feed one plugin DSP state (delays,
   reverbs, oscillators), so the file is neither right nor reproducible.
3. The plugin starts the export with whatever the live stream left in it (a tail
   of a previous playback).

Measured on 2026-11-25 with the VST3 test effect (half gain) and a simulated live
stream playing the clip beside 20 exports: without the fix the first export already
differs from an export made alone (and not by a little: 249 920 frames against
168 000). The cause of that difference was not isolated further (dry block, shared
state, or both).

The old note "the export shifts by the master latency and cuts tails" was already
fixed on 2026-10-26 (`d3fc66e`); this ADR is about the remaining sharing problem.

## Decision

1. **The export takes the plugins away from the live stream.** A
   `LiveStreamGate` (`audio_engine/stream_gate.rs`), one atomic byte shared by the
   engine and the audio callback:
   - the callback claims it **once per block** with a compare-and-swap that
     succeeds only when the byte is 0, never waits, and holds the claim to the end of
     the block; when an export holds it, the callback writes silence and returns
     before touching the scheduler, the transport or the swap;
   - the export (`AudioEngine::hold_stream_for_export`, called by
     `render_project_to_stereo` in `main.rs`, so both export commands) sets its own bit
     first, which keeps new live blocks out, then waits up to 2 s, off the audio
     thread, for the block in flight to end. If it does not end, the export fails with
     "Le flux audio ne répond pas : export impossible" and the live stream goes on.
     The hold is an RAII guard released on every path.
2. **The export starts with `GraphScheduler::reset()`**, which resets every node, so
   a plugin forgets what the live stream played (VST3: `setProcessing(false)` then
   `(true)`; CLAP: `reset`; sandboxed: the reset flag of the next block).
3. **Nothing else changes.** No new instance (ADR-007 and migration C-6 stay), no new
   thread, no lock, no allocation, no `drop` on the audio thread. The render
   harness path `render_project_in_blocks` (C-2 golden hashes) does not reset and does
   not take the gate, so the golden hashes do not move.

What the user sees: the live stream is silent while the export runs (the front end
already shows "Export en cours…"); the transport position stays where it was and
playback resumes from there.

## Consequences

- Audio-thread cost: one compare-and-swap and one atomic clear per block, with
  Acquire / Release ordering (no `SeqCst`, `rt_audio_rules.md`).
- An export while a plugin is also held by an off-RT caller (plugin window state
  save, program change) can still meet that caller's FFI gate and bypass a block.
  Not covered here; the exports take the engine mutex for their whole length, which
  already keeps the command thread's own operations out.
- A hung live block (a plugin that never returns) makes the export fail after 2 s
  rather than wait for ever.
- `GraphScheduler::reset()` had no caller; the export is the first, so its
  behaviour on a fresh scheduler is now pinned by the export tests.

## Alternatives rejected

- **Refuse the export while playback runs.** Not enough: the live stream calls the
  plugins even when stopped.
- **Separate plugin instances for the export.** Violates ADR-007 / C-6 and costs a
  second load, state copy and activation of every plugin (a sampler or a large
  instrument is heavy), and the sandbox would need a second bridge per plugin.
- **A blocking wait for the instance in the export's `process`** (offline mode of
  the wrapper). It fixes the dry block, not the two streams in one state, and the live
  callback would still bypass during the export's blocks.
- **Pause the CPAL stream** (`Stream::pause`). Device-dependent, takes the stream
  down and up and glitches on resume, and gives no guarantee that a callback is not
  in flight.

## Tests

- `audio_engine/stream_gate.rs`: a live block and an export exclude each other, an
  export waits for the block in flight, gives up when it never ends (and leaves the
  stream running), two exports never hold together, and a stream that never pauses
  cannot starve the export.
- `render_harness::vst3_plugins::an_export_is_the_same_file_while_the_live_stream_runs_beside_it`:
  a live scheduler plays the clip on another thread through the gate while 20
  exports render on the shared instance (checked with `Arc::ptr_eq`); each equals an
  export made alone, bit for bit. Checked to fail without the hold.
- `render_harness::vst3_plugins::an_export_resets_the_plugins_it_shares_with_the_live_stream`
  (the test plugin counts `setProcessing(false)` in parameter 27). Checked to fail
  without the reset.

Not covered by an automatic test: the CPAL callback itself (it needs a device); its
use of the gate is two lines, reviewed. To test by hand: play a project with a VST3
or CLAP effect, export during playback, listen to the file and compare with an
export made while stopped.
