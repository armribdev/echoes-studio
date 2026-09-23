# Plugin System Architecture

> **Canonical.** Merges the former `architecture/plugin-system.md` (model) and the
> legacy `plugin-system-architecture.md` (Device/PluginHost internals). The
> legacy flat file is archived under `docs/archive/plugin-system-architecture.md`.

## Core Design Rule

> **The plugin system is a support layer. The audio graph is the core.**

Plugins are `AudioNode` implementations. The graph runtime has no knowledge of
plugin formats, vendors, or loading mechanisms. From the scheduler's
perspective, a VST3 reverb and a built-in gain node are identical.

---

## Abstraction Stack

```
External binary (VST3 / AU / CLAP / WASM)
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

The `Device` trait extends `AudioNode` with plugin-specific concerns: parameter
maps, lifecycle state, and opaque state persistence. The graph calls only
`AudioNode` methods. `Device` is for the `PluginHost`, not the scheduler.

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

---

## Internal vs External Plugins

**Internal plugins** (built-in nodes): `TrackNode`, `GainNode`, `PanNode`,
`MeterNode`, `BusNode`, `MasterNode`. Native Rust structs that implement
`AudioNode` directly. Always available, never scanned, no lifecycle beyond graph
compilation.

**External plugins** (VST3 / AU / CLAP / future WASM): loaded from disk at
runtime, wrapped by format-specific code in `plugin_formats/`. They implement
`Device` (which extends `AudioNode`) and are managed by `PluginHost`. The graph
sees them as opaque `Box<dyn AudioNode>`.

The distinction matters for:
- **Stability**: internal nodes cannot crash; external plugins can.
- **Latency**: external plugins may report latency via `latency_samples()`.
- **State**: internal nodes have no serializable state beyond their `NodeKind`
  parameters; external plugins carry a `DeviceState` blob.
- **Lifecycle**: internal nodes have no lifecycle beyond prepare/process;
  external plugins go through Created → Loaded → Initialized → Running.

---

## PluginHost

The `PluginHost` manages plugin instances on the **command thread** (never the
audio thread):

1. **Discovery**: scans `PluginRegistry` for installed plugins.
2. **Instantiation**: creates `Box<dyn AudioNode>` wrappers.
3. **Parameter routing**: translates UI parameter changes to device format.
4. **Lifecycle management**: enforces valid state transitions.
5. **State serialisation**: saves/loads `DeviceState` for project files.

### Instantiation flow

```
PluginHost::instantiate(node_id, "vst3:my-plugin")
    │
    ▼  lookup in PluginRegistry
PluginManifest { format: Vst3, path, ... }
    │
    ▼  format dispatch
VstWrapperNode::new(node_id, ...)   // implements AudioNode + Device
    │
    ▼
Box<dyn AudioNode>  → handed to GraphCompiler → placed in ExecutionPlan
```

### PluginRegistry

The registry holds `PluginManifest` entries for every known plugin, populated at
startup by:
1. Built-in device registration (Gain, Pan, Bus, Master, etc.).
2. Platform-specific scanners (VST3 dirs, AU database, CLAP dirs, WASM dir).

Scanning is lazy and non-blocking. The registry is **never** on the audio path.

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
transitions are rejected with `LifecycleError` before any format-specific API is
called. **Plugins are never initialized or destroyed from the audio thread.**

### Record teardown on recompile

The host keeps one `Arc<PluginInstance>` clone per **plugin placement** in its
`records` map, keyed by the per-instance `instance_key` — the project `effect.id`
(or `instrument:{track_id}` for a track instrument) — **not** by `device_id` (the
plugin *type*). This is what lets the same plugin used on two tracks live as two
distinct records instead of one silently overwriting the other (engine audit
point 2). The UI/IPC surface addresses plugins by `instance_key` too (ADR-017):
parameter pushes (`push_plugin_param`), the GUI editor (`instance_by_key`), and
saved state (`plugin_states`, see below) are all per-placement. The legacy
`record_by_device_id` helper remains only for type-scoped queries and the
legacy-file state mapper.

After every successful compile, `GraphCompiler::compile` calls
`PluginHost::retain_devices(active)` with the set of `device_id`s the new graph
still references (matched against each record's `instance.device_id`); records for
plugin types the user removed entirely are dropped (marked `Destroyed`). This
releases the host-side clone so the FFI handle is torn down once the retiring
`CompiledGraph` frees its audio-side clone through the snapshot drop ring — always
off the audio thread (ADR-007 removal path).

> **Instance reuse (engine audit point 1 — ADR-016, 2026-06-03).** `instantiate`
> reuses the live instance held under a placement's `instance_key` instead of
> rebuilding it, when the slot still hosts the same plugin type and the format is
> reuse-safe (**internal, CLAP, VST3**; AU/WASM are stubs). A reused node is a
> fresh wrapper around the shared `Arc<PluginInstance>` — no new native handle —
> so the plugin's internal DSP state survives a structural recompile. The plugin
> activation lifecycle is owned by `PluginInstance` (`clap_ensure_active` /
> `vst3_ensure_active` + the CLAP `Drop`), not the wrapper, so a displaced wrapper
> never deactivates an instance the new graph still shares. State-restoring
> compiles (project load, staged blobs) skip reuse so the blob is applied to a
> fresh instance at instantiation (ADR-010).

---

## Parameter System

Parameters belong to the graph, not the plugin binary. Each device has a
`ParameterMap` of `Param` entries:

```rust
Param {
    key: ParamKey,           // stable string ID ("gain", "cutoff")
    name: String,            // display name
    range: ParameterRange,   // min/max/default + mapping (linear/log/dB)
    value: f32,              // current plain value
    modulation_offset: f32,  // sum of modulation for this block
}
```

### Normalisation

All parameters support `normalise(plain) → [0,1]` and
`denormalise(normalised) → plain`. The modulation system operates in normalised
space; the node receives the plain value.

### Parameter change path

A VST3 parameter change from the UI follows this path:

```
UI knob change
  ─► dispatcher.ts (typed command)
  ─► send_command (Tauri IPC)
  ─► engine_gateway: UPDATE_PLUGIN_PARAMETER
  ─► PluginHost::set_parameter_normalised(key, value)
  ─► queued in ring buffer (pre-allocated, lock-free)
  ─► delivered via ProcessData.inputParameterChanges on next process()
