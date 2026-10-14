# Plugin Wrapper System

> **Canonical.** Merges the former `systems/plugin-wrapper-system.md` (VST3 + AU
>
> - CLAP responsibilities) and the legacy `vst-wrapper-rules.md` (detailed VST3
>   code). The legacy flat file is archived under `docs/archive/vst-wrapper-rules.md`.

## Purpose

Plugin wrappers are thin adapters that translate between an external plugin
format's API (VST3 COM interfaces, CoreAudio AudioUnit handles, CLAP C ABI) and
the engine's `AudioNode` trait. They are the **only** code that bridges external
plugin formats into the graph system — an isolation layer, not a first-class
engine object.

---

## The Isolation Boundary

```
External plugin binary
        │  (VST3 COM / AU handle / CLAP module / WASM module)
        ▼
[ ISOLATION BOUNDARY — wrapper is the only code that crosses this ]
        │
VstWrapperNode / AuWrapperNode / ClapWrapperNode  (implements AudioNode + Device)
        │  (only AudioNode is visible to the graph)
        ▼
AudioGraph / GraphScheduler  — has zero knowledge of plugin formats
```

The graph runtime **must never** see:

- `IAudioProcessor`, `IEditController`, or any VST3 COM pointer
- `AudioUnit` or any CoreAudio handle
- `clap_plugin` or any CLAP struct pointer
- `vst3-sys` types or raw plugin binary handles

If any `plugin_formats` type leaks into `audio_graph/`, it is an architectural
regression.

---

## VST3 Wrapper Responsibilities

### Buffer translation

VST3 uses pointer arrays (`float** inputs, float** outputs`). The scheduler uses
`&[&AudioBuffer]` / `&mut [AudioBuffer]`. Translation happens in
`VstWrapperNode::process` using **pre-allocated** pointer arrays:

```rust
fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], transport: TransportInfo) {
    // 1. Fill pre-allocated ptr arrays (no allocation — done in prepare())
    for (i, input) in inputs.iter().enumerate() {
        for ch in 0..self.num_channels {
            self.input_ptrs[i][ch] = input.channel(ch).as_ptr() as *mut f32;
        }
    }
    for (i, output) in outputs.iter_mut().enumerate() {
        for ch in 0..self.num_channels {
            self.output_ptrs[i][ch] = output.channel_mut(ch).as_mut_ptr();
        }
    }

    // 2. Build ProcessData with pre-allocated struct (no allocation)
    let process_data = ProcessData {
        inputs:      self.input_bus_buffers.as_ptr(),
        outputs:     self.output_bus_buffers.as_ptr(),
        num_samples: transport.block_size as i32,
        // ... transport sync
    };

    // 3. Call plugin — no allocation, no locks; unsafe scoped to the FFI call only
    unsafe { (*self.processor).process(&process_data); }
}
```

**Key rule:** `self.input_ptrs`, `self.output_ptrs`, `self.input_bus_buffers`,
and `self.output_bus_buffers` are all allocated in `prepare()`. The `process()`
call only fills pointer values — no `Vec::push`, no `Box`.

### Parameter mirroring

VST3 parameters use integer IDs (`ParamID: u32`); the engine uses string keys.
The wrapper builds the mapping in `prepare()`:

```
string key "gain"  ↔  VST3 ParamID 0x12345678
```

When the host calls `set_parameter_normalised("gain", 0.75)`:

1. Look up the VST3 ID in the mapping.
2. Queue a `ParamChange { id, value: 0.75 }` in a pre-allocated ring buffer
   (on the command thread).
3. On the next `process()` call, deliver the queued changes via
   `ProcessData.inputParameterChanges` — the VST3 sample-accurate mechanism.

**Never** call `IEditController::setParamNormalized` from the audio thread.

### Transport sync

> **Not implemented yet (verified 2026-09-23):** the VST3 wrapper passes
> `process_context = null`, so VST3 plugins get no tempo or position. The CLAP
> path below is the reference to copy.

