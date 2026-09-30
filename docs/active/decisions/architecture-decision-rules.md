# Architecture Decision Rules

These rules govern how architectural changes are handled in Echoes Studio.
They apply to every Claude Code session working on this project.

---

## Before Implementing Any Structural or Architectural Change

1. Read the relevant files in:
   - `../architecture/`
   - `../threading/realtime-safety.md`, `../architecture/anti-patterns.md`
   - `../decisions/` (this folder)

2. Determine whether the change:
   - **Follows** existing architecture — proceed, no ADR needed.
   - **Extends** existing architecture — proceed; document the extension
     in the relevant `architecture/`, `dsp/`, or `plugins/` file.
   - **Conflicts** with existing architecture — do **not** proceed silently.
     Raise the conflict with the user before touching code.

3. If the change introduces a **new architectural decision**:
   - Create a new ADR file in `../decisions/` using the next
     available number.
   - Format: title, context, decision, consequences, alternatives considered.
   - Commit it in the same PR as the implementation.

4. If the change **modifies a previous architectural assumption**:
   - Update the related ADR file.
   - Explain what changed, why, and what the new tradeoffs are.
   - Note the date of the modification in the ADR.

---

## Hard Constraints — Never Change Silently

The following properties may not be changed without an explicit ADR and user
approval:

| Constraint                                                                              | Governed by                                                    |
| --------------------------------------------------------------------------------------- | -------------------------------------------------------------- |
| Real-time safety (audio callback prohibitions)                                          | `../threading/realtime-safety.md`                              |
| Threading model (which thread owns what)                                                | `../threading/threading-model.md`                              |
| Graph execution model (topological order, immutable plan)                               | `../architecture/audio-graph.md`                               |
| Plugin ownership rules (graph sees only `AudioNode`)                                    | `../plugins/plugin-system.md`                                  |
| State management boundaries (event sourcing scope)                                      | `../architecture/state-management.md`                          |
| UI / engine separation (no direct state sharing)                                        | `005-ui-engine-separation.md`                                  |
| **Bit-exact determinism** (same project + transport ⇒ identical render, cross-platform) | migration C-2 / C-10, `../migration/target_architecture.md` §0 |

Changing these without an ADR is an architectural regression, not a
convenience shortcut.

### Determinism is a governed invariant, not an emergent property

C-2 (bit-identical offline) and C-10 (no flag may gate determinism) were
previously enforced only inside the migration docs. They are listed here because
the 2026-06-03 architecture review found three ADRs that **erode determinism
without any rule naming it as the thing at stake**: the topological-sort claim
reduces determinism to node order alone (ADR-001), `MidiNoteSnapshot` resolves
beat→sample in `f64` with no cross-platform proof (ADR-012), and block-rate vs
sample-accurate automation sits behind a feature flag in tension with C-10
(ADR-008). Any change that affects render output must:

1. Keep the C-2 offline harness green, **and**
2. Be validated by the cross-OS golden-master `.wav` (migration plan harness V1
   — still to land in CI). Until V1 exists, a determinism-affecting change must
   say so explicitly in its ADR and PR.

> **Build-profile note (anti-foot-gun).** The only in-process crash guard
> (`catch_unwind` around `AudioNode::process`, ADR-006) works **solely** under
> `panic = "unwind"`. The build currently has no `[profile]` override, so unwind
> is in effect and the guard is live — but nothing _pins_ it. Adding
> `panic = "abort"` (a common size/speed optimisation) would silently turn the
> guard into a no-op. Treat the panic strategy as a governed property: do not set
> `panic = "abort"` without an ADR that first replaces the guard (e.g. the
> out-of-process bridge, ADR-015).

---

## Principle

> Architectural consistency is more important than short-term implementation
> convenience.

When in doubt about whether a change conflicts with existing architecture:
stop, read the relevant docs, and ask. The cost of a wrong architectural
decision compounds over time. The cost of pausing to verify is one message.
