# ADR-022 — Live hardware MIDI input via queue-injection

## Status

Accepted. New capability, not part of a previously scheduled migration phase
— see the new **Phase 5f** added to
[`migration_plan.md`](../migration/migration_plan.md).

## Context

`target_architecture.md` §6.5 anticipates hardware MIDI input
("Pas d'input live" → `MidiInputNode` + capture device → SPSC → audio thread),
but this was never scheduled in `migration_plan.md`, and no MIDI I/O crate
existed in the project.

Scope for this iteration (agreed with the user): **live note input only** —
plug in a keyboard/controller, enable it globally in Settings, arm a track,
and play through its instrument in real time. Explicitly out of scope:
recording captured notes into a clip, MIDI CC/Learn, MIDI output to hardware,
and per-track device routing.

## Decision

### Bypass `PortKind::Midi` graph edges

The compiler still rejects `PortKind::Midi` edges
(`compiler.rs::validate_port_kinds`, guarded by
`midi_edge_is_rejected_until_phase_5c`) — that guard is for Phase 5c
(plugin instruments via graph-typed MIDI edges) and is untouched by this ADR.

Instead, live input reuses the architecture already shipped for piano-roll
audition: a per-track lock-free SPSC ring owned by `LiveParams`, drained
directly by `InstrumentNode` every block, entirely outside the graph/edge
model. This required no compiler or scheduler changes.

### New SPSC ring, not a shared one

`audio_graph/midi.rs` gains `MidiLiveInputMessage` / `MidiLiveInputQueue`,
structurally identical to the existing `MidiPreviewMessage` /
`MidiPreviewQueue` (fixed-capacity ring, `UnsafeCell` slots, drop-newest when
full). It is a **separate** type/instance rather than a reuse of the preview
queue: SPSC correctness requires exactly one producer per ring, and the
producers differ (command thread for audition clicks, MIDI capture thread
here).

`LiveParams` exposes `live_input_queue(track_id)` / `push_live_input(...)`,
mirroring `preview_queue` / `push_preview`. `InstrumentNode` gains
`live_input` / `set_live_input` / `drain_live_input`, mirroring
`preview` / `set_preview` / `drain_preview`, drained in `process()` right
after `drain_preview()`. Hardware notes get `offset: 0` — the same
fire-at-top-of-block latency the audition path already accepts.

### New dependency: `midir`