```rust
let position = transport.musical_position();   // shared, format-agnostic
let ctx = ProcessContext {
    state:              if transport.is_playing { kPlaying } else { 0 },
    sampleRate:         transport.sample_rate as f64,
    projectTimeSamples: transport.position_samples as i64,
    projectTimeMusic:   position.beats,
    barPositionMusic:   position.bar_start_beats,
    tempo:              transport.bpm as f64,
    timeSigNumerator:   transport.time_signature.numerator,
    timeSigDenominator: transport.time_signature.denominator,
    // ...
};
```

All transport data comes from the `TransportInfo` passed by the scheduler —
never from global state or a separate lookup. Straightforward data translation,
no allocation.

### State serialisation

```rust
// Save: VST3 → binary chunk → DeviceState::Binary
fn save_state(&self) -> DeviceState {
    let mut stream = VstMemoryStream::new();  // pre-allocated
    unsafe { (*self.component).getState(&mut stream) };
    DeviceState::from_binary(&self.device_id, stream.into_bytes())
}

// Load: DeviceState::Binary → VST3 chunk
fn load_state(&mut self, state: &DeviceState) {
    if let Some(bytes) = state.as_binary() {
        let mut stream = VstMemoryStream::from_bytes(bytes);
        unsafe { (*self.component).setState(&mut stream) };
    }
}
```

State travels as `IBStream` bytes, not JSON — VST3 spec compliance. See
`../decisions/010-plugin-state-persistence.md`.

---

## AU Wrapper Responsibilities (macOS)

> **Status (2026-09-29):** AU and WASM are stubs compiled only with the Cargo
> features `au` / `wasm` (off by default). Without them, `DeviceFormat::Au` /
> `DeviceFormat::Wasm` do not exist. Nothing produces these formats yet, so no
> saved project depends on them, and the render path is unaffected (C-10).

The AU wrapper has the same responsibilities as VST3 but adapts to Apple's
`AudioUnit` C API. Key differences:

- State is retrieved/set via `AudioUnitGetProperty` / `AudioUnitSetProperty`
  with `kAudioUnitProperty_ClassInfo`.
- Parameters use `AudioUnitParameterID` (also integer) mapped to string keys.
- Buffer format is non-interleaved `AudioBufferList`.

The isolation principle and real-time safety rules are identical.

---

## CLAP Wrapper Responsibilities

CLAP plugins are loaded via the CLAP C ABI. `clap_entry.init` **must** be called
before any other entry point (skipping it is the documented root cause of the
Vital crash — see the plugin-GUI audit). Parameters use `clap_id` mapped to
string keys; audio uses `clap_process` with `clap_audio_buffer` arrays. CLAP
instruments and capability negotiation are covered by
`../decisions/013-clap-plugin-instruments.md` and
`../decisions/014-capability-driven-port-sizing.md`.

**Sidechain input ports (ADR-014, 2026-09-25).** `plugin_formats/clap/host.rs`
probes `clap.audio-ports` at scan time — a throwaway create → init → query →
destroy cycle, SEH-guarded — so `PluginManifest.num_sidechain_inputs` is the
plugin's real aux input count, not always `0`. `ClapWrapperNode` builds one
`ClapAudioBuffer` per input port (main at 0, aux at `1..`) and reports the real
total from `num_input_ports()`. VST3 `getBusInfo` introspection is untouched.

**Sidechain routing (ADR-014 update, 2026-09-25).** `PluginParams.sidechain_source`
names the track whose post-fader signal feeds a plugin's aux port 1.
`AudioGraph::from_project` resolves it in a second pass, once every track's
post-fader tap (`TrackGain` node) is known, so a source track can appear later
in the project than the plugin sidechaining from it. A self-reference or a
routing that would close a track-level cycle is dropped silently — the aux
port just keeps reading silence, same as a plugin with no declared aux bus —
rather than failing the whole compile; `UPDATE_EFFECT` additionally rejects a
self-reference outright (`patch_effect`, `engine_gateway/api.rs`). UI
(2026-10-14): `SidechainPicker.svelte`, shown in an expanded plugin effect whose
scanned `num_sidechain_inputs` is above 0 (exposed on `PluginInfo`), lists the
other tracks — minus any that would close a loop
(`frontend/routing/sidechain.ts`) — and sends `UPDATE_EFFECT { sidechain_source }`.
The picker is disabled for an isolated plugin: the sandbox bridge carries a single
input (`BridgedNode::num_input_ports() == 1`), so the aux port is not fed there.

