# Change Impact Mapping

When you change a code area on the left, update the docs/ADRs on the right (per
`doc_sync_rules.md`). Paths are under `docs/active/`. This map is the lookup
table for step 1 of the sync procedure.

| Code area | Update these docs | Relevant ADRs |
|-----------|-------------------|---------------|
| `audio_graph/scheduler.rs`, `worker_pool.rs` | `dsp/dsp-runtime.md`, `threading/realtime-safety.md` | 004 |
| `audio_graph/compiler.rs`, `execution_plan.rs`, `node.rs`, `graph.rs` | `dsp/graph-compiler.md`, `architecture/audio-graph.md` | 001, 011, 014 |
| `audio_graph/snapshot.rs`, `snapshot_dto.rs`, `audio_engine/snapshot.rs` | `architecture/snapshot-rebuild-flow.md`, `dsp/dsp-runtime.md` | 004 |
| `audio_graph/buffer.rs`, `denormal.rs`, `oversampling.rs` | `architecture/audio-graph.md` (buffer model), `dsp/dsp-runtime.md` | — |
| `audio_graph/modulation.rs`, `parameter_block.rs`, `live_params.rs` | `modulation/modulation-system.md`, `plugins/plugin-system.md` | 009 |
| `audio_graph/midi.rs` | `architecture/audio-graph.md` | 012 |
| `audio_engine/transport.rs` | `threading/realtime-safety.md`, `threading/threading-model.md` | — |
| `audio_engine/offline.rs`, `render_harness.rs` | `migration/*` (bit-identical C-2), `dsp/dsp-runtime.md` | 019 |
| `audio_engine/engine.rs`, `graph.rs` | `threading/threading-model.md`, `architecture/audio-graph.md` | 004, 005 |
| `plugin_system/host.rs`, `lifecycle.rs`, `instance.rs`, `registry.rs` | `plugins/plugin-system.md` | 003, 007, 013 |
| `plugin_system/param_queue.rs`, `parameter.rs` | `plugins/plugin-system.md`, `plugins/plugin-wrapper-system.md` | 008, 009 |
| `plugin_system/state.rs` | `plugins/plugin-system.md` | 010 |
| `plugin_system/capabilities.rs` | `plugins/plugin-system.md`, `plugins/plugin-wrapper-system.md` | 006, 014 |
| `plugin_system/bridge.rs`, `bridged_node.rs` | `plugins/plugin-wrapper-system.md` (sandbox) | 015 |
| `plugin_system/gui.rs` | `plugins/plugin-wrapper-system.md`, `audits/plugin-gui-audit-2026-05-31.md` | — |
| `plugin_system/wrapper.rs`, `plugin_formats/{vst3,au,clap,internal}/` | `plugins/plugin-wrapper-system.md` | 003, 013 |
| `engine_gateway/{api,commands,events,store}.rs` | `architecture/state-management.md`, `threading/threading-model.md` | 002, 005 |
| `project.rs`, `audio_clip.rs`, `audio_settings.rs` | `architecture/state-management.md` | 002 |

**Cross-cutting triggers:**
- Touched the **scheduler / audio thread** → also re-check `rt_audio_rules.md` and `threading_rules.md` still hold.
- Touched **plugin capabilities / ports** → check sidechain/PDC assumptions (ADR-006, 014) and `plugins/`.
- Touched **anything on the RT path** → confirm the C-2 offline harness still passes (`migration_rules.md`).

If your change spans an area not listed here, add a row in the same change.
