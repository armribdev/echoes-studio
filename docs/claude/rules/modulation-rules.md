# Modulation Rules

## Graph-Level Modulation Philosophy

Modulation in Echoes Studio is a **graph-level** system, not a plugin-level
system. A plugin does not contain its own LFO or envelope. Instead, modulator
nodes exist in the graph alongside DSP nodes, and their output is routed to
target parameters via modulation edges.

```
LfoNode ──[modulation edge]──► reverb.room_size
LfoNode ──[modulation edge]──► delay.feedback
MacroKnob ──[modulation edge]──► filter.cutoff
```

One LFO node can modulate 20 parameters with a single instance running once per
block. Plugin-internal modulators would require one instance per plugin per
modulated parameter, wasting DSP budget.

---

## Parameter Modulation Ownership

**The graph owns the modulation routing. The plugin owns nothing except its
current parameter values.**

When the scheduler applies modulation to a parameter before calling `process()`:

```
effective_value[frame] = base_value + mod_signal[frame] * depth
```

The plugin receives the *effective* value as a per-frame float array
(`ParameterBlock`). It does not know whether the value came from a user knob,
an LFO, automation, or a macro.

This uniformity is the entire point:
- The plugin author does not need to handle modulation logic.
- The modulation system works identically for every plugin format (internal,
  VST3, AU, future WASM).
- Modulation can be added, rerouted, or removed without touching the plugin.

---

## Automation vs Modulation — The Distinction

Both deliver per-frame float values to a parameter. They differ in source and timing:

| | Automation | Modulation |
|-|-----------|-----------|
| **Source** | Pre-baked timeline curve (f32 slice) | Live DSP signal from another node |
| **Computed** | Offline, before playback | Real-time, every block |
| **Editable** | In the timeline/curve editor | In the graph |
| **Example** | Volume fade over 4 bars | LFO on filter cutoff |

Both are unified at the `ParameterBlock` level. The node's `process()` receives
a single per-frame float array regardless of source.

---

## Modulator Node Requirements

Any node that acts as a modulation source must:

1. Implement `AudioNode` normally (produce output on port 0).
2. Output a signal in the normalised range `−1.0 … +1.0`.
3. Be topologically ordered before its targets (the modulation edge creates the
   dependency; the compiler handles this automatically).
4. Declare `num_output_ports() == 1` minimum (port 0 = modulation output).

The scheduler's modulation application step runs between the modulator's
`process()` call and the target node's `process()` call, so the modulator's
output is always fresh for the current block.

---

## Modulation Depth and Units

Depth is expressed in **plain (denormalised) units** of the target parameter:

```rust
ModulationTarget {
    param: ParamId { node: reverb_id, name: "room_size" },
    depth: 0.3,  // ± 0.3 in room_size plain units
}
```

This means the modulator's `−1.0 … +1.0` output is scaled by `depth` and
added to the base value before being passed to the node. The result is clamped
to the parameter's `ParameterRange` by the scheduler.

---

## Current Status

The `ModulationMap` is compiled into every `ExecutionPlan` but currently
contains zero routes. The data structures (`ModulationRoute`, `ModulationMap`,
`ModulationSource`, `ModulationTarget`) are in `audio_graph/modulation.rs` and
are complete. The runtime application pass in the scheduler is the next
milestone.

Until modulation routes exist in the graph, this system has no runtime cost.

---

## Future Extensibility Goals

1. **Audio-rate modulation**: a source node producing a full audio signal on
   its modulation port (ring modulation, AM synthesis). The scheduler already
   supports per-frame values — the only change is using the full block buffer
   instead of a scalar.
2. **Modulation matrix view**: a UI showing all active routes. Because routing
   lives in the graph, the view is just a filtered edge list.
3. **Stacked modulation**: multiple sources summed onto the same target.
   The scheduler sums all routes targeting the same parameter before clamping.
4. **MIDI-triggered envelopes**: an `EnvelopeNode` with a gate input port
   connected to a MIDI node's gate output.

---

## The Anti-Pattern: Plugin-Internal Modulation

Never implement a modulator inside a plugin. See `anti-patterns.md` §7 for the
full explanation and the correct approach.

---

## Further Reading

- `../modulation-system-design.md` — data types, execution model with code examples,
  current status, sample-accurate automation roadmap