**Multi-out (ADR-014 update, 2026-09-25).** `PluginInstance` reads the
plugin's `clap.audio-ports` outputs once, right after the first `init`
(main port first, then aux ports — `PluginInstance::output_ports`).
`ClapWrapperNode` and `ClapInstrument` share `ClapOutputBuses`
(`plugin_formats/clap/audio_ports.rs`), built in `prepare`: one
`clap_audio_buffer` per declared port, in CLAP index order, so the plugin now
gets every output it declared (it used to get exactly one). Graph output 0 is
the main bus (engine channel count, unchanged); outputs `1..` are aux buses at
their declared channel count, a mono one spread over both channels. Any port
without a graph buffer writes into a spare buffer, never a null pointer.
`PluginHost::plugin_outputs` / the `get_plugin_outputs` command list the aux
ports with their names; `AppTrack.input_source` routes one into a track (see
ADR-014). Sandboxed plugins and VST3 still expose one output.

**Parameters (ADR-021).** `plugin_formats/clap/params.rs` is shared by the
effect wrapper and the instrument: it reads `clap.params` at `prepare`, and
each block fills one time-sorted `ClapEventBuffer` with queued edits and
automation (`PARAM_VALUE`), modulation (`PARAM_MOD` on modulatable parameters;
`PARAM_VALUE` of `base + offset` on automatable-only ones — ADR-021 amendment)
and, for instruments, notes plus live keyboard controls (pitch bend, CC,
aftertouch) as raw `CLAP_EVENT_MIDI` on the main note port — ADR-022
amendment 2026-10-13; the note-port dialect is not checked, a plugin without
MIDI support ignores them. `get_plugin_params` exposes the parameter list to
the UI. The plugin's output events go to `observe_output_event`, which tracks
`PARAM_GESTURE_BEGIN/END` and the dragged value of value-modulated parameters
(everything else is still discarded).

**Unsaved-changes flag.** Edits made in a plugin window bypass `send_command`,
so the plugins report them: the host now answers `get_extension("clap.state")`
with `clap_host_state.mark_dirty`, and `get_extension("clap.thread-check")`.
`is_audio_thread` is a role, as CLAP defines it (the thread that processes, never
two at once): a thread holds it only inside `clap_ffi::audio_thread_scope()`,
a const-initialised thread-local (no allocation), which wraps every
`start_processing` / `process` / `stop_processing` — the wrappers' `process`,
the sandbox bridge's audio loop, and the `stop_processing` of a teardown or
reactivation run off the callback (last reference dropped, or FFI gate held, so
no `process` can overlap). Plugins that check threads (u-he, clap-helpers) no
longer report those calls. Every other thread counts as main, because
`clap.params` runs on the command thread and the editor on the Win32 thread.
Without the audio-thread bit, playing the plugin's keyboard while the editor is
open takes the message lock inside `process` and crackles. `init` runs before
any `clap.state` load. `start_processing` runs on the audio thread, on the first
block, not during graph prepare. `observe_output_event` treats a
`GESTURE_END` or a `PARAM_VALUE` that cannot be an echo of a host value as an
edit. Both set a global atomic in `clap_ffi` (`mark_plugin_state_edited`,
lock-free); `main.rs::watch_plugin_edits` polls it every 250 ms and marks the
project dirty. Loading, creating and saving a project clear it
(`mark_saved`). Verified with Vital: no false report on load, playback, value
modulation or save. `PluginInstance::save_state_blob` flushes
value-modulated parameters back to their base (`clap.params.flush`, under the
FFI gate) before `clap.state.save`.

**Per-instance host context (CL3, 2026-09-25).** `make_host` (`crates/clap_ffi`)
gives every plugin its own `ClapHostContext` behind `clap_host.host_data`,
owned and freed by that `ClapHost` (which outlives the plugin). It holds the
plugin's editor HWND (set by `open_clap_editor`, cleared on close) and the
`request_callback` coalescing flag, so two open CLAP editors no longer share
one global window slot. It also holds the plugin's pending `request_restart`
(see Latency Handling). `get_extension` also answers `clap.gui` (host side:
`request_resize` posts `CLAP_GUI_RESIZE_MESSAGE` to that HWND and the WndProc
resizes the container; `closed` posts `WM_CLOSE`; `request_show/hide` return
false because the editor is embedded) and `clap.log` (stderr, dropped on the
audio thread). All of these callbacks only post messages — nothing blocks,
whatever thread the plugin calls from.

