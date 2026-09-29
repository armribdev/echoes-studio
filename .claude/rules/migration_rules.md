# Migration Rules

Echoes Studio evolves toward a modern DAW **without a rewrite**. Every target
brick attaches to a current brick that is kept, extended, or replaced in place.

**The C-1…C-10 constraints are authoritative in
`docs/active/migration/target_architecture.md` (§0).** Summary:

| | Constraint |
|--|------------|
| C-1 | Zero mutex on the RT path |
| C-2 | Bit-identical offline render preserved at every step |
| C-3 | Migrate variant by variant (e.g. Eq → Compressor → Reverb → Delay) |
| C-4 | Backward compatibility (legacy projects load and behave unchanged) |
| C-5 | Lock-free command ↔ audio communication |
| C-6 | A single plugin instance (no duplicate hosting) |
| C-7 | Extend the graph compiler — do not rewrite it |
| C-8 | Preserve the track-linear UX |
| C-9 | Sandbox is opt-in, never on by default |
| C-10 | No feature flag may gate C-2 (determinism is non-optional) |

**Operating rules:**
- **C-2 is the gate.** Any engine change must keep the bit-identical offline
  render harness green (`audio_engine/render_harness.rs`). If the harness doesn't cover
  your area yet, extend it **before** the change.
- Work in the order defined by `docs/active/migration/migration_plan.md`. Each
  phase is independently shippable and references its ADR.
- New routing (MIDI/modulation edges) defaults to `Audio`/no-op so existing
  projects stay bit-identical (ADR-011, ADR-012).
- A migration step that touches a hard invariant still needs an ADR
  (`architecture_rules.md`).

> Target: `target_architecture.md`. Progress + next steps: `migration_plan.md`.
> Historical problem catalogue: `critical_issues.md` (status table at top).
> Pre-migration structure (archived): `docs/archive/current_engine_diagram.md`.
