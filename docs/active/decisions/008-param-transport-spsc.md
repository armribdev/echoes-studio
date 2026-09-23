# ADR-008 — Plugin parameter transport via SPSC `ParamChange` queue (Phase 3)

## Status

Accepted. Implements the **ADR-007 param-transport** half of **Phase 3** of
[`docs/active/migration/migration_plan.md`](../migration/migration_plan.md) (the `ADR-007` referenced
in [`docs/active/migration/target_architecture.md`](../migration/target_architecture.md) §9 — file
numbering continues from the existing `decisions/` series).

Builds on [`007-plugin-instance-arc.md`](007-plugin-instance-arc.md): without
the shared `Arc<PluginInstance>` there was nowhere to anchor a per-device queue.

Companion: [`009-parameter-block.md`](009-parameter-block.md) (internal-effect
hot params). Together they retire the "every edit recompiles" shape.

---

## Context

Audit [`plugin-system-audit-2026-05-31.md`](../audits/plugin-system-audit-2026-05-31.md)
**P0-1** — *"Aucun transport de paramètres UI → plugin DSP"*:

- `PluginHost::set_parameter_normalised` stashed the value in a `DeviceState`
  JSON blob the audio thread never read.
- The VST3 wrapper passed `ProcessData::input_param_changes = null` every block;
  the CLAP wrapper passed only a (always-empty) note event list.
- Net effect: a knob moved in the host (automation, modulation, MIDI-learn,
  preset morph) had **zero** audible effect on an external plugin.

The frontend already addresses plugin parameters by their **format-native
`u32` id** and stores them normalised: `PluginParams.params: Vec<(u32, f32)>`
([`project.rs`](../../../src-tauri/src/project.rs)). So the missing piece was a
real-time-safe delivery channel, not an addressing scheme.

## Decision

Give every `PluginInstance` a bounded **single-producer / single-consumer**
ring of `ParamChange`. The command thread (gateway → host) pushes; the
audio-graph wrapper drains at the top of `process` and converts each change to
the format-native parameter event.

### Module map

1. **New [`plugin_system/param_queue.rs`](../../../src-tauri/src/plugin_system/param_queue.rs)**
   - `ParamChange { native_id: u32, normalised: f32, sample_offset: u32 }`.
   - `ParamQueue` — fixed-capacity lock-free SPSC ring (power-of-two capacity,
     monotonic `head`/`tail` `AtomicUsize`, `Release`/`Acquire` pairing).
     `push` (producer) drops-newest on a full ring; `drain` (consumer) is FIFO.
   - Unit-tested: power-of-two rounding, FIFO roundtrip, overflow drop-newest,
     1000-cycle wraparound.

2. **[`plugin_system/instance.rs`](../../../src-tauri/src/plugin_system/instance.rs)**
   - `PluginInstance` gains `param_queue: ParamQueue` (capacity 1024).
   - `push_param(native_id, normalised) -> bool` (producer, command thread).
   - `drain_params(f)` (consumer, audio thread).
   - Constructed in all three `new_*`. Integration test: push → drain roundtrip
     on an internal instance.

3. **[`plugin_system/host.rs`](../../../src-tauri/src/plugin_system/host.rs)**
   - `set_parameter_normalised` (JSON blob) **replaced** by
     `push_plugin_param(device_id, native_id, normalised) -> Result<bool>`:
     looks up the record and pushes onto the instance's ring. `Ok(false)` ==
     ring full (newest dropped).

4. **[`engine_gateway/api.rs`](../../../src-tauri/src/engine_gateway/api.rs)**
   - `UPDATE_EFFECT` classifies the edit into a local `HotEffectUpdate`:
     `Plugin(device_id, params)` pushes every `(native_id, value)` onto the
     ring and **skips recompile** when the device is already instantiated; an
     un-instantiated device or an enable/disable toggle falls back to
     `recompile_if_needed` (dual-path T4).