**Transport.** Every in-process `clap_process` call (effect wrapper and
instrument) carries a `clap_event_transport` built on the stack by
`plugin_formats/clap/transport.rs::clap_transport_event`: tempo, position in
beats and seconds, bar start / bar number, time signature, playing flag. The
musical math lives in `TransportInfo::musical_position` (`audio_graph/node.rs`)
so the VST3 `ProcessContext` can reuse it. `steady_time` is a sample counter
advanced by the block size on every call (including a skipped block), so a
plugin can place GUI notes on a continuous clock. Not covered: loop points
(Echoes has no loop). The out-of-process bridge receives the same transport
event as the first event of every block (`BridgeRoundTrip::write_events`).

**Plugin sleep (2026-10-14).** The host stops calling `process()` on a CLAP
plugin that has nothing left to play, using the status `process()` returns and
the `clap.tail` extension. The rule is one pure type, `ProcessSleep`
(`crates/clap_ffi/src/process_sleep.rs`), shared by the in-process wrappers
(`ClapWrapperNode`, `ClapInstrument`, through `plugin_formats/clap/sleep.rs`)
and the bridge process (`ClapBlockProcessor`).

- _Falling asleep_ (after a `process` call): the block's output must be silent
  (every sample within 1e-6, −120 dBFS) **and** either the plugin answered
  `SLEEP`, or it answered `CONTINUE_IF_NOT_QUIET` / `TAIL`, declares
  `clap.tail`, and the input (all ports, sidechain included) and the event list
  have been quiet for at least the tail, counted in samples. `CONTINUE`,
  `ERROR` and unknown statuses never put it to sleep. Without `clap.tail` (or
  with a tail of `i32::MAX` samples or more, which the spec calls infinite)
  only `SLEEP` can; the tail is never guessed.
- _Asleep_: no FFI call at all; every output is zeroed (an effect too: its
  input is silent by construction) and `steady_time` keeps advancing.
- _Waking_ (checked every block while asleep): any incoming event (notes, MIDI,
  queued edits, modulation, automation), a non-silent input, the plugin's own
  `request_process`, a play/stop change, `reset`, a reactivation. A plugin that
  claims to sleep while still sounding is not followed (its output is not
  silent), and one that gets any event is woken, so it cannot stay mute. The
  CLAP contract makes `request_process` the plugin's way to wake itself (e.g.
  from its own window), so a plugin that skips it keeps sleeping until the
  next event. Active modulation or automation sends events every block, so a
  modulated plugin does not sleep.
- _Cost_: the input is scanned only when a decision needs it (while asleep, or
  while a tail is being counted), so the common `CONTINUE` plugin pays nothing.
- _Tail_: read in `prepare` under the instance FFI gate (like `clap.latency`),
  and again on the audio thread after the plugin's `clap_host_tail.changed`
  (`clap.tail.get` is callable from the audio thread). The host answers
  `clap.tail` in `host_get_extension`; `request_process` and `tail.changed`
  set lock-free flags on the plugin's `ClapHostContext`.
- _Determinism_ (C-2): the decision depends only on samples and sample counts,
  never on wall time, so offline and live sleep at the same blocks. For a real
  plugin whose silent output is not exactly zero but under 1e-6, sleeping
  replaces it by zero (below −120 dBFS, reproducible). The harness proves bit
  identity with the test plugin (`render_harness.rs::clap_plugins`).
- _Sandbox_ (ADR-015): the decision is taken inside the bridge process
  (`ClapBlockProcessor`); the engine still exchanges every block with the
  bridge, so the bounded wait, the offline wait and the protocol are
  unchanged. A sleeping bridge answers the block with silence.

Not done: VST3 (no equivalent wired); not yet tried with real plugins (Vital,
Surge XT, u-he) — only the test plugin (`crates/test_clap_plugin`: `Status`,
`Tail`, `Liar`, `Process calls` parameters and the `echoes.test.sleepy` plugin).

---

## Editor Windows — DPI (Windows)

