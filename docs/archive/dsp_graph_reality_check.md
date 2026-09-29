> **⚠️ ARCHIVED — SUPERSEDED.** Point-in-time audit of 2026-06 (« reality audit » series), kept for history only. **Do not edit.**
> Many findings were fixed since. Current status: `docs/active/migration/migration_plan.md` (section « État d'avancement et suite »), `.claude/state_index.md` and `docs/active/decisions/000-index.md`.

---

# DSP Graph — Reality Check

> Focus #2 de l'audit. Question : *quel graphe tourne vraiment ? Y a-t-il un
> double système ? Incohérences de scheduling, nodes non exécutés, edges
> invalides ?*
>
> Fichiers : `audio_engine/graph.rs`, `audio_graph/{graph,compiler,scheduler,execution_plan,snapshot}.rs`,
> `audio_engine/{engine,offline,snapshot}.rs`.

---

## 0. Réponse directe

- **Quel graphe tourne ?** Un seul : `audio_graph::GraphScheduler`, piloté par un
  `audio_graph::ExecutionPlan` immuable, dans le callback CPAL.
- **Double système ?** **Non.** `audio_engine/graph.rs` **n'est pas** un second
  moteur de graphe : c'est un **bridge mince** (`AudioGraphCompiler`) qui appelle
  `AudioGraph::from_project` + `GraphCompiler::compile`. La collision de nom
  (« graph ») prête à confusion mais ce sont deux couches distinctes.
- **Le seul ancien « second moteur DSP »** (le rendu offline via Web Audio
  `OfflineAudioContext` côté frontend) a été **supprimé** et remplacé par
  `audio_engine/offline.rs`, qui réutilise le **même** pipeline (cf.
  [`cleanup_regressions.md`](cleanup_regressions.md)).

---

## 1. Les deux fichiers « graph » — clarification

| Fichier | Rôle réel | Contient un graphe exécutable ? |
|---------|-----------|---------------------------------|
| [`audio_graph/graph.rs`](../../src-tauri/src/audio_graph/graph.rs) | **Modèle de topologie** (`AudioGraph`, `NodeKind`, `Edge`, `from_project`) | Non — topologie éditeur, non-RT |
| [`audio_engine/graph.rs`](../../src-tauri/src/audio_engine/graph.rs) | **Bridge** `AudioGraphCompiler::compile_scheduler` : `from_project`→`compile`→`GraphScheduler` | Non — orchestration, 96 lignes |
| [`audio_graph/compiler.rs`](../../src-tauri/src/audio_graph/compiler.rs) | Compile topologie → `ExecutionPlan` + `Vec<Box<dyn AudioNode>>` | Produit le plan |
| [`audio_graph/scheduler.rs`](../../src-tauri/src/audio_graph/scheduler.rs) | **Driver RT unique** : `process_block` | **Oui — c'est lui qui tourne** |

> `audio_engine/graph.rs` est le « single integration point » documenté dans son
> propre header. Garder son nom public évite de toucher `engine_gateway`. Ce
> n'est **pas** une duplication de moteur.

---

## 2. Cohérence de scheduling (ce qui est sain)

- **Topo-sort déterministe** (Kahn, BFS) avec tie-break `sort_unstable()` sur
  `NodeId` → ordre stable indépendant de l'itération `HashMap`
  ([`compiler.rs:430-481`](../../src-tauri/src/audio_graph/compiler.rs#L430)).
  Underpin de C-2 (bit-identique).
- **Niveaux d'exécution (R1)** : `compute_execution_levels` plie les arêtes
  audio **et** modulation dans la relation de dépendance → garantit
  `source_step < target_step`. Un seul passage forward suffit (steps déjà en
  ordre topo).
- **Exécution** : `run_levels` dispatche chaque niveau soit inline (séquentiel /
  niveau d'1 step), soit au `WorkerPool` (si `len ≥ 2` et pool présent).
  `ECHOES_AUDIO_WORKERS=0` par défaut → **purement séquentiel** sur le thread
  audio. Parité prouvée par test `parallel_pool_produces_same_output_as_sequential`.
- **PDC** : `compute_latency_compensation` aligne les chemins parallèles via des
  `DelayLine` par port (rings pré-alloués). Tests `pdc_*` verts.
- **Swap immuable** : `RuntimeSnapshot` (AtomicPtr + drop-ring SPSC). L'ancien
  scheduler est libéré sur le thread commande, **jamais** dans le callback
  ([`snapshot.rs`](../../src-tauri/src/audio_graph/snapshot.rs)). Conforme ADR-004.
- **Master** : `MasterNode` somme + soft-clip et écrit `output_buffers[master][0]`,
  lu directement par `write_master_output`. `num_outputs=1` déclaré pour
  pré-allouer ce slot.

---

## 3. Incohérences / nodes non exécutés / edges

### 3.1 ⚠️ Comptes de ports : trait vs compiler (R-7)
Le `AudioNode` trait expose `num_input_ports()`/`num_output_ports()`, **mais le
pipeline de production ne les consulte pas** :
- le compiler dimensionne via `num_inputs_for_kind` / `num_outputs_for_kind`
  (codés en dur par `NodeKind`, [`compiler.rs:485-530`](../../src-tauri/src/audio_graph/compiler.rs#L485)) ;
- le scheduler lit `plan.steps[].num_outputs` et `input_connections.len()`.

Exemple concret : un `NodeKind::Plugin` → compiler alloue **2 in / 2 out**, alors
que `ClapWrapperNode`/`VstWrapperNode` déclarent **1 in / 1 out**. Au runtime :
- les wrappers n'utilisent que `inputs[0]`/`outputs[0]` ; le port 1 alloué reste
  **silence en entrée / non écrit en sortie** (inoffensif mais gaspillé) ;
- les méthodes `num_*_ports()` des nodes sont **vestigiales** hors tests.
  `execution_plan.rs:62` ne fait que documenter l'égalité attendue — non vérifiée
  à l'exécution.

> Pas de bug audible, mais une **incohérence de contrat** : la source de vérité
> des ports est `num_*_for_kind` du compiler, pas le node.

### 3.2 ⚠️ Sends/Returns : tap non routé
`NodeKind::Send` produit un tap pass-through (1 in/1 out) mais **aucun
`ReturnNode` n'est câblé** : `NodeKind::Return` compile en `BusNode(1)` qui ne
collecte rien ([`compiler.rs:670-673`](../../src-tauri/src/audio_graph/compiler.rs#L670)),
et `from_project` ne crée jamais de node Send/Return. Les envois auxiliaires sont
donc **inertes** (conforme `critical_issues.md` §2.4). Pas un bug, une feature
absente.

### 3.3 ✅ Edges typés : seul `Audio` accepté
`validate_port_kinds` rejette toute arête non-`Audio`
([`compiler.rs:414-428`](../../src-tauri/src/audio_graph/compiler.rs#L414)). Comme
`from_project` ne crée que des arêtes `Audio` (defaut `connect`), c'est un no-op
qui préserve C-2. Les arêtes MIDI/Modulation sont rejetées tant que les ports ne
sont pas câblés (Phase 5c/5e) — tests `midi_edge_is_rejected`/`modulation_edge_is_rejected`.

### 3.4 ✅ Modulation : hors arêtes audio
Les routes de modulation vivent dans `ModulationMap` (vecteur parallèle), pas
dans `edges`. Le scheduler les pré-compile en `CompiledModAction` par step cible
et les applique block-rate (échantillonne `channel(0)[0]` du source). Source
toujours à un niveau strictement inférieur (garanti par `compute_execution_levels`).
**Block-rate seulement** (pas sample-accurate — `critical_issues.md` §5.2, exact).

### 3.5 ✅ Modulators sources
`NodeKind::Modulator` (LFO) : node source (0 in / 1 out), exécuté, son buffer lu
par les actions de modulation. Les variantes Envelope/Macro/AudioRate compilent
en LFO silencieux (`rate 0`) — stubs, conforme §5.1.

### 3.6 Nodes potentiellement non exécutés
- **Aucun node orphelin n'est dropped** : tous les `order` du topo-sort sont
  instanciés et processés. Un node sans arête sortante est quand même processé
  (gaspillage CPU mineur), mais `from_project` ne crée que des chaînes connectées
  au master.
- **Meter** : traité **avant** `build_node` (cas spécial, [`compiler.rs:239`](../../src-tauri/src/audio_graph/compiler.rs#L239))
  pour créer ses cellules atomiques ; pass-through audio.

---

## 4. Edges invalides / sécurité mémoire du scheduler

- `process_one_step` rassemble les pointeurs d'entrée depuis
  `output_buffers[src_step]` avec `src_port.min(len-1)` (garde contre port
  hors-borne). Entrée non connectée → `silence_ptr` (buffer zéro partagé).
- `unsafe` justifié par l'invariant `src_step < step` (niveaux antérieurs
  terminés) + ownership disjoint par worker. Le module documente le contrat ;
  `debug_assert` sur la taille du scratch.
- **Robustesse** : `catch_unwind` autour de `process` → en cas de panic Rust, le
  buffer de sortie du step est ré-clearé (silence ce block). Ne capte pas les SEH
  (cf. R-5).

---

## 5. Verdict graphe

| Question | Verdict |
|----------|---------|
| Graphe réellement utilisé | `audio_graph::GraphScheduler` (unique) |
| Double système de graphe | **Non** (bridge + modèle, pas deux moteurs) |
| Incohérences de scheduling | Non sur le chemin RT ; déterminisme tenu |
| Nodes non exécutés | Non (tous processés) ; ports plugin surdimensionnés (1 vs 2) inoffensif |
| Edges invalides | Bloqués au compile (`validate_port_kinds`) |
| Incohérence de contrat | `num_*_ports()` du node ignorés au profit de `num_*_for_kind` (R-7) |
| Features inertes (≠ bug) | Sends/Returns, modulators non-LFO, ports MIDI/mod |

Le cœur DSP est **cohérent et unique**. Les seules « incohérences » sont des
contrats de ports vestigiaux (R-7) et des features câblées-mais-inertes, pas des
défauts d'exécution.
