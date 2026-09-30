> **⚠️ ARCHIVED — SUPERSEDED.** Point-in-time audit of 2026-06 (« reality audit » series), kept for history only. **Do not edit.**
> Many findings were fixed since. Current status: `docs/active/migration/migration_plan.md` (section « État d'avancement et suite »), `.claude/state_index.md` and `docs/active/decisions/000-index.md`.

---

# Reality Audit — Echoes Studio runtime (état réel, 2026-06-03)

> **But** : reconstruire la _vérité runtime_ du moteur tel qu'il tourne
> aujourd'hui, indépendamment de ce que décrivent les docs `docs/active/` et les
> audits datés. Aucune proposition d'archi, aucun refactor. On constate.
>
> **Méthode** : lecture directe du code Rust (`src-tauri/src/` + crates
> `clap_ffi`, `plugin_bridge`, `plugin_host_bridge`), confrontée aux ADR
> (`docs/active/decisions/`) et au catalogue `critical_issues.md`.
>
> **Livrables liés** :
> [`plugin_instantiation_failure_analysis.md`](plugin_instantiation_failure_analysis.md) ·
> [`dsp_graph_reality_check.md`](dsp_graph_reality_check.md) ·
> [`adr_runtime_validation.md`](adr_runtime_validation.md) ·
> [`cleanup_regressions.md`](cleanup_regressions.md)

---

## 0. TL;DR

1. **Le pipeline DSP est unique et cohérent.** Il n'y a **pas** de double
   système de graphe. `AppProject → AudioGraph::from_project → GraphCompiler →
GraphScheduler → RuntimeSnapshot(AtomicPtr) → callback CPAL`. L'export
   offline (`offline.rs`) emprunte exactement le même chemin. Les fondations
   (DAG, topo-sort déterministe, snapshot atomique + drop-ring SPSC, PDC
   interne, FTZ/DAZ, worker-pool optionnel) sont saines et réellement câblées.

2. **Les plugins SONT instanciés, insérés, schedulés et processés.** Le focus
   « plugin instantiation failure » ne correspond **pas** à un point unique où la
   création échoue dans le happy-path. Le plugin VST3/CLAP est créé par
   `PluginHost`, embarqué comme `Box<dyn AudioNode>` dans le `CompiledGraph`,
   reçoit `prepare()` puis `process()` à chaque block. Voir
   [`plugin_instantiation_failure_analysis.md`](plugin_instantiation_failure_analysis.md).

3. **Là où ça casse réellement, c'est le cycle de vie / l'identité**, pas la
   création :
   - **Ré-instanciation à CHAQUE recompile** (et un recompile a lieu à chaque
     tour de bouton d'effet — cf. `critical_issues.md` §1.4). Le plugin natif est
     détruit/recréé, son état interne est perdu, et deux instances coexistent
     transitoirement.
   - **`records` indexé par `device_id`, pas par `NodeId`** → deux instances du
     même plugin sur deux pistes s'écrasent mutuellement côté host
     (paramètres + éditeur GUI adressent la mauvaise / une seule instance).
   - **Le teardown host est mort** : `destroy_device` / `suspend_device` /
     `resume_device` n'ont **aucun appelant**. Retirer un plugin d'une piste ne
     libère jamais l'`Arc` host → l'instance FFI fuit jusqu'à la fermeture de
     l'app.

4. **Beaucoup d'items « critical » de `critical_issues.md` (2026-05-31) sont
   périmés** : le code a depuis livré les Phases 2a/2b/2c, 3, 4, 5c/5d, 6. Les
   transports de paramètres, les instruments CLAP, la persistance d'état FFI, la
   latence/PDC plugin, l'instance partagée GUI, le `clap_entry.init` — tous
   résolus dans le code. Détail dans
   [`adr_runtime_validation.md`](adr_runtime_validation.md) et
   [`cleanup_regressions.md`](cleanup_regressions.md).

5. **Divergence de gouvernance** : `architecture_rules.md` (invariant #3) affirme
   « `audio_graph/` ne doit pas importer `plugin_system` ». Le code **viole** cette
   formulation : [`audio_graph/compiler.rs:36`](../../src-tauri/src/audio_graph/compiler.rs#L36)
   importe `plugin_system::PluginHost` et l'appelle. Or **ADR-003 sanctionne
   explicitement** cet appel (« The graph compiler calls
   `PluginHost::instantiate()` »). La règle `.claude` est donc trop stricte vs son
   propre ADR. C'est le _runtime_ (scheduler) qui est agnostique, pas le compiler.

---

## 1. Carte runtime réelle (qui appelle quoi)

```text
Tauri command (api.rs / main.rs)
  │  structural change → AudioEngine::update_from_project(project)
  ▼
AudioGraphCompiler::compile_scheduler(project, cfg, &live_params, &mut plugin_host)   [audio_engine/graph.rs]
  │   ├─ live_params.prune_to_project(project)
  │   ├─ AudioGraph::from_project(project)                  topologie  [audio_graph/graph.rs]
  │   └─ GraphCompiler::compile(graph, project, cfg, live_params, plugin_host)  [audio_graph/compiler.rs]
  │         ├─ topo-sort (Kahn, tie-break NodeId)           déterministe
  │         ├─ build_node(kind) pour chaque step
  │         │     └─ NodeKind::Plugin     → plugin_host.instantiate(node_id, device_id, sandboxed)
  │         │        NodeKind::Instrument → plugin_host.instantiate_instrument(device_id)   (CLAP only)
  │         ├─ node.prepare(ctx)  ── active/start_processing du plugin natif
  │         ├─ compute_latency_compensation (PDC)
  │         └─ compute_execution_levels (R1)
  ▼
GraphScheduler::with_workers(compiled, ECHOES_AUDIO_WORKERS)   [audio_graph/scheduler.rs]
  ▼
RuntimeSnapshot::enqueue(scheduler)     [audio_graph/snapshot.rs]  (drain drop-ring + publish AtomicPtr)
  ▼  (audio thread)
callback CPAL (engine.rs) → snapshot.try_swap(&mut box) → scheduler.process_block(transport, mix)
                                                            └─ run_levels → process_one_step → node.process()
```

- **Un seul** owner du `PluginHost` : `AudioEngine.plugin_host`
  ([`engine.rs:241`](../../src-tauri/src/audio_engine/engine.rs#L241)), peuplé par
  `scan_plugins()` au démarrage ([`engine.rs:258`](../../src-tauri/src/audio_engine/engine.rs#L258)).
- Le **thread audio ne touche jamais** le `PluginHost` : les instances voyagent
  dans le `GraphScheduler` via le snapshot. Conforme au modèle threading.
- **Params plugin** : `UPDATE_*` hot → `api.rs:797-811` →
  `plugin_host.push_plugin_param(device_id, native_id, value)` → ring SPSC de
  l'instance → drainé par le wrapper en tête de `process`. Réellement câblé.

---

## 2. Ce qui tourne vraiment vs ce que disent les docs

| Sujet                                  | Doc/`critical_issues.md` (2026-05-31) | Réalité du code (2026-06-03)                                                                    |
| -------------------------------------- | ------------------------------------- | ----------------------------------------------------------------------------------------------- |
| Transport params UI→plugin (§3.1)      | « aucun, critical »                   | **Câblé** (ADR-007/008, `param_queue.rs`, drain VST3/CLAP)                                      |
| Instruments VST3i/CLAPi (§3.2)         | « aucun, critical »                   | **CLAP câblé** (`ClapInstrument`, `instantiate_instrument`, ADR-013)                            |
| State plugin save/load (§3.5)          | « no-op, critical »                   | **FFI réel** VST3+CLAP (`save_state_blob`/`load_state_blob`, ADR-010) — mais perdu au recompile |
| Latence/PDC plugin (§3.6/§7.1)         | « toujours 0, critical »              | **Rapportée** : `latency_samples()` overridé via `clap.latency` / `getLatencySamples`           |
| Lifecycle bicéphale (§3.7)             | « deux sources, medium »              | **Unifié** (Phase 2b : atomic unique sur l'instance)                                            |
| Host sans ref instance (§3.8)          | « critical »                          | **Résolu** : `Arc<PluginInstance>` dans `records` (ADR-007)                                     |
| Double instance audio+GUI (§3.11/§6.3) | « critical »                          | **Résolu** (Phase 2c : `gui.rs` réutilise `instance.vst3_handles()/clap_plugin()`)              |
| `clap_entry.init` (§8.6)               | « corrigé »                           | **Confirmé** : `ClapLibrary::load` l'appelle 1×/DLL, cache process-wide                         |
| Sandbox out-of-process (§8.1)          | « aucun, critical »                   | **Opt-in câblé** (ADR-015 : `BridgedNode` + `plugin_host_bridge`, CLAP)                         |
| SEH C++ uncaught (§1.5/§8.2)           | « critical »                          | **Toujours vrai** : `catch_unwind` ne capte que les panics Rust                                 |
| Watchdog durée process (§1.6)          | « aucun, critical »                   | **Partiel** : budget spin côté `BridgedNode`; in-process toujours sans                          |
| Recompile sur param effet (§1.4/§4.6)  | « critical »                          | **Toujours vrai** : chemin param effet/plugin = recompile complet                               |
| `audio_engine/snapshot.rs` mort (§8.7) | « low »                               | **Confirmé mort** (aucun consommateur réel)                                                     |

> Conclusion : `critical_issues.md` et la mémoire projet décrivent un état
> **antérieur** aux phases plugin. La frontière moteur↔plugin est aujourd'hui
> bien plus fonctionnelle que ne le disent ces documents. Voir
> [`adr_runtime_validation.md`](adr_runtime_validation.md) pour la validation
> ADR par ADR.

---

## 3. Vrais risques runtime actuels (non couverts / mal couverts par les docs)

| #   | Risque réel                                                                                                                                                                                    | Sévérité               | Preuve                                                                                                                              |
| --- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------- | ------ | ----------------------------------------------------------------------------------------- |
| R-1 | **Ré-instanciation plugin à chaque recompile** → perte d'état interne du plugin, deux instances FFI transitoires                                                                               | critical               | `compile()` appelle `build_node`→`instantiate` à chaque fois ; aucune réutilisation par `NodeId`. ADR-007 §"t2" documente le cycle. |
| R-2 | **`records` indexé par `device_id`** : même plugin sur 2 pistes → un seul record survit ; params + GUI adressent la mauvaise instance                                                          | high                   | [`host.rs:206`](../../src-tauri/src/plugin_system/host.rs#L206) `records.insert(plugin_id, …)`                                      |
| R-3 | **Teardown host mort** : `destroy_device`/`suspend_device`/`resume_device` sans appelant → fuite des instances de plugins retirés                                                              | high                   | grep : définitions seules, 0 site d'appel                                                                                           |
| R-4 | **State plugin perdu au recompile** : `load_device_state` n'est rappelé qu'au load projet (`main.rs:252`), pas après une ré-instanciation déclenchée par un tour de bouton                     | high                   | flux `update_from_project` ne re-pousse pas `saved_state` dans la nouvelle instance                                                 |
| R-5 | **Crash plugin C++ (SEH) = crash DAW** en in-process (défaut)                                                                                                                                  | critical               | `catch_unwind` only, `scheduler.rs:630`                                                                                             |
| R-6 | **Params d'instrument CLAP jetés** : `ClapInstrument::process_midi` fait `drain_params(                                                                                                        | \_                     | {})`                                                                                                                                | medium | [`clap_instrument.rs:146`](../../src-tauri/src/audio_graph/nodes/clap_instrument.rs#L146) |
| R-7 | **`num_input_ports/num_output_ports` du trait non consultés** par le compiler/scheduler (le compiler utilise `num_*_for_kind` codé en dur) → comptes de ports déclarés par les nodes = vestige | low                    | `compiler.rs` `num_inputs_for_kind`, scheduler lit `plan.steps[].num_outputs`                                                       |
| R-8 | **`sandboxed` ignoré dans `PluginHost::instantiate`** (`let _ = sandboxed`), la vraie branche sandbox est dans `compiler.build_node`                                                           | low (cohérent, inerte) | [`host.rs:163`](../../src-tauri/src/plugin_system/host.rs#L163)                                                                     |

Détail R-1…R-4 et R-6 : [`plugin_instantiation_failure_analysis.md`](plugin_instantiation_failure_analysis.md).
Détail R-7 : [`dsp_graph_reality_check.md`](dsp_graph_reality_check.md).

---

## 4. Verdict

- **Cœur DSP** : sain, unique, déterministe, RT-safe sur le chemin interne.
- **Frontière plugin** : fonctionnellement bien plus avancée que la doc, mais la
  **gestion d'identité et de cycle de vie** des instances est le vrai point
  faible runtime (ré-instanciation, indexation `device_id`, teardown mort).
- **Crash-safety in-process** : reste le risque critique non résolu (SEH) ; le
  sandbox opt-in CLAP est la seule mitigation réelle, désactivé par défaut.
- **Documentation** : `critical_issues.md` et les mémoires plugin sont **périmés**
  et doivent être re-synchronisés (hors scope de cet audit, mais signalé).
