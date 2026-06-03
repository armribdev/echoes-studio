# Documentation Rules

The architecture memory lives in `docs/active/`. It must stay small, canonical,
and in sync with the code.

**Structure:**
- `docs/active/` — the single source of truth, one canonical doc per topic.
- `docs/archive/` — superseded docs, banner-marked, **never edited**.
- `docs/active/decisions/` — ADRs + `000-index.md` + `architecture-decision-rules.md`.
- `.claude/rules/` — short strict rules (this folder) that **point to** the
  detailed `docs/active/` docs. Rules never restate the detail — no duplication.
- `.claude/state_index.md` — the subsystem → source-of-truth → code map.

**Hard rules:**
1. **Never delete a useful doc — archive it** with a banner pointing to its
   replacement. Never overwrite history.
2. **One canonical home per topic.** If two docs cover the same thing, merge into
   the canonical one and archive the other.
3. **No duplication across layers.** `.claude/rules/` and scoped `CLAUDE.md`
   files are short pointers; depth lives in `docs/active/`.
4. **Keep the root `CLAUDE.md` compact** — routing + invariants only, no history.
5. **A significant decision gets an ADR** (DSP graph, threading, plugin
   lifecycle, routing, modulation, scheduling, sandboxing). Add a row to
   `decisions/000-index.md` in the same change.
6. **Scoped `CLAUDE.md`** in an engine subdir holds only local constraints +
   links to active docs.

**After any change, run `doc_sync_rules.md` + `completion_protocol.md`.**
