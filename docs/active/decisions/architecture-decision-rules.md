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

| Constraint | Governed by |
|-----------|------------|
| Real-time safety (audio callback prohibitions) | `../threading/realtime-safety.md` |
| Threading model (which thread owns what) | `../threading/threading-model.md` |
| Graph execution model (topological order, immutable plan) | `../architecture/audio-graph.md` |
| Plugin ownership rules (graph sees only `AudioNode`) | `../plugins/plugin-system.md` |
| State management boundaries (event sourcing scope) | `../architecture/state-management.md` |
| UI / engine separation (no direct state sharing) | `005-ui-engine-separation.md` |

Changing these without an ADR is an architectural regression, not a
convenience shortcut.

---

## Principle

> Architectural consistency is more important than short-term implementation
> convenience.

When in doubt about whether a change conflicts with existing architecture:
stop, read the relevant docs, and ask. The cost of a wrong architectural
decision compounds over time. The cost of pausing to verify is one message.
