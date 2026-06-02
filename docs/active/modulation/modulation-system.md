# Modulation System

> **Canonical.** Merges the former `rules/modulation-rules.md` (ownership rules)
> and the legacy `modulation-system-design.md` (data types + execution model).
> The legacy flat file is archived under `docs/archive/modulation-system-design.md`.

## Graph-Level Modulation Philosophy

Modulation in Echoes Studio is a **graph-level** system, not a plugin-level
system. A plugin does not contain its own LFO or envelope. Instead, modulator
*nodes* exist in the graph alongside DSP nodes, and their output is routed to
target parameters via modulation edges.

```
LfoNode   ──[modulation edge]──► reverb.room_size
LfoNode   ──[modulation edge]──► delay.feedback
MacroKnob ──[modulation edge]──► filter.cutoff
```

One LFO node can modulate 20 parameters with a single instance running once per
block. Plugin-internal modulators would require one instance per plugin per
modulated parameter, wasting DSP budget. This also means:
- Modulators are reusable and visible/editable in the graph.
- Plugins have zero internal modulation logic.
- The modulation system is uniform across every plugin format.

---

## Parameter Modulation Ownership

> **The graph owns the modulation routing. The plugin owns nothing except its
> current parameter values.**

When the scheduler applies modulation to a parameter before calling `process()`:

```
effective_value[frame] = base_value + mod_signal[frame] * depth
```

The plugin receives the *effective* value as a per-frame float array
(`ParameterBlock`). It does not know whether the value came from a user knob, an
LFO, automation, or a macro. This uniformity is the entire point:
- The plugin author does not handle modulation logic.
- The system works identically for every plugin format (internal, VST3, AU,
  CLAP, future WASM).
- Modulation can be added, rerouted, or removed without touching the plugin.

---

## Data Types

```rust
struct ModulationSource {
    node: NodeId,   // the modulator node
    port: u32,      // output port (0 = main modulation output)
}

struct ModulationTarget {
    param: ParamId,  // { node, name }
    depth: f32,      // modulation depth in plain (denormalised) units (signed)
}

struct ModulationRoute {
    source: ModulationSource,
    target: ModulationTarget,
}

// Compiled routing table, stored in ExecutionPlan:
struct ModulationMap {
    routes: Vec<ModulationRoute>,
}
```

These live in `audio_graph/modulation.rs`.

---

## Modulator Node Types

| Kind        | Description                                          |
|-------------|------------------------------------------------------|
| `Lfo`       | Periodic waveform (sine/tri/saw/square); rate in Hz  |
| `Envelope`  | ADSR / multi-stage, triggered by gate or MIDI        |
| `Macro`     | User-controlled knob, fans out to multiple targets   |
| `AudioRate` | Audio-rate (sample-by-sample) modulation signal      |

Any node acting as a modulation source must:
1. Implement `AudioNode` normally (produce output on port 0).
2. Output a normalised signal in the range `−1.0 … +1.0`.
3. Be topologically ordered before its targets (the modulation edge creates the
   dependency; the compiler handles this automatically).
4. Declare `num_output_ports() >= 1` (port 0 = modulation output).

---

## Execution Model

During a block:

1. Modulator nodes run **before** their target nodes (enforced by topological
   order — the modulator → parameter-target edge creates the dependency).
2. After the modulator runs, its output buffer contains the block's modulation
   curve.
3. Before calling `node.process()`, the scheduler applies modulation:

```
for each route targeting this node:
    mod_signal = output_buffers[source_step][source_port]
    for frame in 0..block_size:
        effective_value[frame] = base_value + mod_signal[frame] * depth
```

The result is clamped to the parameter's `ParameterRange` by the scheduler. The
node receives the per-frame parameter curve in a `ParameterBlock` (future:
sample-accurate interpolation). The modulation application step runs between the
modulator's `process()` and the target's `process()`, so the modulator's output
is always fresh for the current block.

### Modulation depth and units

Depth is expressed in **plain (denormalised) units** of the target parameter:

```rust
ModulationTarget {
    param: ParamId { node: reverb_id, name: "room_size" },
    depth: 0.3,  // ± 0.3 in room_size plain units
}
```

The modulator's `−1.0 … +1.0` output is scaled by `depth` and added to the base
value before being passed to the node.

---

## Automation vs Modulation — The Distinction

Both deliver per-frame float values to a parameter. They differ in source and
timing:

|            | Automation                          | Modulation                        |
|------------|-------------------------------------|-----------------------------------|
| **Source** | Pre-baked timeline curve (f32 slice)| Live DSP signal from another node |
| **Computed** | Offline, before playback          | Real-time, every block            |
| **Editable** | In the timeline/curve editor      | In the graph                      |
| **Example** | Volume fade over 4 bars            | LFO on filter cutoff              |

Both are unified at the `ParameterBlock` level — the node's `process()` receives
a single per-frame float array regardless of source.

---

## Current Status

The `ModulationMap` is compiled into every `ExecutionPlan` but currently
contains zero routes (no modulation edges in the `AudioGraph` yet). The data
structures are complete; the runtime application pass in the scheduler is the
next milestone after the base graph is stable. Until routes exist, this system
has no runtime cost.

---

## Future Extensibility

1. **Audio-rate modulation**: a source node producing a full audio signal on its
   modulation port (ring modulation, AM synthesis). The scheduler already
   supports per-frame values — the only change is using the full block buffer.
2. **Modulation matrix view**: a UI showing all active routes (a filtered edge
   list, since routing lives in the graph).
3. **Stacked modulation**: multiple sources summed onto the same target before
   clamping.
4. **MIDI-triggered envelopes**: an `EnvelopeNode` with a gate input port
   connected to a MIDI node's gate output.

---

## Anti-Pattern: Plugin-Internal Modulation

**Do NOT** implement a modulator inside a plugin:

```rust
// ✗ WRONG: LFO inside a plugin
fn process(&mut self, ...) {
    let mod_val = self.lfo.advance();  // plugin owns LFO
    let gain = self.base_gain + mod_val * self.depth;
    ...
}
```

This makes the LFO invisible to the modulation routing system, prevents reuse
across parameters, and couples DSP to project-level routing. **Correct
approach:** the plugin exposes a `gain` parameter, the graph has an LFO node, and
the scheduler applies the modulation before calling process. See
`../architecture/anti-patterns.md`.

---

## Further Reading

- `../architecture/audio-graph.md` — graph-first philosophy
- `../dsp/graph-compiler.md` — how `ModulationMap` is built into the plan
- `../plugins/plugin-system.md` — the parameter system modulation feeds into
