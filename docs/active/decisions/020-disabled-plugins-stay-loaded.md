# ADR-020 — Disabled plugin effects stay loaded (`BypassNode`)

## Status

**Accepted — implemented 2026-09-23.** Approved by the user after the bug below
was reported to them in plain terms. Partially addresses audit P2-1 /
`critical_issues.md` §6.4 (the plugin now survives a bypass, but toggling bypass
still recompiles).

## Context

`AudioGraph::from_project` skipped every effect with `enabled == false`. For a
plugin effect, that meant:

1. The compiler's `active_keys` no longer contained the placement, so
   `PluginHost::retain_devices` destroyed its instance.
2. Re-enabling it built a fresh instance with **no state**: the user's preset
   was gone.
3. Saving while it was disabled wrote no state for it, since
   `collect_plugin_states` only harvests live records. A project loaded with a
   disabled plugin also never applied its staged blob, so the next save erased
   it.
4. Its editor could not be opened while disabled.

Professional DAWs keep a bypassed plugin instantiated and route audio around it.

## Decision

- Disabled **plugin** effects stay in the graph. `NodeKind::Plugin` gains
  `bypassed: bool` (= `!effect.enabled`).
- The compiler builds the plugin node as usual (`build_plugin_node`: sandbox
  bridge or in-process host, unchanged) and, when `bypassed`, wraps it in
  `BypassNode` (`audio_graph/nodes/bypass.rs`).
- `BypassNode` forwards `prepare` / `reset` so the plugin stays active, keeps
  the wrapped node's port counts, and copies the main input to the main output.
  It never calls the inner `process`.
- Disabled **built-in** effects are still skipped: their settings live in the
  project, so nothing is lost.

Because the placement stays in the graph, its host record survives
(ADR-016 reuse applies on re-enable), its state is harvested at save, and a
staged project-load blob is applied at instantiation.

## Real-time impact

| Change | RT cost | RT risk |
|---|---|---|
| `BypassNode::process` | one `copy_from_slice` per channel | none — no alloc, no lock |
| Disabled plugin kept active | none on the audio thread (its `process` is not called) | none |
| Build / wrap | compile time, off-RT | none |

The inner node is dropped with the retiring graph through the snapshot drop
ring, exactly like any plugin node (ADR-004, ADR-016).

## Compatibility

- **C-2.** No change for projects without disabled plugins. A disabled plugin
  now produces the same audio as before (its input, unchanged), through a
  `BypassNode` instead of a missing node. Built-in effects are untouched, so the
  render harness golden hashes are unchanged.
- **C-4.** No schema change; `enabled` already existed.
- **Modulation / automation** routes aimed at a disabled plugin resolve to the
  `BypassNode`, whose `param_id` is `None`, so they are dropped exactly as
  before (when the node was absent).
- **PDC.** A bypassed plugin reports 0 latency, as when it was absent.

## Consequences

- **Positive.** Bypass no longer loses plugin settings; the editor stays
  usable while bypassed; project save/load round-trips disabled plugins.
- **Limits / debt.**
  - Toggling bypass still triggers a recompile (instance reused, so no FFI
    churn). An atomic bypass flag read in `process` (target §6.4) remains the
    way to make it instant.
  - A disabled plugin keeps its memory and its CPU-idle footprint.
  - Latency is not held constant across a bypass toggle.

## Alternatives considered

- **Keep skipping disabled plugins, but keep their state in the host** (like
  missing plugins). Rejected: the plugin would still be destroyed and rebuilt on
  every toggle, its editor would be unavailable, and it adds a second
  state-keeping path.
- **Atomic bypass flag in `process_one_step` now.** Deferred: it touches the
  scheduler hot path; the wrapper solves the data-loss bug without it.
