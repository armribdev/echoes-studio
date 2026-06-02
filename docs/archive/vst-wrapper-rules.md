> **⚠️ ARCHIVED — SUPERSEDED.** This legacy flat document has been merged into
> the canonical doc and is kept for history only. **Do not edit.**
> Canonical: `docs/active/plugins/plugin-wrapper-system.md`

---

# VST Wrapper Rules

## Purpose

VST3 (and AU) wrappers are the **only** code that bridges external plugin
formats into the graph system. They act as isolation layers, not as
first-class engine objects.

## The Isolation Principle

```
External plugin
    │  (VST3 COM interface / AU AudioUnit handle)
    │
    ▼  ISOLATION BOUNDARY
VstWrapperNode  (implements AudioNode + Device)
    │  (only AudioNode is visible to the graph)
    ▼
AudioGraph  (has no knowledge of VST3)
```

The graph runtime **must never** see:
- `IAudioProcessor` or any VST3 COM pointer
- `AudioUnit` or any CoreAudio handle
- `vst3-sys` types
- Raw plugin binary handles

## Buffer Translation

VST3 uses pointer arrays: `float** inputs, float** outputs`.
The scheduler uses `&[&AudioBuffer]` / `&mut [AudioBuffer]`.

Translation in `VstWrapperNode::process`:

```rust
fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], transport: &TransportInfo) {
    // 1. Fill pre-allocated ptr arrays (no allocation — done in prepare())
    for (i, input) in inputs.iter().enumerate() {
        for ch in 0..num_channels {
            self.input_ptrs[i][ch] = input.channel(ch).as_ptr() as *mut f32;
        }
    }
    for (i, output) in outputs.iter_mut().enumerate() {
        for ch in 0..num_channels {
            self.output_ptrs[i][ch] = output.channel_mut(ch).as_mut_ptr();
        }
    }

    // 2. Build ProcessData with pre-allocated struct (no allocation)
    let process_data = ProcessData {
        inputs: self.input_bus_buffers.as_ptr(),
        outputs: self.output_bus_buffers.as_ptr(),
        num_samples: transport.block_size as i32,
        // ... transport sync
    };

    // 3. Call plugin — no allocation, no locks
    unsafe {
        (*self.processor).process(&process_data);
    }
}
```

Key rules:
- `self.input_ptrs`, `self.output_ptrs`, `self.input_bus_buffers`, etc. are
  all pre-allocated in `prepare()`.
- The `process()` call only fills pointer values (no `Vec::push`, no `Box`).
- `unsafe` is scoped to the FFI call only.

## Parameter Mirroring

VST3 parameters have integer IDs; our system uses string keys.

During `prepare()`, build a mapping:
```
String key ("gain") ↔ VST3 parameter ID (u32: 0x12345678)
```

When the host calls `set_parameter_normalised("gain", 0.75)`:
1. Look up the VST3 ID in the mapping.
2. Queue a `ParamChange { id, value: 0.75 }` in a pre-allocated ring buffer.
3. On the next `process()` call, pass the queued changes via `ProcessData.inputParameterChanges`.

**Never** call `IEditController::setParamNormalized` from the audio thread.
Parameter changes must go through `ProcessData` — this is the VST3 sample-
accurate parameter change mechanism.

## Transport Sync

VST3 expects `ProcessContext` with transport position, BPM, and time signature.
Map from `TransportInfo`:

```rust
let ctx = ProcessContext {
    state: (if transport.is_playing { kPlaying } else { 0 }),
    sampleRate: transport.sample_rate as f64,
    projectTimeSamples: transport.position_samples as i64,
    tempo: transport.bpm as f64,
    // ...
};
```

This is straightforward data translation — no allocation.

## State Serialisation

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

## Latency Reporting

Plugins that introduce latency (linear phase EQ, look-ahead compressor)
must report it:

```rust
fn latency_samples(&self) -> u32 {
    unsafe { (*self.processor).getLatencySamples() as u32 }
}
```

The graph compiler collects these values to compute **latency compensation**
(future work): delay non-latency paths so all signals align at the master.

## Crash Isolation (Best Effort)

VST3 plugins run in-process. A crashing plugin takes down the entire engine.
Best-effort mitigations:
1. Validate all plugin-provided pointers before dereferencing.
2. Wrap the `process()` call in platform-specific signal handling (future).
3. Out-of-process hosting via a subprocess bridge (major future work).

## Rules Summary

| Rule                                             | Rationale                              |
|--------------------------------------------------|----------------------------------------|
| Graph runtime never imports `plugin_formats`     | Format isolation                       |
| No allocation in `VstWrapperNode::process`       | Real-time safety                       |
| Pre-allocate all pointer arrays in `prepare`     | Real-time safety                       |
| Parameter changes via `ProcessData`, not `IEditController::setParamNormalized` | Sample-accurate, thread-safe |
| Transport from `TransportInfo`, not global state | Thread safety                          |
| State via `IBStream` bytes, not JSON             | VST3 spec compliance                   |
| Latency reported via `latency_samples()`         | Future compensation support            |
