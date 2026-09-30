# Engine Quirks

Non-obvious, surprising behaviours of the **current** engine that will trip you
up. This is gotcha memory — not architecture truth. The full problem catalogue
lives in `docs/active/migration/critical_issues.md`; the plugin/GUI specifics in
`docs/active/audits/`. Verify against the code before relying on any item.

## Audio thread / scheduler

- **`catch_unwind` does NOT catch C++ SEH exceptions.** A Windows SEH shim
  (`plugin_formats/crash_guard.rs`) guards FFI calls, and the opt-in sandbox
  (ADR-015, CLAP only) is the real isolation. Non-isolated VST3 remains exposed.
- **XRuns are detected and logged** on the reporter thread (`engine.rs`), but
  there is no UI surface yet.
- **Worker pool is disabled by default** (`ECHOES_AUDIO_WORKERS=0`) and has no
  cost model. Parallelism is not a free win here.

## Graph / recompile

- Parameter-only `UPDATE_EFFECT` edits are hot (ADR-009/007); enabling or
  disabling an effect still recompiles the whole graph.
- MIDI is baked in beats (ADR-012); tempo changes flow through the transport.
- Oversampling is fixed at 2×.

## Plugins

- **One `Arc<PluginInstance>` per device** (ADR-007/016): the editor and the
  audio thread share it. Never add a second instance.
- **CLAP:** `clap_entry.init()` must run first (the Vital crash, C1). Host
  callbacks have a fixed field order (see `plugin_wrapper_system.md`).
- **VST3:** IIDs need COM byte order on Windows; no auxiliary buses
  (`getBusInfo`) and `kLatencyChanged` is ignored.
- Plugin editors have **no Win32 owner** on purpose (see recurring problem 9).

## Modulation

- Modulation runs on a sub-block grid (ADR-019), not per sample. LFOs reset
  phase on transport start. Modulators are per track. Automation exists in the
  engine only — no UI yet.
