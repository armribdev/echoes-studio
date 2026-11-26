# Plan de migration incrémental — Echoes Studio

> **Base** : [`../audits/plugin-system-audit-2026-05-31.md`](../audits/plugin-system-audit-2026-05-31.md) + [`target_architecture.md`](target_architecture.md)
> **Cadre** : C-1 à C-10 du target (zéro mutex RT, bit-identique offline, migration variant par variant, compat ascendante, lock-free cmd↔audio, instance plugin unique, compileur étendu pas réécrit, UX track-linéaire préservée, sandbox opt-in, pas de flag pour C-2).
> **Date** : 2026-05-31. **Statut** : proposition pour ratification.
> **Numéros d'ADR** : les sections « Phase 1 » à « Phase 6 » ci-dessous citent la numérotation _proposée_ dans `target_architecture.md` (ADR-006 à 014), qui ne correspond plus aux ADR réels de [`../decisions/000-index.md`](../decisions/000-index.md). Correspondance : Arc unique 006→**007** (capabilities/PDC = **006**), SPSC params 007→**008**, PortKind 008→**011**, `ParameterBlock` 009→**009**, sandbox 010→**015**, automation 011→**021** (+ **019**), MIDI en beats 012→**012**. Les propositions 013 (WorkerPool partagé) et 014 (containers) n'ont pas d'ADR ; les ADR réels 013 et 014 portent sur les instruments CLAP et le dimensionnement des ports. La section « État d'avancement » ci-dessus utilise déjà les numéros réels.

---

## État d'avancement et suite (mis à jour 2026-11-26)

Les statuts détaillés par sous-système sont dans [`.claude/state_index.md`](../../../.claude/state_index.md) ; cette section donne la vue d'ensemble.

### Fait

