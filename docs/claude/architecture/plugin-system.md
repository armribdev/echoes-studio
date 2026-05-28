# Plugin System Architecture

## Core Design Rule

> **The plugin system is a support layer. The audio graph is the core.**

Plugins are `AudioNode` implementations. The graph runtime has no knowledge of
plugin formats, vendors, or loading mechanisms. From the scheduler's perspective,
a VST3 reverb and a built-in gain node are identical.

---

## Abstraction Stack

```
External binary (VST3 / AU / WASM)
        │
        ▼  format-specific wrapper (plugin_formats/)
Device trait    ← plugin lifecycle, parameters, state
        │
        ▼  graph sees only this
AudioNode trait ← process(), prepare()
        │
        ▼
AudioGraph / GraphScheduler  ← format-agnostic DSP execution
```

The `Device` trait extends `AudioNode` with plugin-specific concerns:
parameter maps, lifecycle state, and opaque state persistence. The graph
calls only `AudioNode` methods. `Device` is for the `PluginHost`, not the
scheduler.

---

## Internal vs External Plugins

**Internal plugins** (built-in nodes): `TrackNode`, `GainNode`, `PanNode`,
`MeterNode`, `BusNode`, `MasterNode`. These are native Rust structs that
implement `AudioNode` directly. They are always available, never scanned, and
have no lifecycle beyond graph compilation.

**External plugins** (VST3 / AU / future WASM): loaded from disk at runtime,
wrapped by format-specific code in `plugin_formats/`. They implement `Device`
(which extends `AudioNode`) and are managed by `PluginHost`. The graph sees
them as opaque `Box<dyn AudioNode>`.

The distinction matters for:
- **Stability**: internal nodes cannot crash; external plugins can.
- **Latency**: external plugins may report latency via `latency_samples()`.
- **State**: internal nodes have no serializable state beyond their `NodeKind`
  parameters; external plugins carry a `DeviceState` blob.
- **Lifecycle**: internal nodes have no lifecycle beyond prepare/process;
  external plugins go through Created → Loaded → Initialized → Running.

---

## Plugin Lifecycle

```
Created → Loaded → Initialized → Running ⇄ Suspended
                                     │
                                     ▼
                                Destroyed (terminal)
                                Error     (terminal)
```

The `PluginHost` enforces valid transitions on the command thread. Invalid
transitions are rejected before any format-specific API is called.

**Plugins are never initialized or destroyed from the audio thread.** Lifecycle
management is a command-thread concern.

---

## Parameter Ownership

Parameters belong to the graph, not the plugin binary. A VST3 parameter
change from the UI follows this path:

```
UI knob change
  ─► dispatcher.ts (typed command)
  ─► send_command (Tauri IPC)
  ─► engine_gateway: UPDATE_PLUGIN_PARAMETER
  ─► PluginHost::set_parameter_normalised(key, value)
  ─► queued in ring buffer (pre-allocated, lock-free)
  ─► delivered via ProcessData.inputParameterChanges on next process()
```

The plugin's parameter state lives in `ParameterMap` on the command thread.
What reaches the audio thread is the queued change in `ProcessData` — not a
reference to the map.

Parameters are identified by stable string keys (`"gain"`, `"cutoff"`) in our
system, mapped to format-specific integer IDs (e.g. VST3 `ParamID`) by the
wrapper at prepare time.

---

## Opaque Plugin State

`DeviceState` is a black box stored in the project file:

```rust
DeviceState {
    device_id: String,
    format: StateFormat,    // Json | Binary | Empty
    data: StateData,        // JSON value or raw bytes
}
```

The host stores and restores this blob but never interprets it. Internal
devices use JSON for legibility. VST3/AU plugins use their native binary chunk
format (IBStream bytes). The host does not need to understand either.

This is important for long-term compatibility: if a plugin's internal state
format changes across versions, the host is unaffected. The plugin handles its
own migration.

---

## Graph Integration Rules

1. The `audio_graph` crate **must not** import `plugin_system` or
   `plugin_formats`. The compiler calls `build_node()` which returns
   `Box<dyn AudioNode>` — that is the boundary.
2. Plugins are handed to the compiler as opaque `AudioNode` boxes.
3. After compilation, plugin nodes are indistinguishable from built-in nodes
   from the scheduler's perspective.
4. Plugin discovery (`PluginRegistry`) is a startup concern, not a hot path.
   It is never called from the audio thread.

---

## Format Isolation

Only these files may touch format-specific code:

- `plugin_formats/vst3/wrapper.rs`
- `plugin_formats/au/wrapper.rs`
- `plugin_formats/internal/wasm_runtime.rs`
- `plugin_system/host.rs` (dispatches to format wrappers)

If a future change requires the graph compiler or scheduler to know the plugin
format, that is an architectural regression and should be refused.

---

## Further Reading

- `../plugin-system-architecture.md` — Device trait, PluginHost internals,
  parameter normalisation, lifecycle state machine
- `systems/plugin-wrapper-system.md` — VST3/AU wrapper implementation rules
- `decisions/003-plugin-wrapper-strategy.md` — why wrappers instead of deep integration
