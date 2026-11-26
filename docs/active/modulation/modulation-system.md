# Modulation System

> **Canonical.** Merges the former `rules/modulation-rules.md` (ownership rules)
> and the legacy `modulation-system-design.md` (data types + execution model).
> The legacy flat file is archived under `docs/archive/modulation-system-design.md`.

## Graph-Level Modulation Philosophy

Modulation in Echoes Studio is a **graph-level** system, not a plugin-level
system. A plugin does not contain its own LFO or envelope. Instead, modulator
_nodes_ exist in the graph alongside DSP nodes, and their output is routed to
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

The plugin receives the _effective_ value as a per-frame float array
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

| Kind            | Description                                                             |
| --------------- | ----------------------------------------------------------------------- |
| `Lfo`           | Periodic waveform (sine/tri/saw/square); rate in Hz or tempo-synced     |
| `Envelope`      | ADSR (0…1), gated by the track's `InstrumentNode` (any note held)       |
| `Macro`         | User-controlled value (0…1), fans out to multiple targets               |
| `AudioFollower` | Level (0…1, RMS or peak) of the track's post-FX signal or of one effect |

All four are exposed in the UI (`ModulationPanel` "+" menu, one card per kind).

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
smoother (12 Hz) whose target switches at each grid point. It de-zippers
modulated/automated gain. In `TrackGainNode` its base is the volume fader,
itself ramped in dB at 4 Hz to de-click fader moves; at rest the fader
contributes its exact value, so modulated renders are unchanged from ADR-019
(`SmoothedGain::process_over_fader`). Mute/solo is a gate multiplied in after
modulation, so modulation can never make a muted track audible. All four internal
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

| Target                                                                              | `depth` unit                                                   |
| ----------------------------------------------------------------------------------- | -------------------------------------------------------------- |
| `TrackGain`                                                                         | linear gain (fader 0…100 → 0…1); the result never goes below 0 |
| `EffectParam` (internal effect)                                                     | plain units (Hz, dB, ms…)                                      |
| `EffectParam` on `low_freq_octaves` / `mid_freq_octaves` / `high_freq_octaves` (EQ) | octaves: `freq × 2^delta` (ADR-019 amendment)                  |
| `PluginParam`                                                                       | normalised (1.0 = the parameter's full range, ADR-021)         |

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

> **Absolute automation (ADR-021, ADR-025):** automation reaches nodes through
> `apply_automation*` and **sets** the parameter — plugin parameters (ADR-021),
> track volume and the four internal effects (ADR-025, 2026-09-29); modulation
> **offsets** it around that value (CLAP `PARAM_MOD` for plugins). An internal
> node maps the lane's 0…1 value to its own range (`automated_value` in
> `nodes/mod.rs`; linear, or logarithmic for EQ frequencies and compressor
> attack/release; track volume = the fader's 0…2 linear gain) and forwards
> `target − current setting` to its existing modulation path, so `modulated()`
> lands exactly on the target. The lane wins over the knob while it has points.

Both deliver per-frame float values to a parameter. They differ in source and
timing:

|              | Automation                           | Modulation                        |
| ------------ | ------------------------------------ | --------------------------------- |
| **Source**   | Pre-baked timeline curve (f32 slice) | Live DSP signal from another node |
| **Computed** | Offline, before playback             | Real-time, every block            |
| **Editable** | In the timeline/curve editor         | In the graph                      |
| **Example**  | Volume fade over 4 bars              | LFO on filter cutoff              |

Both are unified at the `ParameterBlock` level — the node's `process()` receives
a single per-frame float array regardless of source.

**Editor (2026-09-29).** Lanes sit under their track in the arrange view
(`AutomationLane.svelte`, `AutomationLaneHeader.svelte`; row heights in
`arrangeInteraction.ts`). The UI offers track volume, internal effect
parameters and automatable plugin parameters (`buildAutomationOptions`). It
edits a lane as a single clip from 0 s to its last point (`laneClipFor`), since
the engine drops points outside their clip window; each gesture sends one
`UPDATE_AUTOMATION_CLIP` (one undo step). The lane header toggles linear / step
with `UPDATE_AUTOMATION_LANE` (`AUTOMATION_LANE_UPDATED`, one undo step; the
recompile picks up the new `InterpolationMode`). A header menu fills the active loop region with a
ramp or one sine cycle (`withShape` in `automation/lanes.ts`, one `UPDATE_AUTOMATION_CLIP`): it replaces the
points under the span, on the full 0..1 range; ramps on a step lane are sampled in 16 steps. Times are in seconds, so points do not
follow a tempo change. The pencil (`Ctrl/Cmd` + drag) samples one value per
pixel column and simplifies the stroke with Ramer–Douglas–Peucker (1.5 px
tolerance) before sending it. Displayed values are the plain value in the
parameter's own unit (`automationRange` / `formatAutomationValue`, using the same
range and scale as the engine — keep `INTERNAL_PARAMS` in `targets.ts` in sync
with the Rust `*_RANGES` and `*_SCALES`); for plugins the plugin's own text
formatting is not queried.

