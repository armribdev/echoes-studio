---
name: plugin-hosting
description: Add or modify plugin hosting (VST3/AU/CLAP load, lifecycle, parameters, state, editor GUI, sandbox). Use when touching plugin_system/ or plugin_formats/ code.
---

# Plugin Hosting

## Objective

Host external plugins correctly across formats while keeping the format isolation
boundary and RT safety intact.

## Workflow

1. Read `docs/active/plugins/plugin-system.md` +
   `docs/active/plugins/plugin-wrapper-system.md`, the relevant ADRs (003, 006,
   007, 008, 010, 013, 014, 015), and `.claude/memory/engine_quirks.md`.
2. Decide the layer: hosting/lifecycle/params/state → `plugin_system/`;
   format-specific FFI → `plugin_formats/`. Keep format types out of `audio_graph`.
3. For audio-path code (wrapper `process`), run **rt-validation**.
4. For editor/GUI work, follow the platform/format notes in the GUI audit.

## Checklist

- [ ] No `vst3-sys` / CoreAudio / `clap_*` type escapes into `audio_graph/`.
- [ ] One `Arc<PluginInstance>` per device (ADR-007); don't add a 3rd instance.
- [ ] Params via SPSC `param_queue` + sample-accurate delivery, never a setter
      from the audio thread (ADR-008).
- [ ] State is opaque native-FFI bytes (ADR-010); not interpreted.
- [ ] **CLAP:** `clap_entry.init()` called first; `ClapHost` pinned.
- [ ] **VST3:** component initialized; `IComponentHandler` + `IPlugFrame` set
      before editor show; dual-component not silently rejected.
- [ ] Sandbox path stays opt-in per device (ADR-015, C-9).

## Anti-patterns

- Calling a plugin setter from the callback.
- Reaching into the plugin's GUI instance to read DSP state (two-instance trap A1).
- Assuming `catch_unwind` protects against a crashing C++ plugin (it doesn't —
  SEH). Sandbox is the real mitigation.

## Validation (done when)

- The plugin loads, processes, persists/restores state, and (if applicable) its
  editor opens — with the boundary and RT rules intact, and docs/ADRs synced.