5. **[`plugin_formats/vst3/ffi.rs`](../../../src-tauri/src/plugin_formats/vst3/ffi.rs)**
   - Host-side `IParameterChanges` / `IParamValueQueue` implemented in Rust:
     `HostParameterChanges` (≤ `MAX_PARAM_QUEUES = 128` params) +
     `HostParamValueQueue` (≤ `MAX_POINTS_PER_QUEUE = 16` points). `#[repr(C)]`
     with the vtable at offset 0 so the plugin dispatches through it. `boxed()`
     allocates + wires the static vtables once; `reset()` / `push()` fill in
     place. VST3 consumes **normalised** values directly.

6. **[`plugin_formats/vst3/wrapper.rs`](../../../src-tauri/src/plugin_formats/vst3/wrapper.rs)**
   - Holds `param_changes: Option<Box<HostParameterChanges>>` (allocated in
     `prepare`). `process` resets it, drains the ring into it, and sets
     `ProcessData::input_param_changes` to its pointer (null when empty).

7. **[`plugin_formats/clap/ffi.rs`](../../../src-tauri/src/plugin_formats/clap/ffi.rs)**
   - `ClapEventParamValue` (`CLAP_EVENT_PARAM_VALUE = 5`) + `FixedParamEventList`
     (a `clap_input_events` vtable over a fixed array — no alloc in `process`).
   - `clap.params` extension binding (`ClapPluginParams`, `ClapParamInfo`,
     `CLAP_EXT_PARAMS`) for range enumeration.

8. **[`plugin_formats/clap/wrapper.rs`](../../../src-tauri/src/plugin_formats/clap/wrapper.rs)**
   - At `prepare` (post-activate, main thread): enumerate `clap.params` into
     `param_ranges: HashMap<u32,(f32,f32)>`.
   - `process` drains the ring, **denormalises** each value to its plain range
     (`min + n·(max−min)`; identity for unknown ids), pushes a
     `clap_event_param_value`, and routes `in_events` to the param list.

### Why normalised in the queue, plain at the edge

VST3 `IParameterChanges` is normalised — a 1:1 match for the queued value, so
the VST3 wrapper does no conversion. CLAP `clap_event_param_value` is **plain**,
so the CLAP wrapper denormalises using ranges it enumerated off-RT at `prepare`.
Keeping the queue normalised matches what the project already stores and keeps
the producer format-agnostic.

## Real-time impact

| Change | RT cost | RT risk |
|---|---|---|
| `ParamQueue::push` | off-RT (command thread) | none |
| `ParamQueue::drain` in `process` | N atomic loads + N struct copies (N = pending) | none — bounded, no alloc, no lock |
| Fill `HostParameterChanges` | in-place writes to pre-allocated arrays | none |
| Fill `FixedParamEventList` + denormalise | in-place + HashMap *lookup* (no insert) | none — lookup does not allocate |
| Allocation of the param structures | `prepare` only (off-RT) | none |

C-1 (zero mutex / zero alloc / zero syscall in `process`) is preserved: every
buffer is pre-allocated at `prepare`; the ring is lock-free and bounded;
overflow degrades to drop-newest, never to a blocking or allocating path.

## Compatibility

- **C-2 (bit-identique offline).** Preserved. Offline export issues no
  `UPDATE_EFFECT` between blocks, so the ring stays empty, `input_param_changes`
  stays null, and the plugin `process` call is byte-for-byte as before.
- **C-3 (variant-by-variant).** VST3 and CLAP migrate independently; the
  internal `PassThroughDevice` ignores the queue.
- **C-4 (saved projects).** No schema change — `PluginParams.params` already
  existed and is already normalised.
- **C-5 (lock-free cmd ↔ audio).** The SPSC ring is the canonical mechanism.
  No mutex, no channel on the hot path.
- **C-7 (compiler not rewritten).** Untouched — the queue lives on the
  instance, addressed by the gateway via the host.

## Consequences

