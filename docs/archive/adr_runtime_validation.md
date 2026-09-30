> **⚠️ ARCHIVED — SUPERSEDED.** Point-in-time audit of 2026-06 (« reality audit » series), kept for history only. **Do not edit.**
> Many findings were fixed since. Current status: `docs/active/migration/migration_plan.md` (section « État d'avancement et suite »), `.claude/state_index.md` and `docs/active/decisions/000-index.md`.

---

# ADR Runtime Validation

> Focus #3 de l'audit. Pour chaque ADR : est-il **réellement appliqué** dans le
> code qui tourne ? Classification : **VALID** (appliqué fidèlement) /
> **PARTIAL** (appliqué en partie ou inerte) / **BROKEN** (contredit par le
> runtime) / **DANGEROUS** (appliqué mais source de bug réel).
>
> Source : `docs/active/decisions/001…015` confrontés au code.

---

## Tableau de synthèse

| ADR | Thème                         | Statut runtime | Note                                                                                              |
| --- | ----------------------------- | -------------- | ------------------------------------------------------------------------------------------------- |
| 001 | Graph-first                   | **VALID**      | Tout DSP = `AudioNode` dans le graphe                                                             |
| 002 | Event-sourcing scope          | **VALID**      | Borné ; non re-vérifié en profondeur (store.rs)                                                   |
| 003 | Thin wrapper / boundary       | **VALID** ⚠️   | Scheduler agnostique ; _mais_ la règle `.claude` qui interdit l'import est trop stricte (voir §3) |
| 004 | Immutable runtime snapshots   | **VALID**      | `AtomicPtr` + drop-ring SPSC réels                                                                |
| 005 | UI/engine separation          | **VALID**      | Rust possède l'état ; UI = miroir                                                                 |
| 006 | Capabilities + PDC            | **PARTIAL**    | PDC + latence plugin OK ; introspection capacités quasi inerte                                    |
| 007 | Shared `Arc<PluginInstance>`  | **DANGEROUS**  | Partage OK _dans_ un compile ; ré-instanciation + index `device_id` + teardown mort               |
| 008 | Param transport SPSC          | **VALID** ⚠️   | Effets OK ; params **jetés** pour instruments CLAP                                                |
| 009 | ParameterBlock hot params     | **VALID**      | Eq/Comp/Reverb/Delay via `LiveParams`                                                             |
| 010 | State persistence FFI         | **DANGEROUS**  | FFI réel, mais état **perdu au recompile** + non sequencé vs audio                                |
| 011 | PortKind typed edges          | **VALID**      | Défaut `Audio`, validation au compile                                                             |
| 012 | MIDI snapshot en beats        | **VALID**      | `from_project` bake en `start_beats`/`end_beats` (≠ `critical_issues` §2.1 périmé)                |
| 013 | CLAP instruments              | **PARTIAL**    | CLAP câblé ; VST3i/AU non ; params instrument jetés                                               |
| 014 | Capability-driven port sizing | **PARTIAL**    | Foundation présente, **inerte** (`num_sidechain_inputs` toujours 0)                               |
| 015 | Plugin sandbox opt-in         | **PARTIAL**    | `BridgedNode`+bridge réels (CLAP), opt-in, défaut OFF (C-9)                                       |

---

## Détails

### ADR-001 — Graph-First — **VALID**

Chaque feature DSP est un `NodeKind` → `AudioNode`. Aucun champ DSP spécial sur
`AppTrack` côté engine ; les effets sont des nodes dans la chaîne. Conforme.

### ADR-002 — Event-Sourcing Scope — **VALID** (non approfondi)

La portée event-sourcing reste bornée (transport ≠ event, recompile structurel).
Non re-vérifié ligne à ligne dans `engine_gateway/store.rs` — pas de
contradiction observée.

### ADR-003 — Thin Wrapper Strategy — **VALID** (avec réserve de gouvernance)

Le **runtime** (`GraphScheduler`) ne voit que `Box<dyn AudioNode>` : agnostique
format, conforme. **Réserve** : `architecture_rules.md` invariant #3 dit
« `audio_graph/` ne doit pas importer `plugin_system` ». Or
[`compiler.rs:36`](../../src-tauri/src/audio_graph/compiler.rs#L36) importe
`plugin_system::PluginHost` et l'appelle (`instantiate`, `instantiate_instrument`,
`sidechain_input_ports`). **ADR-003 lui-même autorise** ceci (« The graph
compiler calls `PluginHost::instantiate()` », lignes 37/40). → La **règle
`.claude` est plus stricte que son ADR et que le code**. À reconcilier : le
compiler **est** couplé au host par design ; seul le scheduler est agnostique.

### ADR-004 — Immutable Runtime Snapshots — **VALID**

`RuntimeSnapshot { pending: AtomicPtr, drop_ring: DropRing }`. `try_swap` sur le
thread audio, `enqueue`+`drain` sur le thread commande. Aucun mutex, aucun
`drop` dans le callback. Implémentation exemplaire, tests/justification présents.

### ADR-005 — UI/Engine Separation — **VALID**

Toute mutation passe par les commandes gateway → `AppProject` → recompile →
snapshot. L'UI reçoit des events/snapshots dérivés. Optimistic UI côté front.

### ADR-006 — Capabilities + PDC — **PARTIAL**

- **PDC** : pleinement implémenté (rings, `compute_latency_compensation`).
- **Latence plugin rapportée** : ✅ wrappers overrident `latency_samples()`
  (CLAP `clap.latency`, VST3 `getLatencySamples`) — contredit `critical_issues`
  §3.6/§7.1 (périmé).
- **Introspection capacités** : `capabilities()` renvoie `from_legacy_info`
  (stéréo codé en dur) + latence. Pas de vraie énumération de bus. → Le volet
  « capabilities » reste **quasi inerte**.

### ADR-007 — Shared `Arc<PluginInstance>` — **DANGEROUS**

Le partage `Arc` host↔wrapper↔GUI fonctionne **dans un compile donné**. Mais :

- **Ré-instanciation** à chaque recompile (R-1) : nouvelle `PluginInstance`, pas
  de réutilisation. L'objectif « single live plugin per device » est **churné**.
- **Index `device_id`** (R-2) : `records.insert(plugin_id, …)` → collision pour
  deux usages du même plugin.
- **Teardown mort** (R-3) : `destroy_device`/`suspend`/`resume` sans appelant ;
  le modèle de suppression décrit dans l'ADR (« host.destroy_device(id) ») **n'est
  pas branché** → fuite des instances retirées.

→ L'ADR est _implémenté_ mais ses hypothèses de cycle de vie ne tiennent pas au
runtime. Cf. [`plugin_instantiation_failure_analysis.md`](plugin_instantiation_failure_analysis.md).

### ADR-008 — Param Transport SPSC — **VALID** (réserve instruments)

`ParamQueue` (ring 1024) sur l'instance ; `push_plugin_param` (host/api) →
`drain_params` dans `process` (VST3 `IParameterChanges`, CLAP `param_value`).
Réellement câblé. **Réserve** : `ClapInstrument` (instrument) **draine et jette**
les params (R-6) → pas d'automation de params pour les instruments CLAP.

### ADR-009 — ParameterBlock — **VALID**

Eq/Compressor/Reverb/Delay reçoivent un `ParameterBlock` via
`live_params.get_or_init_effect_block` + `.with_block(...)`. Les éditions hot
écrivent le block (`write_effect_block`, `api.rs`). **Attention** : ceci accélère
les params _internes_ uniquement ; les params _plugin_ passent toujours par
recompile (lié à `critical_issues` §1.4).

### ADR-010 — State Persistence FFI — **DANGEROUS**

`save_state_blob`/`load_state_blob` appellent vraiment le FFI (VST3
`get/setState`, CLAP `state.save/load`). Mais :

- **Perte au recompile** : `load_device_state` n'est rappelé qu'au load projet
  (`main.rs:252`) ; la ré-instanciation (R-1) ne re-restaure pas l'état (R-4).
- **Non séquencé** vs le thread audio : save/load supposent transport stoppé
  (documenté dans `instance.rs`, pas de barrière between-blocks).

### ADR-011 — PortKind Typed Edges — **VALID**

`Edge.kind: PortKind`, défaut `Audio { channels: 2 }` via `connect`. `from_project`
ne crée que de l'`Audio` → bit-identique. `validate_port_kinds` garde le reste.

### ADR-012 — MIDI Snapshot en beats — **VALID** (corrige une entrée périmée)

`from_project` bake les notes en `start_beats`/`end_beats` + `clip_start_sec`
([`graph.rs:561-567`](../../src-tauri/src/audio_graph/graph.rs#L561)), pas en samples
absolus. → **Contredit `critical_issues.md` §2.1** qui prétend que les notes sont
bakées en samples absolus avec le BPM du compile. L'entrée §2.1 est **périmée** ;
la résolution tempo per-block existe (ADR-012/Phase implémentée). _(Le câblage
`UPDATE_TEMPO → recompile` reste à confirmer côté gateway, hors scope.)_

### ADR-013 — CLAP Instruments — **PARTIAL**

`instantiate_instrument` (CLAP only) → `ClapInstrument` (générateur, 0 in/1 out,
notes via `FixedEventList`). VST3i/AU renvoient `Err` → fallback `PolySynth`.
Params instrument jetés (R-6). Contredit `critical_issues` §3.2 (« aucun
support », périmé) pour CLAP.

### ADR-014 — Capability-driven Port Sizing — **PARTIAL / inerte**

Le compiler ajoute `plugin_host.sidechain_input_ports(device_id)` aux entrées
d'un node Plugin. Mais `num_sidechain_inputs` n'est **jamais peuplé** (le scan
CLAP le met à 0, [`clap/host.rs:93`](../../src-tauri/src/plugin_formats/clap/host.rs#L93)).
→ Toujours 0 → strictement bit-identique → **chemin dormant**. Foundation
présente, capacité réelle absente.

### ADR-015 — Plugin Sandbox — **PARTIAL**

- `BridgedNode` ([`bridged_node.rs`](../../src-tauri/src/plugin_system/bridged_node.rs))
  spawn un `plugin_host_bridge` enfant, échange l'audio via shared-memory
  (`plugin_bridge::session`), budget spin + fallback pass-through. Watchdog
  (6e) présent.
- Branché dans `compiler.build_node` quand `sandboxed && CLAP && bridge exe`.
- **Opt-in, défaut OFF** (C-9 respecté). VST3 sandbox non encore fait.
- **Note** : `PluginHost::instantiate` **ignore** son param `sandboxed`
  (`let _ = sandboxed`, [`host.rs:163`](../../src-tauri/src/plugin_system/host.rs#L163)) —
  la vraie décision est dans le compiler. Cohérent mais le param host est mort.

---

## Contradictions ADR ↔ runtime à retenir

1. **ADR-007** : le modèle de suppression (`destroy_device`) **n'est pas câblé**
   → fuite (DANGEROUS).
2. **ADR-010** : persistance FFI réelle mais **non re-appliquée** après
   ré-instanciation (DANGEROUS).
3. **architecture_rules.md #3 ↔ ADR-003 ↔ code** : la règle interdit un import
   que l'ADR autorise et que le code effectue → **règle à corriger**.
4. **`critical_issues.md` §2.1, §3.1, §3.2, §3.5, §3.6, §3.7, §3.8, §3.11** :
   **périmés** — résolus par les phases plugin. À re-synchroniser.
