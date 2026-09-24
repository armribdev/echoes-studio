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
modulation application step runs between the modulator's `process()` and the
target's `process()`, so the modulator's output is always fresh for the current
block.

### Sub-block delivery (ADR-019, Tranche 1)

The scheduler does **not** sample one value per block. It evaluates each route on
an **absolute control grid** of period `MODULATION_CONTROL_PERIOD = 32` samples
(`node.rs`): at offset 0 plus every offset where `(position_samples + offset)`
is a multiple of the period.

- **Block start** is delivered via `AudioNode::apply_modulation`. When the
  block starts off-grid, the scheduler re-delivers the held signal of the
  previous grid point (`CompiledModAction::held_signal`) instead of sampling a
  new value.
- **Interior grid points** are delivered via the defaulted
  `AudioNode::apply_modulation_at(param_id, sample_offset, value)`. The default is
  a no-op. The VST3 / CLAP wrappers record the points into `PluginParamControl` and
  emit one format-native param event per point at its real `sample_offset`.
  Internal nodes split their processing at these offsets.

The value in force at any frame is therefore the signal at the latest absolute
grid point, whatever the block size, so a modulated render is bit-identical
across block sizes (offline == live, C-2). `render_harness.rs` enforces this.
`GainNode` / `TrackGainNode` share `SmoothedGain`, a per-sample one-pole
smoother whose target switches at each grid point. It de-zippers
modulated/automated gain and de-clicks fader/mute moves. All four internal
effects (`Delay`, `Compressor`, `Reverb`, `Eq`) segment their `process` loop at
the grid points. `PanNode`/`MasterNode` are not modulation targets.

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

Units differ per target kind (the stored `depth` is always in the engine's unit):

| Target | `depth` unit |
|--------|--------------|
| `TrackGain` | linear gain (fader 0…100 → 0…1); the result never goes below 0 |
| `EffectParam` (internal effect) | plain units (Hz, dB, ms…) |
| `EffectParam` on `low_freq_octaves` / `mid_freq_octaves` / `high_freq_octaves` (EQ) | octaves: `freq × 2^delta` (ADR-019 amendment) |
| `PluginParam` | normalised (1.0 = the parameter's full range, ADR-021) |

**Bounds on internal effects (2026-09-24).** `Eq`, `Compressor`, `Reverb` and
`Delay` clamp `base + delta` to a per-parameter range (`EQ_RANGES`,
`COMP_RANGES`, `REVERB_RANGES`, `DELAY_RANGES`, via `nodes::modulated`), so a
strong LFO cannot push e.g. delay feedback above 0.95 or an EQ frequency below
20 Hz. The range is widened to include the base value, so an unmodulated
parameter — including a legacy out-of-range one — is bit-identical to before.
Automation deltas on internal nodes go through the same clamp.

**UI convention.** The modulation panel's depth slider means "± % of the
parameter's range" for every target. The frontend
(`src/frontend/modulation/targets.ts`, which mirrors the Rust range tables)
multiplies the slider by `max − min` before sending an `EffectParam` depth, and
divides back for display; `TrackGain` and `PluginParam` use the slider as is.
EQ frequencies are offered only as octave targets (scale `log2(max / min)` ≈ 10
octaves); an existing Hz route on `mid_freq` etc. is still named ("… (Hz)") and
editable but no longer offered.
The picker lists the track volume, internal effect params (those the node
actually uses — `dry` and `ping_pong` are excluded) and plugin params returned
by `get_plugin_params` with `(modulatable || automatable) && !hidden` —
automatable-only CLAP parameters (e.g. every Vital parameter) are modulated by
value (ADR-021 amendment).

---

## Automation vs Modulation — The Distinction

> **Plugin parameters (ADR-021):** automation reaches nodes through
> `apply_automation*` and **sets** a plugin parameter; modulation **offsets** it
> around its current value without moving it (CLAP `PARAM_MOD`). Internal
> nodes still receive automation additively, as described below.

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

_Verified 2026-09-24._ The runtime application pass is live: the scheduler
evaluates every route on the absolute control grid (ADR-019, Tranches 1–3 done).

| Target | Sub-block delivery |
|--------|--------------------|
| VST3 / CLAP plugin params (effects and instruments) | one native event per grid point; modulation offsets the value, automation sets it (ADR-021); CLAP automatable-only params get `base + offset` as values |
| `GainNode` / `TrackGainNode` | per-sample one-pole smoother whose target switches at each grid point |
| `Delay` / `Compressor` / `Reverb` / `Eq` | `process` loop segmented at grid points, params recomputed per segment and clamped to their range |
| `PanNode` / `MasterNode` | not modulation targets |

The block-rate `ModSmooth` (`nodes/mod_smooth.rs`) is no longer used by
production code; it is kept because ADR-009 references it. Projects without
modulation or automation render exactly as before ADR-019.

**Verification:** `audio_engine/render_harness.rs` renders modulated and
automated reference projects at several block sizes and checks them against
golden hashes. **Open gap:** the scheduler is never reset on a live seek, so the
first ≤ 31 samples after a seek can use a stale held value (see ADR-019
Follow-ups).

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