The process is per-monitor DPI aware v2 (set by tao at startup), and Win32
plugin GUIs size themselves in physical pixels, so `plugin_system/gui.rs` tells
each plugin the monitor scale (`dpi / 96`): CLAP `clap.gui.set_scale` before
`set_parent`, VST3 `IPlugViewContentScaleSupport::setContentScaleFactor` after
`attached` (optional interface). The window then fits the size the plugin
reports, plus the drag strip scaled to the window's own DPI. `WM_DPICHANGED`
(window dragged to a monitor with another scale) repeats scale → size. A
plugin that reads the DPI itself ignores the host scale.

## Editor Windows — layout (Windows, 2026-09-26)

Both editor windows — in-process (`plugin_system/gui.rs`) and sandboxed
(`crates/plugin_host_bridge/src/editor.rs`) — share one layout:

- **Borderless** `WS_POPUP` container (no native title bar), with
  `WS_EX_TOOLWINDOW` so editors stay out of the taskbar / Alt+Tab, and Windows 11
  rounded corners requested via `DwmSetWindowAttribute`
  (`DWMWA_WINDOW_CORNER_PREFERENCE`; ignored on Windows 10).
  Editors are **not** owned by Echoes' main HWND: Win32 ownership keeps them
  above the owner (wrong when clicking the arrange), and a sandboxed editor
  owned by Echoes can deadlock `OpenEditor` (UI waits on the bridge while
  `CreateWindowExW` waits on the owner's message queue → control-channel
  timeout → "bridge control channel closed").
- An **8 px (logical) drag strip** on top, painted `#222229`
  (`$panel-background-color`). The container answers `WM_NCHITTEST` with
  `HTCAPTION`; only the strip can reach it, since the plugin covers the rest.
