# plugin_formats/ — the format isolation boundary

VST3 / AU / CLAP / internal wrappers. This is the **only** code allowed to touch
format-specific APIs (COM, AudioUnit, CLAP C ABI). Everything below the wrapper
sees only `AudioNode` / `Device`.

## Local hard constraints
- **Never** let `vst3-sys`, CoreAudio, or `clap_*` types escape into
  `audio_graph/`. Leaking format types is an architectural regression (ADR-003).
- Wrapper `process()` obeys all RT rules: pointer arrays pre-allocated in
  `prepare()`, `unsafe` scoped to the FFI call only (`.claude/rules/rt_audio_rules.md`).
- **CLAP:** call `clap_entry.init()` before any other entry point (skipping it is
  the documented Vital-crash root cause). Keep `ClapHost` pinned.
- **VST3:** initialize the component; set `IComponentHandler` + `IPlugFrame`
  before showing an editor. Dual-component plugins must not be silently rejected
  (audit V4, open).
- Parameter changes delivered via the format's sample-accurate mechanism, never a
  direct setter from the audio thread.

## Read before changing
- `docs/active/plugins/plugin-wrapper-system.md`
- `docs/active/audits/plugin-gui-audit-2026-05-31.md` (C1, V1–V5, CL1–CL5, W1–W3)
- `.claude/memory/engine_quirks.md` · ADRs 003, 013 · then `completion_protocol.md`.
