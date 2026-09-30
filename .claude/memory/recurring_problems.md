# Recurring Problems

Mistakes that keep happening on this codebase, and how to avoid them. When you
hit one, fix it and (if it's a new class) add it here. Detail lives in
`docs/active/migration/critical_issues.md` and `docs/active/audits/`.

## 1. Allocation/locks creeping onto the audio path

**Symptom:** a "small" `Vec`, log line, or lock added inside `process()` or a
helper it calls.
**Avoid:** run the **rt-validation** skill on every audio-path change; allocate
in `prepare()`/compile time only.

## 2. Forgetting plugin init/handshake order

**Symptom:** plugin loads but crashes or shows no editor.
**Avoid:** CLAP → `clap_entry.init()` first; VST3 → initialize component + set
`IComponentHandler`/`IPlugFrame` before show. See `engine_quirks.md` + the GUI
audit. Use the **plugin-hosting** skill.

## 3. Recompile-on-every-change performance traps

**Symptom:** UI feels heavy with many tracks/effects; param tweaks lag.
**Cause:** `UPDATE_EFFECT`/`UPDATE_TEMPO` recompile the full graph.
**Avoid:** prefer atomic/`LiveParams` param updates over structural recompiles;
don't add new code paths that recompile on hot interactions.

## 4. Breaking the bit-identical offline render (C-2)

**Symptom:** offline harness diverges after an engine change.
**Avoid:** ensure the harness covers your area first; new routing defaults to a
no-op shape (ADR-011/012); never gate determinism behind a flag (C-10). Use the
**architecture-migration** skill.

## 5. Leaking plugin-format types into the graph

**Symptom:** `audio_graph/` ends up importing `vst3-sys`/`clap_*`.
**Avoid:** keep all format FFI in `plugin_formats/`; the graph sees only
`Box<dyn AudioNode>` (ADR-003).

## 6. Docs drifting from code

**Symptom:** a doc/memory names a file/flag that no longer matches reality.
**Avoid:** run the **doc-sync** skill after changes; trust the code and fix the
doc when they disagree; update `state_index.md`.

## 7. Assuming crash isolation that doesn't exist

**Symptom:** relying on `catch_unwind` to contain a bad plugin.
**Avoid:** it doesn't catch C++ SEH. Use the opt-in sandbox (ADR-015) for
untrusted plugins.

## 8. Wrong `Self` in struct literals (safe-refactor passes)

**Symptom:** `E0411` in free functions, or `E0308` / wrong fields when `Self` is
used in `impl AppProject { fn to_dto() -> ProjectDto { Self { … } } }` or in a
closure that must build `MidiClipDto` / `DeviceRecord`.
**Avoid:** follow [`.claude/rules/rust_self_rules.md`](../rules/rust_self_rules.md);
run `cargo build` after each batch; grep `-> OtherType` + `Self {`.

## 9. Win32 owner HWND on plugin editors

**Symptom:** editor stays above Echoes when clicking the arrange; sandboxed
`OpenEditor` freezes the UI for seconds then later calls fail with
`bridge control channel closed`.
**Cause:** owned popups are always above their owner; a bridge window owned by
Echoes can deadlock (UI waits on the control reply while `CreateWindowExW`
waits on the owner's queue → 10 s timeout → channel marked broken).
**Avoid:** never pass Echoes' HWND as `CreateWindowExW` owner for plugin
editors. Keep `WS_EX_TOOLWINDOW` for taskbar hiding; sync minimise/hide via
explicit host events if needed.
