# Modulation System Design

## Core Principle

Modulation is a **graph-level** system. Plugins do not own their modulators.
A reverb plugin does not contain an LFO — instead, an LFO *node* in the graph
connects to the reverb node's parameter via a modulation edge.

```
LFO Node (ModulatorNode)
    │  modulation edge
    ▼
ModulationTarget { node_id: reverb_node, param: "room_size", depth: 0.3 }
```

This means:
- Modulators are reusable (one LFO can target 20 parameters)
- Modulation routing is visible and editable in the graph
- Plugins have zero internal modulation logic
- The modulation system is uniform across all plugin formats

## Data Types

### ModulationSource

```rust
struct ModulationSource {
    node: NodeId,   // the modulator node
    port: u32,      // output port (0 = main modulation output)
}
```

### ModulationTarget

```rust
struct ModulationTarget {
    param: ParamId,     // { node, name }
    depth: f32,         // modulation depth in plain units (signed)
}
```

### ModulationRoute

```rust
struct ModulationRoute {
    source: ModulationSource,
    target: ModulationTarget,
}
```

### ModulationMap

The compiled routing table, stored in `ExecutionPlan`:

```rust
struct ModulationMap {
    routes: Vec<ModulationRoute>,
}
```

## Modulator Node Types

| Kind      | Description                                          |
|-----------|------------------------------------------------------|
| `Lfo`     | Periodic waveform (sine/tri/saw/square); rate in Hz |
| `Envelope`| ADSR / multi-stage, triggered by gate or MIDI       |
| `Macro`   | User-controlled knob, fans to multiple targets       |
| `AudioRate`| Audio-rate (sample-by-sample) modulation signal     |

All modulator types implement `AudioNode` — they produce a normalised
modulation signal (−1.0 … +1.0) on their output port.

## Execution Model

During a block:

1. Modulator nodes run **before** their target nodes (enforced by topological
   order — modulator → parameter target edge creates the dependency).
2. After the modulator runs, its output buffer contains the block's modulation
   curve.
3. Before calling `node.process()`, the scheduler applies modulation:

```
for each route targeting this node:
    mod_signal = output_buffers[source_step][source_port]
    for frame in 0..block_size:
        effective_value[frame] = base_value + mod_signal[frame] * depth
```

4. The node receives the per-frame parameter curve in a `ParameterBlock`
   (future: sample-accurate interpolation).

## Current Status

The `ModulationMap` is built by the compiler but contains zero routes
(no modulation edges in the `AudioGraph` yet). The full runtime application
of modulation offsets is the next milestone after the base graph is stable.

## Future: Sample-Accurate Automation

The same mechanism handles automation:

```
TimelineCurve
    │  automation lane
    ▼
parameter value per frame (pre-computed as f32 slice)
    │
    ▼
node.process() reads per-frame values instead of a single base value
```

The difference from modulation:
- **Automation** is read from pre-baked `f32` slices (offline curve evaluation)
- **Modulation** is a live signal from another DSP node (real-time)

Both are unified at the `ParameterBlock` level — the node sees a single
per-frame f32 array regardless of source.

## Anti-Pattern: Plugin-Internal Modulation

**Do NOT** implement modulation inside a plugin:

```rust
// ✗ WRONG: LFO inside a plugin
fn process(&mut self, ...) {
    let mod_val = self.lfo.advance();  // plugin owns LFO
    let gain = self.base_gain + mod_val * self.depth;
    ...
}
```

This makes the LFO invisible to the modulation routing system, prevents
reuse across parameters, and couples DSP to project-level routing.

**Correct approach**: the plugin has a `gain` parameter, the graph has an
LFO node, and the scheduler applies the modulation before calling process.
