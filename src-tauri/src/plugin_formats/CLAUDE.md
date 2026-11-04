# plugin_formats/ — the format isolation boundary

VST3 / AU / CLAP / internal wrappers. This is the **only** code allowed to touch
format-specific APIs (COM, AudioUnit, CLAP C ABI). Everything below the wrapper
sees only `AudioNode`.

## Local hard constraints

- **Never** let `vst3-sys`, CoreAudio, or `clap_*` types escape into
  `audio_graph/`. Leaking format types is an architectural regression (ADR-003).
- Wrapper `process()` obeys all RT rules: pointer arrays pre-allocated in
  `prepare()`, `unsafe` scoped to the FFI call only (`.claude/rules/rt_audio_rules.md`).
- **CLAP:** call `clap_entry.init()` before any other entry point (skipping it is
  the documented Vital-crash root cause). Keep `ClapHost` pinned.
- **VST3:** initialize the component; set `IComponentHandler` + `IPlugFrame`
  before showing an editor. Dual-component plugins are handled by creating a
  separate `IEditController` (`create_separate_controller`, audit V4 done) — do
  not regress to QI-only.
- A VST3 module is loaded as the SDK says: `InitDll` after `LoadLibrary`, `ExitDll`
  before `FreeLibrary` (`Vst3Library`); the host never initialises COM (ADR-033).
- The VST3 host objects (attribute lists, messages, host context, buses, event list,
  handler, program lists, MIDI mapping, transport context) live in the `vst3_host` crate
  and must not import the engine (ADR-033); `wrapper.rs`, `instrument.rs`, `host.rs`
  and `param_delivery.rs` are the engine side.
- VST3 COM bindings live in the `vst3_ffi` crate (`vst3/ffi.rs` only re-exports
  them, ADR-033); a new `IID_*` needs its header in `crates/vst3_ffi/sdk_headers/`.
- Parameter changes delivered via the format's sample-accurate mechanism, never a
  direct setter from the audio thread.

## Read before changing

- `docs/active/plugins/plugin-wrapper-system.md`
- `docs/active/audits/plugin-gui-audit-2026-05-31.md` (C1, V1–V5, CL1–CL5, W1–W3)
- `.claude/memory/engine_quirks.md` · ADRs 003, 013 · then `completion_protocol.md`.
