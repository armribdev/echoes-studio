# Engine Quirks

Non-obvious, surprising behaviours of the **current** engine that will trip you
up. This is gotcha memory — not architecture truth. The full problem catalogue
lives in `docs/active/migration/critical_issues.md`; the plugin/GUI specifics in
`docs/active/audits/`. Verify against the code before relying on any item.

## Audio thread / scheduler
- **`catch_unwind` does NOT catch C++ SEH exceptions** from plugins. A crashing
  VST3/CLAP still takes down the process. Real mitigation = the sandbox (ADR-015),
  not panic catching. (critical_issues 1.5)
- **`unsafe impl Send` on the scheduler** carries an incorrect justification —
  treat its safety as load-bearing on the snapshot-swap invariants, not on the
  comment. (1.2)
- **No XRun detection / telemetry.** Underruns are currently silent — don't
  assume "it sounds fine" means deadlines were met. (1.1)
- **Worker pool is disabled by default**, respawns on every structural recompile,
  and has no RT priority. Parallelism is not a free win here. (1.7, 4.2, 4.3)

## Graph / recompile
- **`UPDATE_TEMPO` does not recompile** → silent MIDI desync against beat-based
  snapshots. Tempo changes don't flow everywhere you'd expect. (2.1)
- **`UPDATE_EFFECT` recompiles the whole graph.** Per-effect param tweaks are far
  more expensive than they look; this is the main scaling cliff. (1.4, 4.6)
- The engine is **locked for the entire compile** (reliquat of audit I11). Long
  compiles stall the command thread. (1.3)
- Ports are **untyped** today (audio/MIDI/modulation indistinguishable until the
  ADR-011 `PortKind` work lands). (2.2)

## Plugins
- **Two plugin instances per effect** (audio + GUI) — audit debt A1/3.11. Do not
  read DSP state via the GUI instance; do not add a third.
- **CLAP:** `clap_entry.init()` must be called before anything else (skipping it
  was the Vital crash, C1). `Box<ClapHost>` is **not Pinned** (CL5, open) and
  `make_host` is rebuilt on every editor open (CL3, open).
- **VST3:** dual-component plugins are currently rejected silently (V4, open).
- **No DPI handling** for plugin editor windows (W3 / 3.12, open).

## Modulation
- Modulation is **block-rate only**, **LFO only**, targets **internal params
  only**, and LFOs **don't phase-reset on transport restart**. The `ModulationMap`
  compiles but carries **zero routes** at runtime. (5.x)
