# ADR-003 — Plugin Wrapper Strategy

## Status

Accepted. Architecture scaffolded in `plugin_formats/`. Implementation pending
(Phase 4).

---

## Context

Echoes Studio will support third-party audio plugins (VST3, AU). The question
is how deeply to integrate plugin formats into the engine's core abstractions.

Three approaches were considered:

1. **Deep integration** — the graph runtime understands VST3/AU natively.
   `GraphScheduler` dispatches on plugin format.
2. **Thin wrapper** — format-specific code is isolated in wrapper structs that
   implement `AudioNode`. The graph sees only `AudioNode`.
3. **Out-of-process hosting** — plugins run in a subprocess, communicate with
   the engine via shared-memory ring buffers.

---

## Decision

**Thin wrappers.** Each format (VST3, AU, WASM) has a wrapper struct in
`plugin_formats/` that implements `AudioNode + Device`. The graph runtime
never imports `plugin_formats` or `plugin_system`. The isolation boundary is
the `AudioNode` trait.

```
plugin_formats/vst3/wrapper.rs  ←  VstWrapperNode: AudioNode + Device
plugin_formats/au/wrapper.rs    ←  AuWrapperNode:  AudioNode + Device
plugin_system/host.rs           ←  PluginHost: lifecycle, dispatch
audio_graph/compiler.rs         ←  calls build_node() → Box<dyn AudioNode>
```

The graph compiler calls `PluginHost::instantiate()`, receives
`Box<dyn AudioNode>`, and proceeds identically to how it handles built-in nodes.

---

## Consequences

**Positive:**

- **Format-agnostic engine.** Adding support for a new plugin format (CLAP,
  LV2) requires only a new wrapper module. The scheduler, compiler, and
  buffer allocator are untouched.
- **Isolation of unsafe code.** VST3 requires `unsafe` FFI. This is
  contained in `plugin_formats/vst3/wrapper.rs`. The rest of the codebase
  is safe Rust.
- **Uniform DSP execution.** Built-in nodes and plugin nodes execute via the
  same code path. No conditional branches in the hot loop.
- **Format independence of automation/modulation.** Because plugins are just
  `AudioNode`s, modulation and automation work identically for built-in nodes
  and plugins.

**Negative:**

- **In-process isolation.** A crashing VST3 plugin takes down the engine when it
  runs in-process. Thin wrappers provide no process isolation by themselves; since
  ADR-033 step 5 (2026-11-04) a VST3 **effect** can run in the sandbox bridge (opt-in,
  ADR-015), and instruments follow with step 8. Documented in
  `../plugins/plugin-wrapper-system.md`.
- **Translation overhead.** The wrapper must translate buffer formats and
  parameter representations on every block. For VST3 this is pointer filling
  (negligible). For formats with heavier marshalling it could be measurable.
- **Format differences leak slightly.** Latency reporting, state serialisation
  format (binary vs JSON), and parameter ID types differ per format. The
  wrapper handles these differences, but `DeviceState` must accommodate
  both JSON and binary blobs.

---

## Alternatives Considered

### Deep Integration (Rejected)

Requiring the `GraphScheduler` to know about VST3 COM interfaces couples the
hot audio path to external SDK types. It makes the scheduler non-portable and
requires every future plugin format to touch core engine code. The compile-time
dependency graph becomes unmanageable.

### Out-of-Process Hosting (Deferred to Phase 4+)

Running plugins in a subprocess provides crash isolation — a crashing plugin
cannot take down the engine. The mechanism is shared-memory ring buffers for
audio data + IPC for parameter changes.

This is architecturally sound but complex: it requires a plugin host subprocess,
shared-memory buffer management, and latency compensation for the IPC round-trip.

The thin wrapper architecture does not block this future. The
`VstWrapperNode::process()` call currently invokes in-process FFI. It could
later be replaced with a shared-memory IPC call without changing the
`AudioNode` interface.
