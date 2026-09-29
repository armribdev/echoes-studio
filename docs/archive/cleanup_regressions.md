> **⚠️ ARCHIVED — SUPERSEDED.** Point-in-time audit of 2026-06 (« reality audit » series), kept for history only. **Do not edit.**
> Many findings were fixed since. Current status: `docs/active/migration/migration_plan.md` (section « État d'avancement et suite »), `.claude/state_index.md` and `docs/active/decisions/000-index.md`.

---

# Cleanup Impact & Regressions

> Focus #4 de l'audit. *Ce qui a été supprimé / déprécié, ce qui est encore
> référencé indirectement, les fallbacks, et les systèmes hybrides legacy+new
> qui coexistent.*

---

## 0. Réponse directe

Pas de régression fonctionnelle introduite par un nettoyage cassé. Les seuls
points sont : **du code mort résiduel**, **des stubs**, **des chemins de
fallback silencieux**, et surtout **une documentation périmée** (`critical_issues.md`,
mémoires plugin) qui décrit un état antérieur — c'est la « régression »
principale : *drift doc ↔ code*.

---

## 1. Code mort résiduel (toujours compilé, jamais utilisé)

### 1.1 `audio_engine/snapshot.rs` — types legacy morts
`AudioGraphSnapshot` / `TrackSnapshot` / `ClipSnapshot`
([`audio_engine/snapshot.rs`](../../src-tauri/src/audio_engine/snapshot.rs)) ne sont
**référencés que dans leur propre fichier** + un commentaire de `audio_engine/mod.rs`.
Aucun consommateur réel (le runtime utilise `audio_graph::RuntimeSnapshot`). Le
header le dit lui-même (« no longer used … retained … will be removed »). Confirme
`critical_issues.md` §8.7. **Impact** : confusion de lecture, risque d'usage
accidentel. Aucun effet runtime.

### 1.2 Lifecycle host — méthodes sans appelant
`PluginHost::{destroy_device, suspend_device, resume_device}` : définies, **0
appelant** (grep `src/`). La machine d'états `lifecycle.rs` (`transition`) n'est
donc jamais exercée pour suspend/resume au runtime ; seuls `Running` (forcé à
l'instanciation/prepare) et `Destroyed` (forcé, mais inaccessible faute
d'appelant) sont atteints. **Impact** : fuite des instances retirées (R-3), pas
d'erreur. Voir [`plugin_instantiation_failure_analysis.md`](plugin_instantiation_failure_analysis.md).

### 1.3 Param `sandboxed` ignoré côté host
`PluginHost::instantiate(…, sandboxed)` fait `let _ = sandboxed`
([`host.rs:163`](../../src-tauri/src/plugin_system/host.rs#L163)). La branche sandbox
réelle est dans `compiler.build_node` (`BridgedNode::try_new`). Le paramètre host
est donc **mort** (threadé « pour 6d » mais 6d a mis la logique ailleurs).

### 1.4 Comptes de ports des nodes — vestigiaux
`AudioNode::num_input_ports/num_output_ports` : implémentés partout mais **non
consultés** par le compiler (qui utilise `num_*_for_kind`) ni par le scheduler
(qui lit `plan.steps[].num_outputs`). Vestige hors tests. Détail dans
[`dsp_graph_reality_check.md`](dsp_graph_reality_check.md) §3.1.

---

## 2. Stubs (présents, intentionnellement vides)

- **AU / WASM** : `PluginHost::instantiate` renvoie `PassThroughDevice` +
  `eprintln` « not yet implemented »
  ([`host.rs:182-194`](../../src-tauri/src/plugin_system/host.rs#L182)).
  `plugin_formats/au/`, `plugin_formats/internal/wasm_runtime.rs` existent comme
  squelettes. Non câblés au runtime. Conforme `critical_issues` §3.16.
- **Modulators non-LFO** (Envelope/Macro/AudioRate) → compilent en LFO silencieux
  (`rate 0`) ([`compiler.rs:729-737`](../../src-tauri/src/audio_graph/compiler.rs#L729)).
- **`registry.scan_directory`** : stub `// TODO platform-specific scanner`
  ([`registry.rs:110`](../../src-tauri/src/plugin_system/registry.rs#L110)) — le vrai
  scan VST3/CLAP passe par `Vst3Host::scan`/`ClapHost::scan`, pas par cette
  méthode. `scan_directory` est donc un stub mort.

---

## 3. Fallbacks silencieux (chemins de repli réels)

Ces fallbacks **masquent** les échecs au lieu de les remonter à l'UI — à
surveiller car ils peuvent rendre un bug invisible :

| Fallback | Déclencheur | Conséquence | Lieu |
|----------|-------------|-------------|------|
| Plugin → `GainNode` unity | registry miss / FFI échoue | Le plugin disparaît, audio passe-plat, seul `eprintln` | `compiler.rs:705-710` |
| Instrument → `PolySynth` | `instantiate_instrument` échoue (non-CLAP, FFI) | Synthé interne au lieu du plugin demandé | `compiler.rs:580-585` |
| `BridgedNode` → in-process | sandbox indisponible (no bridge exe, non-CLAP) | Plugin chargé in-process malgré l'opt-in sandbox | `compiler.rs:683-695` |
| `BridgedNode` → pass-through | spawn bridge échoue / block partiel | Audio passe-plat, `eprintln` | `bridged_node.rs:176-181` |
| Compile error → graphe vide | erreur de compile | **Silence total**, `eprintln` « using empty graph » | `audio_engine/graph.rs:85-92` |
| CLAP/VST3 `process` erreur | status d'erreur FFI | pass-through input→output | wrappers `process` |

> **Régression latente** : aucun de ces fallbacks n'émet d'event Tauri. Un plugin
> qui échoue à charger devient un `GainNode` silencieux sans notification UI —
> l'utilisateur croit le plugin actif. (Recoupé par `critical_issues` §8.5 :
> aucune télémétrie d'instanciation.)

---

## 4. Systèmes hybrides legacy + new qui coexistent

### 4.1 Rendu offline : Web Audio (supprimé) → `offline.rs` (actif)
Ancien : le frontend rendait un **second graphe** via `OfflineAudioContext` et
envoyait les samples à `export_wav/mp3`. Supprimé. Nouveau
[`offline.rs`](../../src-tauri/src/audio_engine/offline.rs) réutilise **le même**
`AudioGraphCompiler::compile_scheduler` + `process_block`. **Bon nettoyage, pas de
régression** : le risque de divergence DSP (gain laws, plugins, modulation, PDC
absents en Web Audio) est éliminé. À vérifier : que **plus aucun** chemin
frontend ne génère d'audio (hors scope code Rust).

### 4.2 `bridge.rs` — re-export « dead until 6d »
[`plugin_system/bridge.rs`](../../src-tauri/src/plugin_system/bridge.rs) re-exporte
`plugin_bridge::{protocol, region, session, shmem}` avec un commentaire « Nothing
in the engine consumes these yet — dead until 6d ». **Le commentaire est
périmé** : `bridged_node.rs` consomme bien `plugin_bridge::session::{BridgeLayout,
BridgeSession}` (mais via la dépendance crate directe, pas via ce re-export). Le
re-export `bridge.rs` lui-même reste potentiellement inutilisé — à confirmer si
on veut le supprimer.

### 4.3 Persistance d'état : transitionnel `saved_state` + FFI
`DeviceRecord.saved_state` (cache `DeviceState`) **coexiste** avec le chemin FFI
(`save_state_blob`/`load_state_blob`). `save_device_state` tente le FFI puis
retombe sur le cache. Hybride documenté (ADR-007/010), pas cassé, mais c'est une
**double source** de vérité d'état tant que le FFI n'est pas universel.

### 4.4 VST3 sans cache DLL vs CLAP avec cache
CLAP : `CLAP_LIBRARIES` (cache process-wide + init unique). VST3 :
`Vst3Library::load` refait `LoadLibraryW` à chaque instanciation (l'OS refcount le
module). Asymétrie historique, pas un bug, mais incohérence de stratégie.

---

## 5. Régression documentaire (la plus importante)

`critical_issues.md` (2026-05-31), `current_engine_diagram.md`, et les mémoires
`plugin-gui-audit-pointer` / `audio-engine-audit-pointer` décrivent un moteur
**antérieur** aux Phases 2a/2b/2c, 3, 4, 5c/5d, 6. Items marqués « critical /
présent » mais **résolus dans le code** :

- §3.1 transport params · §3.2 instruments CLAP · §3.5 state FFI · §3.6/§7.1
  latence plugin · §3.7 lifecycle unifié · §3.8 ref host · §3.11/§6.3 double
  instance GUI · §2.1 MIDI en samples (en réalité beats).

**Recommandation** (hors scope exécution) : repasser `critical_issues.md` au
filtre du code et archiver/réviser via `doc_sync_rules.md`. Tant que ce n'est pas
fait, toute décision basée sur ces docs partira d'un état faux.

---

## 6. Synthèse cleanup

| Catégorie | Élément | Risque |
|-----------|---------|--------|
| Code mort | `audio_engine/snapshot.rs` | low (hygiène) |
| Code mort | `destroy/suspend/resume_device` | **high** (fuite, lifecycle non câblé) |
| Code mort | param `sandboxed` host, `scan_directory`, comptes de ports node | low |
| Stub | AU/WASM, modulators non-LFO | medium (features absentes) |
| Fallback silencieux | plugin/instrument/bridge → repli sans event UI | **medium** (échecs invisibles) |
| Hybride | offline (résolu), `bridge.rs` re-export, `saved_state`+FFI, cache VST3 vs CLAP | low |
| **Doc drift** | `critical_issues.md` + mémoires périmés | **high** (décisions sur état faux) |
