# ADR-025 — Absolute automation for track volume and internal effects

## Status

**Accepted — 2026-09-29 (approved by the user). Engine implemented 2026-09-29 (track volume, EQ, compressor, reverb, delay); picker offers the new targets. Right-click-on-knob entry and the automated-knob ring done 2026-09-29.**
Resolves the item ADR-021 left open ("Make automation absolute for internal
effects as well — deferred: it would change existing renders and needs its own
decision"). Touches determinism (C-2), so it needs approval before any code
(`architecture-decision-rules.md`).

---

## Context

An automation lane carries a **normalised value 0…1** per point (`AutomationPoint.value`).
The engine delivers it in two different ways (ADR-021):

| Target                                      | What the node does with the 0…1 value today                                                                                                     |
| ------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| CLAP plugin parameter                       | **Sets** the parameter (absolute). The plugin's knob follows the curve.                                                                         |
| Track volume, EQ, compressor, reverb, delay | **Adds** it to the current setting, in the node's own unit, through `apply_modulation*` (the trait default forwards `apply_automation*` there). |

For internal targets that is not usable as automation:

- Track volume is a **linear gain** (`set_track_mix`: `volume / 100`, clamped 0…2).
  A curve from 0 to 1 adds at most +1.0 linear gain on top of the fader.
- EQ gains are in dB and frequencies in Hz: a 0…1 curve moves a frequency by at most 1 Hz.
- The result depends on where the user left the knob, so the same lane sounds
  different after a fader move.

The first automation editor (2026-09-29, ADR-021 scope) therefore offers plugin
parameters only. Track volume is the most common thing to automate, so this
limit blocks the "best automation UI" goal, including the planned "right-click
a knob → Automatiser" entry (Echoes' internal knobs are exactly the internal targets).

### Facts verified in the code (2026-09-29)

- Nodes read their base value from atomics: `ParameterBlock` cells (ADR-009) for
  effects, `TrackParams.volume` for the fader. Both are readable from the audio thread.
- Every effect node combines base and delta with `modulated(base, delta, range)`,
  clamped to its `*_RANGES` table; the UI mirrors those tables in `targets.ts`.
- The scheduler evaluates each automation curve on the absolute control grid
  (`MODULATION_CONTROL_PERIOD` = 32 samples, ADR-019) and calls
  `apply_automation(_at)` on the node.
- A lane with no point is not compiled (`graph.rs`): the knob keeps control.
  A lane with points holds its first value before the first point and its last
  value after the last one, for the whole timeline.
- The render harness project `automated` automates `delay.wet` and `TrackGain`
  through the additive path; its golden hash is part of `GOLDEN_HASHES`.

---

## Decision

### 1. Automation on internal targets becomes absolute

The 0…1 value is mapped to the parameter's **own range** and **replaces** the
knob's value while a lane with points exists. Modulation (LFO, envelope…) keeps
adding on top of the automated value, as it does for plugin parameters.

Implementation shape (extension of the existing path, no new node kind, no
compiler rewrite — C-7):

- `TrackGainNode` and the four effect nodes **override `apply_automation` /
  `apply_automation_at`**. Each converts the normalised value to a target in the
  parameter's unit, reads the current base from its own atomic, and forwards
  `target − base` to the existing `apply_modulation*`. The existing
  `modulated(base, delta, range)` then yields exactly the target. Nothing new is
  stored, allocated or locked.
- The trait default is unchanged, so any node that does not override it behaves
  as today.

### 2. Range and scale per parameter

One helper in `audio_graph/nodes/mod.rs` maps 0…1 to a plain value from the
`*_RANGES` entry plus a **scale**:

- **Linear** — gains (dB), Q, ratio, wet, room size, damping, width, feedback,
  delay time, threshold, knee, makeup.
- **Logarithmic** — EQ low/mid/high frequency, compressor attack and release.
  A linear 20…20 000 Hz curve would put nearly all the useful range in the first
  few percent of the lane. Log scale uses `min · (max/min)^v`, computed with
  `exp2`/`log2` like the EQ octave path already does.
- **Track volume** — the fader's own scale: linear gain 0…2 (0…200 %). The
  lane's height then matches the fader position. A dB taper is a separate,
  later decision because it would change the fader too.

The scale is added to the range table of each node and mirrored in
`targets.ts` (`INTERNAL_PARAMS`), so the UI shows the same plain value the
engine plays. This extends the existing rule that the two tables stay in sync.

### 3. The knob is overridden, not fought

While a non-empty lane targets a parameter, the lane wins (as ADR-021 does for
plugin parameters). Turning the knob does not change what is heard. The UI must
show that the parameter is automated (knob ring in the lane colour, same idea as
the modulator colour coding). A "temporary override" latch is out of scope.

### 4. Rollout, one variant at a time (C-3)

1. Extend the harness first (C-2 gate): one reference project per target family
   with absolute automation.
2. `TrackGainNode`, then EQ, compressor, reverb, delay — one commit each, harness
   green after each.
3. UI: offer track volume and internal effect parameters in the picker
   (`buildAutomationOptions`), then the right-click-on-knob entry.

**Done 2026-09-29** (steps 1, 2 and step 3: picker, right-click on a knob or the volume fader, solid ring on an automated knob). The harness gained one
project per family (`automated_gain`, `_eq`, `_compressor`, `_reverb`, `_delay`),
recorded with the old additive behaviour first; each node conversion then moved
only its own hash (plus `automated`, which uses `TrackGain` and `delay.wet`).
**Cost measured 2026-09-29** (release build, Windows x86_64, ignored test
`automation_cost_report` in `render_harness.rs`: `cargo test --release --bin
echoes-studio automation_cost -- --ignored --nocapture`):

| Measure                                       | Result                 |
| --------------------------------------------- | ---------------------- |
| `automated_value`, linear                     | 1.0 ns per call        |
| `automated_value`, logarithmic                | 3.2 ns per call        |
| 16 tracks × full effect chain, offline render | 193 ms (13× real time) |
| same + 3 linear EQ lanes per track (48 lanes) | 196 ms                 |
| same + 3 log EQ lanes per track (48 lanes)    | 197 ms                 |

A lane is evaluated 1 500 times per second (32-sample grid at 48 kHz), so the log
mapping costs about 5 µs per lane per second of audio; 48 lanes add under 2 % to
a render that already runs 13× faster than real time, and the log-vs-linear
difference is inside the measurement noise. The condition set above is met.

Still open before this is called proven: the cross-OS golden run (harness V1).

### 5. Time base stays in seconds

`AutomationPoint.time_s` is **not** changed to beats. Verified: `UPDATE_TEMPO`
moves no clip (audio and MIDI clip positions are stored in seconds, there is no
tempo map), so an automation in seconds stays aligned with the clips exactly as
they stay aligned with each other. Beats would make lanes drift away from clip
placement unless clips moved to beats too, which is a separate, larger decision.

---

## Real-time impact

| Change                                                 | RT cost                                                                              | RT risk                                                                                      |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------- |
| `apply_automation_at` override per grid point per lane | one `Relaxed` atomic load + a few float ops; `exp2`/`log2` for log-scaled parameters | none — no allocation, lock, drop or I/O; bounded by the existing grid (`MAX_CONTROL_POINTS`) |
| Mapping helper                                         | pure function                                                                        | none                                                                                         |

Threading model, snapshot swap and the ownership rules are unchanged. Only
`audio_graph/nodes/*` and the UI table change.

## Compatibility

- **C-2 (bit-identical offline).** The engine stays deterministic, but the
  render of any project with automation on an internal target **changes on
  purpose**. The golden hash of the harness project `automated` must be
  regenerated in the same commit as the change, with this ADR as the stated
  reason (`architecture-decision-rules.md`, determinism rule 2). Other golden
  hashes must not move. Cross-OS validation is still open (harness V1): `exp2` /
  `log2` are the same class of risk the EQ octave path already carries; the
  cross-OS golden run must cover them before this is called proven.
- **C-4 (saved projects).** No schema change. Projects with an internal-target
  lane change behaviour (additive → absolute). None can have been created from
  the UI (no editor offered those targets); only the raw commands could. Treated
  as acceptable; noted in the release notes.
- **C-10.** No feature flag: determinism is not gated.
- **Plugins.** Unchanged (ADR-021).

## Consequences

- **Positive.** Track volume and internal effects can be automated with the
  same lane editor; a lane means the same thing on every target; the additive
  quirk disappears from the user's model.
- **Limits / debt.**
  - An automated knob cannot be moved by hand until its lane is emptied or
    removed; the UI has to make that visible.
  - Pan, sends and time-signature are not automatable targets (no target kind).
  - Beat-based lanes and a dB fader taper remain open questions.
  - Log scaling adds two transcendental functions to the audio path; measured
    (see Rollout), negligible.

## Alternatives considered

- **Keep additive semantics and rescale in the UI.** Rejected: the offset depends
  on the knob position, so the lane changes meaning after a fader move.
- **Bake the absolute value at compile time as a delta.** Rejected: the base is
  edited without a recompile (ADR-009), so the baked delta would be wrong as soon
  as the knob moves.
- **Only linear mapping.** Rejected for frequencies and times: unusable curves.
- **Points in beats.** Rejected here, see Decision 5.
- **A per-project "absolute" flag.** Rejected: C-10 forbids gating determinism.
