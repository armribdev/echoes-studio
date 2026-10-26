# ADR-019 — Sub-block deterministic modulation & automation (absolute control grid)

## Status

**Accepted — fully implemented (Tranches 1, 2 & 3 done, 2026-06-07; amended
2026-09-23, see below).** Drafted
following the 2026-06-05 engine audit (finding #4a: block-rate modulation →
zipper noise) with the user's explicit acceptance that **existing modulated
projects may re-render differently** (the bit-identical break below is sanctioned,
not silent).

**Amendment 2026-09-23 — block-size independence made real.** The new offline
render harness
([`render_harness.rs`](../../../src-tauri/src/audio_engine/render_harness.rs))
showed that the modulated render was **not** block-size independent, despite
the §Decision claim. There were two causes, both fixed:

1. **Off-grid block starts.** The scheduler sampled a fresh value at offset 0
   even when `position_samples` was not on the grid, which added a point that a
   larger block does not have. The scheduler now keeps the raw signal of each
   route's latest grid point (`CompiledModAction::held_signal`) and reuses it
   when a block starts off-grid. Grid-aligned blocks (every block size that is a
   multiple of 32, including the 4096-frame offline export) are unchanged.
2. **Gain smoother on a block-rate target.** The Tranche 2 choice below ("tracks
   the block-rate target") made gain modulation depend on block boundaries.
   `GainNode` / `TrackGainNode` now share `SmoothedGain`, which switches its
   target at each grid point. This **changes the export** of projects that
   modulate or automate track gain. The change is covered by this ADR's
   re-baselining of modulated projects.

The harness now proves bit-identical output for block sizes 4096, 512, 441, 64
and 37, with and without the worker pool.

**Amendment 2026-09-24 — internal effects: bounds and EQ frequencies in
octaves** (approved by the user).

1. **Bounds.** `Eq`, `Compressor`, `Reverb` and `Delay` clamp each modulated
   value to a per-parameter range (`*_RANGES`, `nodes::modulated`), widened to
   include the base so unmodulated values are untouched. Golden hashes unchanged.
2. **Octaves.** A route may target `low_freq_octaves` / `mid_freq_octaves` /
   `high_freq_octaves` on an EQ: the delta is in octaves and applied as
   `base × 2^delta` (then clamped to 20 Hz…20 kHz). Routes on `low_freq` etc.
   keep their additive Hz delta, bit for bit, so existing projects and the
   `modulated_effects` golden render are unchanged (C-2, C-4). The UI creates
   only octave routes for EQ frequencies; ±100 % on its slider is ±log2(1000)
   ≈ ±10 octaves, the full range. New reference project `eq_octave_modulation`
   (block-size independent, golden hash recorded). Cost: one `exp2` per
   modulated segment per frequency, no allocation.

Tranche 3 — Delay, Compressor, Reverb & EQ all landed: each consumes the interior
grid points via `apply_modulation_at` and **segments its `process` loop** at the
absolute grid, recomputing effective params per segment (Compressor segments its
phase-1 envelope detection, phase-2 oversampled gain unchanged; Reverb re-applies
comb feedback/damping + wet-dry mix per segment; EQ recomputes its 3 biquads per
segment and calls `Oversample2x::process_channel` on consecutive sub-slices —
seamless because the oversampler state lives in the node and `recompute_coeffs`
preserves the biquad state). The block-rate `ModSmooth` was **dropped** from all
four (the grid is continuous across blocks and 32-sample steps are inaudible, so
no extra smoother is needed). `ModSmooth` (`mod_smooth.rs`) was unused by
production code and was **deleted on 2026-09-29** (recoverable from git history).
Unmodulated output is bit-identical.

Tranche 2 landed: `GainNode` and `TrackGainNode` gained a per-sample one-pole
gain smoother (coeff from the sample rate → block-size independent) that de-zippers
modulated/automated gain and de-clicks fader/mute/solo moves. Unmodulated static
gain is bit-identical (the smoother step is exactly 0 at target). `PanNode` and
`MasterNode` were **excluded**: neither implements `apply_modulation`/`param_id`,
so they are not modulation targets — the ADR's earlier listing of them was
aspirational. Suite: 110 tests green (2 new gain tests).

_Note 2026-09-24:_ `PluginModEntry` was replaced by `PluginParamControl`
(`audio_graph/plugin_params.rs`), which keeps modulation and automation apart
(ADR-021).

Tranche 1 landed: absolute control grid + sub-block sampling in
[`scheduler.rs`](../../../src-tauri/src/audio_graph/scheduler.rs) (`eval_mod_signal`),
the `apply_modulation_at` defaulted trait method + per-param point array on
[`PluginModEntry`](../../../src-tauri/src/audio_graph/node.rs), VST3 / CLAP
wrappers emit one param event per point at its `sample_offset`, and the event
caps raised to hold a full block of points (VST3 `MAX_POINTS_PER_QUEUE = 128`;
CLAP `MAX_PARAM_EVENTS = 1024`, sized for several modulated params at the offline
4096-frame block). Internal nodes are untouched (default no-op → bit-identical);
the 108-test suite stays green.

Resolves the open **block-rate vs C-10** debt recorded in
[`008-param-transport-spsc.md`](008-param-transport-spsc.md) §"Known risk" #3 and
the zipper limit (BP-7) noted in [`009-parameter-block.md`](009-parameter-block.md).
Determination-affecting change — see §Compatibility.

**Design target.** Echoes is a modulation-first DAW in the Bitwig mould:
_everything_ is a modulation target and modulation must feel continuous, not
stepped. The decisions below therefore (a) pick a control period fine enough to
read as continuous (`P = 32`), and (b) commit the full path so **every** node —
plugins, effects, and the volume/pan/master chain — de-zippers, not just the
plugin params. The one line not crossed is `AudioNode::process` arity: no
per-sample / sub-range execution-model refactor (that would be a separate, larger
ADR).

---

## Context

Modulation and automation are currently delivered **once per block**:

- The scheduler samples each modulator's output buffer at **frame 0 only**
  (`src_bufs[port].channel(0)[0]`) and evaluates automation curves at the
  block-start transport position
  ([`scheduler.rs:469-488`](../../../src-tauri/src/audio_graph/scheduler.rs#L469-L488)).
- Plugin wrappers push the resulting value as a single param event at
  `sample_offset = 0` ([ADR-008](008-param-transport-spsc.md); the
  `ParamChange.sample_offset` field exists but is always `0`).

Two consequences:

1. **Zipper / stair-stepping.** A held modulation value across an entire block is
   audible on fast LFOs / envelopes, especially on external plugin parameters,
   which (unlike internal effects) have **no** `mod_smooth` interpolation.
2. **Offline ≠ online divergence.** Because the held value depends on where block
   boundaries fall, the modulation _timeline_ changes with the device buffer
   size. Offline export (fixed block) and live playback (WASAPI buffer) therefore
   do not produce the same modulated render — the unspecified-automation risk in
   [ADR-008 known-risk #2/#3](008-param-transport-spsc.md).

Crucially, the expensive prerequisite already exists and is already
block-size-independent: **modulator nodes fill their whole output buffer
per-sample** (phase accumulates on the absolute sample clock — LFO
[`modulator.rs:176-177`](../../../src-tauri/src/audio_graph/nodes/modulator.rs#L176-L177),
envelope/macro/follower likewise). The full-resolution control signal is
generated every block and discarded down to one sample. This ADR stops
discarding it.

## Decision

Evaluate modulation and automation on a **fixed control-period grid defined in
absolute sample positions**, and deliver the resulting points to plugin
parameters at their true `sample_offset`. This is the **single deterministic
behaviour — no feature flag** (closing the C-10 tension head-on).

### 1. Absolute control grid

- A fixed control period `P = 32 samples` (a power of two ≈ 0.67 ms @ 48 kHz —
  fine enough to read as continuous, the Bitwig-grade control rate). One engine
  constant, no per-project setting, no flag.
- Within a block `[pos, pos + nf)`, modulation is evaluated at offset `0` and at
  every offset `o` such that `(pos + o) % P == 0`. Each absolute P-boundary falls
  in **exactly one** block, so the point sequence is independent of how the
  stream is chopped → **offline render == live render**, byte-for-byte, and the
  cross-OS golden master is well defined.
- Modulator buffers are sampled at those offsets (`channel(0)[o]`); automation
  curves are evaluated at the matching transport position
  (`pos + o` → seconds). Both are already O(1) per point.

### 2. One uniform sub-block mechanism (all node types)

The same funnel feeds plugins and internal nodes — no special plugin path:

- A new **defaulted** trait method on `AudioNode`,
  `apply_modulation_at(param_id, sample_offset, value)`. The default
  implementation ignores the offset and forwards to the existing
  `apply_modulation` (so any not-yet-migrated node keeps today's scalar
  behaviour). The scheduler calls it once per grid point per route.
- Each modulation-aware node owns a **pre-allocated per-param point array**
  (`[f32; MAX_CONTROL_POINTS]` + count; `MAX_CONTROL_POINTS = ceil(max_block /
P)`, sized once in `prepare`, off-RT). `apply_modulation_at` writes into it; no
  allocation on the audio thread.

### 3. Plugin targets

- The pre-allocated `PluginModEntry`
  ([`node.rs:28-43`](../../../src-tauri/src/audio_graph/node.rs#L28-L43)) carries
  the point array. In `process`, the VST3 / CLAP wrappers emit **one param event
  per recorded point** at its real `sample_offset`, instead of a single offset-0
  event. The mechanism is already present (VST3 `HostParamValueQueue`, CLAP
  `FixedParamEventList`); only the point count grows.

### 4. Internal targets — committed, not deferred

A Bitwig-style engine must smoothly modulate **every** node, so internal
de-zippering is in scope, achieved **without** changing `process` arity.

> **Revised scope (2026-06-05, after reading the effect code).** The original
> wording assumed the effects "read points inside a per-sample loop they already
> run". They do **not**: `ModSmooth` is block-rate (one `advance()` per block,
> coeff from `sample_rate / max_block_size`) and every effect applies modulation
> at **block granularity** — `EqNode` recomputes its 3 biquads once and processes
> the whole block through an oversampled closure (no per-sample mod loop);
> Delay/Compressor/Reverb derive their modulated params once before the sample
> loop. A correct sub-block upgrade therefore needs (a) a **control-rate
> `ModSmooth`** (advance per `MODULATION_CONTROL_PERIOD` segment) and (b)
> **segmenting each effect's `process` loop** at grid boundaries with per-segment
> coeff/param recompute. Because `ModSmooth` is shared by all four effects this is
> done **variant by variant (C-3)**, not in one swing.

- `Gain / TrackGain / Pan / Master` have **no** smoother today (`GainNode` applies
  `mod_delta` raw), so they zipper on modulation now — the lower-risk, higher-value
  target. They adopt a lightweight per-sample gain ramp toward the grid target.
- Once each effect's smoother steps toward targets at **absolute** grid positions,
  the internal modulated render also becomes block-size-independent (offline ==
  live), closing the internal half of the ADR-008 #2 divergence.

### Implementation tranches (each independently shippable, harness re-validated)

1. **Tranche 1 (core).** Absolute grid + sub-block sampling in the scheduler's
   modulation loop; `apply_modulation_at` default + per-param point array;
   `PluginModEntry` carries it; VST3/CLAP wrappers emit per-point events; raise
   the VST3/CLAP per-param point caps to `MAX_CONTROL_POINTS` (off-RT, in
   `prepare`). Removes the plugin zipper and makes the timeline
   block-size-independent.
2. **Tranche 2 — done (reordered 2026-06-05).** `GainNode` / `TrackGainNode` got a
   per-sample one-pole gain smoother ramping toward the block target
   (`gain + mod_delta`); coeff from the sample rate (block-size independent).
   Removes the raw-`mod_delta` zipper and de-clicks fader/mute/solo. `PanNode` /
   `MasterNode` excluded — not modulation targets (no `apply_modulation`). The
   smoother originally tracked the block-rate target only; _superseded
   2026-09-23_: it now follows the interior grid points, because the block-rate
   target made the render depend on the block size (see the amendment above).
3. **Tranche 3 (variant by variant, C-3).** Internal effects consume the interior
   grid points (`apply_modulation_at`) and **segment their `process` loop** at the
   absolute grid, recomputing effective params per segment. No per-effect smoother
   is needed (the grid is continuous across blocks; 32-sample steps are inaudible).
   One effect per step: **Delay ✅ · Compressor ✅ · Reverb ✅ · EQ ✅** (all done
   2026-06-07; EQ costliest — oversampled biquads recomputed per segment). Each
   step re-validated RT and is covered by a mid-block sub-block test. Lower payoff
   than Tranches 1–2 (the block-rate smoother already de-zippered these), so done
   incrementally.

## Real-time impact

| Change                                                    | RT cost                                     | RT risk                            |
| --------------------------------------------------------- | ------------------------------------------- | ---------------------------------- |
| Sample modulator buffer at ≤ `ceil(nf/P)` offsets (was 1) | a few `f32` loads per route per block       | none — bounded, no alloc           |
| Evaluate automation curve at grid points                  | binary search ×`ceil(nf/P)` on a `Box<[…]>` | none — no alloc                    |
| Fill `PluginModEntry` point array                         | in-place writes to a pre-sized array        | none                               |
| `apply_modulation_at` (default)                           | one forward call, offset ignored            | none — same as today               |
| Emit N param events in wrapper `process`                  | N in-place pushes to a pre-allocated list   | none — N bounded by the raised cap |
| Point-array / event-list allocation                       | `prepare` only (off-RT)                     | none                               |

C-1 preserved: no allocation, no lock, no syscall in `process`. The per-block
work is bounded by `MAX_CONTROL_POINTS = ceil(max_block_size / P)` (≤ 128 with
`P=32`, `max_block=4096`; ≤ 16 at a typical 512-frame buffer) and pre-allocated.
`Relaxed`/`Acquire` orderings are unchanged. The internal `mod_smooth` step is
already per-sample today — Tranche 2/3 only change _which_ target it ramps to, at
no extra per-sample cost.

## Compatibility

- **C-2 (bit-identical offline) — intentionally re-baselined.** This change
  **alters the render output** of any project that uses modulation or automation.
  Per the 2026-06-05 decision this regression is **accepted**: the offline golden
  references are regenerated once at merge, and bit-identity is preserved
  _forward_ from this ADR. Projects with no modulation/automation are byte-for-byte
  unchanged (empty grid → same single value as before).
- **C-10 (no flag may gate determinism) — now satisfied.** Sub-block delivery is
  the _only_ behaviour; there is no `sample_accurate_modulation` toggle. This
  retires the ADR-008 known-risk #3 tension rather than deferring it.
- **Cross-OS determinism.** The grid is integer sample arithmetic on
  `position_samples`; modulator/automation evaluation is already `f32`/`f64` as
  before (no new cross-platform float surface beyond what ADR-001/012 already
  carry). The golden-master `.wav` (migration harness V1) must cover a modulated
  project once this lands.
- **C-4 (saved projects).** No schema change — `P` is an engine constant;
  `ParamChange.sample_offset` already exists.
- **C-5 / C-7.** Lock-free; the compiler is untouched (the grid lives in the
  scheduler runtime, not the plan). `AudioNode` gains one **defaulted** method —
  additive, no existing impl breaks.
- **Plugins.** External plugins receive _more_ param events per block, all at
  valid `sample_offset`s within `[0, nf)` — standard VST3/CLAP usage. No plugin
  that works today can break from finer-grained, in-range parameter automation.

## Consequences

- **Positive.** Removes the zipper on **every** modulation target — plugin
  params (Tranche 1), internal effects (Tranche 2) and the volume/pan/master
  chain (Tranche 3) — delivering the continuous, modulate-anything feel a
  Bitwig-class engine needs. Makes the whole modulated render
  buffer-size-independent (offline == live); closes ADR-008 #3 and the C-10
  tension; reuses the per-sample modulator output already computed and discarded
  today.
- **Limits / debt.**
  - Tranches 1→3 land in sequence; until Tranche 3 ships, `Gain/Pan/Master` keep
    today's block-rate value (the `apply_modulation_at` default), so the engine is
    never in a broken state between tranches. The _plugin_ path ships first
    because it had **no** smoothing at all (worst artefact).
  - `P = 32` is a fixed engine constant; an oversample-aware or user-exposed
    control rate is a later refinement, **never** a determinism flag.
  - The VST3/CLAP point caps grow to `MAX_CONTROL_POINTS`; memory per modulated
    param rises from 1 to ≤ 128 floats (trivial, `prepare`-time).
- **Reversibility.** Additive. Reverting restores the offset-0-only delivery; the
  `apply_modulation_at` default already behaves like the old scalar path.

## Alternatives considered

- **Full per-sample modulation for all nodes.** Rejected for now — requires
  splitting `AudioNode::process` into sub-ranges (or passing control buffers),
  touching every node and the scheduler's execution model. Large refactor for a
  marginal gain over a 32-sample grid + per-sample smoother; that combination
  captures ~all audible benefit at a fraction of the risk.
- **Keep block-rate, add smoothing only in the plugin wrapper.** Rejected — a
  one-pole smoother in the wrapper hides the stair-step but does **not** fix
  offline≠online divergence (the timeline is still block-boundary dependent) and
  adds plugin-specific DSP state with its own determinism caveats.
- **Sample-accurate behind a feature flag (ADR-008's original plan).** Rejected —
  collides with C-10 (two renders behind a toggle). Making it the single
  behaviour is the governed resolution.
- **Grid keyed to block subdivision (`block/K`) instead of absolute samples.**
  Rejected — reintroduces buffer-size dependence (the exact offline≠online bug).

## Follow-ups

- ~~Tranches 2 & 3~~ — done 2026-06-07 (see §Status).
- ~~ADR-008 known-risk #3~~ — annotated as resolved 2026-09-23.
- ~~Harness~~ — built 2026-09-23 (`audio_engine/render_harness.rs`), with
  modulated and automated reference projects. Golden hashes are recorded on
  Windows x86_64 only; the cross-OS check is still open.
- **Seek reset.** `GraphScheduler::reset` (which clears the held grid values) is
  never called in production. After a live seek, a held value from before the
  jump can apply to the first ≤ 31 samples until the next grid point. This is
  live-only and inaudible, but it is not yet deterministic.

**Amendment 2026-10-26 — export length and alignment.** The export path
(`render_project_offline`) now removes the master latency and keeps the tail after
the last clip (until −80 dBFS for 2.5 s, capped at 10 s). The 4096-frame block, the
grid and the held-value rule are unchanged, and `render_project_in_blocks` (the
harness path) still renders exactly the timeline length, so every golden hash holds.
See `dsp/dsp-runtime.md` § Offline Export.
