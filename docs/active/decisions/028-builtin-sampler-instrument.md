# ADR-028 — Built-in sampler instrument (drum pads, V1)

## Status

Accepted — 2026-10-09 (approved by the user after the five choices below were
explained in plain terms). Builds on ADR-001 (graph-first), ADR-004 (immutable
snapshots), ADR-012 (beat-space MIDI), ADR-013 (`InstrumentDevice` seam) and
ADR-024 (shared decoded PCM).

## Context

Echoes has one built-in instrument (`PolySynth`) and CLAP plugin instruments.
Drum one-shots — one pad or note = one sound — are the most requested missing
instrument, and they unblock the hybrid `drums` library source
(`docs/library-browser-roadmap.md`, Phase 4).

`AppTrack.instrument: Option<String>` names a plugin today, and seven code
sites read `Some(_)` as "a plugin is chosen" (compiler ×3, `graph.rs`,
instrument validation, legacy plugin-state mapping, recording mode choice).

## Decision

### 1. Identification — reserved id in the existing field

A track uses the sampler when `instrument == Some("builtin:sampler")`
(`project::SAMPLER_INSTRUMENT_ID`). `AppTrack::plugin_instrument()` returns the
id only when it names a plugin; every "is it a plugin?" site uses it. No schema
change for the identifier; `instrument == None` stays `PolySynth` (C-4). An
older Echoes build opening a newer project sees an unknown plugin id: the
track is silent and reported missing, never re-voiced.

### 2. Kit configuration — typed `AppTrack.sampler`

`AppTrack.sampler: Option<SamplerKit>` (`#[serde(default,
skip_serializing_if = "Option::is_none")]`) holds the pad settings. This is
device configuration, like `EffectParams` for a built-in EQ in `effects`; all
DSP stays in the `Sampler` device inside `InstrumentNode` (ADR-001 holds). The
kit is kept when the track switches to another instrument, like plugin state.
Selecting the sampler on a track without a kit creates the default kit in the
same event (one undo step) — the factory kit since 2026-10-13, see Amendments. Rejected: storing the kit as an opaque blob in
`plugin_states` — unreadable, untestable, outside the event-sourced history.

### 3. Sample storage — file reference

