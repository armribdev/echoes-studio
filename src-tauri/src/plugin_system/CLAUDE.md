# plugin_system/ — plugin hosting (command thread)

Discovers, instantiates, and manages external plugins. Runs on the **command
thread**, never the audio thread.

## Local hard constraints
- This crate may use `plugin_formats`, but **must not** leak format types into
  `audio_graph/`. The graph only ever receives `Box<dyn AudioNode>` (ADR-003).
- One `Arc<PluginInstance>` per device — no duplicate hosting (ADR-007). Audit
  debt A1/3.11 (audio + GUI as separate instances) is closed: `open_clap_editor`
  / `open_vst3_editor` (`gui.rs`) reuse the audio-side `Arc<PluginInstance>`.
  Remaining gap (CL3, Phase 4): the CLAP audio-side host is still the bare
  `make_host` (no extensions), so a plugin-initiated `request_resize` /
  `clap.log` from its GUI has nowhere to land — don't reintroduce a second
  instance to fix that; enrich the shared host instead.
- Lifecycle transitions (`lifecycle.rs`) happen here, never in the callback.
- UI→DSP parameter changes go through the SPSC `param_queue.rs`, delivered via
  `ProcessData` — never call the plugin's setter from the audio thread (ADR-008).
- Plugin state is an opaque blob via native FFI (`state.rs`, ADR-010). Don't
  interpret it.
- Sandbox (`bridge.rs`, `bridged_node.rs`) is **opt-in per device** (ADR-015,
  migration C-9) — never default-on.

## Key files
`host.rs`, `registry.rs`, `instance.rs`, `lifecycle.rs` (hosting) ·
`param_queue.rs`, `parameter.rs` (params) · `capabilities.rs` (caps/PDC) ·
`state.rs` (persistence) · `wrapper.rs` (format dispatch) · `gui.rs` (editor) ·
`bridge.rs`, `bridged_node.rs` (sandbox).

## Read before changing
- `docs/active/plugins/plugin-system.md`, `.../plugin-wrapper-system.md`
- ADRs 003, 006, 007, 008, 010, 013, 014, 015
- Known traps: `.claude/memory/engine_quirks.md` · then `completion_protocol.md`.
