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

### Lifecycle and edit controller (2026-10-21)

- **Host context.** `IComponent::initialize` and a separate controller's
  `initialize` receive the static `IHostApplication` of
  `plugin_formats/vst3/host_context.rs` (name "Echoes Studio"; `createInstance`
  makes an `IMessage` or an `IAttributeList`, see "Host `IInfoListener` /
  `IMessage`").
- **Activation order** (`PluginInstance::vst3_ensure_active`):
  `setBusArrangements` (every audio bus named, see "VST3 multi-out") →
  `activateBus` (audio in when the plugin has one, its auxiliary inputs — a
  sidechain, see "VST3 sidechain" —, audio out and the auxiliary outputs the
  graph reads, event in when the plugin has one) → `setupProcessing`
  → `setActive(1)` → `setProcessing(1)`, as the spec requires. A config change,
  a `restartComponent`, or a change of the auxiliary outputs read on a live
  instance deactivates and reactivates it under the FFI gate, like CLAP.
- **Edit controller.** It lives with the instance, not with the editor window.
  `PluginHost::instantiate_vst3` builds it right after the component
  (`ffi::create_controller`):
  - **Single-component plugin:** the controller is found by QI on the component.
  - **Dual-component plugin:** it is created through the factory, initialised,
    synced with `setComponentState`, and connected through `IConnectionPoint`.
  - It receives the instance's `IComponentHandler` (see the next section).
  - Failure leaves it `None`. The plugin still plays, but has no parameters and
    no editor.
  - `load_state_blob` passes the restored blob to a separate controller.
  - The editor borrows the controller (`PluginInstance::vst3_controller`).
  - `PluginInstance::Drop` releases it before the component's `terminate`.
- **Interface IIDs** are checked against the SDK's `DECLARE_CLASS_IID` form by
  `interface_iids_match_the_sdk` (`vst3/ffi.rs`). Four IIDs were wrong before
  2026-10-21 (`IEditController`, `IConnectionPoint`, `IHostApplication`,
  `IComponentHandler`; `IPlugFrame` too). With these wrong values, no controller
  was ever reached, so the VST3 editor and dual-component wiring could not work.
  All values were verified byte for byte in the TDR Nova binary.

### Plugin window edits (`IComponentHandler`, 2026-10-21)

A VST3 controller does not talk to its processor: the host forwards. The handler
(`vst3/component_handler.rs`, one per instance, owned by `NativeHandle::Vst3`)
is lock-free and keeps its state in `EditState`:

- **`performEdit`** pushes the value into a second SPSC ring, producer = the
  plugin window thread, consumer = the audio thread. `VstWrapperNode::process`
  drains it into `inputParameterChanges` right after the queue of host edits.
  It also moves the base of a value-modulated parameter and marks the project
  unsaved (`mark_plugin_state_edited`, the flag CLAP uses).
- **`beginEdit` / `endEdit`** open and close a gesture (up to 8 at once). While
  a parameter is being dragged its modulation is paused. `endEdit` marks the
  project unsaved.
- **`restartComponent`:**
  - `kLatencyChanged`, `kIoChanged` and `kReloadComponent` flag the instance
    for reactivation and request a recompile (`mark_plugin_restart_requested`,
    polled by `watch_plugin_edits`). `vst3_ensure_active` then reactivates the
    plugin under the FFI gate, and `prepare` reads the new latency. A plugin busy
    in a block is retried at the next recompile.
  - `kParamValuesChanged` and `kParamTitlesChanged` report a params rescan
    (`TEXT`, plus `INFO` for titles), so the UI refetches the lists and texts.
- **`IComponentHandler2`** (2026-10-23). The same COM object answers it: the
  handler holds two vtable pointers, the second is what a query returns for
  `IComponentHandler2`, and a query from either pointer gives back the same
  `FUnknown`. The IID was checked against the SDK and found in the TDR Nova binary.
  - `setDirty(true)` marks the project unsaved; `setDirty(false)` does nothing
    (the flag belongs to the project).
  - `startGroupEdit` / `finishGroupEdit` answer OK without state: window edits
    already reach the processor in order, at offset 0 of one block.
  - `requestOpenEditor` answers `kResultFalse`: Echoes does not open a plugin
    window on the plugin's own request (it would need an app handle and a new
    command, and the SDK's use case is a plugin asking while a project loads).
- Not done yet:
  - `IComponentHandler3` (native context menus, would be new UI),
    `IUnitHandler`, `IComponentHandlerBusActivation`, `IProgress`.
  - `performEdit` is expected on the window thread; a plugin that calls it from
    several threads breaks the single-producer rule of the ring.

### The controller follows the processor (ADR-008 amendment, 2026-10-23)

A plugin does not write back what the host sent it: for the knobs of an open
window to follow automation, the host calls `setParamNormalized` itself. Two
sources reach the controller through one channel, `ParamMirror`
(`vst3/param_mirror.rs`, in `EditState`):

- **Host values**, published by `fill_param_changes` (`vst3/param_delivery.rs`):
  the host queue (`drain_params`: the Echoes UI, programs), the last modulation
  point of the block and the last automation point. The window's own edits
  (`drain_edits`) are not published.
- **The plugin's own changes**, published after `process` by
  `publish_output_changes`: `ProcessData.outputParameterChanges` is a second
  preallocated `HostParameterChanges` in the effect and in the instrument, emptied
  each block. They do not mark the project unsaved and do not move a modulation
  base. A plugin that writes more than 128 parameters per block gets a null queue
  beyond that.

The audio thread only writes atomics, and only while an editor is open. The
window's `WM_TIMER` (the VST3 `on_timer` of `plugin_system/gui.rs`) calls
`PluginInstance::mirror_to_controller`, which applies the latest value of each
parameter with `setParamNormalized` on the window thread, without the FFI gate,
under the crash marker `Ui`. It skips a parameter being dragged (open gesture)
and a crashed plugin, and while it applies a value it ignores the controller's own
`beginEdit` / `performEdit` / `endEdit`, so a value given to the controller never
reaches the processor.

`open_vst3_editor` takes a `ParamMirrorWatch` (`PluginInstance::watch_vst3_params`)
before the view is created: the table starts filling, and the controller first
receives the processor's state (`sync_vst3_controller_with_processor`, the path
programs use) so the window opens on current values. The guard drops with the
window, also when the opening fails. That sync holds the FFI gate for the length
of a `getState` / `setComponentState`, so the plugin may be bypassed for a block
once per opening.

Tests: `vst3/param_mirror.rs` (table), `plugin_system/vst3_mirror_tests.rs`
(hand-made plugin: queue, modulation, automation, plugin outputs for the effect
and the instrument, drag, echo of a faulty controller, closed window), and
`vst3_real_controller_follows_the_values_the_host_sends` on a real plugin. Not
verified: a real plugin that writes `outputParameterChanges` (TDR Nova may not;
Surge XT would tell), and the real `WM_TIMER` (the test calls
`mirror_to_controller` from the test thread).

### Parameters (ADR-021)

VST3 parameters are addressed by their `ParamID` (`native_id: u32`; lanes name
them `plugin_param:<id>`, `parse_plugin_param_name`). Values travel
**normalised** end to end:

- **Listing.** `PluginHost::plugin_params` reads the controller under the FFI
  gate (`vst3/params.rs::read_vst3_params`).
  - Range `0..1`, `default = defaultNormalizedValue`.
  - `automatable = kCanAutomate && !kIsReadOnly`, `hidden = kIsHidden`.
  - `modulatable = false`: VST3 has no host modulation, so modulation moves the
    value around a base.
  - `module` is empty; units are read only for the program lists of the preset
    picker (see "Host VST3 presets").
- **Text.** `PluginInstance::param_text` calls `getParamStringByValue` for a
  normalised value.
- **Value.** `PluginInstance::param_value` reads `getParamNormalized`.
- **Delivery.** UI edits (`push_plugin_param` → SPSC `ParamChange`), automation
  and modulation are delivered in `process()` via
  `ProcessData.inputParameterChanges`, the VST3 sample-accurate mechanism.
- **Modulation base.** A newly modulated parameter starts from the plugin's
  current value. When modulation is removed, the base is pushed back
  (`VstWrapperNode::track_value_modulation`).