- The plugin is embedded in a **child "plugin area" window** below the strip
  (`plugin_area()` = the container's only child), not in the container itself:
  CLAP `set_parent` and VST3 `attached` receive that child. Resize messages and
  the `ClapHostContext` HWND still target the container, whose resize handler
  sizes both windows.
- Closing goes through Echoes' editor toggle (`post_close` / bridge `close`);
  there is no native close button.

A plugin that resizes itself (e.g. Vital's corner handle) sends
`request_resize` continuously; the host window follows within ~2 ms. Any lag
in the plugin's _own_ redraw during the drag is the plugin's rendering, not
the host. Such plugins may report `can_resize = false`, so the host does not
call `set_size` after `request_resize` (the CLAP spec makes it optional).

---

## Latency Handling

Plugins that introduce latency (look-ahead compressors, linear-phase EQs) must
report it:

```rust
fn latency_samples(&self) -> u32 {
    unsafe { (*self.processor).getLatencySamples() as u32 }
}
```

The graph compiler collects these values to compute **plugin delay compensation
(PDC)**: non-latency paths receive delay so all signals align at the master. See
`../decisions/006-plugin-capabilities-and-pdc.md`.

**Runtime latency changes (CLAP, 2026-09-25).** An active CLAP plugin may only
change its latency inside `activate`, so it calls `host->request_restart()`
(any thread, the audio thread included). The host sets a flag in the plugin's
`ClapHostContext` and a global atomic; `main.rs::watch_plugin_edits` takes it
(`clap_ffi::take_plugin_restart_requested`) every 250 ms and recompiles. On
that recompile the reused instance's `clap_ensure_active` sees the request and
deactivates + reactivates the plugin while holding the FFI gate, so the
outgoing graph's blocks pass through meanwhile (gate busy → the request is
re-armed for the next recompile). The wrapper then reads `clap.latency` again
and the compiler recomputes PDC. `stop_processing` runs on the command thread
inside an audio-thread scope (see the thread-check note above); the gate
guarantees no `process` runs concurrently. A sample-rate change reactivates
under the same gate (best effort: it proceeds if the gate stays busy). The host also answers
`get_extension("clap.latency")`: its `changed` does nothing, because the
latency is read after every activation.

Sandboxed plugins: the bridge returns `BLOCK_RESTART_REQUESTED` with the block,
`BridgedPlugin::request_restart` raises the same global flag, and on the
recompile `BridgedPlugin::activate` skips its activation cache; the bridge
reactivates the plugin (same hold handshake as a config change) and replies
with the new latency.

Not covered: VST3 `restartComponent(kLatencyChanged)` — the component handler
is still a no-op, so a VST3 latency change waits for the next re-instantiation.

---

## Opaque State Philosophy

Plugin state is treated as an opaque blob — the wrapper saves and restores it
without interpretation. The host never needs to understand VST3/AU/CLAP preset
format; migration across plugin versions is the plugin's responsibility.

---

## FFI Access Gate (single-threaded contract)

VST3/CLAP mandate single-threaded instance access. A reused instance (ADR-016)
can be touched by the audio thread (`process()`) and an off-RT caller at the same
time — during a recompile-while-playing, or a project save. A per-instance,
non-blocking gate (`PluginInstance::{rt_try_enter, offrt_try_acquire}`) serialises
them:

- The audio thread claims the gate around its `process()` FFI; if an off-RT caller
  holds it, the wrapper **bypasses** that block (input→output) and returns — it
  never waits, so there is no priority inversion. The CLAP instrument adapter
  (`ClapInstrument::process_midi`) takes the same gate since 2026-09-24 (it did
  not before, so a save could race a playing synth); a skipped block is silent
  and its notes are dropped.
- Off-RT callers (`getState`/`setState`, and the wrappers' `prepare`-time
  introspection — VST3 `getLatencySamples`, CLAP `clap.latency` + param
  enumeration) claim the gate before their FFI, with a short spin budget, and fall
  back to a cached value on timeout.

Rationale and the state-direction specifics live in
`../decisions/010-plugin-state-persistence.md`.

---

## Crash Containment

VST3, AU, and CLAP plugins run in-process by default. A crashing plugin takes
down the engine. Best-effort mitigations (current):

1. Validate all plugin-provided pointers before dereferencing.
2. Wrap `process()` calls in platform signal handlers where feasible.

**Out-of-process hosting** is the durable mitigation, opt-in: per CLAP effect
("Isoler" in the effect chain, `PluginParams.sandboxed`), per plugin type
(right-click on an effect or instrument → "Toujours isoler", an app
preference), or for every CLAP plugin through the app preference _Isolation
des plugins_ (Aucun / Choisis / Tous). A track instrument has no per-placement
flag: it follows the last two only (`BridgedInstrument`).
All three reach the engine through `PluginHost::set_isolation`. The plugin then runs
in a `plugin_host_bridge` process behind the same `Arc<PluginInstance>`
(`NativeHandle::Bridged`): state, parameters, automation, modulation, latency
and the editor (a window of the bridge process) all work; a crash or hang only
silences that device, which is reported as crashed and restarted by the next
recompile. Design and RT strategy: `../decisions/015-plugin-sandbox.md`
(amendments 2026-09-25). Not sandboxable yet: VST3.
A crashed plugin (sandbox condemned, or in-process crash caught by the SEH
guard) is shown red in the effect chain with « Recharger », which rebuilds it
from its last saved state. In-process plugins keep the "best effort" guarantee
above; when one kills
Echoes, the next start names it and offers to isolate it (crash marker,
ADR-015).

---

## Wrapper Checklist

Before merging any wrapper `process()` implementation:

| Rule                                      | Check                                                                    |
| ----------------------------------------- | ------------------------------------------------------------------------ |
| No `Vec::push` / `Box::new`               | all pointer arrays allocated in `prepare()`                              |
| No `setParamNormalized` from audio thread | use `ProcessData.inputParameterChanges` (VST3) / host event queue (CLAP) |
| All transport from `TransportInfo`        | never from global state                                                  |
| `unsafe` scoped to the FFI call only      | no wider `unsafe` blocks                                                 |
| Latency reported via `latency_samples()`  | required for PDC                                                         |
| State via native chunk bytes, not JSON    | format spec compliance                                                   |

---

## Further Reading

- `plugin-system.md` — how wrappers fit into the abstraction stack
- `../threading/realtime-safety.md` — audio-thread prohibitions the wrapper must obey
- `../decisions/003-plugin-wrapper-strategy.md` — why wrappers instead of deep integration
- `../decisions/015-plugin-sandbox.md` — out-of-process sandbox design
- `../audits/plugin-gui-audit-2026-05-31.md` — CLAP/VST3 editor hosting issues
