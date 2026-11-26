# ADR-037 — Writing automation live from a plugin window

## Status

**Accepted — 2026-11-26** (the user approved the design, "lecture seulement,
remplace la plage"). Implemented step by step; the table at the end says which
step is done. Builds on ADR-021 (plugin parameter automation, output events),
ADR-025 (lanes), ADR-022 / ADR-029 (the recording pattern: time-stamped capture,
the clip is added by a normal command), ADR-008 (the SPSC pattern) and ADR-033
(sandbox bridge).

> **Governed surface touched:** threading model — a new audio → command-thread
> channel (`threading/threading-model.md`), and the audio callback's output-event
> path (`threading/realtime-safety.md`).

## Context

ADR-021 lists as debt: "edits made in the plugin window during playback are not
reported to Echoes". The engine already sees them: a CLAP plugin pushes
`PARAM_GESTURE_BEGIN`, `PARAM_VALUE`, `PARAM_GESTURE_END` to its output events
(`observe_output_event`), the bridge relays them (`observe_edit`), and VST3 reports
them through `IComponentHandler` / the block reply. But they are used only to move
the base of a value-modulated parameter. Nothing keeps their value or their
position on the timeline, so a gesture cannot become automation points.

The only audio → command-thread mechanism today is one global atomic flag
(`mark_plugin_state_edited`), which carries no value.

## Decision

1. **A bounded SPSC ring per plugin instance** carries the gesture from the audio
   thread to a watcher thread: `ParamEdit { kind, native_id, normalised, time_s }`
   (kinds: gesture begin, value, gesture end). One producer (the node that processes
   the instance), one consumer (the watcher). Allocated with the instance, never
   resized; a full ring drops the newest edit (the audio thread never waits). The
   ring type is the `ParamQueue` ring made generic (`SpscRing<T>`).
2. **Armed globally, filtered per lane.** One process-wide atomic, "automation write
   armed" (true while at least one lane is armed in the UI), gates the capture. When
   it is false the output-event callback does one relaxed load and nothing else.
   Which lane receives a gesture is decided by the frontend.
3. **Playback only.** The audio side records only while the transport is playing
   (`TransportInfo::is_playing`); a value outside a gesture is never recorded
   (a plugin that echoes the value the host sent cannot write points).
4. **Time stamp on the audio thread**: `position_seconds` of the block plus the
   event's sample offset, in the lane's own unit (project seconds). No clock is read
   on the consumer side, so the 25 ms poll adds no jitter.
5. **Touch semantics.** While a gesture is open on a parameter and writing is armed,
   the host stops sending that parameter's automation, so the lane does not fight the
   hand. Automation resumes with the next block after the gesture ends.
6. **One undo step per gesture.** The watcher thins the points (one per 5 ms, as the
   MIDI controls) and, at the gesture end, emits `plugin-param-gesture` with the
   points. The frontend replaces the lane's points inside `[first, last]` of the
   gesture with them (`withRecordedGesture`) and sends the usual
   `UPDATE_AUTOMATION_CLIP`. The project is still changed only by a command
   (ADR-005); the watcher never touches it.
7. **No change to rendering.** Recording goes through the project, so the offline
   render and its golden hashes (C-2) do not move; the capture is a read-only side
   path of the audio callback.

## Consequences

- **Positive.** Dragging a plugin knob during playback writes automation; undo
  removes the whole gesture; no allocation, lock or emit on the audio thread.
- **Cost.** One 4-field ring per instance (about 8 KB at 256 entries) and one
  watcher thread that wakes every 25 ms only while armed.
- **Limits.** A parameter without a gesture (a preset change inside the plugin) is
  not written. A gesture longer than the ring's capacity between two polls loses its
  newest points. Writes while stopped ("latch") are out of scope.

## Alternatives considered

- **Record from the command thread by polling parameter values.** Rejected: 25 ms
  resolution at best, and CLAP `get_value` is main-thread only.
- **One global MPSC ring.** Rejected: nodes run in parallel on the worker pool, so
  the ring would have several producers; a per-instance ring keeps the SPSC proof.
- **Emit from the audio thread.** Forbidden (`rt_audio_rules.md`).
- **Write the project from the watcher thread.** Rejected: one command per gesture
  keeps the event store, undo and the frontend mirror consistent (ADR-005).

## Steps

| Step | Scope                                                                                                         | State                                   |
| ---- | ------------------------------------------------------------------------------------------------------------- | --------------------------------------- |
| 1    | CLAP plugin in the Echoes process (effects and instruments), ring, watcher, event, arm button, frontend write | implemented 2026-11-26, not hand-tested |
| 2    | CLAP plugin in the bridge (`observe_edit`, the block reply carries the edits with their offsets)              | implemented 2026-11-26, not hand-tested |
| 3    | VST3 in the process (`IComponentHandler`)                                                                     | open                                    |
| 4    | VST3 in the bridge                                                                                            | open                                    |