**Writing live (ADR-037, 2026-11-26).** A plugin lane has a record button in its
header (`stores/automationWrite.ts`). Armed, and during playback only, a drag of the
parameter in the plugin window becomes points: the engine stamps each edit with the
project time, `watch_param_edits` pairs them into a gesture (points thinned to one per
5 ms) and the frontend replaces the lane's points between the first and last point of
the gesture (`withRecordedGesture`) with one `UPDATE_AUTOMATION_CLIP`. While the
parameter is dragged its automation is not sent. Steps 1 and 2 cover CLAP plugins, in the
Echoes process and in the bridge (the block reply carries the edits, so the touch
pause of the lane lags one block there); VST3 follows (ADR-037, table of steps). Not hand-tested.

**Automating from a knob (2026-09-29).** Right-clicking a `Knob` or the volume
`FaderMeter` that carries a `modTargetKey` posts an `automationRequest`
(`stores/automation.ts`); `AutomationKnobMenu.svelte` (mounted in `MainPanel`)
resolves the key against `buildAutomationOptions` for the selected track and adds
or removes the lane. The same key marks the knob as automated: `automatedTargets`
lists, per track, the targets whose lane has at least one point (an empty lane is
not compiled, so the knob keeps control) and the knob draws a solid ring in the
track color. While a lane drives it, the knob or fader shows the lane's value at the
playhead (`automatedValues`: `automationValueAt` mirrors the engine's evaluation —
held before the first and after the last point, linear or step — mapped to the
parameter's plain value with `toPlainValue`); it shows its own value while the user
drags it. This is a display derived from the mirrored lanes and the transport
position, computed in the UI: nothing is sent to the engine.

---

## Current Status

_Verified 2026-09-24._ The runtime application pass is live: the scheduler
evaluates every route on the absolute control grid (ADR-019, Tranches 1–3 done).

| Target                                              | Sub-block delivery                                                                                                                                                                                                                                                                                                    |
| --------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| VST3 / CLAP plugin params (effects and instruments) | one native event per grid point; modulation offsets the value, automation sets it (ADR-021); CLAP automatable-only params get `base + offset` as values; VST3 always gets `base + offset` as normalised values, in the bridge too (the engine computes it, the bridge converts nothing; ADR-021 amendment 2026-11-04) |
| `GainNode` / `TrackGainNode`                        | per-sample one-pole smoother (12 Hz) whose target switches at each grid point; in `TrackGainNode` it rides on the dB-smoothed fader                                                                                                                                                                                   |
| `Delay` / `Compressor` / `Reverb` / `Eq`            | `process` loop segmented at grid points, params recomputed per segment and clamped to their range                                                                                                                                                                                                                     |
| `PanNode` / `MasterNode`                            | not modulation targets                                                                                                                                                                                                                                                                                                |

The block-rate `ModSmooth` was removed on 2026-09-29 (unused since ADR-019;
recoverable from git history). Projects without
modulation or automation render exactly as before ADR-019.

**Editing a modulator (`UPDATE_MODULATOR`):**

| Kind          | Hot (atomics, no recompile)                                              | Recompiles                                                                                                                                                                     |
| ------------- | ------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| LFO           | `rate_hz`, `tempo_sync`, `beat_division` (`ModulatorCells`)              | `waveform` — baked into `LfoNode`; the new node restarts its phase                                                                                                             |
| Envelope      | `attack_ms`, `decay_ms`, `sustain_level`, `release_ms` (`EnvelopeCells`) | —                                                                                                                                                                              |
| Macro         | `value` (`MacroCells`)                                                   | —                                                                                                                                                                              |
| AudioFollower | —                                                                        | `window_ms`, `attack_ms`, `release_ms`, `mode` — baked into `AudioFollowerNode` (ring buffer sized in `prepare`); `source_effect_id` — rewires the follower's audio input edge |

Every field is replayed from `MODULATOR_UPDATED` events (`apply_modulator_updated`).
The follower's `source_effect_id` taps the output of that effect (`null` = track
post-FX, pre-fader). In `UPDATE_MODULATOR` an absent field leaves it unchanged
and `null` clears it, so `MODULATOR_UPDATED` only carries the key when it was
edited. The command rejects an id not on the track; a tap whose effect is later
removed, or is a disabled internal effect, falls back to post-FX at compile time.

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