| Phase                                                                                  | État                                                                                                                                                                                                                                                                        | Repères                                                                                                                                                                                                     |
| -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1 Fondations safe                                                                      | livré                                                                                                                                                                                                                                                                       | latence/PDC, `catch_unwind` ; les types `DeviceCapabilities` et le trait `Device`, jamais consommés, ont été retirés le 2026-09-29 (amendement ADR-006)                                                     |
| 2 `Arc<PluginInstance>`                                                                | livré                                                                                                                                                                                                                                                                       | ADR-007, ADR-016 (une seule instance audio/GUI)                                                                                                                                                             |
| 3 Param transport + `ParameterBlock`                                                   | livré                                                                                                                                                                                                                                                                       | ADR-008, ADR-009, ADR-017                                                                                                                                                                                   |
| 4 Séparation DSP/UI + état plugin                                                      | livré                                                                                                                                                                                                                                                                       | ADR-010 (sauvegarde/restauration d'état)                                                                                                                                                                    |
| 5a–5e PortKind, MIDI en beats, instruments CLAP, capabilities→compilateur, modulateurs | livré côté moteur                                                                                                                                                                                                                                                           | ADR-011 à 014, 019, 021 ; sidechain + multi-sorties CLAP réels                                                                                                                                              |
| 5e automation                                                                          | moteur livré (`audio_graph/automation.rs`, commandes `*_AUTOMATION_*`) ; éditeur dans l'arrangement (2026-09-29) : paramètres de plugins (testé), volume de piste et effets internes en automation absolue (ADR-025, moteur + sélecteur livrés, pas encore testé à la main) | voir « Suite »                                                                                                                                                                                              |
| 5f Entrée MIDI matérielle                                                              | livré (notes uniquement)                                                                                                                                                                                                                                                    | ADR-022                                                                                                                                                                                                     |
| 6 Sandbox                                                                              | livré, opt-in : CLAP et VST3 (effets, instruments, sidechain, sorties auxiliaires, presets, éditeur), scan des plugins hors processus ; VST3 testé à la main le 2026-11-07                                                                                                  | ADR-015, ADR-033, ADR-034                                                                                                                                                                                   |
| V1 Harnais C-2                                                                         | livré sous forme légère                                                                                                                                                                                                                                                     | `audio_engine/render_harness.rs`                                                                                                                                                                            |
| Hors plan initial                                                                      | livré                                                                                                                                                                                                                                                                       | trim/offset de clips (ADR-018), PCM progressif (ADR-024), sampler intégré à pads (ADR-028, projet harnais `sampler_kit`), bibliothèque de sons phase 1 en cours (ADR-023), UX arrangement et panneau gauche |
| Enregistrement MIDI dans un clip                                                       | livré                                                                                                                                                                                                                                                                       | ADR-022 (amendement 2026-09-29), ADR-029                                                                                                                                                                    |
| Hôte VST3 et extensions CLAP                                                           | livré : VST3 au niveau de CLAP (instruments, presets, sidechain, sorties auxiliaires, suivi des paramètres), remote-controls, presets mémorisés avec le projet, latence des instruments CLAP, noms de notes                                                                 | ADR-013, 014, 031, 035                                                                                                                                                                                      |
| Export                                                                                 | possède les plugins pendant le rendu ; clips MIDI liés                                                                                                                                                                                                                      | ADR-036 (pas encore testé à la main), ADR-032                                                                                                                                                               |

### Suite (vérifiée dans le code le 2026-11-26, ordre recommandé)

Chaque point dit ce qui a été constaté. « Non revérifié » = repris d'un document sans relire le code.

1. **Livraison : fait le 2026-11-26.** Sans le bridge à côté de l'exécutable (`locate_bridge_exe`, `plugin_system/bridge.rs`), `PluginScanner::scan` s'arrête avec « scan skipped » et un build installé ne liste aucun plugin. `pnpm tauri:build` déclare maintenant le bridge en sidecar (`src-tauri/tauri.bundle.conf.json`, passé par `--config`) ; `beforeBuildCommand` lance `scripts/prepare-bridge.mjs` (build release, copie en `src-tauri/binaries/plugin_host_bridge-<triplet>`, ignoré par git). Vérifié : l'installeur NSIS contient `plugin_host_bridge.exe` à côté de `echoes-studio.exe`. Pas encore essayé : installer, lancer l'app, voir la liste des plugins. Un `tauri build` lancé sans `--config` produit toujours un installeur sans bridge. Aucun invariant touché, pas d'ADR.
2. **CI : le job Rust échoue probablement.** `cargo fmt --all -- --check` (étape du job) signale 8 fichiers non formatés : `plugin_host_bridge/src/{clap_adapter,vst3_adapter}.rs`, `plugin_host_bridge/tests/bridge.rs`, `src/main.rs`, `src/plugin_system/{bridge,ffi_lifecycle_tests,host,instance}.rs`. `cargo clippy --workspace -- -D warnings` n'a pas été relancé (d'anciennes erreurs dans `vst3_host` étaient connues). À faire : un commit `style` qui ne formate que ces fichiers, relu, puis un clippy.
3. **Vérifications manuelles en attente** (code livré et testé automatiquement, jamais essayé dans l'app) : export pendant la lecture (ADR-036) ; preset CLAP mémorisé puis projet rouvert (ADR-035) ; noms de notes VST3 dans la piano roll (aucun plugin installé n'en donne) ; latence d'un instrument CLAP (aucun plugin installé n'en annonce) ; nom et couleur de piste d'un plugin VST3 en sandbox ; sorties auxiliaires de Surge XT (Scene A) ; durée du scan au premier démarrage et fermeture pendant un scan (ADR-034) ; éditeur de plugin sur écran HiDPI.
4. **UI d'automation, ce qui reste** :
   - Choix linéaire / escalier par lane : fait le 2026-11-26 (`UPDATE_AUTOMATION_LANE` → `AUTOMATION_LANE_UPDATED`, bouton dans `AutomationLaneHeader.svelte`). Pas encore essayé dans l'app.
   - Formes rapides : fait le 2026-11-26 (menu de l'en-tête de lane : rampe montante / descendante / sinus sur la boucle active, `withShape`). Pas encore essayé dans l'app.
   - Écriture en direct (enregistrer un geste sur un bouton de plugin) : ADR-037 accepté le 2026-11-26. Étapes 1 à 4 faites (plugins CLAP et VST3, dans le process d'Echoes et dans le bridge : anneau SPSC par instance, watcher, bouton ● dans l'en-tête de lane, remplacement de la plage du geste ; pas encore essayé dans l'app). Reste : l'essai à la main avec de vrais plugins.
   - Validation cross-OS des empreintes (ADR-025, harnais V1) : la CI exécute les tests Rust sur Windows seulement.
   - Fait depuis la dernière mise à jour : le texte de valeur fourni par le plugin (`get_plugin_param_text`, utilisé par `AutomationLane.svelte`).
5. **Bibliothèque, phase 4 : fait le 2026-11-26** (étiquettes, collections intelligentes, mémoire de contexte opt-in, Devices = no-go dans l'ADR-023, packs, vue grille ; vérifié dans le code, pas testé à la main) : [`../../library-browser-roadmap.md`](../../library-browser-roadmap.md).
6. **Note FX autres que l'arpégiateur : fait le 2026-11-26** (transposition, gamme, accord : ADR-038, étages sans état devant l'arpégiateur dans `InstrumentNode`, `NoteStage`, `AppTrack.note_effects`, bloc « Effets de notes » du panneau gauche ; pas encore essayé dans l'app ni à l'oreille). Le vrai nœud MIDI (option B de l'ADR-030) attend un besoin de routage MIDI entre nœuds ou d'une chaîne réordonnable.
6b. **Expressions par note (ADR-039, accepté le 2026-11-26)** : étape A faite (`MidiNote.expressions`, `audio_graph/note_expressions.rs`, `MidiEventKind::NoteExpression`, répétition par voix dans `NoteStage`, `CLAP_EVENT_NOTE_EXPRESSION` visé sur la touche, projet harnais `note_expressions` égal à `static_mix`, test avec le plugin CLAP de test ; pas entendu sur un vrai synthé). Reste : étape B (transport dans le sandbox, version du protocole du bridge), étape C (édition dans la piano roll), puis cycle 2 (entrée MPE et capture, à valider avec un contrôleur).
7. **Warp à la dépose d'un loop** : ni code ni ADR (l'ADR-023 le diffère explicitement). Impact sur le rendu offline : ADR obligatoire avant tout.
8. **Phase 7 (nettoyage legacy)** : après validation en production ; l'optimisation (nœuds mesurés dans `worker_pool.rs`, `ECHOES_AUDIO_WORKERS` vaut 0 par défaut — vérifié —, suréchantillonnage ×2 fixe) est listée dans [`critical_issues.md`](critical_issues.md) sans avoir été revérifiée.

**Décidé de ne pas faire (ne pas reproposer sans fait nouveau)** : `IComponentHandlerBusActivation` (2026-11-06, ADR-014 / ADR-033) ; lecture de la queue (`tail`) VST3 et mode `kOffline` (2026-10-23, wrapper doc « VST3 tail and processing mode ») ; drapeau « Isoler » par piste (ADR-015, une préférence par type de plugin suffit) ; `IUnitHandler` (ADR-031) ; articulations `IKeyswitchController` (volontairement non lues, à relancer seulement avec une maquette).

**Documentation à rattraper** : plusieurs lignes de [`critical_issues.md`](critical_issues.md) étaient en retard sur le code (corrigées le 2026-11-26 : 3.2, 3.3, 5.7, 6.6, 7.2, 8.1–8.5) ; les lignes de `state_index.md` « not committed » de la série du 2026-11-24/25 ont reçu leur hash de commit.

---

## 0. Lecture rapide

| Phase | Nom                                     | Effort   | Risque RT | Vague target | ADRs                      |
| ----- | --------------------------------------- | -------- | --------- | ------------ | ------------------------- |
| 1     | Fondations safe                         | ~3 j     | nul       | 1 (partiel)  | —                         |
| 2     | Plugin capabilities + Arc unique        | ~8–10 j  | faible    | 1            | ADR-006                   |
| 3     | DSP graph param transport               | ~7–10 j  | faible    | 1 + 2        | ADR-007, ADR-009          |
| 4     | Séparation DSP/UI stricte               | ~5–7 j   | faible    | 1 (clôture)  | ADR-006 (suite)           |
| 5     | Modulation + automation + MIDI/PortKind | ~12–15 j | modéré    | 3 + 4 + 5    | ADR-008, ADR-011, ADR-012 |
| 6     | Sandboxing opt-in                       | ~15–20 j | élevé     | 6            | ADR-010                   |
| 7     | Cleanup legacy                          | ~4–6 j   | nul       | —            | —                         |

**Total court terme (P1→P5)** : ~35–45 j. **Avec sandbox (P6)** : ~50–65 j. Cohérent avec l'estimation audit § _Estimation dette technique_ (~24 j P0/P1 + ~20 j bridge).

---

## Phase 1 — Fondations safe

### 1.1 Objectif

Mettre en place les types et hooks structurels sans changer aucun comportement runtime. Aucune ligne de code dans le callback audio n'est modifiée. Sortie : `DeviceCapabilities`, `PortKind` (déclaré, non encore validé), `latency_samples()` câblé, `catch_unwind` autour du `process` plugin (déjà partiellement fait selon audit _PARTIE 3_).

### 1.2 Justification (audit)

- Audit _PARTIE 3_ documente que cette fondation est déjà partiellement livrée (capabilities types, latence, catch_unwind). Il faut **clôturer** : types présents mais consommateurs absents.
- Couvre P0-6 (latence) et le court-terme de P0-5 (catch_unwind).
- Précondition de toutes les autres phases (voir Phase 2 → BP-1 dépend de capabilities).

### 1.3 Fichiers impactés

- `src-tauri/src/plugin_system/capabilities.rs` — vérifier que `DeviceCapabilities`, `AudioBusInfo`, `MidiPortInfo`, `ParameterInfo` sont complets selon audit P0-3.
- `src-tauri/src/plugin_system/wrapper.rs` — `Device::capabilities()` avec default sûr (= `DeviceInfo` actuel hardcodé).
- `src-tauri/src/plugin_formats/vst3/wrapper.rs` — appel `getLatencySamples` au `prepare()`.
- `src-tauri/src/plugin_formats/clap/wrapper.rs` — extension `clap.latency` au `prepare()`.
- `src-tauri/src/plugin_formats/clap/ffi.rs` — `ClapPluginLatency`.
- `src-tauri/src/audio_graph/scheduler.rs` — `catch_unwind` autour de `processors[i].process` (existe selon audit, à vérifier).
- `src-tauri/src/audio_graph/node.rs` — déclarer `PortKind { Audio, Midi, Modulation, Control }` (énum publique, **pas encore utilisée** par le compileur).
- ADR : `../decisions/006-plugin-capabilities-and-pdc.md` (existe selon audit).

### 1.4 Changements exacts

- Types capabilities : aucun consommateur, pas de match runtime.
- `Device::capabilities()` : default impl retourne `DeviceCapabilities` dérivée du `DeviceInfo` hardcodé (rétrocompat C-2).
- `latency_samples()` override sur les deux wrappers, valeur mise en cache au `prepare`.
- `PortKind` énum déclarée mais l'`Edge` reste `{ from: PortId, to: PortId }`. La validation arrive Phase 5.

### 1.5 Risques RT

- Nuls. Aucun changement dans `process_block` au-delà du `catch_unwind` (déjà coût négligeable, mesuré comme tel par l'audit).

### 1.6 Compatibilité avec ancien système

- Bit-identique offline garanti (C-2). Tous les match `NodeKind` inchangés.
- Capabilities lues mais ignorées : le compileur force toujours 2/2 (P0-3 non corrigé en Phase 1, on prépare seulement le terrain).

### 1.7 Rollback

- Revert direct : tous les changements sont additifs côté types, et `catch_unwind` est isolé à une fonction. Le revert n'affecte aucun caller.

### 1.8 Tests obligatoires

- Suite de tests `cargo test` actuelle verte.
- Test unitaire : `vst3::wrapper::latency_samples()` retourne la valeur reportée par un plugin mock.
- Test unitaire : `clap::wrapper::latency_samples()` idem.
- Test offline render bit-identique sur 2–3 projets de référence (C-2). Si pas encore en CI, créer le harness avant tout autre travail — **prérequis** de toute la migration.

### 1.9 Validation perf RT

- Mesure CPU moyenne `process_block` avant/après. Tolérance : ±0.5 %.
- Pas de XRun ajouté sur un projet test (32 effets, buffer 256, 48 kHz, 30 min).

### 1.10 Dette technique introduite

- `DeviceCapabilities::audio_input_buses` etc. peuplés mais inutilisés → tentation de les utiliser ailleurs avant Phase 5. **Discipline requise** : ne pas brancher au compileur tant que BP-2 (PortKind) n'est pas validé.

---

## Phase 2 — Abstraction plugin capabilities (PluginInstance unique)

### 2.1 Objectif

Refonder le lifecycle plugin sur un `Arc<PluginInstance>` partagé (ADR-006). Le host conserve l'instance, le node graphe et l'éditeur GUI clonent l'`Arc`. Couvre P0-2 (préparation), P1-2 (lifecycle bicéphale), P1-7 (host n'enregistre pas Weak), P1-9 (PluginInfo sans capacités) et la condition de A1 du GUI audit.

### 2.2 Justification (audit)

- P1-2 : « Deux états de cycle de vie maintenus en parallèle, jamais synchronisés ».
- P1-7 : « le host ne garde aucune référence vers l'instance — `set_parameter_normalised` ne sait pas vers _qui_ envoyer ».
- BP-1 du target : « pas de fallback possible : la double instance audio/GUI est cassée par construction ».
- Phase indispensable avant la Phase 3 (sans `Arc<PluginInstance>`, on ne sait pas adresser une SPSC par device).

### 2.3 Fichiers impactés

- `src-tauri/src/plugin_system/instance.rs` (nouveau) — `PluginInstance { native: NativeHandle, caps: DeviceCapabilities, params: ParameterMap, state: AtomicLifecycle, … }`.
- `src-tauri/src/plugin_system/host.rs` — `instances: HashMap<DeviceId, Arc<PluginInstance>>`, `instantiate` retourne `(Arc<PluginInstance>, Box<dyn AudioNode>)`.
- `src-tauri/src/plugin_formats/vst3/wrapper.rs` — `VstWrapperNode` détient `Arc<PluginInstance>` au lieu de posséder le handle FFI.
- `src-tauri/src/plugin_formats/clap/wrapper.rs` — idem `ClapWrapperNode`.
- `src-tauri/src/plugin_system/lifecycle.rs` — l'état devient un `AtomicU8` interne à `PluginInstance`. Supprimer la copie côté wrapper.
- `src-tauri/src/audio_graph/compiler.rs::build_node(NodeKind::Plugin)` — récupère la paire (Arc, Box) depuis le host.
- Côté GUI : `open_vst3_editor` / `open_clap_editor` consultent `host.instances[device_id]` et clonent l'`Arc`.
- `src-tauri/src/plugin_system/info.rs` — `PluginInfo { …, capabilities: DeviceCapabilities }` (P1-9).
- ADR : `../decisions/007-plugin-instance-arc.md` (matérialise ADR-006).

### 2.4 Changements exacts

- Lifecycle : `Created → Loaded → Initialized → Running → Suspended → Destroyed` géré atomiquement (`AtomicU8`).
- `PluginHost::set_parameter_normalised` : résolution `device_id → Arc<PluginInstance>`. **Pas encore** de SPSC (vient en Phase 3) — pour cette phase, on continue à stocker dans `DeviceState::Json` comme avant. **Pas de régression**, juste l'addressabilité.
- Capabilities introspectées au `prepare()` du `PluginInstance` et stockées dans son champ.
- Le wrapper `AudioNode` délègue : `prepare`, `process`, `latency_samples` lisent depuis `Arc<PluginInstance>`.

### 2.5 Risques RT

- **Risque : `Arc::clone` sur audio thread**. À éviter strictement. Stratégie : le `Box<dyn AudioNode>` détient _déjà_ un clone, capturé à la compilation. Aucun `clone` runtime.
- **Risque : drop d'`Arc` sur audio thread**. Lorsque le snapshot précédent est libéré via drop ring SPSC (audit _fondations 4_), un `Arc::drop` peut décrémenter le compteur sur le thread drop-worker (off-RT). C'est le pattern existant — vérifier que le drop ring est bien le worker qui drop, **jamais l'audio thread**.
- **Risque : modification de `PluginInstance` (params map, caps) pendant `process`**. Convention : la map params est figée après `prepare` ; toute évolution dynamique passe par recompile (snapshot atomique).

### 2.6 Compatibilité ancien système

- Le projet sérialisé est inchangé (le `device_id` reste l'identifiant).
- Les wrappers exposent toujours `Box<dyn AudioNode>` au compileur → `CompiledGraph.processors` inchangé en type.
- Comportement DSP strictement identique : `process` continue à passer `input_param_changes: null` (la SPSC arrive Phase 3). Bit-identique offline préservé.

### 2.7 Rollback

- Refactor structurel large → rollback = revert PR.
- Pour limiter le risque, livrer en **3 sous-PRs séquentielles** :
  - 2a : créer `PluginInstance`, le host le détient, wrappers délèguent.
  - 2b : lifecycle unifié.
  - 2c : GUI consume le même `Arc` (ferme A1 GUI audit).

### 2.8 Tests obligatoires

- Test : instancier un plugin, vérifier que `host.instances[id]` et le wrapper référencent le même `Arc` (compteur = 2 minimum).
- Test : suspend/resume sur le host propage l'état au wrapper (single source of truth).
- Test : ouvrir l'éditeur, modifier preset dans le GUI natif (Vital), vérifier que l'instance audio voit le changement (clôt A1 GUI audit). **Manuel** ou via le plugin Rust de test (cf. B4 audit).
- Test offline bit-identique préservé (C-2).

### 2.9 Validation perf RT

- Pas de différence mesurable attendue (un déréf supplémentaire par `process` call).
- Mesure : projet 32 plugins, 256 frames, 30 min, comparer CPU moyenne ±0.5 %.

### 2.10 Dette introduite

- `PluginHost::set_parameter_normalised` continue à écrire dans un blob JSON sans effet DSP. Documenté comme dette à clôturer en Phase 3.
- `ParameterMap` de `PluginInstance` peuplée au prepare mais aucun chemin pour la modifier dynamiquement.

---

## Phase 3 — DSP graph parallèle (param transport + ParameterBlock)

### 3.1 Objectif

Implanter la SPSC `ParamChange` (ADR-007) **et** le `ParameterBlock` par effet interne (ADR-009). Couvre P0-1 (transport UI→DSP), P1-1 (énumération params), P1-8 (extension `LiveParams`), et casse la cause majeure des recompiles UI documentée dans target §1 (audit critical_issues 1.3/1.4/4.6).

### 3.2 Justification (audit)

- P0-1 : « Aujourd'hui un plugin chargé est essentiellement un boîtier opaque ».
- Target ADR-009 : « la recompile est réservée aux changements **structurels** ».
- Cette phase débloque immédiatement l'expérience utilisateur (knob → son), donc **quick win majeur** dès que Phase 2 est posée.

### 3.3 Fichiers impactés

- `src-tauri/src/plugin_system/param_queue.rs` (nouveau) — `SpscQueue<ParamChange>` borné (1024 slots préalloués), wrapper sûr autour de `rtrb`/`ringbuf`.
- `src-tauri/src/plugin_system/instance.rs` — `PluginInstance.param_queue: ParamQueue` + `native_id_for_key: HashMap<ParamKey, u32>`.
- `src-tauri/src/plugin_formats/vst3/wrapper.rs::process` — drain SPSC → `IParameterChanges` via une `ParameterChanges` allouée au `prepare` (pré-allocation RT-safe).
- `src-tauri/src/plugin_formats/clap/wrapper.rs::process` — drain → `clap_input_events` rempli via slots préalloués.
- `src-tauri/src/audio_graph/live_params.rs` — `effect_params: HashMap<EffectId, Arc<ParameterBlock>>` (cmd-side `RwLock` pour insert, audio-side clone d'Arc capturé au compile, cf. BP-8).
- `src-tauri/src/audio_graph/parameter_block.rs` (nouveau) — `ParameterBlock { cells: Vec<AtomicF32> }` indexé par enum local au node.
- `src-tauri/src/audio_graph/nodes/eq.rs`, `compressor.rs`, `reverb.rs`, `delay.rs` — lecture `ParameterBlock` au lieu de `EffectParams` figé. Recompile uniquement sur changement structurel.
- `src-tauri/src/engine_gateway/api.rs::update_effect_param` — code path dual (T4 du target §5.2) : si bloc alloué → write atomique, sinon → recompile legacy.
- `src-tauri/src/engine_gateway/api.rs::set_plugin_parameter` — push dans `PluginInstance.param_queue` au lieu d'écrire JSON.
- `src-tauri/src/plugin_system/host.rs` — `set_parameter_normalised` route vers la queue.
- ADRs : `008-param-transport-spsc.md`, `009-parameter-block.md`.

### 3.4 Changements exacts

- `ParamChange { node_id, native_id, normalised, sample_offset }` (audit P0-1, cible PARTIE 2 du target).
- `sample_offset: 0` (block-rate, BP-7) jusqu'à activation flag `sample_accurate_modulation` (différé Phase 7).
- SPSC drainée **au début** de `process` par le wrapper, avant l'appel FFI plugin.
- `ParameterBlock` alloué par node au `prepare`, `Arc` cloné dans `LiveParams.effect_params[effect_id]` côté cmd.
- Migration variant par variant : Eq d'abord (le plus simple), puis Compressor, Reverb, Delay. Plugin path activé après.

### 3.5 Risques RT

- **Risque : taille SPSC**. 1024 slots à 60 Hz UI = 17 s de buffer worst case. Largement suffisant. Si plein → drop des plus anciens (politique à acter dans ADR-008 ; recommandé : _drop newest_, l'UI peut renvoyer la valeur courante au prochain tick).
- **Risque : allocation `IParameterChanges` à chaque block**. VST3 SDK l'expose comme allocable. Solution : implémenter notre propre `IParameterChanges` Rust qui pointe sur des slots préalloués au `prepare`. Pareil pour CLAP : `clap_input_events` est un vtable avec un payload `void*` qui peut être un slab préalloué.
- **Risque : `RwLock` sur `effect_params`**. Lecture cmd-only au compile (off-RT). Audio thread possède son clone d'`Arc` via le snapshot. Aucun `lock` sur audio thread.
- **Risque : zipper noise sur jump de paramètre**. Le node interne doit smoother (one-pole / ramping). Convention à documenter ; les plugins externes ramping interne.

### 3.6 Compatibilité

- T4 : tant qu'un effet n'a pas son `ParameterBlock`, l'`update_effect_param` continue à recompiler. Aucune régression.
- Projet sérialisé : `EffectParams` inchangés.
- Bit-identique offline préservé tant que le projet ne déclenche pas d'UPDATE entre 2 blocks (cas trivial : export sans automation).

### 3.7 Rollback

- Toggle interne `engine.use_param_block: bool` (pas un feature flag user, un flag de compilation conditionnelle). Si bug détecté, retour à la recompile.
- Granularité par effet : on peut désactiver `ParameterBlock` pour Reverb tout en gardant Eq.

### 3.8 Tests obligatoires

- Test unitaire : push 2000 `ParamChange` dans une queue de 1024 → 976 sont droppées selon politique. Pas de panique.
- Test intégration : Eq freq sweep via UI knob, mesurer absence de XRun sur 10 min.
- Test offline bit-identique sur projet sans UPDATE (C-2).
- Test : plugin Vital, modifier knob host → entendre changement audio (clôt P0-1).
- Test : modifier paramètre Eq sans recompile (vérifier compteur `compile_count` constant).
- Test plugin mock Rust (B4 audit) qui rapporte les `ParamChange` reçus pour vérifier le drain.

### 3.9 Validation perf RT

- Coût drain SPSC : ~50 ns par change drainée, mesuré sur un projet test.
- Mesure : 16 plugins, automation 60 Hz sur 32 params chacun, vérifier que le coût total < 1 % CPU sur i7-12700.
- Benchmark `process_block` avec 0 changes vs 100 changes : delta < 5 µs.

### 3.10 Dette introduite

- `sample_offset` toujours à 0 : automation rapide produira du stepping (BP-7, acté dans target).
- `ParameterBlock` peut diverger de `EffectParams` projet si un UPDATE structurel survient sans flush. **Solution** : recompile force re-prepare qui re-aligne. Documenter.
- Politique de drop SPSC (drop newest vs oldest) à fixer par ADR avant merge.

---

## Phase 4 — Séparation DSP / UI stricte (clôture)

### 4.1 Objectif

Achever la séparation DSP/UI : l'UI ne touche plus jamais un état partagé via lock. Toute communication passe par (a) `LiveParams` atomics, (b) SPSC `ParamChange`, (c) snapshot atomique pour structurel. **Clôture A1 du GUI audit** (instance unique GUI/audio, déjà préparée Phase 2). Couvre P0-4 (save/load state) puisque sans cette phase, le state save est tiré entre cmd thread et GUI thread.

### 4.2 Justification (audit + target)

- Target §0 principe 4 + C-1 + C-5.
- Audit _fondations 3_ : « DSP/UI séparés ». Validé pour le moteur, à étendre aux plugins.
- A1 GUI audit : « le path GUI ouvre un **second** ClapPlugin/IComponent ». Clos en Phase 2 sur le plan structurel ; cette phase **purge** les chemins legacy résiduels.

### 4.3 Fichiers impactés

- `src-tauri/src/plugin_gui/vst3_editor.rs` — supprimer toute instanciation propre. L'éditeur reçoit `Arc<PluginInstance>` depuis le `PluginHost`.
- `src-tauri/src/plugin_gui/clap_editor.rs` — idem.
- `src-tauri/src/plugin_system/host.rs::open_editor(device_id)` — point d'entrée unique pour le GUI.
- `src-tauri/src/plugin_system/state.rs` (nouveau) — `save_state` / `load_state` via FFI (`IComponent::getState`, `clap_plugin_state.save`), stockés dans `DeviceState::Binary`. Implémente P0-4.
- `src-tauri/src/project.rs::AppProject` — ajouter `#[serde(default)] plugin_state_blobs: HashMap<DeviceId, Vec<u8>>` (cf. target §3.1, T2).
- `src-tauri/src/engine_gateway/api.rs::save_project` / `load_project` — appelle `instance.save_state()` / `load_state()`.
- ADR : `010-plugin-state-persistence.md`.

### 4.4 Changements exacts

- L'UI plugin GUI ne crée plus aucun handle FFI : elle reçoit l'`Arc` et appelle `instance.attach_editor(hwnd)`.
- Save/load state : FFI `IBStream` (VST3) / `clap_istream`/`clap_ostream` (CLAP), via helpers sûrs encapsulés (B1 audit).
- Bump de version `ProjectFile` (cf. B3 audit) : champ `state_b64` optionnel, no-op si absent.
- Suppression du blob JSON dans `DeviceRecord` (devenu obsolète après Phase 3).

### 4.5 Risques RT

- Nuls côté audio. Save/load state s'exécutent côté cmd thread, hors RT.
- Risque réentrance host (B2 audit) : si un plugin demande un rescan depuis son worker thread pendant un save, sérialiser via `WM_APP_DRAIN` (déjà partiellement en place côté GUI).

### 4.6 Compatibilité

- Projet legacy : `plugin_state_blobs` absent → no-op au load (T2 + C-4).
- Comportement DSP inchangé. Bit-identique offline préservé (C-2).
- Suppression du blob JSON : pas de régression car jamais lu côté DSP.

### 4.7 Rollback

- Granulaire : 3 sous-PRs.
  - 4a : GUI consume `Arc` (closure A1).
  - 4b : save/load state VST3.
  - 4c : save/load state CLAP.
- Toggle save/load par plugin : si un plugin foire au load, fallback empty + log.

### 4.8 Tests obligatoires

- Test E2E : ouvrir Vital, configurer un preset custom, save projet, fermer, rouvrir → preset intact (clôt P0-4).
- Test idem Surge XT (CLAP).
- Test : ouvrir l'éditeur, modifier dans GUI natif → save projet → reload → état GUI restauré (vérifie unicité instance).
- Test load d'un projet master (sans `plugin_state_blobs`) → comportement actuel (C-4).
- Test fuzz : blob state corrompu → erreur propre, plugin reset à defaut, pas de crash.

### 4.9 Validation perf RT

- Mesure save state : < 100 ms pour un Vital (référence Bitwig).
- Pas de modif audio thread.

### 4.10 Dette introduite

- Format binaire VST3 vs CLAP non interopérable (normal, mais à documenter dans le format projet).
- Schéma projet bumpé : prévoir un migrateur pour les projets futurs.

---

## Phase 5 — Modulation system (PortKind + MIDI + Automation + Modulators étendus)

### 5.1 Objectif

Étendre le système de modulation : PortKind typé (ADR-008), MIDI vers plugins instruments (P0-2), modulators étendus (P1-4), automation lanes (ADR-011), `MidiNoteSnapshot` en beats (ADR-012, corrige UPDATE_TEMPO desync). **Phase la plus large** : à découper en 5 sous-phases.

### 5.2 Justification (audit + target)

- P0-2 : « Vital, Surge XT, Diva, Serum — chargés comme insert FX uniquement ».
- P0-3 + sidechain : dépend de capabilities + PortKind.
- P1-4 : « pas d'ADSR routable, pas de macros ».
- Target ADR-008, ADR-011, ADR-012.

### 5.3 Découpage en sous-phases

**5a — PortKind validé par le compileur** (~2 j)

- `Edge { from, to, kind: PortKind }` avec default `Audio` (BP-2).
- `GraphCompiler::compile` valide les connexions homogènes.
- Aucune nouvelle arête MIDI/Modulation encore créée → bit-identique (C-2).

**5b — MidiNoteSnapshot en beats** (~2 j, ADR-012)

- `MidiNoteSnapshot` bake en beats, résolution sample per-block via `TransportInfo`.
- `UPDATE_TEMPO` n'invalide plus la timeline MIDI.
- Test bit-identique exigé (BP-3).

**5c — MIDI ports first-class + NodeKind::Plugin instrument** (~4 j)

- `PortKind::Midi` arêtes entre nodes.
- `InstrumentNode.inner: Box<dyn AudioNode>` polymorphe (BP-4) — accepte `Arc<PluginInstance>`.
- `PluginHost.midi_queues: HashMap<DeviceId, SpscQueue<MidiEvent>>` (target §3.1).
- Wrapper draine et convertit en `clap_event_note` / `IEventList`.
- Couvre P0-2.

**5d — Capabilities → compiler (sidechain, multi-out)** (~2 j)

- `num_inputs_for_kind(NodeKind::Plugin)` lit `capabilities.audio_input_buses`.
- Bus aux/sidechain exposé comme port séparé.
- Couvre P0-3 + P1-5 (sidechain UI routing).

**5e — Modulators étendus + automation** (~5 j)

- `ModulatorParams::{Envelope, Macro, Random, AudioFollower}` (P1-4).
- `ModulationTargetRef::PluginParam { device_id, param_key }` (target §6.4) — pousse dans la SPSC de Phase 3.
- `AutomationLane` projet, baked en `AutomationCurve` au compile.
- `AutomationEvaluator` appelé block-rate par `process_block` avant les nodes (target §3.1).
- LFO tempo-sync + phase reset (P2-6, P2-7).

### 5.4 Fichiers impactés (synthèse)

- `audio_graph/node.rs`, `graph.rs`, `compiler.rs`, `scheduler.rs`.
- `audio_graph/midi.rs` (MidiNoteSnapshot), `live_params.rs` (automation_cells).
- `audio_graph/nodes/{instrument,lfo,envelope,macro,follower,random}.rs`.
- `project.rs` (`automation_lanes`, `ModulatorParams` étendu, `ModulationTargetRef::PluginParam`).
- `plugin_system/host.rs` (midi_queues).
- `plugin_formats/{vst3,clap}/wrapper.rs` (drain MIDI).
- ADRs réels : `011-portkind-typed-edges.md`, `012-midi-snapshot-beats.md`, `013-clap-plugin-instruments.md`, `014-capability-driven-port-sizing.md`, `019-sub-block-modulation.md`, `021-plugin-parameter-modulation-and-automation.md`.

### 5.5 Risques RT

- **5c MIDI drain** : analogue à param drain, même pattern SPSC. Coût ~50 ns/event.
- **5d capabilities → compiler** : aucun impact RT (compile-time).
- **5e automation** : evaluator block-rate écrit dans atomics. Coût mesuré : ~1 µs / 100 lanes / block. Acceptable.
- **5a PortKind** : zéro coût RT (compile-time validation).
- **Sample-accurate automation** : explicitement différé sous flag (BP-7 + 8.4 target). Pas dans cette phase.

### 5.6 Compatibilité

- 5a : default `Audio` → bit-identique.
- 5b : `MidiNote` projet déjà en beats → pas de migration projet. Snapshot interne change : test bit-identique BP-3 critique.
- 5c : `NodeKind::Instrument.inner` reste `PolySynth` tant qu'aucun `device_id` instrument n'est routé. Migration variant par variant.
- 5d : sidechain n'est exposée que si un plugin déclare `is_aux`. Plugins existants → comportement actuel.
- 5e : `AutomationLane`, modulators étendus, `PluginParam` cible → tous `Vec::new()` / variant inutilisé par défaut.

### 5.7 Rollback

- Chaque sous-phase rollback-able indépendamment.
- 5b : le plus risqué (BP-3). Toggle dev `engine.midi_baking: legacy | beats` pendant 1 cycle de release.
- 5c : feature flag `plugin_instruments` côté UI (autorise/interdit la sélection d'un plugin comme instrument).

### 5.8 Tests obligatoires

- 5a : test offline bit-identique (C-2).
- 5b : bit-identique sur projet à tempo constant + test dédié projet avec `UPDATE_TEMPO` runtime (avant/après doivent diverger d'une manière contrôlée, et la version cible doit garder le grid aligné).
- 5c : charger Diva sur track MIDI, jouer note, vérifier audio (clôt P0-2 — cf. suggestions audit _Tests de validation_).
- 5d : charger Pro-C 2 avec sidechain, brancher track aux, vérifier ducking.
  Routage implémenté 2026-09-25 (ADR-014, `PluginParams.sidechain_source` →
  `AudioGraph::from_project`), couvert par des tests unitaires de topologie
  (`audio_graph/graph.rs::routing_tests`) ; le test manuel "charger un vrai
  plugin, entendre le ducking" reste à faire.
- 5d multi-out : charger une boîte à rythmes CLAP multi-sorties, prendre sa
  sortie « snare » comme entrée d'une autre piste, vérifier qu'on l'entend
  là. Implémenté 2026-09-25 (ADR-014), couvert par des tests FFI réels
  contre `test_clap_plugin` ; le test manuel reste à faire.
- 5e : ADSR envelope sur Eq freq, vérifier shape audio. Automation Eq freq via clip, vérifier rampe.
- Stress : 64 lanes automation simultanées, 16 plugins MIDI, pas de XRun.

### 5.9 Validation perf RT

- Budget total automation + MIDI drain + modulation extended : < 5 % CPU sur projet « showcase » (16 tracks MIDI, 8 plugins, 32 lanes).
- Benchmark dédié `process_block` avant/après chaque sous-phase.

### 5.10 Dette introduite

- Modulation hors-bande (`Vec<ModulationRoute>`) **et** arêtes `PortKind::Modulation` coexistent (target §6.2). Cleanup différé Phase 7.
- Sample-accurate non traité : automation rapide stepping. Documenté.
- `InstrumentNode` accepte les deux types d'inner → matcher reste à maintenir.

---

## Phase 5f — Entrée MIDI hardware live (ADR-022)

### Objectif

Jouer en direct sur un instrument via un clavier/contrôleur MIDI matériel.
Périmètre volontairement restreint : notes uniquement (pas de CC/Learn, pas de
sortie MIDI, pas de routing par piste — une seule piste « armée » globale reçoit
tous les devices activés). **Enregistrement dans un clip livré le 2026-09-29**
(amendement ADR-022, `midi_recording.rs` ; pas encore testé à la main) :
bouton ● de la barre de transport, un clip créé à l'arrêt/pause/re-clic.
**Contrôles (pitch bend, CC, aftertouch) enregistrés et rejoués le 2026-10-13**
(ADR-029 : stockés en beats dans le clip, rattrapage de valeur au seek / boucle /
bords de clip, lignes de contrôle dans la piano roll ; pas encore testé à la
main).
**Arpégiateur livré le 2026-10-13** (ADR-030 : étage de notes dans
`InstrumentNode`, réglages `AppTrack.arpeggiator`, grille sur le transport en
lecture / ancrée sur l'accord à l'arrêt ; l'enregistrement ● capte toujours les
notes jouées ; pas encore testé à l'oreille).

### Justification

`target_architecture.md` §6.5 anticipe cette capacité (`MidiInputNode` +
capture device → SPSC → audio thread) mais elle n'avait jamais été planifiée
ici, et aucune lib MIDI n'était présente dans `Cargo.toml`. Livrée hors du
chemin critique P1→P5d : ne dépend d'aucune autre sous-phase de la Phase 5 et
ne bloque rien.

### Fichiers impactés

- `Cargo.toml` — dépendance `midir`.
- `audio_graph/midi.rs` — `MidiLiveInputMessage` / `MidiLiveInputQueue`
  (miroir de `MidiPreviewMessage` / `MidiPreviewQueue`).
- `audio_graph/live_params.rs` — `midi_live_input` map, `live_input_queue` /
  `push_live_input`.
- `audio_graph/nodes/instrument.rs` — `live_input` / `set_live_input` /
  `drain_live_input`, câblé dans `process()` à côté de `drain_preview()`.
- `audio_graph/compiler.rs::build_node(NodeKind::Instrument)` — wiring de
  `live_input_queue`.
- `midi_input.rs` (nouveau) — `MidiInputRegistry` (énumération + connexions
  `midir`, thread de capture par device).
- `project.rs::AppState` — `midi_input: Mutex<MidiInputRegistry>`,
  `armed_track_id: Arc<Mutex<Option<String>>>`.
- `engine_gateway/api.rs`, `main.rs` — commandes
  `list_midi_input_devices`, `set_midi_input_enabled`,
  `arm_track_for_midi_input` (hors `CommandEnvelope`, comme
  `preview_midi_note`).
- Frontend : `src/lib/midiBackend.ts`, `src/stores/armedTrack.ts`,
  section MIDI de `SettingsPanel.svelte` (`settings/SettingsMidiSection.svelte`), bouton « Arm » dans
  `TrackStrip.svelte`.

### Risques RT

- Thread de capture MIDI (interne à `midir`) : parsing + lecture d'un
  `Mutex<Option<String>>`, hors thread audio — aucun risque.
- `InstrumentNode::drain_live_input()` : même forme que `drain_preview()`
  (ring pré-alloué, pas d'alloc/lock/drop) — RT-safe.
- Aucun nouveau mutex atteignable depuis `process_block`.

### Compatibilité

- **C-2** : aucun impact — l'export offline n'ouvre jamais de device MIDI ni
  ne draine cette queue (même non-interaction que l'audition piano-roll).
- **C-4** : aucun changement de schéma projet. Piste armée et devices
  activés sont de l'état de session/préférence, pas `AppProject`.

### Tests obligatoires

- Unitaires : `MidiLiveInputQueue` push/drain/overflow ;
  `InstrumentNode::drain_live_input` fusionne les événements matériels dans
  `event_buf`.
- `render_harness.rs` (C-2) reste vert — chemin offline non touché.
- Manuel : brancher un clavier MIDI, l'activer dans Réglages → MIDI, armer
  une piste, jouer, entendre l'instrument.

### Amendement 2026-09-26

« Local Control Off » envoyé au port de sortie MIDI du même nom (préférence,
activée par défaut) pour couper les haut-parleurs d'un piano numérique ;
rétabli à la désactivation et à la fermeture. Correctif : les claviers
activés sont rouverts au démarrage (abonnement aux préférences dans
`routes/+page.svelte`). Détails : amendement d'ADR-022.

### Amendement 2026-10-13

Pitch bend, CC (molette, sustain…) et aftertouch du clavier transmis aux
instruments CLAP (`MidiEventKind::Control` → `CLAP_EVENT_MIDI`, y compris en
sandbox) ; ignorés par les instruments intégrés et non enregistrés dans les
clips. File live portée à 1024 messages. Pas encore testé à la main.
Détails : amendement d'ADR-022.

### Suites (hors périmètre, notées dans ADR-022)

Enregistrement des CC/pitch bend dans les clips, MIDI Learn, sortie MIDI matérielle générale, enregistrement en boucle
(overdub / take), routing par piste (plusieurs pistes armées / device →
piste).

---

## Phase 6 — Sandboxing (ADR-010, opt-in par device)

### 6.1 Objectif

Bridge plugin out-of-process pour devices marqués `sandboxed: true`. Couvre P0-5 long-terme + 8.1–8.4 critical_issues. **Jamais activé par défaut** (C-9).

### 6.2 Justification

- P0-5 : « pas d'isolation process — un plugin 32-bit ou mal codé peut SEGV → tout le moteur tombe ».
- ADR-010 target.
- Vague 6 target.

### 6.3 Fichiers impactés

- `plugin_host_bridge/` (nouveau crate binaire) — process séparé qui charge le wrapper VST3/CLAP réel.
- `src-tauri/src/plugin_system/bridge/` (nouveau module) — proxy `Arc<PluginInstance>` qui communique via shared-memory ring buffers.
- `src-tauri/src/plugin_system/bridge/shmem.rs` — ring buffers audio in/out/params/events, pré-alloués.
- `src-tauri/src/plugin_system/bridge/watchdog.rs` — timeout sur process → mark `Dead`, silence.
- `src-tauri/src/plugin_system/host.rs::instantiate(sandboxed: bool)`.
- `project.rs::EffectParams::Plugin` — flag `sandboxed: bool` (default false, C-4).
- ADR : `015-plugin-sandbox.md`.

### 6.4 Changements exacts

- Proxy `BridgedPluginInstance` expose la même interface que `PluginInstance` (Arc-clonable, params queue, MIDI queue, state save/load).
- Communication : shmem ring buffers, sémantique SPSC ping-pong par block.
- Audio thread écrit input dans le ring, signale via futex/event, attend completion avec timeout, lit output.
- **Latence ajoutée** : 1 block (BP-5). Rapportée par `latency_samples()` → PDC interne absorbe (déjà câblé Phase 1).
- Prototype sur **un seul plugin** (Vital recommandé selon audit / target §8.4) avant généralisation.

### 6.5 Risques RT

- **Élevé.** L'audio thread attend la complétion du bridge. Si timeout (watchdog), insertion de silence + invalidation. Risque XRun si le timeout est mal calibré.
- Choix critique : `WaitForSingleObject` ou spin sur AtomicBool ? Recommandation : spin bornée (~10 µs) puis fallback silence + telemetry.
- Risque priority inversion entre bridge process et audio thread → priorité RT requise sur le bridge (Vague 6 target + 1.7 critical_issues).

### 6.6 Compatibilité

- Opt-in par device → projets existants inchangés (C-9 + C-4).
- API publique `Arc<PluginInstance>` inchangée (le proxy implémente la même interface).
- Bit-identique offline préservé pour les devices non-sandboxed.

### 6.7 Rollback

- Toggle global `engine.sandbox_enabled: bool` en plus du toggle per-device.
- Si bug critique : flip global → tous les plugins repassent in-process. Comportement = master actuel.

### 6.8 Tests obligatoires

- Test crash : plugin mock qui SEGV à un tick donné → DAW reste up, plugin marqué Dead, silence (clôt P0-5).
- Test perf : Vital sandboxed vs in-process, mesurer overhead latence + CPU.
- Test watchdog : plugin qui boucle infinie → timeout détecté, silence, recovery au prochain block.
- Test save/load avec plugin sandboxed (state passe par shmem).
- Tests sur les 3 OS (Vague 8.4 target — Windows en priorité).

### 6.9 Validation perf RT

- Overhead par-block sandbox : cible < 0.5 ms à 256 frames.
- XRun rate : 0 sur 30 min avec 4 plugins sandboxed.
- Si > 0.5 ms : la cible « live monitoring » impose de désactiver le sandbox sur ces devices.

### 6.10 Dette introduite

- Code path dual (in-process / bridged) → maintenance double. Inhérent à C-9.
- Pas de support 32-bit plugins dans le bridge initial (à acter dans ADR).
- IPC protocol versionning : prévoir bump pour évolutions futures (ajout de capabilities transport).

---

## Phase 7 — Cleanup legacy

### 7.1 Objectif

Supprimer la dette accumulée une fois les phases 1–6 validées en production sur ≥ 2 cycles de release.

### 7.2 Justification

- Target §5.3 garanties non-régression : les paths legacy sont conservés _tant que_ la migration n'est pas complète. Une fois tous les effets migrés vers `ParameterBlock`, tous les plugins vers SPSC params, toutes les modulations vers PortKind, on supprime.

### 7.3 Fichiers / éléments impactés

- Suppression du fallback recompile dans `update_effect_param` (Phase 3 dual path).
- Suppression du blob JSON `DeviceState` legacy (Phase 4 a déjà migré, mais le type reste).
- Suppression de `Vec<ModulationRoute>` hors-bande si toutes les modulations sont passées en arêtes typées (Phase 5e).
- Suppression du double matcher `InstrumentNode.inner` si la version polymorphe couvre 100 % des cas.
- Audit final coverage des `match NodeKind` → exhaustivité stricte sans branches mortes.

### 7.4 Changements exacts

- À déterminer par audit en début de Phase 7 (mesure de couverture sur projets utilisateurs).
- Pas de feature flag retiré tant que sample-accurate / sandbox global ne sont pas matures (cf. 8.4 target différé).

### 7.5 Risques RT

- Nuls. Suppression de code mort.

### 7.6 Compatibilité

- Suppression du fallback recompile → impose que tous les effets sérialisables aient leur `ParameterBlock` opérationnel.
- Migrateur projet pour les `EffectParams` legacy résiduels si besoin.

### 7.7 Rollback

- Git revert. Aucun risque utilisateur si Phases 1–6 sont stables.

### 7.8 Tests obligatoires

- Suite complète C-2 (bit-identique) sur ≥ 10 projets de référence.
- Aucun warning `unused` ni `dead_code` après cleanup.

### 7.9 Validation perf RT

- Vérifier que la suppression du fallback n'a pas changé le coût `update_effect_param` côté cmd.

### 7.10 Dette restante

- Sample-accurate modulation (BP-7) — Vague 7 target, différé sine die.
- AU / WASM plugins formats — Vague 7 target.
- Oversampling sélectable — Vague 7 target.

---

## Dépendances entre phases (graphe)

```
P1 (fondations safe)
 ├──► P2 (PluginInstance Arc)
 │     ├──► P3 (param SPSC + ParameterBlock)
 │     │     ├──► P4 (DSP/UI strict + state save/load)
 │     │     │     └──► P5e (modulators étendus + automation)
 │     │     └──► P5c (MIDI ports + plugin instruments)
 │     │           └──► P5d (capabilities → compiler)
 │     └──► P6 (sandbox) ← dépend P3 (params via shmem) + P4 (state via shmem)
 ├──► P5a (PortKind typage) ──► P5c
 └──► P5b (MIDI beats) ──► P5c
                              └──► P7 (cleanup)
```

**Règles** :

- P3 _exige_ P2 (sans `Arc<PluginInstance>`, pas d'adressage SPSC par device).
- P4 _exige_ P2 + P3 (state via instance partagée + persistance projet).
- P5c _exige_ P2 + P3 + P5a + P5b (MIDI ports typés + queue MIDI + capabilities lifecycle).
- P6 _exige_ P3 + P4 stables (bridge réutilise les mêmes abstractions).
- P7 _exige_ P3 → P6 complètes.

---

## Chemin critique (critical path)

**P1 → P2 → P3 → P5c → P5d** est le chemin critique pour « DAW utilisable avec plugins instruments ».

Effort minimum chemin critique : ~3 + 10 + 10 + 4 + 2 = **~29 j**.

Tout le reste peut être parallélisé partiellement (P4 en parallèle de P5a/P5b par exemple, par un dev distinct).

---

## Quick wins (livrables visibles utilisateur rapides)

| Quick win                    | Phase       | Effort       | Impact UX                                              |
| ---------------------------- | ----------- | ------------ | ------------------------------------------------------ |
| Latence plugin → PDC         | P1          | 0.5 j        | Linear-phase EQ, lookahead comp, reverb conv : alignés |
| catch_unwind autour process  | P1          | 0.5 j        | Moins de crashes (panics Rust capturées)               |
| Knob UI host → plugin DSP    | P3          | 3 j visibles | Spectaculaire : « ça marche enfin »                    |
| Save/load preset plugin      | P4          | 2 j visibles | Projet réutilisable                                    |
| MIDI vers plugin instrument  | P5c         | 4 j visibles | Vital/Diva jouables                                    |
| Bypass plugin sans recompile | P3 (option) | 0.5 j        | A/B test rapide                                        |
| Eq freq sans recompile       | P3          | 1 j          | Plus de glitch à l'édition                             |

**Priorité quick-wins** : P1 (latence + catch*unwind) en semaine 1, puis P3 \_post* P2 pour le coup d'éclat « knob → son ».

---

## Zones à risque élevé RT (focus d'attention)

| Zone                                 | Phase | Mitigation                                                                                        |
| ------------------------------------ | ----- | ------------------------------------------------------------------------------------------------- |
| `IParameterChanges` pré-allouée VST3 | P3    | Implémentation Rust custom avec slots fixes (pas d'alloc) ; revue audit dédiée                    |
| `clap_input_events` slab préalloué   | P3    | Idem ; vérifier qu'aucun `host_callback` n'alloue côté CLAP                                       |
| Drain MIDI dans `process`            | P5c   | Même pattern SPSC ; relire audit BP-2 + C-1                                                       |
| Bridge shmem audio thread wait       | P6    | Spin borné + timeout + telemetry ; benchmark sur 3 OS                                             |
| Drop d'`Arc<PluginInstance>`         | P2    | Drop ring SPSC (déjà en place pour scheduler), vérifier qu'il couvre aussi les instances libérées |
| AutomationEvaluator block-rate       | P5e   | Borner le nombre de lanes ; benchmark 100+ lanes                                                  |
| `MidiNoteSnapshot` bake beats        | P5b   | Test bit-identique BP-3 + CI dédiée                                                               |

**Convention défensive** : avant tout merge d'une phase touchant RT, un _audit RT pass_ (Grep `alloc`, `lock`, `Mutex`, `RwLock`, `Box::new`, `Vec::new`, `String::from` dans le sous-graphe de `process_block`) — partiellement automatisable.

---

## Estimation effort par phase

| Phase     | Min      | Max      | Médian   | Critical path ?             |
| --------- | -------- | -------- | -------- | --------------------------- |
| 1         | 2 j      | 4 j      | 3 j      | oui                         |
| 2         | 8 j      | 12 j     | 10 j     | oui                         |
| 3         | 7 j      | 12 j     | 10 j     | oui                         |
| 4         | 5 j      | 8 j      | 7 j      | non (mais bloque P5e fully) |
| 5         | 12 j     | 18 j     | 15 j     | oui (5a→5d sur critical)    |
| 6         | 15 j     | 25 j     | 20 j     | non                         |
| 7         | 4 j      | 7 j      | 5 j      | non                         |
| **Total** | **53 j** | **86 j** | **70 j** |                             |

Cohérent avec _« ~44 j avec bridge »_ de l'audit (légère réévaluation à la hausse pour P5 qui agrège 5 sous-phases).

---

## Stratégie de validation continue

### V1 — Harness offline bit-identique (prérequis Phase 1)

- 5–10 projets de référence rendus en `.wav` → SHA256 figé.
- CI rejoue à chaque PR. Diff → rouge.
- Couvre C-2.
- **Statut 2026-09-23 : livré** sous une forme plus légère, dans
  `src-tauri/src/audio_engine/render_harness.rs`. Il contient 4 projets
  synthétiques (mix statique, effets modulés, gain modulé, automation), sans
  fichier audio. Il vérifie l'indépendance à la taille de bloc (offline == live)
  et compare un hash FNV-1a figé, pas un `.wav` + SHA256. La CI le rejoue à
  chaque PR (Windows uniquement). Restent ouverts : vérification cross-OS,
  projets avec plugins réels.
- **Mise à jour 2026-10-26 : chemin d'export.** `render_project_in_blocks` (le
  chemin du harness, hashes figés) rend exactement la durée de la timeline, sans
  compensation. `render_project_offline` (l'export) retire la latence du master
  et garde la queue après le dernier clip. Les hashes n'ont pas changé ; le
  harness couvre l'export par `export_keeps_the_echoes_after_the_last_clip`,
  `export_tail_stops_at_the_cap_for_a_sound_that_never_ends` et
  `export_lines_up_with_the_timeline_despite_plugin_latency` (propriétés, pas de
  hash).

### V2 — Suite XRun longue durée (à partir de Phase 1)

- Projet « stress » (32 effets + 8 plugins + automation), buffer 256, 48 kHz, 30 min.
- Compteur XRun (CPAL `output_callback_info`) → seuil 0.
- Exécutée par CI nightly + avant chaque release.

### V3 — Benchmark `process_block` (à partir de Phase 1)

- Mesure CPU moyenne/p95/p99 par block sur projet ref.
- Régression > 3 % → alerte.
- Branche dédiée par phase pour comparer.

### V4 — Tests E2E plugin (à partir de Phase 2)

- Plugin mock Rust embarqué (audit B4) qui rapporte tous les events FFI reçus.
- Vérifie : params reçus, MIDI reçu, state sauvé/chargé, lifecycle propre.
- Couvre l'absence de CI capable de charger Vital.

### V5 — Smoke test plugins réels (manuel, releases)

- Liste : Vital, Surge XT, Diva, Pro-C 2, Serum (si licence), un linear-phase EQ.
- Checklist par release : load, save, jouer, automation, sidechain.

### V6 — Telemetry runtime (à partir de Phase 3)

- `xrun_count`, `compile_count`, `param_queue_drops`, `bridge_timeout_count` exposés.
- Dashboard interne (perso) pour détecter régressions silencieuses en usage réel.

### V7 — Migration projet (à partir de Phase 5)

- Test : charger un projet `master` pré-migration → comportement identique.
- Test : sauver après migration → projet rechargeable par `master` (rétro-compat asymétrique acceptable si nouveaux variants non utilisés).

---

## Synthèse exécutive

Sept phases, un seul moteur, aucun fork. Le chemin critique (29 j) couvre les P0 audit sauf sandbox. P1–P3 livrent en ~23 j l'expérience « plugin réellement contrôlable », qui est la valeur utilisateur la plus forte. P5 est la phase à plus haut risque conceptuel (PortKind + MIDI + Automation), à découper strictement. P6 (sandbox) est différable sans bloquer aucune autre phase et reste opt-in. P7 nettoie une fois la production validée.

Toutes les évolutions sont **additives** au niveau enum (`NodeKind`, `ModulatorParams`, `EffectParams`, `PortKind` default `Audio`) et **dual-path** au niveau update (T4 target §5.2). Bit-identique offline (C-2) garanti à chaque étape par V1.