Cross-platform MIDI I/O. It owns and drives its own OS thread per open input
connection internally (the connection's callback) — we never `thread::spawn`
for this ourselves.

### New thread role: MIDI capture thread(s)

Documented in `threading/threading-model.md`. One per enabled device, owned
by `midir`. The callback parses raw bytes (Note On/Off only; everything else
ignored for this iteration — controls added by the 2026-10-13 amendment), reads the armed-track id, and calls
`LiveParams::push_live_input`. No lock held across the push; parsing
allocates nothing beyond primitive byte reads. This thread never touches the
audio thread or its data directly — only the lock-free ring.

`midir::MidiInputConnection` is not `Send` on every backend, mirroring CPAL's
`Stream`. It is wrapped the same way `audio_engine::engine::SendStream`
wraps `cpal::Stream`: a newtype with `unsafe impl Send`, justified because it
is only ever touched behind `Mutex<MidiInputRegistry>` on the command thread,
and the connection's own thread runs internally, not through this handle.

### Armed track — new ephemeral state, not project data

A `Mutex<Option<String>>` in `AppState`. Set via a plain
`#[tauri::command] arm_track_for_midi_input`, outside the `CommandEnvelope`
event-sourcing pipeline — the exact precedent is `preview_midi_note`
(`engine_gateway/api.rs`), which the code comments already describe as
"no project event, no recompile". This respects the event-sourcing scope
boundary (ADR-002): arming a track is a live-session choice, not project
history.

### Device registry

`src-tauri/src/midi_input.rs::MidiInputRegistry` owns open connections keyed
by **device name** (midir port indices are not stable across replugs/reboots
— this is a known limitation; the frontend persists enabled devices by name
in the same way). Two commands: `list_midi_input_devices`,
`set_midi_input_enabled`.

## Real-time impact

| Change                               | RT cost                                           | RT risk                              |
| ------------------------------------ | ------------------------------------------------- | ------------------------------------ |
| `InstrumentNode::drain_live_input()` | pre-allocated ring drain, same as `drain_preview` | none — no alloc/lock/drop            |
| Capture thread → `push_live_input`   | off-RT (not the audio thread)                     | none                                 |
| Armed-track `Mutex`                  | only touched by command thread + capture thread   | never reachable from `process_block` |

C-1 preserved: no new mutex on the RT path. C-5 preserved: the new
cross-thread channel is the same lock-free SPSC pattern already used
elsewhere in this module.

## Compatibility

- **C-2 (bit-identical offline).** Unaffected. Offline export never opens
  MIDI devices or drains this queue — the same non-interaction the
  piano-roll audition queue already has.
- **C-4 (saved projects).** No project schema change. Armed track and
  enabled devices are session/preference state, not part of `AppProject`.

## Amendment 2026-09-26 — Local Control Off, startup reconnection

**Local Control Off.** A digital piano plays its own speakers in parallel
with the MIDI it sends. When the `midi.localControlOff` preference is on
(default), enabling a device sends CC 122 value 0 on all 16 channels to the
MIDI **output** port of the same name; disabling it, turning the preference
off, or quitting Echoes (`RunEvent::Exit`) sends CC 122 value 127. The
registry remembers exactly which devices it silenced, so it never
"un-silences" a keyboard the user configured that way themselves. Best
effort: a device without a same-named output port keeps its own sound, and
the failure is only logged. Each send opens a short-lived `midir`
`MidiOutput` connection on the command thread — no audio-thread involvement,
no new thread role. This is the first (narrow) use of MIDI output; general
MIDI output to hardware stays a follow-up. Known limit: if Echoes crashes
instead of exiting, the piano stays silent until it is power-cycled
(Yamaha pianos restore Local Control at power-on).

**Startup reconnection (bug fix).** Enabled devices were persisted in the
preferences but never reopened on launch — the toggle showed "on" with no
connection. The frontend now applies `preferences.midi` from a subscriber in
`routes/+page.svelte` (same shape as plugin isolation): at startup and after
every save, it sets Local Control first, then opens/closes each visible
device by diffing against the backend's `enabled` flags. The Settings panel
only edits the draft, re-reads the device list on every open, and keeps
remembered devices that are not plugged in.

**Known devices (same day).** `preferences.midi.devices` keeps every keyboard
seen once — `{ name, alias, enabled }` — so an unplugged keyboard stays listed
(greyed out) with its setting. Double-clicking a row renames it; the alias is
display-only: the device is still identified by its system port name. There is
no way to remove a known device yet ("Réinitialiser" on the MIDI section drops
the unplugged ones). Opening
Settings re-applies the saved preferences first, so a keyboard plugged back in
reconnects without restarting Echoes (no background hot-plug detection yet).
Preferences saved by the earlier `enabledDeviceNames` shape are migrated on
load.

**Hardware ID — considered, deferred.** `midir` on Windows (WinMM) exposes only
a name. The PnP instance id (`USB\VID_…&PID_…&MI_…\…`) is reachable through
SetupAPI or the WinRT MIDI API, but it is stable per unit only when the device
has a USB serial number; many consumer keyboards have none, and Windows then
derives the id from the USB port. Name matching is what most DAWs do and stays
until two identical keyboards on one machine become a real case.

## Amendment 2026-09-26 (later same day) — Live hotplug detection

Reopening a keyboard required reopening Settings (or restarting Echoes).
`midir`/WinMM give no OS-level connect/disconnect callback, so
`midi_input::spawn_hotplug_watcher` runs a plain 1 s re-enumeration loop on
its own named thread (spawned once from `main.rs::setup`, alongside
`start_native_audio`) — cheap (an OS device query, no I/O) and only acts when
the visible set actually changed. On a change it calls the new
`MidiInputRegistry::mark_disconnected` for any device that dropped out (drops
the stale connection object without reaching now-absent hardware, and clears
it from `silenced` so a replug is correctly seen as "not enabled" instead of
being masked by a dead `contains_key`), then emits `MIDI_DEVICES_EVENT`
(`"midi-devices-changed"`, payload `Vec<MidiInputDeviceInfo>`) — the same
plain-event-constant shape as the existing `PLUGIN_AVAILABILITY_EVENT`.

`routes/+page.svelte` subscribes for the app's lifetime: it updates the new
`stores/midiDevices.ts::midiDeviceList` store and unconditionally re-applies
`applyMidiPreferences` (a device reappearing doesn't change the preference,
so the existing "skip if unchanged" guard on the preferences-subscribe path
does not apply here). `SettingsPanel.svelte` no longer polls anything itself —
it subscribes to `midiDeviceList` and merges only `connected` plus newly
discovered devices into `midiRows`, preserving in-progress alias/enabled edits
so a hotplug tick can never discard what the user is typing.

## Amendment 2026-09-26 (later same day) — Timestamped live input, multi-producer queue

**Timing.** Live notes were placed at offset 0 of the next rendered block, so
their delay jittered between zero and one full period depending on when the
key was pressed. They are now timestamped and placed like professional hosts
do: exactly one callback period after arrival, at the matching sample —
constant latency (one period, e.g. 5.3 ms at 256 frames / 48 kHz) instead of
0…one period. This does **not** lower the worst case (a rendered block cannot
be changed after the fact); it trades ~half a period of average latency for
no jitter.

- `midi.rs::LiveInputClock` (owned by `LiveParams`, shared as `Arc`): one
  `Instant` epoch. The capture callback stamps `MidiLiveInputMessage.received_at`
  first thing, before any lock. The CPAL callback reads the clock once at entry
  and, before each rendered slice (loop wrap splits a callback), stores the
  wall time mapped to sample 0 of that slice: `callback_start - frames_left / sr`
  (`engine.rs::live_event_window_start`).
- `MidiLiveInputQueue::drain_due` places each message at
  `(received_at - window_start) × sr`; a message due after this slice stays
  queued for the next one; an older one (stall) or one with no clock
  (offline, window 0) plays at offset 0. `InstrumentNode` now also sorts its
  events when the transport is stopped, since live offsets are no longer 0.
- Resolution is our own `Instant`, not midir's timestamp (WinMM gives 1 ms
  resolution on an unrelated base).
