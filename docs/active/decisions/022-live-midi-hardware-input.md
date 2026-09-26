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
ignored for this iteration), reads the armed-track id, and calls
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

| Change | RT cost | RT risk |
|---|---|---|
| `InstrumentNode::drain_live_input()` | pre-allocated ring drain, same as `drain_preview` | none — no alloc/lock/drop |
| Capture thread → `push_live_input` | off-RT (not the audio thread) | none |
| Armed-track `Mutex` | only touched by command thread + capture thread | never reachable from `process_block` |

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

- Recording captured hardware notes into a MIDI clip (record-arm → project
  write).
- MIDI CC / Learn mapping to parameters.
- MIDI output to external hardware (beyond Local Control, see amendment).
- Per-track device routing (today: one global armed track receives every
  enabled device).