```

The plugin's parameter state lives in `ParameterMap` on the command thread. What
reaches the audio thread is the queued change in `ProcessData` — never a
reference to the map. Parameters are identified by stable string keys mapped to
format-specific integer IDs by the wrapper at prepare time.

### Automation

Parameters record `automation_lane_ids`. The timeline engine reads these to
deliver sample-accurate automation buffers before each block. See
`../modulation/modulation-system.md` for how automation and modulation unify at
the `ParameterBlock` level.

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

The host stores and restores this blob but never interprets it. Internal devices
use JSON for legibility. VST3/AU/CLAP plugins use their native binary chunk
format. This is important for long-term compatibility: if a plugin's internal
state format changes across versions, the host is unaffected — the plugin
handles its own migration.

**Per-instance persistence (ADR-017).** Saved state lives in the project file's
`plugin_states` map, keyed by `instance_key`, so two instances of the same plugin
type keep distinct presets. `PluginHost::collect_plugin_states` harvests it at
save (keyed by placement), fencing each live `getState` with the per-instance FFI
gate (`offrt_try_acquire`) so the harvest is safe even during playback and never
races the audio thread's `process()` (ADR-010); restore stages it by
`instance_key` and applies it at instantiation (ADR-010 race-free). **Restore is not load-only:** a controlled
re-instantiation (e.g. a terminal instance rebuilt, or a non-reuse-safe format)
carries the previous instance's state forward via `PluginInstance::restore_from`
(same plugin type only), so settings survive a rebuild and not just a project
reload (R-4).

**Missing plugins keep their state (2026-09-23).** When a placement's plugin
fails to load (not installed, failed to instantiate — effect or instrument),
the host keeps that placement's last known state in `unavailable_states`: the
project-load blob, or what an earlier failed attempt kept. `collect_plugin_states`
saves it unchanged, so saving a project with a missing plugin no longer erases
its preset. If the plugin loads later in the session, `restore_state_for`
applies the kept state. The kept state is dropped when the placement is removed
(`retain_devices`) or switched to a different plugin. Before this fix, the save
harvested only live instances and silently discarded the state of every missing
plugin. Projects saved before ADR-017 carry a legacy
`plugin_state_blobs` map keyed by `device_id`; `AppProject::from_file` maps those
onto the matching placements (C-4 backward compat), and new saves stop writing the
legacy field.

---

## Format Isolation Rule

**The graph runtime must never import `plugin_formats` or `plugin_system`.**

Only these files may touch plugin-format specifics:
- `plugin_formats/vst3/wrapper.rs`
- `plugin_formats/au/wrapper.rs`
- `plugin_formats/clap/wrapper.rs`
- `plugin_formats/internal/wrapper.rs`
- `plugin_system/host.rs` (via format dispatch)

The `audio_graph/compiler.rs` calls `build_node()` which returns
`Box<dyn AudioNode>`. That is the boundary. After compilation, plugin nodes are
indistinguishable from built-in nodes from the scheduler's perspective. If a
future change requires the graph compiler or scheduler to know the plugin
format, that is an architectural regression and should be refused.

---

## Further Reading

- `plugin-wrapper-system.md` — VST3/AU/CLAP wrapper implementation rules
- `../architecture/audio-graph.md` — how nodes are scheduled
- `../decisions/003-plugin-wrapper-strategy.md` — why wrappers instead of deep integration
- `../decisions/006-plugin-capabilities-and-pdc.md` — capabilities + PDC
- `../decisions/015-plugin-sandbox.md` — out-of-process sandbox