- RT: one monotonic clock read per callback (`Instant::now` — QPC on Windows,
  no lock/alloc), recorded as permitted in `realtime-safety.md`. The offline
  render never sets the window, so C-2 is unaffected (harness green).

**Multi-producer bug fixed.** The ring was documented SPSC, but each enabled
keyboard has its own capture thread, so two keyboards could push concurrently.
Producers are now serialized by a `Mutex<()>` inside the queue — taken only by
capture threads; the audio-thread consumer stays lock-free.

## Amendment 2026-09-29 — Recording captured notes into a clip

The first follow-up is done: the ● button of the transport bar records the
armed track's hardware notes into a new MIDI clip. Not hand-tested yet.

- **Position of a note.** The CPAL callback now publishes, with the wall time of
  each slice, the transport sample rendered at that time
  (`LiveInputClock::set_event_window`, three relaxed atomic stores guarded by a
  seqlock counter, plus one for the latency below so the reader never pairs two slices). The capture thread
  converts an arrival time into a transport sample with
  `transport_position_at` — the same mapping the instrument uses to place the
  note, so playing the clip back reproduces what was heard. A loop wrap follows
  for free (each slice republishes). This is the only audio-thread change.
- **Recorder.** `midi_recording.rs::MidiRecorder` (in `AppState`) holds the
  events of one recording behind a `Mutex` taken only by the capture threads and
  the command thread. `capture` is called from the capture callback for the
  armed track; nothing is reachable from `process_block`.
- **Lifecycle.** `start_midi_recording` / `stop_midi_recording` are plain Tauri
  commands (session state, outside `CommandEnvelope`, like arming). The frontend
  starts the transport itself. The recording also ends on `PAUSE` and `STOP`
  (`api.rs`, before `STOP` resets the position). Ending emits
  `midi-recording-finished` with the clip or `null`.
- **Clip.** Note ons/offs are paired per pitch; held notes are closed at the
  stop position; a retriggered pitch closes the previous note; notes shorter
  than 1/16 beat are stretched. The clip starts at the record position (or the
  first note, if earlier) and ends at the stop position (or the last note end).
  No note → no clip.
- **Project write.** The frontend adds the clip with the existing
  `ADD_MIDI_CLIP` command (seeded notes, as for a paste): one undo step, one
  recompile, project events unchanged. The buffer itself is not project state.
- **C-2 / C-4.** Offline render never sets the window nor records: harness green,
  hashes unchanged. No project schema change.

**Latency compensation (same day).** A player follows what they hear, so a
note is recorded where the player _heard_ the timeline when pressing the key,
not where the instrument renders it. Every callback publishes
`LiveInputClock::set_record_latency` = callback period + output latency (CPAL
`playback − callback` timestamp, 0 if unavailable), and `transport_position_at`
subtracts it: the period because live notes sound one period after arrival
(above), the output latency because a rendered buffer reaches the speakers
later. Input latency (USB/driver, ~1–3 ms) is not compensated, and the CPAL
output latency is only as accurate as the host API reports it — to be checked by
ear on the target machine.