**Never** call `IEditController::setParamNormalized` from the audio thread. The
controller is brought up to date by the window thread (see "The controller follows
the processor").

### Transport sync

`vst3/transport.rs::vst3_process_context` builds a `ProcessContext` on the stack
every block from the `TransportInfo` passed by the scheduler (same
`musical_position()` as the CLAP event) and hands it to the plugin through
`ProcessData.processContext`. No allocation, no global state.

| Field                                     | Value                                               |
| ----------------------------------------- | --------------------------------------------------- |
| `state`                                   | `kPlaying` when playing, plus the valid flags below |
| `sampleRate`, `projectTimeSamples`        | the engine's                                        |
| `projectTimeMusic`, `barPositionMusic`    | beats, start of the bar in beats                    |
| `tempo`, `timeSigNumerator`/`Denominator` | the engine's                                        |

Valid flags set: `kProjectTimeMusicValid`, `kBarPositionValid`, `kTempoValid`,
`kTimeSigValid`. Not provided (flag off): cycle, system time, continuous time,
SMPTE, clock, chord. `IProcessContextRequirements` is not queried, so a plugin
gets all of the above whether it asks or not. The layout (112 bytes) and the
flag values were checked against Steinberg's `ivstprocesscontext.h`.

### Instruments (ADR-013 amendment, 2026-10-21)

A VST3 instrument is a `VstInstrument` (`vst3/instrument.rs`) behind
`InstrumentDevice`, built by `PluginHost::instantiate_instrument` from the same
`create_vst3_instance` as the effect wrapper. It is reused across recompiles
(ADR-016) and always runs in-process (the sandbox is CLAP only).

- **Detection.** The scan reads `PClassInfo2.subCategories` through
  `IPluginFactory2`: the first `|` token `Instrument` makes an instrument
  (`is_instrument_subcategory`; `Fx|Instrument` is an effect). Without
  `IPluginFactory2` the name guess of `classify_instrument` is used.
- **Buses.** `vst3_ensure_active` asks `getBusCount`. No audio input →
  `setBusArrangements(null, 0, outputs…)` and no input bus; an event input bus is
  activated. The main output is stereo; further output buses: "VST3 multi-out".
- **Events.** Each block, `HostEventList` (512 events, `IEventList`) is reset and
  filled: pending releases at offset 0, then the block's `MidiEventList` as
  `kNoteOnEvent` / `kNoteOffEvent` / `kPolyPressureEvent`, and passed as
  `ProcessData.inputEvents` (`numInputs = 0`). The `Event` layout (48 bytes) and the
  event type values were checked against `ivstevents.h`.
- **Notes never hang.** `SoundingNotes` counts what the plugin received. A NoteOff
  lost to a busy FFI gate or a full list, and every sounding note after `reset`, is
  sent at offset 0 of the next block.
- **MIDI controllers.** Pitch bend, CC and channel aftertouch go through the
  controller's `IMidiMapping` (`vst3/midi_mapping.rs`) and reach the plugin as
  parameter changes at the control's sample offset; an unmapped controller is
  dropped. The table is read in `prepare` under the FFI gate and again after
  `restartComponent(kMidiCCAssignmentChanged)` (a recompile; no reactivation).
- **Parameters.** Queued edits, plugin-window edits, modulation and automation use
  the same delivery as the effect (`vst3/param_delivery.rs`).
- **Latency (2026-10-23).** `VstInstrument::prepare` reads `getLatencySamples` after
  `vst3_ensure_active` (`PluginInstance::vst3_latency_samples`, FFI gate, the same
  read as the effect) and `InstrumentDevice::latency_samples()` hands it to the
  compiler through `InstrumentNode`, so the PDC aligns an instrument track like an
  effect chain (see "Latency Handling"). Built-in and CLAP instruments report 0.
- Not done: note expressions, `outputEvents`.
  `outputParameterChanges` are read (see "The controller follows the processor").
  Extra output buses: see "VST3 multi-out".

### VST3 multi-out (ADR-014 amendment, 2026-10-23)

A VST3 plugin (effect or instrument) with several audio output buses feeds the
same routing as a CLAP multi-out plugin: `AppTrack.input_source`,
`SET_TRACK_INPUT`, the left-panel "Entrée" picker, `get_plugin_outputs`. No
project, command or event change. In-process only (the sandbox is CLAP only).

- **Reading** (`vst3/buses.rs`). At every activation `vst3_ensure_active` reads
  the output buses (`getBusCount`, then `getBusInfo` after `setBusArrangements`)
  into `PluginInstance::vst3_output_buses`, so a `restartComponent(kIoChanged)`
  is followed by the reactivation it already requests. Not at scan time: the
  scan only counts the auxiliary inputs of effects (see "VST3 sidechain"), and
  instantiating every instrument at startup would load their samples. At most 32 buses and 64 channels per bus are read; reading
  stops at the first bus the plugin will not describe. `BusInfo` (276 bytes) was
  checked against `ivstcomponent.h`. A bus flagged `kIsControlVoltage` is never
  offered nor opened.
- **Port ↔ bus.** Graph output port `p` is output bus `p`; the main bus is port 0
  and is always open.
- **Arrangements.** `setBusArrangements` names every audio bus, inputs and
  outputs, as the spec requires. The main bus of each direction is asked for
  stereo (unchanged); any other bus is offered the arrangement it already has
  (`getBusArrangement`), so a mono or surround bus is not forced. The plugin may
  keep its own layout (it answers `kResultFalse`): the buses are read again after,
  so its answer wins. Auxiliary input buses (a sidechain) are named like the
  others and opened, see "VST3 sidechain".
- **Which buses open.** What a plugin does with a closed output bus is its own
  business: some mix that sound into their main bus (the Steinberg convention for
  multi-out instruments), so opening every bus would take sound out of the main
  bus while nobody reads it; others (Surge XT, measured) keep the main bus whole
  and send a copy to the bus once it is open. Opening a bus only when an edge of
  the graph reads its port is safe for both, and costs the plugin nothing for a
  bus nobody hears:
  `GraphCompiler::demanded_plugin_outputs` collects the ports ≥ 1 read from each
  plugin placement (after cycles are cut), `PluginHost::set_demanded_outputs`
  gives them to the instances (live ones at once, new ones when created), and
  `vst3_ensure_active` compares them with the buses opened at the last
  activation. A change reactivates through the existing FFI-gated branch (a short
  gap, like a latency change); a busy gate re-arms the request for the next
  recompile. A bus stays open across a reactivation, so one no longer read is
  closed explicitly. Nothing routed ⇒ one stereo main bus, `ProcessData` as
  before. So, depending on the plugin, a track taking an aux output either takes
  that sound out of the main bus or only receives a copy of it.
- **Process** (`Vst3OutputBuses`, shared by `VstWrapperNode` and `VstInstrument`).
  Built in `prepare`: one `AudioBusBuffers` per bus up to the last open one, each
  with a pointer array of its channel count. A closed bus between two open ones
  points at a spare buffer (never null). The main bus has the engine's channel
  count; an auxiliary bus its declared one, its first channels on the graph
  buffer and the extra ones (e.g. a 5.1 bus) on the spare buffer, a mono bus
  spread over both channels after `process`. `num_output_ports()` is the number
  of declared buses, which widens the plan steps (graph-compiler step 7).
  `bind` / `finish` only write pointers and copy samples.
- **Listing.** `PluginHost::plugin_outputs` offers the buses ≥ 1 of a loaded VST3
  (name, channels), once it has been activated.
- **Not done:** `kDefaultActive` for outputs, a user-chosen activation,
  `IComponentHandlerBusActivation`.

Tests: `vst3/buses.rs` (layout, arrangements, activation, tables) and
`plugin_system/vst3_output_bus_tests.rs` (effect and instrument rendering on the
hand-made plugin `vst3/fake_plugin.rs`, whose buses are configurable);
`plugin_system/host.rs` (`vst3_demanded_outputs`: demand, listing, and a compiled
project whose second track hears the first track's aux bus);
`audio_graph/graph.rs::routing_tests` (the demand); `ffi_lifecycle_tests.rs`
(`vst3_real_plugins_report_their_output_buses` on TDR Nova: one stereo output,
two audio inputs; and on Surge XT: three stereo outputs "Output", "Scene A",
"Scene B", one input; `vst3_real_instrument_opens_an_aux_output_on_demand` on
Surge XT, 2026-10-23: with only the main bus open "Scene A" is silent, once opened
it carries the note — peak 0.41 — while the main bus keeps its sound, a copy rather
than a move; "Scene B" stays silent on the default patch). Surge XT lives in a
vendor folder (`Surge Synth Team/Surge XT.vst3/Contents/x86_64-win/`), deeper than
the harness's default search: point `ECHOES_TEST_VST3` at the inner `.vst3`.

### VST3 sidechain (ADR-014 amendment, 2026-10-23)

An effect with an auxiliary audio input bus receives the signal of another track
through the same routing as a CLAP effect: `PluginParams.sidechain_source`,
`UPDATE_EFFECT`, and `SidechainPicker.svelte` (shown when `num_sidechain_inputs`
is above 0, whatever the format). No project, command, event or frontend change.
In-process only (the sandbox is CLAP only).

- **Scan** (`vst3/host.rs::probe_aux_input_count`). For each effect class
  (instruments are not probed, as for CLAP) the scan creates a throwaway component
  in the library it already loaded (`instantiate_processor`), reads its auxiliary
  inputs (`vst3/buses.rs::read_aux_inputs`), then `terminate` and `release`, all in
  one `crash_guard` call. A plugin that fails or crashes counts as having none. An
  auxiliary input is an input bus after the main one, of type `kAux`, not flagged
  control voltage, and consecutive from bus 1 (the SDK puts every `kMain` bus
  first), so input port `p` is input bus `p`. The count fills
  `PluginManifest.num_sidechain_inputs`, from which the compiler sizes the node's
  inputs. This is the one place a scan creates a component: startup is a little
  slower for each effect.
- **Activation.** `vst3_ensure_active` opens every auxiliary input bus after
  `setBusArrangements`, which already named it with the arrangement it had (TDR
  Nova's sidechain is mono), then reads their channel counts again
  (`PluginInstance::vst3_aux_inputs`). Unlike an auxiliary output, it is always
  open: nothing depends on whether a track is routed to it, and choosing a source
  needs no reactivation.
- **Process** (`Vst3InputBuses`, built in `prepare`, used by `VstWrapperNode`; an
  instrument has no audio input). `ProcessData.inputs` is the table `[main, aux…]`
  and `numInputs` its length, so a plugin without an auxiliary input is called as
  before (one bus, flags unchanged).
  - A bus nothing feeds gets zeros with every channel's silence flag set.
  - A fed bus gets its source's channels. If the plugin wants one channel and the
    source is stereo, it gets the average `(L+R)/2` from a preallocated mix; a bus
    wider than the engine gets zeros on its extra channels.
  - A silence flag is set only on a channel that is exactly zero over the block
    (never on a non-zero one), and an auxiliary bus never has a null channel
    pointer. `bind` only writes pointers, the mix and the flags.
  - The node reports `1 + n` input ports from the instance, while the compiler
    sizes it from the scan: a bus the graph has no port for is still handed over,
    fed with zeros.
- **One source.** `sidechain_source` feeds port 1, the first auxiliary bus; further
  ones receive silence (the same limit as CLAP).
- **Latency.** Nothing specific: `compute_latency_compensation` aligns every
  converging input port, port 1 included (ADR-006), and the plugin's own latency
  comes from `getLatencySamples`.
- **Not done:** a source per auxiliary bus, a bus added by a later
  `restartComponent(kIoChanged)` is opened but the node keeps the port count of the
  scan, `IAudioPresentationLatency`.

Tests: `vst3/buses.rs` (reading the auxiliary inputs; `Vst3InputBuses`: mono mix,
wide bus, flags, unfed bus, short block), `plugin_system/vst3_sidechain_tests.rs`
(the hand-made plugin records every input bus it receives: signal, silence, no port,
no auxiliary input, a wrapper rebuilt on a live instance), `audio_graph/compiler.rs`
(sidechain alignment), `ffi_lifecycle_tests.rs` on TDR Nova (2026-10-23):
`vst3_real_plugins_report_their_input_buses` (bus 0 "Input" `kMain` stereo, bus 1
"Sidechain" `kAux` mono, both `kDefaultActive`), `vst3_scan_counts_the_sidechain_inputs_of_real_effects`
(1) and `vst3_real_sidechain_effect_processes_with_its_aux_bus_fed` (band 1 dynamic on
"Ext SC": the output energy over 64 blocks is 4057 with a silent auxiliary bus and 98
with a loud one, about −16 dB, so the detector hears the bus). Not hand-tested in the
app.

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

**Modulated bases (ADR-010 amendment, 2026-10-23).** A parameter modulated by
value holds `base + offset` in the processor, so `save_state_blob` first sends the
bases (`EditState::followed_bases`) in a zero-sample `process`
(`param_delivery::flush_param_bases`, SEH-guarded, FFI gate held by the caller),
as CLAP does with `clap.params.flush`. Only when a parameter is modulated by value
and the plugin is active. On TDR Nova a save while modulated held the modulated
value; with the flush it holds the base.

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
total from `num_input_ports()`. VST3 has its own path (see "VST3 sidechain").

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
ADR-014). Sandboxed plugins still expose one output; VST3 has its own path
(see "VST3 multi-out").

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
(see Latency Handling), the info of its track (`clap.track-info`, see Host
`clap.track-info`) and its timers (`clap.timer-support`, see Host
`clap.timer-support`). `get_extension` also answers `clap.gui` (host side:
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

Not done: VST3 (no plugin-driven sleep exists there; see "VST3 tail and processing
mode"); not yet tried with real plugins (Vital,
Surge XT, u-he) — only the test plugin (`crates/test_clap_plugin`: `Status`,
`Tail`, `Liar`, `Process calls` parameters and the `echoes.test.sleepy` plugin).

**Host `clap.params` (2026-10-14).** `host_get_extension` answers `clap.params`
(`clap_ffi::ClapHostParams`); every callback is lock-free and callable from any
thread.

- _`request_flush`_ sets the same flag as `request_process`. A plugin that is
  processing gets its flush through the next `process`; a sleeping one is woken
  for it, so a plugin that changes a parameter from its own window (and asks for
  a flush) is heard again. A plugin that is not active has no flush the host
  could schedule.
- _`rescan(flags)`_ ORs the `CLAP_PARAM_RESCAN_*` flags into a process-wide atomic
  (`clap_ffi::mark_plugin_params_rescan`). `main.rs::watch_plugin_edits` takes it
  every 250 ms (`notify_params_rescan`): `TEXT` / `INFO` / `ALL` emit the
  `plugin-params-changed` event, after which the UI refetches the parameter lists
  of the placements it knows and forgets its cached value texts
  (`src/stores/pluginParams.ts`); `ALL` also recompiles, so the wrappers re-read
  the parameter ranges in `prepare`. `VALUES` is recorded but nothing mirrors the
  plugin's values yet, so it triggers nothing — a plugin that loads a preset
  reports it through `clap.state.mark_dirty`.
- _`clear`_ is **accepted and ignored**: removing a parameter's automation or
  modulation would be a project change made by a plugin callback, outside the
  command/event path (ADR-005). Doing it needs a decision.
- _Value text._ `get_plugin_param_text` calls the plugin's `value_to_text` under
  the instance FFI gate (command thread, never the audio thread) and the
  automation lane shows the result (`-6.0 dB`) instead of the number; without
  text the previous display is kept. The UI asks at most every 100 ms and caches
  answers, because a call that holds the gate makes a block in flight bypass the
  plugin. `text_to_value` still has no caller.
- _Sandbox._ The bridge process hosts the same extension. Its rescan flags travel
  in the block flags (`BLOCK_PARAMS_RESCAN_*`, bits 8–11 of the existing field;
  the shared-memory layout and `BRIDGE_PROTOCOL_VERSION` are unchanged), and
  `request_flush` wakes the plugin inside `ClapBlockProcessor`. `value_to_text`
  is a new control op (`Op::ParamText`, empty reply = no text).

Tests: `crates/clap_ffi` (extension table), `crates/plugin_host_bridge/tests/bridge.rs`
(text, rescan flags, flush wake through the real bridge), `plugin_system/sandbox_tests.rs`
(the same through `PluginHost`, in-process and sandboxed). The test plugin gained
`Rescan` and `Request flush` parameters and renders `Gain` in dB.

**Host `clap.track-info` (2026-10-14).** The host answers `clap.track-info/1` (and
the draft id `clap.track-info.draft/1`, which the spec calls fully compatible):
a plugin reads the name and color of its track, and the host calls the plugin's
`clap_plugin_track_info.changed` when either changes. Layouts and ids were
checked against `clap/ext/track-info.h` (`clap_track_info`: `flags:u64`,
`name[256]`, `clap_color {alpha, red, green, blue}`, `audio_channel_count`,
`audio_port_type`).

- _Source._ The project, nothing new: `plugin_system/track_info.rs` derives, for
  every placement key (the effect id, or `instrument:{track_id}`), the name and
  color of the track the placement sits on. No project state, command, event or
  `.echoes` change.
- _What is reported._ Only `HAS_TRACK_NAME` and `HAS_TRACK_COLOR`. A flag the host
  cannot honestly set stays 0: no audio channels or port type (so no
  `HAS_AUDIO_CHANNEL`), and never `IS_FOR_RETURN_TRACK` / `IS_FOR_BUS` /
  `IS_FOR_MASTER`, because the project has no return, bus or master track — a
  plugin placed there would need a new project concept first.
- _Track without a color._ The track's `color` is `None` until the user picks
  one (the UI derives a default from the track id, which the host does not
  know), so `HAS_TRACK_COLOR` is left unset and the color is zeroed. A color is
  sent opaque (`alpha` 255) from the `#rrggbb` string; anything else counts as
  none. An empty name leaves `HAS_TRACK_NAME` unset; a name longer than 255 bytes
  is cut at a character boundary.
- _`get` and threads._ `clap_host_track_info.get` is `[main-thread]` but reachable
  from any thread through `get_extension`, so the info lives in the plugin's
  `ClapHostContext` as a lock-free sequence lock (`TrackInfoCell`,
  `crates/clap_ffi/src/track_info.rs`): readers never wait and never allocate
  (after 8 tries against a write in progress they report no info), writers are
  off-RT threads. No mutex is taken on any thread a plugin may call from. `get`
  answers `false` for a placement the host never described (e.g. outside a
  project) and `true` with flags 0 for a track with neither name nor color.
- _Flow._ `PluginHost::sync_track_info(project)` runs at the start of every
  compile (`AudioGraphCompiler::compile_scheduler` — live, offline and the C-2
  harness alike) and after an `UPDATE_TRACK` that carries a name or color, which
  does not recompile (`engine_gateway/api.rs::sync_plugin_track_info`; undo, redo
  and project load recompile, so they go through the first path). A new
  instance is created already knowing its track, so it reads it in `init`; a live
  one is told only when the info really differs, so a recompile or an unrelated
  edit calls nothing.
- _`changed()`._ An outgoing FFI call like the other off-RT ones: command thread,
  under the instance FFI gate (`offrt_try_acquire`, which also marks the crash
  marker), no `audio_thread_scope` — it is a main-thread call, and
  `clap.thread-check` reports it as such. A crashed plugin (`has_crashed`) is not
  called. If a block holds the gate past the 3 ms budget the notification stays
  pending on the instance and a process-wide flag (`take_undelivered_track_info`)
  makes `main.rs::watch_plugin_edits` retry it within 250 ms.
- _Sandbox._ The bridge hosts the same extension. The track at creation travels
  as a launch argument (`--track-info <hex>`), because the bridge runs `init`
  before it answers any request; later changes use a new control op,
  `Op::TrackInfo` (the encoded `HostTrackInfo`), which the bridge's main thread
  applies and follows with `changed()`. The engine remembers what it last sent
  and skips a dead bridge. A failed request is retried by the next sync. The
  shared-memory layout and `BRIDGE_PROTOCOL_VERSION` are unchanged.
- _Limits._ VST3 has its own (`IInfoListener`, below). A plugin that calls main-thread
  functions from its own GUI thread while Echoes calls `changed()` from the command
  thread is not serialised (same exposure as `clap.params`). Not yet tried with
  real plugins.

Tests: `crates/clap_ffi` (extension table under both ids, struct layout, flags,
no torn read under a concurrent writer, `changed` under the draft id),
`plugin_system/track_info.rs` (derivation from the project),
`plugin_system/sandbox_tests.rs` and `crates/plugin_host_bridge/tests/bridge.rs`
(name, color, no color, no track, rename → one `changed`, same info → none,
instrument, busy plugin retried, crashed plugin skipped, in process and
sandboxed), `audio_engine/render_harness.rs::clap_plugins` (the plugins read their
track through the real compile, and the golden hash is unchanged). The test
plugin gained seven read-only parameters (ids 12–18, `Track flags` … `Track info
audio calls`), 19 in all.

**Host `IInfoListener` / `IMessage` (2026-10-21).** Step 5 of the VST3 catch-up: a
VST3 edit controller is told the name and color of its track, as a CLAP plugin
is through `clap.track-info`. Interface ids, method order, keys and result codes
were checked against `ivstmessage.h`, `ivstattributes.h`, `ivstchannelcontextinfo.h`,
`ivsthostapplication.h` and `funknown.h` (steinbergmedia/vst3_pluginterfaces) and
against the SDK's reference host (`vst3_public_sdk`, `hostclasses.cpp`). The three
new ids (`IMessage`, `IAttributeList`, `IInfoListener`) are in
`interface_iids_match_the_sdk` and, in COM byte order, in the TDR Nova binary.

- _`createInstance`._ `IHostApplication::createInstance` (`vst3/host_context.rs`)
  makes an `IMessage` or an `IAttributeList` only when `cid` and `iid` are the same
  interface (the SDK's `allocateMessage` passes `IMessage::iid` twice); anything
  else answers `kResultFalse` with a null object, a null output pointer answers
  `kInvalidArgument`. The host context itself stays a static with dummy counts.
- _Objects._ `HostAttributeList` (`vst3/attribute_list.rs`) and `HostMessage`
  (`vst3/message.rs`) are heap objects with a real reference count
  (`vst3/com_object.rs`): a plugin may keep one past the call, `queryInterface`
  adds a reference, the last `release` frees. A message owns its list and
  `getAttributes` returns it without `addRef`, like the SDK; a list the plugin
  kept outlives the message. Limits are host policy, not the SDK's: 32 entries
  per list, key <= 64 bytes, string <= 1024 UTF-16 units, binary <= 64 KiB, message
  id cut at 256 bytes. A full list answers `kOutOfMemory`; an oversized value or
  bad argument `kInvalidArgument`; a missing key or another type `kResultFalse`.
  `getString` takes its size in **bytes**, cuts a too-long string and always ends
  it with a NUL. Values are copied in. The list is behind a mutex: these objects
  live on the plugin's non-real-time threads (the spec calls them from the UI
  thread) and the host never touches one from the audio thread.
- _What is sent._ `vst3/channel_context.rs` builds the list from `HostTrackInfo`
  (the CLAP type, from `placement_track_infos`, so the project stays the source):
  `kChannelNameKey` (`"channel name"`, UTF-16) with `kChannelNameLengthKey`
  (UTF-16 units, terminator excluded: the header says only "number of
  characters"), and `kChannelColorKey` (`"channel color"`, an opaque ARGB
  `ColorSpec` stored with `setInt`). Nothing else: no index, UID, plug-in
  location or image. A track without a color announces none and an empty name is
  left out, as in `clap.track-info`; no default color is invented.
- _Flow._ Unchanged from `clap.track-info`: `sync_track_info` -> `set_track_info`
  -> `notify_track_info_changed`. For a VST3 controller `PluginInstance` keeps the
  last track it handed over (`vst3_track_info`, starting empty, so a plugin never
  hears of an empty track, and one that loses name and color hears an empty
  list). `create_vst3_instance` gives a new instance its track right after the
  controller exists (the spec allows `setChannelContextInfos` from
  `Initialized`); a live instance is told only when the info differs. The
  delivery is `queryInterface(IInfoListener)` on the controller then
  `setChannelContextInfos`, off the audio thread, under the instance FFI gate
  (`offrt_try_acquire`, 3 ms), with the CLAP retry (`take_undelivered_track_info`
  -> `watch_plugin_edits` within 250 ms). The latest info wins after a busy
  period. A crashed plugin is not called; a controller without `IInfoListener` is
  left alone. No project state, command, event or `.echoes` change; nothing runs
  on the audio thread.
- _Messages between the two halves._ The host connects a dual-component plugin's
  component and controller directly (`IConnectionPoint::connect` on both), so
  their messages never pass through the host; there is nothing to relay and the
  host implements no `notify`. What these plugins lacked was `createInstance`
  answering for `IMessage`, which they now get. A relay would matter only for a
  sandboxed VST3, which does not exist.
- _Limits._ Tried with Surge XT on 2026-10-23: its controller implements
  `IInfoListener` and takes the host's messages. What a plugin does
  with the name is not observable from the tests. `vst3/` imports `HostTrackInfo`
  from `plugin_formats::clap::ffi`, a neutral type living in the CLAP crate.
  _Result codes (2026-10-22):_ every host object (`IHostApplication`,
  `IComponentHandler`, `IBStream`, `IEventList`, `IParameterChanges` /
  `IParamValueQueue`, `IPlugFrame`, `IMessage`, `IAttributeList`) now answers with the
  SDK's codes (`K_NO_INTERFACE` = `0x80004002`, `K_INVALID_ARGUMENT` = `0x80070057`,
  `K_RESULT_FALSE` = 1 on Windows, `funknown.h` with `COM_COMPATIBLE`); the older
  code used to answer `-1` for both, and `S_FALSE` for a null `obj` in the event
  list. A `resizeView` with no window attached answers `K_RESULT_FALSE` (refused),
  a null argument `K_INVALID_ARGUMENT`. Tests: one per object, plus a guard on the
  numeric values (`result_codes_are_the_windows_values_of_funknown_h`).

Tests: `vst3/attribute_list.rs`, `vst3/message.rs`, `vst3/com_object.rs` (round
trips, missing key, full list, bad arguments, cut string, reference counts, the
last `release` frees, checked per object with a probe — no global counter),
`vst3/host_context.rs` (`createInstance`: both ids, other ids, `cid != iid`, null
arguments), `vst3/channel_context.rs` (name, no color, no name, neither, UTF-16
length, 255-byte cut, exact ARGB, a listening controller and one without the
interface), `plugin_system/vst3_track_info_tests.rs` (a hand-made plugin,
`vst3/fake_plugin.rs`: announced once, rename, color removed, empty track, busy
gate and retry), `plugin_system/ffi_lifecycle_tests.rs`
(`vst3_real_controller_takes_host_messages_and_track_info`: TDR Nova implements
`IInfoListener` and accepts the list, the host makes messages the plugin ignores
without crashing). The render harness is unchanged: it holds no VST3 plugin.

**Host `clap.note-ports` and `clap.note-name` (2026-10-14).** Checked against
`clap/ext/note-ports.h` and `note-name.h` (no compat id for either).

- _`clap.note-ports` (host)._ `supported_dialects` answers
  `CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI`: the host sends note events and
  raw `CLAP_EVENT_MIDI` (pitch bend, CC, aftertouch; ADR-022), no MPE, no MIDI 2.
  `rescan(ALL)` is a restart request (the same path as `request_restart`: poll →
  recompile → reactivation); `rescan(NAMES)` is accepted and shows nowhere. The
  host **does not read the plugin's note ports** and does not filter what it
  sends by dialect — decided, so what reaches a plugin and the offline render are
  unchanged (ADR-022: a plugin without the MIDI dialect ignores the raw MIDI).
- _`clap.note-name` (host)._ `changed` sets a process-wide lock-free flag
  (`clap_ffi::take_plugin_note_names_changed`, callable from any thread);
  `main.rs::watch_plugin_edits` turns it into the `plugin-note-names-changed`
  event within 250 ms. Like `plugin-params-changed` it carries no payload: the
  listener refetches. Nothing in the UI listens yet.
- _Reading the names._ The `get_plugin_note_names(instance_key)` command returns
  the plugin's names (`name`, `port`, `key`, `channel`, `-1` = every one) through
  `PluginHost::plugin_note_names` — `clap_note_name.count/get` on the command
  thread under the instance FFI gate, never on the audio thread. `None` means the
  placement is unknown, not a CLAP plugin, or busy in a block past the 3 ms
  budget (retry); an empty list means the plugin names no note. At most 4096
  names are read. **No UI uses it yet**: showing drum-pad names in the piano roll
  is a separate step.
- _Sandbox._ The bridge hosts both extensions. `changed` travels in the block
  flags (`BLOCK_NOTE_NAMES_CHANGED`, bit 12, next to the `clap.params` rescan
  bits); the names are read by a new control op, `Op::NoteNames`, in the wire form
  of `clap_ffi::encode_note_names`. The shared-memory layout and
  `BRIDGE_PROTOCOL_VERSION` are unchanged.
- _Limits._ No VST3 equivalent. Not yet tried with real plugins.

Tests: `crates/clap_ffi` (layouts, dialects, a rescan of all ports vs a rename,
`changed` reported once, wire round trip), `plugin_system/sandbox_tests.rs` and
`crates/plugin_host_bridge/tests/bridge.rs` (dialects read by the plugin, names
read and re-read after a change, `changed` reaching the app once, rescan → restart,
in process and sandboxed). The test plugin gained `Note names set` (19),
`Note ports rescan` (20) and `Host note dialects` (21): 22 parameters in all.

**Host `clap.timer-support` (2026-10-14).** Checked against
`clap/ext/timer-support.h` (CLAP main; id `clap.timer-support`, no compat id).
What the spec says: the plugin's `on_timer(plugin, timer_id)` is `[main-thread]`;
the host's `register_timer(host, period_ms, *timer_id)` "registers a periodic
timer", "may adjust the period if it is under a certain threshold", "30 Hz should
be allowed", and `unregister_timer(host, timer_id)` returns whether it worked;
both are `[main-thread]`; `clap_id` is a `uint32_t` with `CLAP_INVALID_ID =
UINT32_MAX` (`clap/id.h`). What it leaves open — and Echoes therefore decides:
how ids are chosen, a maximum period, any link with `clap.gui` (`gui.h` only
advises a hidden window to "stop painting timers"), and what happens when the
plugin is destroyed. The decisions below were validated by the user on 2026-10-14.

- _Registration._ A lock-free table in the plugin's `ClapHostContext`
  (`TimerTable`, `crates/clap_ffi/src/timers.rs`; 16 slots of one atomic each), so
  `register_timer` / `unregister_timer` take no lock and allocate nothing, whatever
  thread a plugin calls from — the audio thread included, although the spec reserves
  them for the main thread. A period of 0 is refused, a period under 16 ms is
  raised to 16 ms (the Windows timer tick is about 15.6 ms, so a shorter period
  could not be honoured), there is no ceiling (a long period costs nothing).
  Refused too: no out parameter (the id would be lost) and a full table (17th
  timer). The out parameter is set to `CLAP_INVALID_ID` on a refusal. Ids start
  at 1 (a plugin may use 0 for "no timer"), are unique for the plugin's lifetime,
  and are never reused after `unregister_timer`; `CLAP_INVALID_ID` is never given.
- _Who calls `on_timer`._ Only a message loop, never a host callback: a plugin that
  registers a timer gets no `on_timer` before `register_timer` has returned.
  In process: the editor window's `WM_TIMER` (`plugin_system/gui.rs`; set to 10 ms,
  i.e. every Windows tick, ~15.6 ms) on the Win32 thread, through
  `PluginInstance::run_due_timers`. Sandboxed: the bridge's main loop
  (`serve_requests`, which wakes at least every 5 ms) through
  `LoadedPlugin::run_due_timers`. No new thread, no new window. `clap.thread-check`
  reports both as the main thread; no `audio_thread_scope`.
- _Editor closed._ **A timer runs only while its plugin's editor window exists**,
  in process and sandboxed alike. A timer registered before `gui.create` or after
  `gui.destroy` is remembered and answers `true`, but is frozen until the editor
  opens; it then starts one period after the window appears, with no catch-up. A
  plugin that needs timers with no editor (rare on Windows, where plugins use their
  own timers) gets none; running them without an editor would need a permanent pump
  on the app's Win32 thread and a registry of instances — not done. The host does
  not unregister timers on `gui.destroy`: that is the plugin's job (the spec
  advises it for painting timers), a timer left registered resumes with the next
  editor. The offline render opens no editor, so no plugin ever gets a timer
  there and the C-2 harness is unchanged.