A pad stores the sample's absolute path, like audio clips. Decoding happens on
the command thread (project load, `SET_SAMPLER_PAD`, event replay) through the
shared path cache (`AudioDataCache`), into an immutable `Arc<Vec<f32>>` (the
cache's existing type, mono, native sample rate). A missing or unreadable file
leaves the pad silent, keeps its path, and is reported to the UI through the
track DTO (`missing: true`); the project still opens. Copying samples into a
project folder is deferred to a later "collect files" command that would also
cover audio clips.

### 4. Data model V1

`SamplerKit { pads: Vec<SamplerPad> }`, 16 pads by default on notes 36–51
(General MIDI drums: kick 36, snare 38, closed hat 42 — the factory drum MIDI
plays as-is). Per pad: `note`, `sample_path`, `gain_db`, `pan` (−1…1, balance
law: centre = unity on both channels, like an audio track), `tune_semitones`
(−24…+24, fractional = cents), `mode` (`one_shot` ignores note-off; `gate`
releases on note-off), `attack_ms` (0–100), `release_ms` (5–2000, gate only),
`choke_group` (0 = none, 1–4). Velocity scales amplitude linearly (`v/127`).
Several pads may share a note; they all trigger (layering). Out of scope:
note ranges, velocity layers, root key, filters, stereo samples, SFZ/SF2,
warp.

### 5. Voices

- 32 playing voices plus 8 fade-out voices, fixed arrays, `Copy` structs that
  index a pad — no `Arc` touched on the audio thread.
- Retriggering a pad layers a new voice (natural tails), except within a choke
  group: a note-on on a pad of group _g_ fades every voice of group _g_.
- Pool full → the oldest voice moves to a fade-out slot and its slot is
  reused. Choke and steal fade over 5 ms; gate release uses the pad's
  `release_ms` (minimum 5 ms). No hard cut anywhere but the sample's own end.
- Playback position = `frames_played × step` (`step` = native/engine rate ×
  2^(tune/12)), linear interpolation like `TrackNode`. Computed from an
  integer frame count, not accumulated, so the render does not depend on
  `block_size` (C-2).

### 6. Automation / modulation — none in V1

Track volume/pan and effects remain automatable. The instrument parameter path
today only knows plugin parameters (`PluginParamControls`, normalised values);
forcing the sampler through it would be a hack. V2 path: a new target
`InstrumentParam { track_id, param }` with a `SAMPLER_RANGES` table and absolute
automation as in ADR-025 — requires amending ADR-021/025 at that time.

### 7. Instrument node without clips

`AudioGraph::from_project` gives a sampler track an `InstrumentNode` even
before its first MIDI clip, so pads can be auditioned and played from a
hardware keyboard on a fresh kit. Plugin-instrument and `PolySynth` tracks keep
the previous rule (a node only once the track has MIDI clips), so no existing
graph changes.

### Commands

- Choosing the sampler: existing `SET_TRACK_INSTRUMENT` / `CREATE_TRACK` with
  `builtin:sampler`.
- `SET_SAMPLER_PAD { track_id, pad, patch }` → event `SAMPLER_PAD_SET` carrying
  the whole resulting pad (replay re-resolves the sample from the cache).
  Structural: recompiles. One user action = one undo step.

## Real-time impact

| Change                  | RT cost                               | RT risk                                                                        |
| ----------------------- | ------------------------------------- | ------------------------------------------------------------------------------ |
| `Sampler::process_midi` | per frame: ≤ 40 voices, one lerp each | none — fixed arrays, no alloc/lock/IO                                          |
| Sample data             | `Arc` cloned at compile only          | none — released with the displaced scheduler on the command thread (drop ring) |
| Decode                  | command thread                        | none                                                                           |

## Compatibility

- **C-2.** Every existing golden hash unchanged (`instrument == None` path
  untouched). New harness project `sampler_kit` (one-shot, gate, choke, tune,
  44.1 → 48 kHz, voice stealing) with its own golden hash and block-size
  independence test, added before the engine code.
- **C-4.** `sampler` defaults to `None`; legacy `.echoes` load unchanged.
- **C-7.** One branch in the compiler's `Instrument` arm.

## Consequences

- **Positive.** A deterministic built-in drum instrument, covered by the
  bit-identical harness (unlike third-party synths, ADR-013 risk #2).
- **Limits.** Mono samples; editing a pad during playback recompiles, so tails
  of sounding pads are cut (ADR-004 "no DSP state transfer"); a long file
  dropped on a pad is decoded synchronously on the command thread.
- **Reversibility.** Additive. Clearing `instrument` returns the track to
  `PolySynth`; the kit stays in the file, ignored.

## Alternatives considered

- **New `InstrumentChoice` enum field.** Rejected — schema churn for no gain
  over a reserved id.
- **Copy samples into the project.** Deferred — needs a project-folder model
  shared with audio clips.
- **Hard voice stealing.** Rejected — audible click.

## Amendments

### 2026-10-13 — Factory drum sounds and starting kit

Approved by the user after four choices were explained in plain terms.

- **Sounds.** 16 drum one-shots (General MIDI notes 36–51, the pad notes) are
  synthesised in code by `factory_drums.rs`: mono, 48 kHz, peak −2 dBFS,
  starting and ending on zero, fixed-seed noise (no clock, no system RNG). No
  licence, no binary in the repository.
- **Files.** Written as float32 WAV to `<app data>/factory_samples/v1/<slug>.wav`
  by the `ensure_factory_drums` command (library module), only for the files
  that are missing (temporary name + rename, so no truncated file). The folder
  carries a version: a changed sound goes in `v2/` and saved projects keep the
  old files. Deleting the folder is safe — the files return on the next call;
  until then the pads concerned are silent and reported `missing` (§3).
- **Library.** The 16 sounds are `one_shot` assets with id
  `factory://drums/<slug>` (stable for favourites and recents) and the real
  file as `path`, listed in the `drums` source before the drum MIDI. Preview,
  drop on an audio lane and drop on a pad use the existing paths (ADR-023).
- **Starting kit (amends §2).** Choosing the sampler on a track without a kit
  (`CREATE_TRACK` or `SET_TRACK_INSTRUMENT`) now gives it the factory kit: one
  sound per pad, the three hi-hat pads (42, 44, 46) in choke group 1. Files are
  written and decoded on the command thread before the project lock is taken;
  if they cannot be written the track gets the empty default kit. The stored
  event carries the kit (`sampler`) when it gave one, so undo/redo replay
  restores it and re-resolves the samples; events without it (older logs)
  replay to the empty 16-pad kit. Still one event, one undo step.
  `SamplerKit::default()` stays empty: the `sampler_kit` harness project and
  every existing golden hash are unchanged (C-2); no engine code changed.
- **Limit (§3).** A factory pad stores an absolute path under the user's data
  folder, so a project opened on another machine finds those pads missing —
  the same limit as any sample file, until the "collect files" command.
- **Tests.** Property tests (length, −3…−1 dBFS peak, zero start/end, no DC
  offset, no silence, two renders identical in-process, WAV read-back) instead
  of a byte hash: f32 `sin`/`exp` may differ between operating systems.