**Live display (2026-09-30).** The clip is drawn while it is recorded, like the
pending audio import: a non-interactive `MidiClip` (`recording` prop) on the
armed track that grows with the playhead and shows notes as they are played
(held notes extend to the playhead). The capture callback emits
`midi-recording-note` (`RecordedNote`: edge, pitch, velocity, latency-compensated
position) for every recorded note — from the capture thread, never the audio
thread; `start_midi_recording` returns the start position. The frontend keeps the
events in `midiRecordingPreview` and rebuilds the clip with
`buildPreviewClip` (same pairing as the Rust `build_clip`, so the shape matches).
On `midi-recording-finished` the preview freezes on the final clip and stays
until `ADD_MIDI_CLIP` has landed (+2 frames), so the swap to the real clip does
not flash. The playhead moves at the ~30 Hz `POSITION_UPDATED` rate, and so does
the growing edge.

**Auto-stop bug (same day).** The position reporter stops playback at the end of
the timeline by calling `transport.stop()` directly, bypassing the `STOP`
command — recording past the last clip left the recording running while the
transport stopped. `SharedTransport` now carries a `recording` flag (set by
`MidiRecorder::start`, cleared by `finish`) and the reporter does not auto-stop
while it is set.

Count-in and metronome: ADR-026. Known limits (follow-ups): with a playback loop, passes stack into
one clip instead of separate takes; `buildPreviewClip` is not unit-tested (no JS
test runner in the repo).

## Amendment 2026-10-13 — Keyboard controls to plugin instruments

A hardware keyboard's pitch bend, control changes (mod wheel, sustain pedal…)
and aftertouch now reach CLAP instruments. Not hand-tested yet.

- **Model.** `MidiEventKind` gains `Control { message: [u8; 3] }`: the raw
  MIDI bytes, channel nibble cleared (notes already play on one channel).
  `MidiLiveInputMessage` now carries a `MidiEventKind` instead of
  note-only fields. `midi_input::parse_live_message` accepts `0xA0`, `0xB0`,
  `0xE0`; program change and system messages are still ignored.
- **Delivery.** `InstrumentNode::drain_live_input` passes controls through
  untouched (no voice bookkeeping). `push_clap_midi_events` turns them into
  `CLAP_EVENT_MIDI` events (`clap_ffi::ClapEventMidi`, main note port) in the
  same time-sorted buffer as notes and parameters; at an equal offset a control
  sorts after a NoteOff and before a NoteOn, so a bend set with a key applies
  to it. The sandbox bridge forwards them too (`ClapEventBuffer::push_bytes`).
  The host does not check the plugin's `clap.note-ports` dialects: a plugin
  without the MIDI dialect ignores the event, as CLAP requires.
- **Built-in instruments** (`PolySynth`, sampler) ignore controls for now.
- **Recording** keeps notes only: `MidiRecorder::capture` skips controls,
  since clips have no controller data (that needs a project-schema change, C-4).
  _Superseded the same day by ADR-029: controls are recorded into the clip and
  replayed. ADR-029 also fixed channel aftertouch (`0xD0`, a 2-byte message),
  which this amendment's parser still dropped._
- **Queue size.** `LIVE_INPUT_CAPACITY` 64 → 1024: a fast bend or mod-wheel
  stream could fill the ring, and a dropped NoteOff would leave a note hanging.
- **RT / C-2.** One more `Copy` enum variant in pre-allocated lists, no
  alloc/lock. Offline render never drains the live queue: harness green,
  hashes unchanged.

## Alternatives considered

- **Route hardware input through `PortKind::Midi` graph edges** (a real
  `MidiInputNode` in the graph). Rejected for this iteration: the compiler
  guard for MIDI edges is Phase 5c work (plugin instruments), unrelated in
  scope; building it just for hardware input would mean implementing that
  guard's removal prematurely, for a feature that doesn't need graph routing
  yet (one global armed track, no per-track device graph).
- **Reuse `MidiPreviewQueue` for hardware input too.** Rejected: two
  concurrent producers (command thread + capture thread) would break the
  ring's SPSC invariant.

## Follow-ups (explicitly out of scope here)

- ~~Recording captured hardware notes into a MIDI clip~~ — done, see the
  2026-09-29 amendment.
- MIDI CC / Learn mapping to parameters. (Raw CC, bend and aftertouch to
  CLAP instruments: done, see the 2026-10-13 amendment. Recording them into
  clips: done, ADR-029. Learn is still open.)
- MIDI output to external hardware (beyond Local Control, see amendment).
- Per-track device routing (today: one global armed track receives every
  enabled device).
