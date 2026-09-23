# audio_engine/ — host, stream, transport, offline

Owns the CPAL stream lifecycle, the transport clock, the runtime snapshot
plumbing, and the offline (non-realtime) render path.

## Local hard constraints
- The CPAL callback closure (`engine.rs`) must capture only lock-free handles
  (`Arc<RuntimeSnapshot>`, transport atomics, meter cells). No locks on the audio
  path. See `.claude/rules/rt_audio_rules.md` + `threading_rules.md`.
- Transport is **read from atomics inside the block** and passed down as
  `TransportInfo` — never polled mid-graph.
- `offline.rs` is the deterministic export renderer; `render_harness.rs` is the
  C-2 gate (block-size independence + golden hashes). Keep it green; add a
  reference project before changing an RT path it doesn't cover. Update a golden
  hash only for an ADR-sanctioned render change.
- Displaced schedulers are freed on the **command thread** via the drop ring,
  never dropped in the callback.

## Key files
`engine.rs` (CPAL stream) · `transport.rs` (clock/atomics) · `snapshot.rs`
(swap plumbing) · `offline.rs` (deterministic render) · `graph.rs`.

## Read before changing
- `docs/active/threading/threading-model.md`, `.../realtime-safety.md`
- `docs/active/architecture/snapshot-rebuild-flow.md`
- ADR-004 · then run `.claude/rules/completion_protocol.md`.