- **Positive.** Host-side knob → plugin DSP works for the first time (audit
  P0-1, quick-win "knob → son"). Foundation for plugin automation / modulation
  targets (`ModulationTargetRef::PluginParam`, Phase 5e) which simply push onto
  the same ring.
- **Limits / debt.**
  - `sample_offset` was always `0` (block-rate, BP-7). _Superseded 2026-06-07
    by ADR-019:_ modulation points are delivered at their grid `sample_offset`,
    with no feature flag.
  - The gateway pushes the **whole** `params` vec on each edit (idempotent;
    a 60 Hz knob still pushes a handful of entries). A per-edit diff is a later
    optimisation.
  - CLAP note input is temporarily not wired through the wrapper (the input
    event list now carries param events). Notes return in Phase 5c, which will
    merge note + param events into one time-sorted stream.
  - `MAX_PARAM_QUEUES = 128` / `MAX_POINTS_PER_QUEUE` are fixed caps;
    excess silently drops. ADR-019 raised `MAX_POINTS_PER_QUEUE` from 16 to 128
    so a full offline block of grid points fits.
- **Reversibility.** Additive. Reverting restores the (inert) JSON-blob path;
  the wrapper drains simply pass null again.

## Known risk — surfaced 2026-06-03 (architecture review), not yet closed

Tracked debt for when the automation path (Phase 5e) is built on this queue:

1. **Drop-newest loses automation targets.** A full ring drops the *newest*
   change. For a continuous automation/modulation stream that is the worst
   policy — the dropped value is the most recent target the DSP should converge
   to. Under dense multi-parameter automation (the "sessions énormes" case) the
   plugin can be left at a stale value. Phase 5e should switch to **coalescing by
   `native_id`** (keep only the latest per parameter) or drop-oldest, not
   drop-newest.
2. **Offline render of *recorded* automation is unspecified.** "Offline issues no
   `UPDATE_EFFECT`, so the ring stays empty" guarantees C-2 only for projects
   with **no** automation. It does **not** say how recorded automation lanes are
   delivered to the plugin during an offline render. If that path differs from
   the live ring, render≠live divergence is possible. There must be **one**
   automation delivery path, exercised by the cross-OS golden master (governance
   determinism rule).
3. **Block-rate vs C-10.** `sample_offset` is always 0; sample-accurate is
   "deferred behind a feature flag". A flag that changes render output collides
   with C-10 (no flag may gate determinism). Resolve by making sample-accurate
   the single deterministic behaviour, or by proving both flag states are
   independently bit-reproducible — not by leaving two renders behind a toggle.
   → **Resolved 2026-06-07** by [`019-sub-block-modulation.md`](019-sub-block-modulation.md):
   sub-block delivery on an absolute 32-sample grid is the single behaviour, with
   no flag. `sample_offset` now carries the real grid offset.
4. **Fixed caps silently drop.** `MAX_PARAM_QUEUES = 128` is a hard cap; a synth
   with >128 simultaneously-automated params overflows silently.

## Alternatives considered

- **Recompile on every plugin-param edit.** Rejected — this is exactly the
  xrun/stall shape the migration removes (audit 1.3/1.4/4.6).
- **`Arc<Mutex<Vec<ParamChange>>>`.** Rejected — a lock on the audio thread
  violates C-1.
- **Push plain values into the queue.** Rejected — would force the producer to
  know per-plugin ranges (only known after `prepare`); normalised keeps the
  command side format-agnostic and matches the stored project values.
- **Generic `clap_input_events` union of note + param now.** Deferred to
  Phase 5c when notes are actually produced; a param-only list keeps Phase 3
  minimal.

## Follow-ups

- **Phase 5c** — merge note + param events into one time-sorted CLAP input
  list; route MIDI to instrument plugins.
- **Phase 5e** — `ModulationTargetRef::PluginParam` writes onto this ring
  instead of calling `apply_modulation`.
- **Sample-accurate** — populate `sample_offset` from intra-block scheduling
  behind the `sample_accurate_modulation` flag.
