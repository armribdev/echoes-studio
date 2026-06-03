# Documentation Sync Rules

Docs and code must not drift. After **every** change that touches engine
behaviour, architecture, or a documented contract, Claude must synchronise the
documentation as part of the same change — not "later".

**Sync procedure:**
1. **Detect impact.** Look up the files you changed in `change_impact_mapping.md`.
   It names the docs and ADRs that depend on each code area.
2. **Update the impacted active docs** so they describe what the code now does.
3. **Check ADR coherence.** If the change alters a recorded decision, update that
   ADR (note the date + what changed). If it's a *new* decision, create the next
   ADR and add a row to `decisions/000-index.md`.
4. **Update `state_index.md`** — bump the status / last-verified date for the
   affected subsystem.
5. **Archive anything now obsolete** (banner + pointer, never delete).
6. **Explain divergences.** If code and a doc disagree and you can't reconcile
   them in this change, say so explicitly in your summary and (if it's a
   contract) open/annotate the relevant doc — do not leave silent drift.

**Divergence rule:** when a recalled doc or memory names a file/function/flag,
verify it still exists before relying on it. If reality contradicts the doc,
trust the code and fix the doc.

> Mapping: `change_impact_mapping.md`. Completion checklist: `completion_protocol.md`.
