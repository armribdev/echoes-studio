# Plugin Wrapper System

## Purpose

Plugin wrappers are thin adapters that translate between an external plugin
format's API (VST3 COM interfaces, CoreAudio AudioUnit handles) and the engine's
`AudioNode` trait. They are the **isolation boundary** between external code
and the graph runtime.

---

## The Isolation Boundary

```
External plugin binary
        │
        │  (VST3 COM / AU handle / WASM module)
        ▼
[ ISOLATION BOUNDARY — wrapper is the only code that crosses this ]
        │
VstWrapperNode / AuWrapperNode  (implements AudioNode + Device)
        │
        │  (only AudioNode is visible to the graph)
        ▼
AudioGraph / GraphScheduler  — has zero knowledge of plugin formats
```

The graph runtime **must never** import:
- `vst3-sys` types (`IAudioProcessor`, `IEditController`, etc.)
- CoreAudio types (`AudioUnit`, `AURenderCallbackStruct`)
- Raw plugin binary handles of any kind

If any `plugin_formats` type leaks into `audio_graph/`, it is an architectural
regression.

---

## VST3 Wrapper Responsibilities

### Buffer Translation

VST3 uses pointer arrays (`float** inputs`). The engine uses `&[&AudioBuffer]`.
The wrapper translates between them using pre-allocated pointer arrays:

```rust
fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], transport: TransportInfo) {
    // Fill pre-allocated ptr arrays (allocated in prepare(), not here)
    for (i, buf) in inputs.iter().enumerate() {
        for ch in 0..self.num_channels {
            self.input_ptrs[i][ch] = buf.channel(ch).as_ptr() as *mut f32;
        }
    }
    // ... same for outputs

    // Build ProcessData from pre-allocated struct
    let process_data = ProcessData { /* ... */ };

    // Call plugin
    unsafe { (*self.processor).process(&process_data); }
}
```

**Key rule:** `self.input_ptrs`, `self.output_ptrs`, `self.input_bus_buffers`,
and `self.output_bus_buffers` are all allocated in `prepare()`. The `process()`
call only fills pointer values — no `Vec::push`, no `Box`.

### Parameter Mirroring

VST3 parameters use integer IDs (`ParamID: u32`). The engine uses string keys
(`"gain"`, `"cutoff"`). The wrapper builds the mapping in `prepare()`:

```
string key "gain"  ↔  VST3 ParamID 0x12345678
```

Parameter changes from the UI are queued in a pre-allocated ring buffer on the
command thread. On the next `process()` call, they are delivered via
`ProcessData.inputParameterChanges` — the VST3 sample-accurate mechanism.

**Never** call `IEditController::setParamNormalized` from the audio thread.

### Transport Sync

```rust
let ctx = ProcessContext {
    state:                 if transport.is_playing { kPlaying } else { 0 },
    sampleRate:            transport.sample_rate as f64,
    projectTimeSamples:    transport.position_samples as i64,
    tempo:                 transport.bpm as f64,
    timeSigNumerator:      transport.time_sig_numerator,
    timeSigDenominator:    transport.time_sig_denominator,
    // ...
};
```

All transport data comes from the `TransportInfo` passed by the scheduler —
never from global state or a separate lookup.

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

## Latency Handling

Plugins that introduce latency (look-ahead compressors, linear-phase EQs) must
report it:

```rust
fn latency_samples(&self) -> u32 {
    unsafe { (*self.processor).getLatencySamples() as u32 }
}
```

The graph compiler collects these values to compute **latency compensation** in
a future milestone: non-latency paths receive delay nodes so all signals align
at the master.

---

## Opaque State Philosophy

Plugin state is treated as an opaque blob. The wrapper saves and restores it
without interpretation:

```rust
// Save: plugin → binary chunk → DeviceState::Binary
fn save_state(&self) -> DeviceState {
    let mut stream = VstMemoryStream::new();  // pre-allocated
    unsafe { (*self.component).getState(&mut stream) };
    DeviceState::from_binary(&self.device_id, stream.into_bytes())
}

// Restore: DeviceState::Binary → plugin
fn load_state(&mut self, state: &DeviceState) {
    if let Some(bytes) = state.as_binary() {
        let mut stream = VstMemoryStream::from_bytes(bytes);
        unsafe { (*self.component).setState(&mut stream) };
    }
}
```

This means the host never needs to understand VST3 preset format. Migration
across plugin versions is the plugin's responsibility.

---

## Crash Containment

VST3 and AU plugins run in-process. A crashing plugin takes down the engine.

Best-effort mitigations (current):
1. Validate all plugin-provided pointers before dereferencing.
2. Wrap `process()` calls in platform signal handlers where feasible.

Future mitigation:
- Out-of-process hosting via a subprocess bridge (separate executable, IPC
  via shared memory ring buffers for audio data). This is major architectural
  work deferred to Phase 4.

Until out-of-process hosting exists, the crash isolation guarantee is
"best effort." Document this limitation honestly in user-facing plugin support.

---

## Wrapper Checklist

Before merging any wrapper `process()` implementation:

| Rule | Check |
|------|-------|
| No `Vec::push` / `Box::new` | all pointer arrays allocated in `prepare()` |
| No `IEditController::setParamNormalized` from audio thread | use `ProcessData.inputParameterChanges` |
| All transport from `TransportInfo` | never from global state |
| `unsafe` scoped to FFI call only | no wider `unsafe` blocks |
| Latency reported via `latency_samples()` | required for future compensation |

---

## Further Reading

- `../vst-wrapper-rules.md` — detailed buffer translation code, transport sync,
  state serialisation, parameter mirroring with code examples
- `architecture/plugin-system.md` — how wrappers fit into the abstraction stack
- `decisions/003-plugin-wrapper-strategy.md` — why wrappers instead of deep integration
