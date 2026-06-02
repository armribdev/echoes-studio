# Plugin Wrapper System

> **Canonical.** Merges the former `systems/plugin-wrapper-system.md` (VST3 + AU
> + CLAP responsibilities) and the legacy `vst-wrapper-rules.md` (detailed VST3
> code). The legacy flat file is archived under `docs/archive/vst-wrapper-rules.md`.

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

```rust
let ctx = ProcessContext {
    state:              if transport.is_playing { kPlaying } else { 0 },
    sampleRate:         transport.sample_rate as f64,
    projectTimeSamples: transport.position_samples as i64,
    tempo:              transport.bpm as f64,
    timeSigNumerator:   transport.time_sig_numerator,
    timeSigDenominator: transport.time_sig_denominator,
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

---

## Opaque State Philosophy

Plugin state is treated as an opaque blob — the wrapper saves and restores it
without interpretation. The host never needs to understand VST3/AU/CLAP preset
format; migration across plugin versions is the plugin's responsibility.

---

## Crash Containment

VST3, AU, and CLAP plugins run in-process by default. A crashing plugin takes
down the engine. Best-effort mitigations (current):
1. Validate all plugin-provided pointers before dereferencing.
2. Wrap `process()` calls in platform signal handlers where feasible.

**Out-of-process hosting** via a subprocess bridge (shared-memory audio rings,
control handshake, watchdog) is the durable mitigation. It is opt-in and
documented in `../decisions/015-plugin-sandbox.md`. Until a plugin runs
sandboxed, the crash-isolation guarantee is "best effort" — document this
honestly in user-facing plugin support.

---

## Wrapper Checklist

Before merging any wrapper `process()` implementation:

| Rule | Check |
|------|-------|
| No `Vec::push` / `Box::new` | all pointer arrays allocated in `prepare()` |
| No `setParamNormalized` from audio thread | use `ProcessData.inputParameterChanges` (VST3) / host event queue (CLAP) |
| All transport from `TransportInfo` | never from global state |
| `unsafe` scoped to the FFI call only | no wider `unsafe` blocks |
| Latency reported via `latency_samples()` | required for PDC |
| State via native chunk bytes, not JSON | format spec compliance |

---

## Further Reading

- `plugin-system.md` — how wrappers fit into the abstraction stack
- `../threading/realtime-safety.md` — audio-thread prohibitions the wrapper must obey
- `../decisions/003-plugin-wrapper-strategy.md` — why wrappers instead of deep integration
- `../decisions/015-plugin-sandbox.md` — out-of-process sandbox design
- `../audits/plugin-gui-audit-2026-05-31.md` — CLAP/VST3 editor hosting issues
