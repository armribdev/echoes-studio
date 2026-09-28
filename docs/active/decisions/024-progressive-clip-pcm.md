# ADR-024 — Progressive Clip PCM via Immutable Prefixes

## Status

Accepted — 2026-09-29.

## Context

Import already streams **peaks** to the UI while decoding (`audio://import-progress`).
PCM (`raw_samples`) only entered the cache after a full decode, and `ADD_CLIP`
ran only after `prepare_audio_import` returned. Arrange playback therefore could
not start until the entire file was decoded.

Users want to hear (and scrub) the portion already decoded while the rest loads.
ADR-004 requires immutable runtime snapshots: the audio thread must not observe
a `Vec` that grows in place under an `Arc`.

## Decision

1. **Immutable prefixes only**  
   During decode, publish successive `Arc<Vec<f32>>` snapshots of the decoded
   prefix (clone of `[..prefix_len]` on the decode/blocking thread). Never mutate
   a `Vec` that the audio thread may be reading.

2. **Partial cache entries**  
   `CachedAudioData` gains `complete: bool`. Incomplete entries may be used by
   `ADD_CLIP` / `TrackNode`. Beyond `samples.len()`, `TrackNode` already outputs
   silence — the timeline `end` may still be the probed full duration.

3. **Throttled publish + recompile**  
   Publish at first usable prefix (~0.35 s or ~5 %), then at progress gates
   (~15 / 35 / 55 / 75 / 92 %), then once at finalize (`complete: true`). Each
   publish updates the path cache, refreshes in-memory `AppClip.raw_samples` for
   matching paths, and triggers `recompile_if_needed` (ADR-004 swap).

4. **Early `ADD_CLIP`**  
   UI may commit `ADD_CLIP` on the first `audio://import-pcm-ready` event while
   `prepare_audio_import` continues. Pending waveform UI may remain until decode
   completes (anti-flash handoff unchanged).

5. **No grow-under-RT**  
   Rejected: preallocated buffer + `AtomicUsize` ready length shared with the
   callback without recompile — would weaken ADR-004’s “immutable clip PCM”
   assumption and needs a separate ADR if revisited.

## Consequences

- Arrange can play the loaded prefix during import; underrun past the prefix is silence.
- Extra recompiles during long imports (bounded by publish gates).
- `cache_audio_clip` prefers an existing path-cache entry (partial or complete)
  and must not start a second decode while prepare owns the path.
- Offline / undo still resolve samples from the path cache after finalize.

## Alternatives considered

| Option | Why not |
|--------|---------|
| WebAudio-only audition during import | Does not play on the arrange / engine path |
| Grow `Vec` under `Arc` with atomics | Conflicts with ADR-004 without a dedicated ADR |
| Wait for full decode (status quo) | No progressive playback |

## Related

- ADR-004 — Immutable Runtime Snapshots  
- ADR-005 — UI / Engine Separation  
- `docs/active/threading/realtime-safety.md`  
- Progressive peaks UI (pre-existing, unchanged contract)