- _Schedule._ Each pump owns a `TimerSchedule` (due time per id). A timer is first
  due one period after the pump first sees it; after a call the next due time is
  `due + period`, which keeps the average rate exact (jitter: at most one pump
  interval), unless the loop was a whole period late, in which case it restarts
  from now. So a stalled loop never makes up for lost ticks, and each timer is
  called at most once per pass. A closed editor drops its schedule.
- _Unregister._ The registration is checked again just before every call, so
  after `unregister_timer` has returned `true` the plugin gets no further
  `on_timer` for that id, even from the same pass, and a plugin may unregister
  (or register) from inside `on_timer`.
- _Destroy and crash._ The table dies with the plugin's `ClapHost`, which outlives
  the plugin; the editor window holds an `Arc<PluginInstance>` until it closes, so
  no pump can outlive the instance, and closing the window ends the pump. A crashed
  plugin (`has_crashed`) is never called. A sandboxed plugin's timers die with its
  bridge process.
- _FFI gate._ In process, `on_timer` does **not** take the instance FFI gate
  (decision: the user's brief asked for it, and was changed after the cost was
  shown). A held gate makes the audio thread bypass the block it overlaps, so a
  periodic call would cut blocks for as long as an editor is open (estimate, not
  measured: about one block per 10 s for a 20 µs `on_timer` at 30 Hz, more for a
  slower one), and CLAP lets `on_timer` run beside
  `process`; `on_main_thread` and the `clap.gui` calls already run without the
  gate. What it does: leave a timer due — retried at the next pass, not one
  period later — while another off-RT caller (state save, `changed()`) holds the
  gate, and mark the crash marker as a UI-thread call. Remaining risk: an off-RT
  caller that takes the gate during an `on_timer` is not excluded (same exposure
  as the editor's other calls). In the bridge, control requests run on the same
  thread as the pump, so they never overlap an `on_timer`.
- _A slow `on_timer`._ It blocks the loop that called it: the Win32 thread (the
  app's UI, as with a slow `on_main_thread`) or the bridge's main thread (control
  requests wait, so a very slow one can hit the engine's control timeout). There is
  no watchdog and no abort; the loop only avoids a burst afterwards.
- _Sandbox._ The bridge hosts the same extension. No control op, no block flag,
  no shared-memory change, `BRIDGE_PROTOCOL_VERSION` unchanged (ADR-015
  amendment 2026-10-14).
- _Limits._ No VST3 equivalent. At most 16 timers per plugin. Not yet tried with
  real plugins (an animated editor: Vital, Surge XT, u-he).
- _Docs vs code._ `gui.rs` used to say a 16 ms `WM_TIMER` drained `on_main_thread`;
  the timer was a no-op (the drain is `CLAP_GUI_DRAIN_MESSAGE`). The timer now
  pumps the plugin's timers and the comments say so.

Tests: `crates/clap_ffi` (`timers.rs`: extension table, ids, refusals, bounds,
schedule — rhythm, stall, declined, unregistered, registered during a pass —
many threads), `plugin_system/sandbox_tests.rs` (in process and sandboxed:
registration and refusals; in process: delivery on a fake clock, registration
from the audio thread, crashed plugin, off-RT holder and block in flight, and a
real editor window pumped by the test; sandboxed: frozen without an editor),
`crates/plugin_host_bridge/tests/bridge.rs` (registration from the audio and main
thread, frozen without an editor, both timers firing in a real editor window
until unregistered, never on the audio thread). The two window tests open a small
real window for a moment. The test plugin gained a stub `clap.gui`, the plugin
`clap.timer-support` and eight parameters (ids 22–29, `Timer register` …
`Timer audio calls`): 30 in all.

**Host `clap.preset-load` (2026-10-14).** Checked against
`clap/ext/preset-load.h` (CLAP main; ids `clap.preset-load/2` and the compat id
`clap.preset-load.draft/2`, "may be removed in 2026"). The host side is
`clap_host_preset_load { on_error(host, location_kind, location, load_key,
os_error, msg), loaded(host, location_kind, location, load_key) }`, both
`[main-thread]`; `location` and `load_key` may be null. The plugin side
(`from_location`, asking a plugin to load a preset file) is not used yet — it
belongs to preset discovery.

- _`loaded`._ A preset picked in the plugin's own window changes its state without
  the host knowing, so `loaded` raises the same unsaved-changes flag as
  `clap.state.mark_dirty` (`mark_plugin_state_edited`, one atomic store). The
  location and key are not kept. The state itself needs nothing more: it is saved
  with the project like any other (ADR-010). Parameter values are refreshed by
  the plugin's own `clap.params.rescan(VALUES)`, as before.
- _`on_error`._ Logged on stderr (`[clap.preset-load] <location> (os error N):
<message>`), never from the audio thread; it changes no state and does not
  mark the project unsaved.
- _Threads._ The spec reserves both for the main thread, but each is one atomic
  store or one log line, so a call from any thread is harmless (the audio thread's
  log line is dropped).
- _Sandbox._ `loaded` reaches the engine through `BLOCK_PLUGIN_EDITED`, the flag
  `mark_dirty` already uses. No op, no new flag, `BRIDGE_PROTOCOL_VERSION`
  unchanged (ADR-015 amendment 2026-10-14).
- _Limits._ No preset list, no `from_location` call, no UI: the user still picks
  presets in the plugin's window. Not yet tried with real plugins.

Tests: `crates/clap_ffi` (`presets.rs`: both ids answered with one table, `loaded`
marks the project unsaved once, `on_error` does not and tolerates null strings),
`crates/plugin_host_bridge/tests/bridge.rs` (the test plugin calls both host
functions from its process: only `loaded` comes back as an edit, once). The test
plugin gained `Preset loaded` (30) and `Preset error` (31): 32 parameters in all.
In process, the real-plugin call is not tested: the unsaved flag is process-wide
and another unit test of the app reads it.

**Host preset discovery (ADR-031, step 1 — listing; 2026-10-15).** Checked against
`clap/factory/preset-discovery.h` (CLAP main; factory id
`clap.preset-discovery-factory/2`, compat `…/draft-2`). The factory comes from the
library entry, so the listing needs no plugin instance. Struct layouts follow the
header field by field; tests pin the order (like the `clap_host` ABI guard).

- _Command._ `list_plugin_presets(instance_key)` (`main.rs`, async): reads the
  placement's library path and CLAP id under the engine lock
  (`PluginHost::preset_source`), releases the lock, then runs
  `plugin_system::presets::list_presets` on a blocking worker thread. Unknown
  placement, VST3, internal or a library without the factory → an empty list.
- _Cycle._ `clap_ffi::discover_presets` (`preset_discovery.rs`), per provider on
  the calling thread: `create` → `init` (the provider declares file types,
  locations, sound packs through the host's indexer) → crawl → `destroy`. A
  `PLUGIN` location is read once with a null location. A `FILE` location is
  walked by extension (case-insensitive, leading dot ignored, empty extension =
  every file) in name order; a path that does not exist is skipped, a path that
  is a file is read as is. Bounds: depth 6, 10 000 files per location, 20 000
  presets, texts cut to 512 characters (guesses until measured on real plugins).
- _What is kept._ A preset is kept when it names the plugin (`add_plugin_id`, abi
  `clap`, the plugin's id); one that names nobody or another plugin is dropped.
  A nameless preset (allowed for single-preset files) takes the file name, else
  its key. Flags are those of the location unless `set_flags` overrides them; the
  sound pack id becomes its declared name. Creator, description and features are
  kept; timestamps and extra info are ignored.
- _Output._ `PluginPreset` (`name`, `load_key`, `location_kind`, `location`,
  `provider_id`, `flags`, `creator`, `description`, `features`, `soundpack`):
  factory presets first, then user ones, then the rest, each by name. Nothing is
  cached or persisted; no file watcher (`set_invalidation_watch_file` is not in
  the header's indexer struct).
- _Safety._ The provider is third-party code in the app process, like the scan's
  probes: the cycle runs behind the SEH guard (`crash_guard`) and a fault lists
  nothing; one listing runs at a time (providers are not thread-safe). Nothing
  runs on the audio thread. A sandboxed plugin is listed the same way (its
  library is already loaded in the app by the scan); isolating discovery is
  deferred (ADR-031).
- _Limits._ No loading yet (step 2) and no UI (step 3). Not yet tried with real
  plugins (Vital, Surge XT): how long a real crawl takes is unknown.

Tests: `crates/clap_ffi` (`preset_discovery.rs`: struct order, filtering by
plugin id, bundled and file locations, nested directories, extensions, several
presets per file, flag overrides, missing directory, no factory, compat id,
failing `init`, crawl bounds) and `plugin_system/sandbox_tests.rs` (the real test
plugin, in process and sandboxed: source, list, sorting, a user directory, the
sleepy plugin's own presets, a provider that raises an exception). The test
plugin gained a `clap.preset-discovery-factory/2` (one provider; the environment
variables `ECHOES_TEST_PRESET_DIR` and `ECHOES_TEST_PRESET_CRASH` steer it).

**Host preset loading (ADR-031, step 2 — 2026-10-15).** Checked against
`clap/ext/preset-load.h`: the plugin side is
`clap_plugin_preset_load.from_location(plugin, location_kind, location, load_key)`,
`[main-thread]`, true on success; a `PLUGIN` location has a null `location`. The
plugin answers through the host's `loaded` / `on_error` (see Host
`clap.preset-load`).

- _Command._ `load_plugin_preset(instance_key, location_kind, location, load_key)`
  (`main.rs`, async). The host loads **only what the plugin's last listing
  offered**: `list_plugin_presets` keeps its result per CLAP plugin id
  (`PluginHost::remember_presets`) and `instance_for_preset` refuses any request
  that is not one of its entries ("this preset is not offered by the plugin"; "this
  plugin has no presets" for an unknown placement). The engine lock is held only
  for that check; the load runs on a blocking worker thread.
- _In process._ `PluginInstance::load_preset` claims the instance FFI gate
  (`offrt_try_acquire`, 50 ms budget; "the plugin is busy, try again" past it),
  calls `clap_ffi::load_preset` and keeps the gate until it is done, so the audio
  thread **bypasses the plugin for the whole load** (an effect passes its input
  through, an instrument is silent). A preset that loads samples can hold it for a
  noticeable time. ADR-031 keeps the gate-free call (L2) as the fallback if this is
  not acceptable by ear.
- _Sandboxed._ `Op::PresetLoad = 11`: payload `PresetRequest::encode` (kind:u32,
  presence flags:u8, then each text as length:u16 + UTF-8), reply empty or the
  plugin's own message. The bridge's main thread calls `from_location` beside
  `process`, as it does for `LoadState`. Shared memory and
  `BRIDGE_PROTOCOL_VERSION` unchanged.
- _Refusal._ `from_location` returning false is an error; the message is the one
  the plugin gave to `on_error` during the call (kept in the plugin's
  `ClapHostContext`, `take_preset_error`), else "the plugin could not load this
  preset". A refused preset changes nothing.
- _After a success._ The project is marked unsaved by the host itself
  (`mark_plugin_state_edited`), whether or not the plugin called `loaded`. A
  sleeping plugin is woken (`request_process`; in the bridge, on the bridge's own
  context). The base of every **value-modulated parameter** (ADR-021 amendment) is
  read again from the plugin — under the gate in process, through `Op::ParamValue`
  sandboxed — so the next save and the next modulated block do not write the old
  base over the preset. The command then emits `plugin-params-changed` so the
  parameter lists, values and texts are fetched again.
- _Not undoable_, like an edit in the plugin window; nothing about the preset is
  stored in the project: the plugin state blob (ADR-010) carries the sound.
  Automation lanes are absolute (ADR-025) and keep overriding the parameters they
  target.
- _Limits._ A plugin that applies a preset after `from_location` has returned
  (deferred to its own thread) is read too early for the base refresh; the base
  then follows the next drag only. Paths and load keys are carried up to 4096
  characters (16 bits on the wire). No UI yet (step 3). Not yet tried with real
  plugins.

Tests: `crates/clap_ffi` (`presets.rs`: wire round trip and malformed requests,
the plugin's message kept once; `preset_discovery.rs`: paths and keys are not cut
like labels), `plugin_host_bridge/tests/bridge.rs` (the op over the real channel:
loaded and forwarded as an edit, a refusal with the plugin's message keeps the
channel open, a malformed payload) and `plugin_system/sandbox_tests.rs`, in
process and sandboxed unless noted: the sound and the saved state change, only an
offered preset loads, a refusal says why and changes nothing, the project is
marked unsaved although the plugin stays quiet (file presets), in process: the
FFI gate is held for the whole load and a busy plugin is refused after the
budget, the base of a value-modulated parameter follows the preset, the state
saved after a preset renders like the preset (C-2 equivalence, offline), a
sleeping plugin wakes. The test plugin gained `clap.preset-load` (bundled `quiet`,
`loud`, `sleepy`; `.testpreset` files `<gain> <tone> [<delay ms>]`) and the
parameter `Preset tone` (32): 33 in all.

**Preset picker (ADR-031, step 3 — 2026-10-15).** `PresetPicker.svelte`, a labelled
`PRESET` select built like `SidechainPicker.svelte`, shown for a **CLAP** or **VST3**
plugin that offers presets: in the expanded plugin effect (between the editor
buttons and the sidechain row) and in the instrument panel (under the instrument
row, above the arpeggiator, compact or not). It is an action menu: choosing an
entry calls `load_plugin_preset`. For CLAP the select shows nothing at rest (the
empty option is hidden) and returns to empty after a load, since the host does not
know the current preset; for a VST3 program it shows the program the plugin says is
in use (`PluginPreset.active`) and is empty when the plugin cannot tell.
Entries are grouped Usine / Utilisateur / Autres by the provider's flags
(`src/frontend/presets/pluginPresets.ts`, engine order kept inside a group). The
row is absent when the list is empty, the plugin unavailable or the call fails.
The list is fetched when the picker appears, when the placement or its
availability changes, and when the select gets focus; an unchanged list leaves
the DOM alone so an open menu stays open. While a load runs the select is
disabled (`cursor: default`); a refusal is a red notification
`Erreur preset : <the plugin's message>`; a success shows nothing, and the
`plugin-params-changed` event refreshes the knobs. Tests:
`src/frontend/presets/pluginPresets.test.ts` (grouping, change detection, keys,
command arguments, errors). Not hand-tested: spacing in the compact instrument
panel, and the behaviour with a real plugin.

**Host VST3 presets (ADR-031 amendment, 2026-10-22).** Step 6 of the VST3 catch-up.
Checked against `ivstunits.h`, `ivsteditcontroller.h`, `vsttypes.h`, the
reference host's `vstpresetfile.h` / `.cpp` (vst3_public_sdk) and `funknown.cpp`;
the `IUnitInfo` IID is in `interface_iids_match_the_sdk` and in the TDR Nova binary.
A VST3 plugin has **two sources of presets**, offered through the same two
commands, the same last-listing whitelist, the same FFI gate and the same
`PresetPicker.svelte` as CLAP (no new command, event, project field or UI element).

- _Listing._ `PluginHost::preset_listing` gives `PresetListing::Vst3 { instance,
  files }`; `list_plugin_presets` runs it on the blocking worker thread. The
  whitelist is kept under the plugin's `device_id`. Order: the plugin's programs
  (its own order), then factory files, then user files (files by name).
  `PluginPreset.active` (added during this step) is true for the program the
  plugin's selector currently names (VST3 programs only; always false for CLAP and
  files) so the picker can show it.
- _Programs (`IUnitInfo`, `vst3/units.rs`)._ The controller is asked for
  `IUnitInfo` by `queryInterface`; a controller without it, or without a list, offers
  none. Units, program lists and program names are read under the FFI gate, every
  count bounded (256 units, 64 lists, 4096 programs per list, 20 000 programs,
  65 536 parameters; names cut at the 128 UTF-16 units of a `String128`; the sizes
  of `UnitInfo` / `ProgramListInfo` and the vtable are asserted at compile time).
  A list is offered only if the host can choose its programs: the **selector** is
  the parameter flagged `kIsProgramChange` (`1 << 15`) of a unit that uses the list.
  TDR Nova leaves its only unit unlinked (`programListId = -1`), so when no unit
  uses the list the only program-change parameter is taken if there is one list and
  `stepCount + 1` equals its number of programs. A nameless program is shown as
  `Program N`; with several lists each name starts with its list's name. Key
  `<parameter id>:<index>`, location kind `PLUGIN`, provider `vst3:program`, factory.
- _Choosing a program (`PluginInstance::load_vst3_program`)._ The FFI gate is
  claimed (50 ms, "the plugin is busy, try again" past it); the plugin's own
  `plainParamToNormalized` turns the index into the selector's value;
  `setParamNormalized` gives it to the controller and the same value goes into the
  parameter SPSC queue for the processor (it reaches `inputParameterChanges` with
  the next block); the bases of value-modulated parameters are read again; the gate
  is released and the project is marked unsaved. If the plugin is active the host
  then waits up to 200 ms for the audio thread to take the value, and gives the
  processor's state (`getState`) to the controller (`setComponentState`, any
  controller kind) under the gate, then reads the bases again — a program changes
  many values only the processor knows, so without this only the selector's knob
  moves. On TDR Nova 23 of 76 values follow. Nothing is waited for when the plugin
  is not active, and the sync is skipped when a block is in flight or the plugin
  reports no state. A plugin that applies a program over several blocks may be read
  too early; the knobs then catch up at the next gesture.
- _Files (`vst3/preset_file.rs`)._ Steinberg's `.vstpreset` layout, little-endian:
  a 48-byte header (`VST3`, version `i32`, the class id as 32 ASCII hex digits, the
  `i64` offset of the chunk list), the chunks, then `List`, an `i32` count and per
  entry the chunk id, its `i64` offset and `i64` size. The version is read and
  ignored like the SDK does; at most 128 entries are read. Only `Comp` (the
  processor's state) is required; `Cont` (the controller's own state) is optional;
  `Prog`, `Info` and unknown chunks are ignored. The file comes from the user's
  disk, so nothing in it is trusted: files over 256 MiB are refused before any
  read; the list and every chunk must lie inside the real file length (checked
  arithmetic, so a huge or negative size or offset is an error, not an allocation);
  a chunk is read with `take(size)`, never into a buffer sized from the file; an
  error is a readable message, never a panic. The class id in the file must be the
  plugin's: on Windows `FUID::toString` prints `l1 l2 l3 l4`, so the first three
  groups of the plugin's class bytes are swapped back before the comparison
  (`class_id_text`; the CID itself is not permuted anywhere else).
- _Where the files are (`vst3/preset_locations.rs`)._ `<root>\VST3 Presets\<vendor>\<plugin
  name>\…` under `Documents` (the shell's folder, so a moved one is found; user
  presets), `%APPDATA%` and `%PROGRAMDATA%` (factory). The scan does not know the
  vendor folder, so each vendor folder is searched for one named like the plugin
  (case-insensitive, characters Windows forbids replaced by `_`). The walk is
  bounded (depth 6, 10 000 entries per folder, 1 000 vendor folders, 20 000
  presets), does not follow links or junctions, takes `.vstpreset` files only, and
  reads **only the 48-byte header** of each to see whether it is this plugin's. A
  file whose path is not valid Unicode is not offered. The preset is named after
  its file; location kind `FILE`, the path as `location`, provider `vst3:file`.
  The `Documents` lookup uses `SHGetKnownFolderPath`, which needs the
  `Win32_UI_Shell` and `Win32_System_Com` features of the existing `windows-sys`
  dependency (no new crate).
- _Loading a file (`PluginInstance::load_vst3_preset_file`)._ Only a file the last
  listing offered is accepted (`instance_for_preset`: kind, path and key must match
  an entry). The file is then **read and checked again, class id included, before
  the gate is taken** (the disk may have changed since the listing). Under the gate
  (50 ms) the `Comp` chunk goes through `load_state_blob` — the ADR-010 state path,
  unchanged: `IComponent::setState`, and `setComponentState` for a separate
  controller — then `Cont`, if present, through `IEditController::setState` (a
  refusal is only logged, as the SDK's `loadPreset` treats it). A processor that
  refuses its state is an error and nothing else changes. Value-modulated bases are
  read again, the project is marked unsaved by the host, the gate is released and
  the command emits `plugin-params-changed`. `setState` on an active component may
  ask for `restartComponent`; the existing handler deals with it.
- _Consequences, as for CLAP._ While the gate is held (a file load, the two gate
  holds of a program) the audio thread bypasses the plugin: an effect passes its
  input through, an instrument is silent. Loading is **not undoable**, like an edit
  in the plugin window. Nothing about the preset is stored: the plugin state
  (ADR-010) carries the sound through a project save and reopen, and there is no
  preset name field. VST3 plugins always run in the app process; the sandbox is
  unchanged. The render harness has no VST3 plugin, so the golden hashes are
  unchanged (C-2); nothing runs on the audio thread that did not already.
- _Not done._ `IUnitHandler` / `IUnitHandler2` (the host does not answer
  `notifyProgramListChange` or `notifyUnitSelection`; the list is read again each
  time the picker is shown or focused, and nothing in the load waits for them),
  `IProgramListData` / `IUnitData` (program data without the selector), program
  pitch names, `selectUnit`, the `Prog` and `Info` chunks, presets bundled inside the
  plugin's `.vst3`, a file watcher, saving a preset from the host, macOS and Linux
  locations, and multi-block program application.
- _Checked and not checked._ The program path and the file path are tested on TDR
  Nova (73 factory programs, all selectable). **No `.vstpreset` exists on this
  machine**, so the file tests use files the tests write themselves, and the byte
  order of the class id rests on reading the SDK, not on a file from another host.
  A plugin's `getState` is not canonical: TDR Nova re-serialises a loaded state
  differently, so the real test compares parameter values, not state bytes. Not
  hand-tested in the app; no VST3 instrument was available.

Tests: `vst3/preset_file.rs` (valid with and without `Cont`, first of a kind wins,
wrong signature / version ignored / other plugin / non-hex id, no `Comp`, a file
truncated at every length, list offset negative / in the header / past the end,
absurd and negative counts, chunks outside the file, overflowing and huge sizes,
size cap, class id order), `vst3/preset_locations.rs` (folder names, vendor
search, bounds and depth, filtering by header, extension case, missing and
non-folder paths), `vst3/units.rs` (no interface, root only, one and several lists,
selector rules and fallback, nameless and very long names, failing and absurd
counts, release of the interface), `plugin_system/presets.rs`,
`vst3_program_tests.rs` and `vst3_preset_file_tests.rs` with the hand-made plugin
`vst3/fake_plugin.rs` (state of the processor and the controller, refusals, busy
plugin, gate freed, bases re-read, controller sync, plugin not active), the
whitelist in `host.rs`, and the real-plugin tests of `ffi_lifecycle_tests.rs`
(`vst3_real_controller_program_lists`, `vst3_real_program_can_be_chosen`,
`vst3_real_preset_file_is_listed_and_loaded`).

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

VST3 `restartComponent(kLatencyChanged)` takes the same path: see "Plugin window
edits" above. Not hand-tested with a plugin that changes its latency.

**VST3 effects and instruments (2026-10-23).** Both read `getLatencySamples` in
`prepare`, after `vst3_ensure_active`, through `PluginInstance::vst3_latency_samples`:
off the audio thread, under the instance FFI gate (3 ms budget). A block that keeps
the gate leaves the node with the value it had (a new node starts at 0); a pending
restart request is re-armed by `vst3_ensure_active`, so the next recompile
reactivates the plugin and reads the new value. An instrument
reaches the compiler through `InstrumentDevice::latency_samples()`, relayed by
`InstrumentNode::latency_samples()`; `audio_graph/` sees no VST3 type (ADR-003).
Without it an instrument track with a latency `L` would sound `L` samples late
against the others. CLAP instruments have the same gap (no `clap.latency` read yet).
Tests: `plugin_system/vst3_latency_tests.rs` (hand-made plugin, restart, busy gate),
`compiler.rs` (an instrument of latency 128 delays the track it meets by 128),
`ffi_lifecycle_tests.rs` (TDR Nova effect: node 184 = plugin 184; Surge XT
instrument: adapter 0 = plugin 0, so no real instrument checks a non-zero value).

### VST3 tail and processing mode (read, not used — 2026-10-23)

Probed on the machine (`vst3_real_plugins_report_their_latency_and_tail`): TDR Nova
reports latency 184 and tail 184 samples and does not implement
`IProcessContextRequirements` (its IID is absent from the binary); Surge XT reports
latency 0 and a tail of 96 000 samples (2 s at 48 kHz) and implements it (IID found
twice in the binary). SDK (`ivstaudioprocessor.h`): `kNoTail = 0`,
`kInfiniteTail = 0xFFFFFFFF`; `kRealtime 0`, `kPrefetch 1`, `kOffline 2`; moving
to or from `kOffline` needs `setupProcessing`, so a deactivated plugin.

- **Tail: nothing reads it.** CLAP sleeps because the plugin answers `SLEEP`; VST3
  has no such answer, so a VST3 sleep would be a guess from silence, which cuts a
  synth's release when it reports a tail of 0. It would also change which blocks are
  processed (C-2, ADR-019) for a CPU gain. The export length is the end of the last
  clip (`timeline_end_secs`), so the tail of every plugin and internal effect is cut
  already: extending the export is an export feature (all formats, a cap for an
  infinite tail), not a VST3 gap.
- **`kOffline`: not set.** `ProcessSetup.processMode` and `ProcessData.processMode`
  stay `kRealtime` in the effect, the instrument and `vst3_ensure_active`. The export
  runs on the live engine's `PluginHost`, so it shares the plugin instance (ADR-016)
  with the audio stream that keeps running; switching it to `kOffline` means a
  reactivation of that shared instance and one back, while the stream still calls
  `process` with mode 0. Only plugins that stream from disk or have a render quality
  would gain. To reconsider once the engine's behaviour during an export is decided.
- **`IProcessContextRequirements` and `canProcessSampleSize`: not asked.** The host
  fills every `ProcessContext` field, which is what a plugin without the interface
  gets; the interface only lets a host skip work. The host processes in 32 bits only.

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
