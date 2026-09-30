> **⚠️ ARCHIVED — SUPERSEDED.** This legacy flat document has been merged into
> the canonical doc and is kept for history only. **Do not edit.**
> Canonical: `docs/active/plugins/plugin-system.md`

---

# Plugin System Architecture

## Core Design Rule

> **The plugin system is a support layer. The audio graph is the core.**

Plugins are nothing more than `AudioNode` implementations. The graph runtime
has zero knowledge of plugin formats, vendors, or loading mechanisms.

## Abstraction Stack

```
External plugin binary (VST3 / AU / WASM)
          │
          ▼  Format-specific wrapper
Device trait  (extends AudioNode)
          │
          ▼  Graph sees only this
AudioNode trait  (process, prepare, reset)
          │
          ▼
AudioGraph  (format-agnostic DSP execution)
```

## Device Trait

```rust
pub trait Device: AudioNode {
    fn device_id(&self) -> &str;
    fn info(&self) -> &DeviceInfo;
    fn parameters(&self) -> &ParameterMap;
    fn parameters_mut(&mut self) -> &mut ParameterMap;
    fn save_state(&self) -> DeviceState;
    fn load_state(&mut self, state: &DeviceState);
    fn lifecycle_state(&self) -> LifecycleState;
    fn suspend(&mut self);
    fn resume(&mut self);
    fn destroy(&mut self);
}
```

The `Device` trait adds plugin-specific concerns (state, parameters, lifecycle)
on top of the `AudioNode` DSP interface. The graph only ever calls `AudioNode`
methods.

## PluginHost

The `PluginHost` manages plugin instances on the **command thread** (never the
audio thread):

1. **Discovery**: scans `PluginRegistry` for installed plugins
2. **Instantiation**: creates `Box<dyn AudioNode>` wrappers
3. **Parameter routing**: translates UI parameter changes to device format
4. **Lifecycle management**: enforces valid state transitions
5. **State serialisation**: saves/loads `DeviceState` for project files

### Instantiation flow

```
PluginHost::instantiate(node_id, "vst3:my-plugin")
    │
    ▼  lookup in PluginRegistry
PluginManifest { format: Vst3, path, ... }
    │
    ▼  format dispatch
VstWrapperNode::new(node_id, ...)
    │  implements AudioNode + Device
    ▼
Box<dyn AudioNode>  → handed to GraphCompiler → placed in ExecutionPlan
```

## PluginRegistry

The registry holds `PluginManifest` entries for every known plugin. It is
populated at startup by:

1. Built-in device registration (Gain, Pan, Bus, Master, etc.)
2. Platform-specific scanners (VST3 dirs, AU database, WASM plugin dir)

Scanning is lazy and non-blocking. The registry is **not** on the audio path.

## Parameter System

Each device has a `ParameterMap` containing `Param` entries:

```rust
Param {
    key: ParamKey,      // stable string ID ("gain", "cutoff")
    name: String,       // display name
    range: ParameterRange,  // min/max/default + mapping (linear/log/dB)
    value: f32,         // current plain value
    modulation_offset: f32, // sum of modulation for this block
}
```

### Normalisation

All parameters support `normalise(plain) → [0,1]` and
`denormalise(normalised) → plain`. The modulation system operates in normalised
space; the node receives the plain value.

### Automation

Parameters record `automation_lane_ids`. The timeline engine reads these to
deliver sample-accurate automation buffers before each block.

## Lifecycle State Machine

```
Created → Loaded → Initialized → Running ⇄ Suspended
                                     │
                                     ▼
                                Destroyed (terminal)
                                Error     (terminal)
```

Invalid transitions are rejected with `LifecycleError`. The host enforces this
before calling any format-specific API.

## State Serialisation

`DeviceState` is an opaque blob stored in the project file:

```rust
DeviceState {
    device_id: String,
    format: StateFormat,    // Json | Binary | Empty
    data: StateData,        // JSON value or raw bytes
}
```

Internal devices use JSON. VST3/AU plugins use their binary chunk format.
The host treats it as a black box — only the device knows how to restore from it.

## Format Isolation Rule

**The graph runtime must never import `plugin_formats` or `plugin_system`.**

Only these files are allowed to touch plugin-format specifics:

- `plugin_formats/vst3/wrapper.rs`
- `plugin_formats/au/wrapper.rs`
- `plugin_formats/internal/wrapper.rs`
- `plugin_system/host.rs` (via format dispatch)

The `audio_graph/compiler.rs` calls `build_node()` which returns
`Box<dyn AudioNode>`. That is the boundary.
