# Critical Issues — Echoes Studio (état au 2026-05-31)

> Analyse exhaustive des problèmes du **système actuel**, organisée par catégorie.
> Source : audits [`audio-engine-audit-2026-05-29.md`](../audits/audio-engine-audit-2026-05-29.md), [`plugin-system-audit-2026-05-31.md`](../audits/plugin-system-audit-2026-05-31.md), [`plugin-gui-audit-2026-05-31.md`](../audits/plugin-gui-audit-2026-05-31.md).
> Aucune solution n'est proposée ici. Voir [`current_engine_diagram.md`](current_engine_diagram.md) pour la structure correspondante.

Légende sévérité :
- **critical** — bloque un usage production, risque de crash / corruption / xrun systématique
- **medium** — bloque l'adoption pro, fonctionnalité standard manquante ou comportement incorrect audible
- **low** — qualité, hygiène, hors chemin critique

Statut : seuls les problèmes **encore présents dans le code** sont listés. Les références (C1, P0-1, A2, …) renvoient aux audits sources.

---

## 1. Audio Thread Safety

### 1.1 Pas de détection XRun ni de télémétrie sous-jacente
- **Référence audit** : M4
- **Description** : `OutputCallbackInfo` fourni par CPAL au callback audio est ignoré. Aucun comptage des underruns, aucune surface UI / log. Aucune télémétrie de latence callback ou de durée `process_block`.
- **Impact réel** : impossibilité de diagnostiquer un xrun en production. L'utilisateur entend un clic mais aucune information ne remonte. Aucun gating possible (ex : « passer en buffer plus grand si > N xruns / minute »).
- **Sévérité** : medium
- **Cause probable** : feature non implémentée — la prise en compte de `OutputCallbackInfo` n'a jamais été câblée au PositionReporter / aux events gateway.

### 1.2 `unsafe impl Send` du scheduler — justification incorrecte
- **Référence audit** : M3
- **Description** : Le doc-comment associé à `unsafe impl Send` sur `GraphScheduler` ([scheduler.rs:91](../src-tauri/src/audio_graph/scheduler.rs#L91)) prétend que « le scheduler n'est pas envoyé entre threads ». Or il est transféré du thread commande vers le thread audio à chaque swap de snapshot.
- **Impact réel** : trompe le futur lecteur sur les invariants de sûreté ; risque de raisonnement faux lors d'une modification ultérieure.
- **Sévérité** : low (documentation)
- **Cause probable** : commentaire écrit avant la mise en place du swap atomique de snapshot, jamais mis à jour.

### 1.3 Lock du moteur pendant tout le compile (reliquat d'I11)
- **Référence audit** : I11 (reste hors scope)
- **Description** : Le deadlock AB-BA entre `recompile_if_needed` et `render_project_to_stereo` a été résolu (ordre canonique project → engine). Mais la **durée de détention** du lock engine pendant `GraphCompiler::compile` reste élevée : tant que le lock est tenu, aucune autre commande engine (transport, levels, snapshots) ne peut être servie.
- **Impact réel** : sur un gros projet (~50 nœuds, plusieurs plugins), une recompile (UPDATE_EFFECT, par exemple) peut bloquer l'UI pendant 50–200 ms. Couplé à 1.4, ce blocage est déclenché trop fréquemment.
- **Sévérité** : medium
- **Cause probable** : `PluginHost` vit dans l'`AudioEngine` ; compiler hors-lock demanderait `Arc<Mutex<PluginHost>>` (refactor reporté).

### 1.4 `UPDATE_EFFECT` continue à recompiler le graphe complet
- **Référence audit** : C3 (reste hors scope)
- **Description** : Le path hot des paramètres internes a été déporté sur `LiveParams` pour volume / pan / mute / solo / mod rate / mod depth / soft clip. Mais **toute** modification d'un paramètre d'effet (Eq freq, Compressor threshold, Reverb wet, Delay feedback, et **tous** les params de plugin externe) continue de passer par `recompile_if_needed`.
- **Impact réel** : tourner un knob EQ pendant la lecture déclenche une recompile complète (rebuild du graphe + `prepare()` sur chaque node, donc réallocation des comb/allpass de Reverb, recalcul des coefs EQ, reset enveloppe Compressor). Couplé à 1.3 cela peut produire des xruns visibles. Queues de reverb/delay cassées à chaque tour de bouton.
- **Sévérité** : critical (chemin standard utilisateur)
- **Cause probable** : il n'existe pas encore d'abstraction `ParameterBlock` par effet ; chaque effet aurait besoin de ses propres cellules atomiques typées, non conçues.

### 1.5 `catch_unwind` autour de `process` n'attrape pas les SEH C++
- **Référence audit** : P0-5 (court terme appliqué, reste : R1 GUI/plugin)
- **Description** : Le scheduler enveloppe `processors[i].process(ctx)` dans `catch_unwind`, ce qui capte les `panic!` Rust. Sur Windows, les access violations (`STATUS_ACCESS_VIOLATION`), divisions par zéro, exceptions C++ levées dans le code natif d'un plugin ne sont **pas** captées par `catch_unwind`.
- **Impact réel** : un plugin tiers qui crashe en C++ (Vital, Diva sur certains presets, plugins 32-bit incompatibles) fait tomber tout le DAW. Perte de la session non sauvegardée.
- **Sévérité** : critical
- **Cause probable** : seul un `__try/__except` Win32 ou un bridge IPC out-of-process peut isoler ces crashs — non implémenté.

### 1.6 Pas de watchdog sur la durée de `process` d'un plugin
- **Référence audit** : P0-5
- **Description** : Aucune limite de temps n'est imposée à un appel `processor.process`. Un plugin qui boucle (deadlock interne, calcul exponentiel, busy-wait sur une primitive mal codée) bloque le thread audio.
- **Impact réel** : underrun continu, audio gelé tant que le plugin ne rend pas la main. L'utilisateur doit tuer l'application.
- **Sévérité** : critical
- **Cause probable** : architecture in-process — pas de moyen non destructif d'interrompre le thread audio.

### 1.7 Pas de priorité RT sur les workers parallèles
- **Référence audit** : R1 (reste hors scope)
- **Description** : Les threads du `WorkerPool` héritent de la priorité du process. Aucun `SetThreadPriority(THREAD_PRIORITY_TIME_CRITICAL)` (Windows) ni `pthread_setschedparam(SCHED_FIFO)` (Linux).
- **Impact réel** : sous charge système (compilation, antivirus, etc.) un worker peut être préempté pendant le block, causant un xrun même quand le calcul DSP global tient dans le budget temps.
- **Sévérité** : medium (limité tant que `ECHOES_AUDIO_WORKERS=0` par défaut)
- **Cause probable** : nécessite gestion par-OS des permissions (Linux : `rtprio` rlimit, capability cap_sys_nice) — repoussé pour PR dédiée.

---

## 2. Architecture DSP

### 2.1 `UPDATE_TEMPO` ne recompile pas → désync MIDI silencieuse
- **Référence audit** : C6
- **Description** : Les `MidiNoteSnapshot` sont bakés en **samples absolus** au moment de la compilation, avec le BPM courant. Le handler `UPDATE_TEMPO` n'appelle pas `recompile_if_needed`. Changer le BPM après avoir placé des notes les laisse jouer à l'ancien tempo jusqu'à ce qu'une autre commande force la recompile.
- **Impact réel** : changer le tempo désaligne silencieusement les pistes MIDI vs les pistes audio. Difficile à diagnostiquer ; l'utilisateur soupçonne d'abord un bug DSP.
- **Sévérité** : critical (silencieux, déforme la grille)
- **Cause probable** : choix de stocker les snapshots en samples plutôt qu'en beats ; pas de path explicite tempo → MIDI dans le compileur.

### 2.2 Ports non typés (audio / MIDI / modulation indistingués)
- **Référence audit** : P1-5
- **Description** : `AudioNode` expose `num_input_ports() / num_output_ports() -> usize`. Tous les ports portent le même type ; le compileur ne peut pas valider qu'une connexion respecte la nature des extrémités.
- **Impact réel** : routing MIDI vers un node audio possible en principe (rejeté par convention plutôt qu'au compile). La modulation est routée via un vecteur parallèle `ModulationMap`, ce qui empêche un routing UI uniforme. Pas d'arête MIDI dans le graphe.
- **Sévérité** : medium (limite l'extension, pas de bug actif)
- **Cause probable** : design initial minimal ; `PortKind` n'a jamais été introduit.

### 2.3 MIDI cantonné à `InstrumentNode` → pas de routing MIDI réel
- **Référence audit** : P0-2 (volet MIDI)
- **Description** : Les types MIDI (`MidiEventList`, `MidiNoteSnapshot`) sont consommés exclusivement par `InstrumentNode`. Il n'existe aucune arête MIDI dans le DAG. La couche MIDI ne quitte jamais le node.
- **Impact réel** : impossible de router des notes d'un clip vers un plugin instrument, d'envoyer du MIDI vers un Note FX, de chaîner instrument → effet sidechain MIDI-triggered.
- **Sévérité** : critical (fonctionnalité fondamentale d'un DAW)
- **Cause probable** : `MidiEventList` traité comme détail d'implémentation interne à `InstrumentNode` plutôt que comme flux de premier ordre.

### 2.4 Routing implicite des sends (pas de Returns câblés)
- **Référence audit** : M7 (corrigé port count) — reste : routage Return
- **Description** : `SendNode` expose 1 in / 1 out et écrit un tap pass-through à gain send. Mais il n'existe pas de `ReturnNode` ni de bus auxiliaire. Le tap n'est routé nulle part.
- **Impact réel** : envois auxiliaires (reverb send, delay send classiques d'un mixer) inutilisables.
- **Sévérité** : medium
- **Cause probable** : nœud créé mais sans contrepartie côté topologie de projet.

### 2.5 Pas de containers (Chain, Layer, Split, Multiband)
- **Référence audit** : P1-6
- **Description** : `NodeKind` n'a aucun variant `Container`. Pas de regroupement, pas de bypass collectif, pas de routing parallèle (layer) ou de split (multibande).
- **Impact réel** : impossible de construire des chaînes typées Bitwig / Ableton (parallel compression in-the-box, layered synths, splits multibande). L'utilisateur duplique manuellement.
- **Sévérité** : medium
- **Cause probable** : modèle initial « track linéaire » ; le DAG ne représente pas les regroupements logiques.

### 2.6 Pas de Note FX
- **Référence audit** : P1-6
- **Description** : Aucun node ne transforme un flux MIDI avant l'instrument (transpose, chord, arpeggiator, scale snap, randomize velocity, etc.). Conséquence directe de 2.3.
- **Impact réel** : pas de workflow MIDI standard.
- **Sévérité** : medium
- **Cause probable** : la chaîne MIDI n'est pas un graphe.

### 2.7 Pas de modulator global / projet
- **Référence audit** : P2-3
- **Description** : `AppTrack.modulators` — les modulators sont stockés au niveau track uniquement. Pas de modulator au niveau master, return bus, ou projet.
- **Impact réel** : impossible de moduler plusieurs tracks depuis une source commune (sidechain global, macro projet, LFO globale).
- **Sévérité** : low
- **Cause probable** : structure de données posée track-centric.

### 2.8 Limite implicite sur le scaling : recompiles à chaque changement d'effet
- **Référence audit** : C3 (reliquat) + I11 (reliquat)
- **Description** : Combinaison de 1.3 + 1.4. À mesure que les projets grossissent, chaque interaction utilisateur sur un paramètre d'effet pèse de plus en plus.
- **Impact réel** : projets de >30 nodes / 5 plugins deviennent désagréables à éditer en lecture.
- **Sévérité** : medium
- **Cause probable** : pas d'invariant « param hot ≠ structure » sur les effets.

### 2.9 Resampling toujours linéaire dans `TrackNode` (post-I3)
- **Référence audit** : I3 (corrigé partiel)
- **Description** : Le nearest-neighbour a été remplacé par une interpolation linéaire entre `samples[idx0]` et `samples[idx0+1]`. Ce n'est pas un resampler polyphase ; un projet 44.1 → 48 kHz garde un aliasing résiduel mesurable.
- **Impact réel** : différence audible vs un DAW broadcast sur du matériel haute fréquence (cymbales, hi-hat). Acceptable pour la majorité des productions, insuffisant pour mastering.
- **Sévérité** : low
- **Cause probable** : interpolation linéaire choisie comme compromis simple ; polyphase non implémenté.

---

## 3. Plugin System

### 3.1 Aucun transport de paramètres UI → plugin DSP
- **Référence audit** : P0-1
- **Description** : `PluginHost::set_parameter_normalised` écrit dans un blob JSON (`DeviceState::Json`) mais n'a aucune référence vers l'instance audio. Les wrappers passent `input_param_changes = null` (VST3) et `input_events = null` (CLAP) à chaque `process`.
- **Impact réel** : tourner un knob plugin *dans Echoes* ne change rien au son. L'automation, la modulation, le MIDI learn, le preset morph — rien ne fonctionne sur les plugins externes.
- **Sévérité** : critical
- **Cause probable** : pas de file SPSC `ParameterChange` entre command thread et audio thread ; `PluginHost` et instance plugin ne partagent pas de référence.

### 3.2 Aucun support des plugins instruments (VST3i / CLAPi)
- **Référence audit** : P0-2
- **Description** : `AudioGraph::from_project` route les tracks MIDI vers `NodeKind::Instrument`, qui est compilé en `InstrumentNode { inner: Box::new(PolySynth::new()) }` (codé en dur). Le path `NodeKind::Plugin` comme **source MIDI** n'existe pas.
- **Impact réel** : Vital, Surge XT, Diva, Serum chargeables uniquement comme insert FX. Impossible d'en faire l'instrument d'une track MIDI. Un DAW sans VSTi n'est pas un DAW.
- **Sévérité** : critical
- **Cause probable** : `InstrumentNode` traite l'inner comme un détail privé ; aucune intégration entre `PluginHost::instantiate` et la décision Instrument vs Track.

### 3.3 Capacités plugin câblées en dur, jamais introspectées
- **Référence audit** : P0-3
- **Description** : `VstWrapperNode::new` et `ClapWrapperNode::new` initialisent `DeviceInfo { num_inputs: 2, num_outputs: 2, has_midi_input: false, has_midi_output: false }`. `GraphCompiler::num_inputs_for_kind(NodeKind::Plugin)` renvoie `2` en dur. Aucun appel à `IComponent::getBusInfo` (VST3) ni `clap.audio-ports` (CLAP).
- **Impact réel** :
  - Plugin mono : on alloue un canal de trop, silence injecté dans le 2ᵉ canal.
  - Plugin surround (Auro-3D, Atmos) : 8+ canaux ignorés.
  - Plugin sidechain : bus aux jamais câblé.
  - Plugin multi-out (Kontakt, Battery) : on ne lit qu'une sortie.
- **Sévérité** : critical
- **Cause probable** : abstraction `PluginCapabilities` introduite (Phase 1 audit plugin) mais le compileur ne la consulte pas encore.

### 3.4 `ParameterMap` jamais peuplée
- **Référence audit** : P1-1
- **Description** : Wrappers VST3/CLAP initialisent `params: ParameterMap::new()`. Aucune énumération via `IEditController::getParameterCount/getParameterInfo` ou `clap.params.count/get_info`.
- **Impact réel** : l'UI Echoes ne peut pas afficher de knobs natifs pour un plugin externe. Pas de MIDI learn, pas d'automation, pas de modulation interne ciblant un param plugin.
- **Sévérité** : critical (corollaire de 3.1)
- **Cause probable** : énumération non implémentée dans les wrappers.

### 3.5 État plugin jamais sérialisé (save/load cassés)
- **Référence audit** : P0-4
- **Description** : `save_state(&self) -> DeviceState::empty(…)` et `load_state(&mut self, _state)` sont des no-op dans les deux wrappers. Aucun appel `IComponent::getState/setState` (VST3, IBStream) ni `clap_plugin_state.save/load`.
- **Impact réel** : à chaque save de projet, l'état des plugins externes est perdu. Au reload, ils repartent au preset par défaut. Idem undo/redo : tout rebuild via le store remet les plugins à zéro.
- **Sévérité** : critical (projet inutilisable en production)
- **Cause probable** : implémentation FFI non écrite.

### 3.6 Aucune latence rapportée par les plugins (PDC = 0)
- **Référence audit** : P0-6
- **Description** : `AudioNode::latency_samples() -> u32` retourne `0` par défaut. Les wrappers VST3/CLAP n'overrident pas (n'appellent ni `IAudioProcessor::getLatencySamples` ni `clap_plugin.get_latency`).
- **Impact réel** : EQ linéaire phase (~1024 samples), convolveur reverb (~4–8 ms), compresseur lookahead — tous **désalignent leur track de la grille** sans compensation. Inutilisable en mixing pro.
- **Sévérité** : critical
- **Cause probable** : implémentation des overrides non faite (note : audit plugin-system mentionne « surface dans cette session », mais l'audit audio-engine note `latency_samples()` toujours 0 pour wrappers — état réel à vérifier en code ; au minimum la notification `restart` VST3 / `clap_host_latency.changed` n'est pas câblée).

### 3.7 Lifecycle bicéphale : wrapper vs PluginHost
- **Référence audit** : P1-2
- **Description** : `PluginHost::instantiate` insère un `DeviceRecord { lifecycle: LifecycleState::Running }` — saut direct à `Running`, sans passer par `Created → Loaded → Initialized`. Parallèlement, `VstWrapperNode::prepare` met `self.lifecycle = LifecycleState::Running`. Deux sources de vérité jamais synchronisées.
- **Impact réel** : `suspend` / `resume` côté host ne désactive pas le plugin réel. La machine à états dans `lifecycle.rs` est morte (transitions jamais déclenchées). Un futur node « monitor » dépendant du lifecycle ne fonctionnerait pas.
- **Sévérité** : medium
- **Cause probable** : le host n'a aucune référence (Arc/Weak) vers l'instance — il ne peut pas appeler les méthodes du wrapper.

### 3.8 `PluginHost::instantiate` ne garde aucune référence vers l'instance
- **Référence audit** : P1-7
- **Description** : Le host crée la `Box<dyn AudioNode>` et la retourne au compileur (qui la place dans `CompiledGraph.processors`). Il ne conserve qu'un `DeviceRecord` opaque indexé par `plugin_id`.
- **Impact réel** : `set_parameter_normalised` ne sait pas vers quelle instance envoyer (cause racine de 3.1). `save_device_state` ne peut pas appeler `device.save_state()` (cause racine de 3.5).
- **Sévérité** : critical (architecture)
- **Cause probable** : pas de type intermédiaire `PluginInstance` partagé via `Arc`.

### 3.9 `LiveParams` ne couvre pas les paramètres plugin
- **Référence audit** : P1-8
- **Description** : L'infrastructure `LiveParams` (atomics partagés) existe pour volume/pan/mute/solo/mod rate/depth/soft clip mais n'est pas étendue aux paramètres plugin. Impossible de moduler un param plugin à 60 Hz sans rebuilder.
- **Impact réel** : conséquence de 3.1 et 3.4 — les plugins externes sont muets côté UI.
- **Sévérité** : critical (lié 3.1)
- **Cause probable** : les paramètres plugin sont découverts au `prepare` (pas connus à l'avance) — `LiveParams` n'a pas de chemin pour ça.

### 3.10 `PluginInfo` ne porte pas les capacités
- **Référence audit** : P1-9
- **Description** : `PluginInfo { id, name, vendor, format, categories }` — aucune capacité (instrument/effet, mono/stéréo, sidechain).
- **Impact réel** : l'UI ne sait pas classer ; le `PluginPicker` est générique alors qu'un browser pro distingue instrument browser et FX browser.
- **Sévérité** : medium
- **Cause probable** : pas de probe au scan.

### 3.11 Deux instances de plugin par effet (audio + GUI)
- **Référence audit** : A1 (GUI audit), CL3, CL5
- **Description** : `open_plugin_editor` crée une **nouvelle** `ClapPlugin` / `IComponent` dans une fenêtre Win32 séparée. Cette instance est distincte de celle utilisée par le thread audio.
- **Impact réel** :
  - Bouger un knob dans le GUI natif n'affecte pas le son.
  - Aucune sync de paramètres / preset / état.
  - Double allocation des ressources lourdes (wavetables, samples, IR).
  - Plugins à enregistrement global (MTS-ESP, iLok) : corruption ou refus de licence.
- **Sévérité** : critical (modèle non-spec pour VST3 et CLAP)
- **Cause probable** : modèle initial « instance audio + instance GUI séparées » ; refonte vers `PluginInstance` unique via `Arc` non faite.

### 3.12 Plugin GUI : pas de gestion DPI
- **Référence audit** : W3 (GUI audit)
- **Description** : Container Win32 créé sans `SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)`. `gui.set_scale` jamais appelé.
- **Impact réel** : sur écran 4K @ 200 % : fenêtre plugin minuscule, hit-testing décalé.
- **Sévérité** : medium
- **Cause probable** : feature non implémentée.

### 3.13 Plugin GUI : VST3 dual-component rejetés silencieusement
- **Référence audit** : V4 (GUI audit)
- **Description** : Modèle « single-component » exclusif. Plugins dual-component (Steinberg pro, Native Instruments) rejetés sans message explicite.
- **Impact réel** : compatibilité limitée. JUCE / open-source OK ; certains plugins commerciaux non.
- **Sévérité** : medium
- **Cause probable** : `getControllerClassId` + `createInstance` séparé non implémentés.

### 3.14 Plugin GUI : `Box<ClapHost>` non Pin
- **Référence audit** : CL5 (GUI audit)
- **Description** : Adresse heap stable par contrat de `Box`, OK en pratique, mais fragile si une refonte déplaçait la boîte.
- **Impact réel** : potentiel UB si déplacée — non observé.
- **Sévérité** : low
- **Cause probable** : `Pin<Box<ClapHost>>` non utilisé.

### 3.15 Types `DeviceFormat` rigides — pas de notion de capabilities runtime
- **Référence audit** : grille anti-patterns #7 / #8
- **Description** : `DeviceFormat` est un enum fermé (VST3, CLAP, AU, WASM, internal). Aucune notion de capacités runtime exposée à l'UI ou au compileur (instrument, sidechain, multi-out, etc.).
- **Impact réel** : impossible de raisonner sur ce que peut un plugin sans le charger.
- **Sévérité** : medium (rejoint 3.3 et 3.10)
- **Cause probable** : design initial enum-only.

### 3.16 Formats AU et WASM stubés
- **Référence audit** : cartographie audio-engine
- **Description** : `plugin_formats/au/` et `plugin_formats/wasm/` existent comme stubs ; `PluginHost::instantiate` retourne `PassThroughDevice` pour ces formats.
- **Impact réel** : AU (macOS) inutilisable. WASM plugin sandbox inutilisable.
- **Sévérité** : medium (selon roadmap plateforme)
- **Cause probable** : implémentation non commencée.

---

## 4. Performance

### 4.1 Worker pool : pas de mesure de coût par node
- **Référence audit** : P2-5
- **Description** : `WorkerPool` dispatch tous les steps d'un niveau sans information de coût. Stratégie work-stealing sur `AtomicUsize`, sans tri par durée moyenne.
- **Impact réel** : pour un graphe inhomogène (1 Diva + 30 GainNode), Diva monopolise un worker, les autres terminent en µs. Faible utilisation des cores.
- **Sévérité** : medium (impact direct sur CPU headroom)
- **Cause probable** : EWMA par step non implémenté.

### 4.2 Worker pool : par-scheduler (respawn à chaque recompile structurelle)
- **Référence audit** : R1 (trade-off documenté)
- **Description** : Le pool vit dans le `GraphScheduler`. Chaque recompile structurelle respawn N threads.
- **Impact réel** : tant que les recompiles sont rares (post-C3 pour params internes) c'est acceptable. Mais `UPDATE_EFFECT` continue à recompiler (cf. 1.4) → respawn fréquent dans certains scénarios d'édition de plugin.
- **Sévérité** : medium
- **Cause probable** : pool non partagé via `Arc<WorkerPool>` sur `AudioEngine`.

### 4.3 Worker pool désactivé par défaut
- **Référence audit** : R1
- **Description** : `ECHOES_AUDIO_WORKERS=0` par défaut → exécution séquentielle sur l'audio thread. Aucun bénéfice multi-core out-of-the-box.
- **Impact réel** : la parallélisation est disponible mais inactive pour l'utilisateur lambda.
- **Sévérité** : low (choix conservateur, lié à 1.7 absence de priorité RT)
- **Cause probable** : décision pragmatique (bit-identique vs ancien scheduler), pas de benchmark de référence pour activer.

### 4.4 Oversampling à facteur fixe 2× sur EQ, Compressor, Master
- **Référence audit** : M9 (reliquat)
- **Description** : `Oversample2x` (linéaire 2× + one-pole anti-imaging) est appliqué sur EQ, Compressor, Master soft-clip. Pas de 4× / 8× sélectionnable.
- **Impact réel** : aliasing résiduel sur transitoires extrêmes (broadcast / mastering). Suffisant pour usage standard.
- **Sévérité** : low
- **Cause probable** : compromis qualité / coût ; détection de crête du compresseur reste block-rate, ce qui limite le bénéfice d'un facteur plus élevé.

### 4.5 Reporter de position lisse mais throttlé à 30 Hz dur
- **Référence audit** : M5 (corrigé) — note de limite
- **Description** : Payloads typés + maps réutilisées → plus d'allocation par tick. Mais le taux est statique 30 Hz, non adaptable au taux d'affichage / batterie / mode performance.
- **Impact réel** : sur un écran 144 Hz, métering visiblement saccadé. Sur batterie, conso fixe.
- **Sévérité** : low
- **Cause probable** : taux hardcodé.

### 4.6 Recompile sur changements d'effet (corollaire perf de 1.4)
- **Référence audit** : C3 (reliquat)
- **Description** : Voir 1.4. Côté performance, chaque recompile = rebuild du graphe, `prepare()` sur tous les nodes (réalloc des comb/allpass de Reverb, recalcul coefs EQ, reset enveloppe Compressor) + `Arc::clone` sur les samples (depuis I1).
- **Impact réel** : spikes CPU + risque xrun à chaque tour de bouton sur un paramètre d'effet.
- **Sévérité** : critical (UX directe)
- **Cause probable** : voir 1.4.

---

## 5. Modulation / Automation

### 5.1 Modulation : uniquement LFO
- **Référence audit** : P1-4
- **Description** : `ModulatorParams` enum n'a qu'un seul variant : `Lfo(LfoParams)`. `ModulatorKind` énumère `Lfo | Envelope | Macro | AudioRate`, mais Envelope / Macro / AudioRate sont stubés (silence dans le compileur).
- **Impact réel** : pas d'ADSR routable, pas de macros (1 knob → N paramètres), pas de sidechain follower, pas d'aléatoire (S&H, smooth random). Toute la sophistication « Bitwig modulators » manque.
- **Sévérité** : critical (fonctionnalité différenciante absente)
- **Cause probable** : implémentation non faite.

### 5.2 Modulation block-rate uniquement (pas sample-accurate)
- **Référence audit** : R2 ; I4 reste
- **Description** : Le scheduler échantillonne la modulation 1× par block (~10 ms à 48 kHz / 512). Pas de programme de modulation par-sample.
- **Impact réel** : stepping audible sur modulations rapides (FM, audio-rate, ramps courtes). Pas d'audio-rate modulation possible.
- **Sévérité** : medium
- **Cause probable** : choix de coût ; refactor lourd reporté (R2).

### 5.3 Modulation : aucun alignement PDC sur la lecture du source
- **Référence audit** : I4 (reste documenté)
- **Description** : Le scheduler lit le buffer du source step au début du block sans appliquer de retard PDC sur le chemin de modulation.
- **Impact réel** : si une chaîne audio fortement compensée (linear phase EQ, lookahead) est modulée par un LFO « avant » dans le graphe, le LFO est temporellement décalé vs l'audio qu'il module.
- **Sévérité** : low (acceptable tant que la modulation reste block-rate ; rejoint 5.2)
- **Cause probable** : concession explicite Phase 3 / R2.

### 5.4 Couplage modulation : routes ne ciblent que les paramètres internes
- **Référence audit** : P1-4 implicite
- **Description** : `ModulationTargetRef` connaît `TrackGain { track_id }` et `EffectParam { track_id, effect_id, param }`. Aucun variant ne cible un paramètre de plugin externe ni un paramètre global / master.
- **Impact réel** : un LFO ne peut pas moduler la cutoff de Vital. Le système de modulation est cloisonné aux nodes internes.
- **Sévérité** : critical (renforce 3.1 et 3.4)
- **Cause probable** : enum fermé, pas de notion de `ParamKey` cross-format.

### 5.5 Pas de phase reset des LFO au transport restart
- **Référence audit** : P2-6
- **Description** : `LfoNode` n'est pas resetté quand on stoppe/repart. Pour production musicale, on veut souvent un LFO sync-tempo qui reprenne au début à chaque play.
- **Impact réel** : LFO « libre » → la phase au play est aléatoire selon le moment de l'arrêt. Pas musical.
- **Sévérité** : medium
- **Cause probable** : la méthode `reset` du trait existe mais n'est pas appelée par le scheduler au transport restart.

### 5.6 LFO : pas de tempo-sync
- **Référence audit** : P2-7
- **Description** : `LfoNode.rate_hz` — rate libre uniquement. Pas de modes « 1/4 », « 1/8 dotted », « 1/16 triplet ».
- **Impact réel** : standard DAW absent. Workflow musical réduit.
- **Sévérité** : medium
- **Cause probable** : implémentation non faite.

### 5.7 Pas d'automation (lanes temporelles)
- **Référence audit** : non explicitement listé (inexistence)
- **Description** : Aucune notion de « lane d'automation » par paramètre, avec points temporels et courbes interpolées.
- **Impact réel** : pas d'automatisation au sens DAW classique. Seul le système modulation (LFO uniquement) anime les paramètres dans le temps.
- **Sévérité** : critical (manque structurel)
- **Cause probable** : feature non commencée.

---

## 6. UX vs Engine mismatch

### 6.1 Abstraction « track » UX vs réalité « DAG » engine
- **Référence audit** : verdict général plugin-system (anti-patterns #1, #10, #11) + P1-6
- **Description** : Le moteur **est** un DAG (Kahn topo-sort, niveaux parallèles, routing arbitraire au niveau graphe), mais l'UI expose un modèle « track linéaire avec chaîne d'effets ». Pas de containers (Chain / Layer / Split), pas de routing UI vers un autre track, pas de bus auxiliaires visibles, pas de visualisation de la topologie.
- **Impact réel** : l'utilisateur ne peut accéder qu'à un sous-ensemble strict de ce que le moteur peut faire. Les capacités latentes (routing arbitraire, parallel chains) sont invisibles.
- **Sévérité** : medium
- **Cause probable** : UI développée en avance sur la maturité plugin / containers ; pas de surface graphe.

### 6.2 UI ne distingue pas instrument / effet / mono / stéréo / sidechain
- **Référence audit** : P1-9 + grille #8
- **Description** : `PluginPicker` montre tous les plugins indistinctement. Pas de filtre instrument browser vs FX browser. Pas d'indication sidechain.
- **Impact réel** : navigation lente, sélection d'un instrument sur une track effets (et inversement) possible.
- **Sévérité** : medium
- **Cause probable** : cf. 3.10 (capabilities pas remontées à l'UI).

### 6.3 Knob plugin dans le GUI natif ≠ knob plugin dans Echoes
- **Référence audit** : A1 (GUI audit)
- **Description** : Deux instances séparées (cf. 3.11). Aucune des deux n'est routée vers le son :
  - Knob dans le GUI natif → bouge l'**instance GUI** (silencieuse).
  - Knob dans Echoes → aucun n'existe car `ParameterMap` vide (cf. 3.4).
- **Impact réel** : aucun knob n'a d'effet audible sur un plugin externe. Inutilisable.
- **Sévérité** : critical
- **Cause probable** : combinaison de 3.1, 3.4, 3.11.

### 6.4 Pas de bypass d'effet / plugin sans recompile
- **Référence audit** : P2-1
- **Description** : `effect.enabled = false` est résolu **au compile** (le node n'est pas créé). Toggler bypass = recompile.
- **Impact réel** : A/B testing rapide impossible (pop / coupure / xrun probable à chaque toggle).
- **Sévérité** : medium
- **Cause probable** : pas de `BypassNode` wrap autour des effets.

### 6.5 Pas de preset morphing / groupes A/B
- **Référence audit** : P2-2
- **Description** : `DeviceState` opaque (blob) ⇒ pas d'interpolation possible.
- **Impact réel** : pas de morph entre presets, pas d'A/B rapide.
- **Sévérité** : low
- **Cause probable** : state opaque par design ; interpolation hors scope court terme.

### 6.6 Pas de surface UI pour l'automation
- **Référence audit** : implicite (5.7)
- **Description** : Aucun mécanisme UI pour automatiser un paramètre dans le temps.
- **Impact réel** : workflow DAW standard absent.
- **Sévérité** : critical (lié 5.7)
- **Cause probable** : feature non commencée.

---

## 7. Latency / PDC

### 7.1 PDC interne ✅ implémenté, mais ignore les plugins
- **Référence audit** : I4 (corrigé interne) + P0-6
- **Description** : Le PDC du graphe est implémenté (delay rings par port, `compute_latency_compensation`, walk topo). Mais aucun plugin externe ne déclare sa latence (`latency_samples()` reste à 0). Cf. 3.6.
- **Impact réel** : EQ linear-phase, lookahead, convolveur reverb — désalignés. Tout autour, le PDC interne est correct.
- **Sévérité** : critical (compensation effective trompeuse : ce qui est mesuré côté nodes internes ne reflète pas la vraie latence du graphe avec plugins)
- **Cause probable** : wrappers n'overridént pas `latency_samples`.

### 7.2 Pas de notification de changement de latence runtime
- **Référence audit** : plan migration P0-6 (point 6)
- **Description** : Même quand le wrapper rapportera la latence au `prepare`, il manque la prise en compte de `IComponentHandler::restartComponent(kLatencyChanged)` (VST3) et `clap_host_latency.changed` (CLAP). Ces signaux ne déclenchent rien aujourd'hui.
- **Impact réel** : un plugin qui change de latence en cours de session (Pro-Q lin-phase ↔ minimum-phase, oversampling toggle, etc.) ne déclenche pas de recompile PDC.
- **Sévérité** : medium
- **Cause probable** : hooks host non implémentés.

### 7.3 Modulation non alignée PDC
- **Référence audit** : I4 reste, cf. 5.3
- **Description** : Voir 5.3.
- **Sévérité** : low
- **Cause probable** : voir 5.3.

---

## 8. Crash safety

### 8.1 Aucune sandboxing — plugin in-process
- **Référence audit** : P0-5
- **Description** : Plugins chargés in-process via `LoadLibrary` (Windows) / `dlopen`. Aucune isolation. Un crash plugin = crash du DAW (perte de session).
- **Impact réel** : Vital, Diva, Serum — tous capables de crasher en certaines conditions. Avec l'archi actuelle : perte de la session non sauvegardée.
- **Sévérité** : critical
- **Cause probable** : pas de bridge IPC (modèle Bitwig « All Plug-ins in Bridge » / REAPER bridge / FL bridge). Module à part entière non développé.

### 8.2 `catch_unwind` Rust ne capte pas les SEH C++
- **Référence audit** : P0-5 court terme, R1
- **Description** : Voir 1.5. `catch_unwind` autour de `processors[i].process` capte les `panic!` Rust uniquement. Les SEH (access violation, divide by zero, exceptions C++ Windows) traversent.
- **Impact réel** : la défense actuelle ne couvre que les panics produits par du code Rust dans la chaîne (rare en plugin externe). Les vrais crashs plugin ne sont pas captés.
- **Sévérité** : critical
- **Cause probable** : pas de `__try/__except` Win32 ni d'isolation process.

### 8.3 Pas de marquage `Dead` d'un plugin défaillant
- **Référence audit** : P0-5 (court terme proposé)
- **Description** : Même si un crash était capté, il n'existe pas de mécanisme pour marquer un device comme « dead » et le bypasser au prochain block (lifecycle bicéphale, cf. 3.7).
- **Impact réel** : un plugin qui plante une fois replantera au prochain block ; pas de quarantaine automatique.
- **Sévérité** : critical
- **Cause probable** : design lifecycle incomplet.

### 8.4 Plugin failure propagation : tous les blocks suivants exposés
- **Référence audit** : P0-5 implicite
- **Description** : Aucun mécanisme de fail-fast → fail-safe au niveau scheduler. Si un plugin tombe, le scheduler tente de continuer (sauf si le crash est non récupérable, auquel cas il abat le process).
- **Impact réel** : comportement non déterministe entre « audio mute soudain » et « crash total ».
- **Sévérité** : critical
- **Cause probable** : architecture in-process + état du scheduler non resettable per-node post-crash.

### 8.5 Pas de test de robustesse / fuzz / golden master plugins
- **Référence audit** : P2-4
- **Description** : Aucun test en CI n'instancie un plugin réel. Le scan + instanciation peuvent régresser sans qu'on le voie.
- **Impact réel** : régressions FFI invisibles avant le retour utilisateur.
- **Sévérité** : medium
- **Cause probable** : pas de plugin de test (Rust minimal embarqué) ni de suite d'instanciation en CI.

### 8.6 `clap_entry.init()` — pattern fragile sans cache strict
- **Référence audit** : C1 (corrigé)
- **Description** : Cause directe du crash initial CLAP Vital : `init()` jamais appelé → vtable non patché → crash dans `create_plugin`. Le fix est un cache process-wide `OnceLock<Mutex<HashMap<PathBuf, Arc<ClapLibrary>>>>`.
- **Impact réel** : aujourd'hui résolu. Mais la double-init reste un risque si un futur code chargeait directement sans passer par le cache.
- **Sévérité** : low (corrigé, vigilance documentaire à maintenir)
- **Cause probable** : historique — commentaire d'origine inversait le raisonnement.

### 8.7 Types `audio_engine/snapshot.rs` morts
- **Référence audit** : M2
- **Description** : Le fichier [audio_engine/snapshot.rs](../src-tauri/src/audio_engine/snapshot.rs) contient des types legacy non utilisés depuis le passage à `RuntimeSnapshot { AtomicPtr<GraphScheduler> }`.
- **Impact réel** : confusion lors de la lecture du code, risque d'utilisation accidentelle.
- **Sévérité** : low (hygiène)
- **Cause probable** : nettoyage post-refactor non fait.

---

## Synthèse par sévérité

| Catégorie                       | Critical | Medium | Low | Total |
|---------------------------------|---------:|-------:|----:|------:|
| 1. Audio Thread Safety          | 3        | 3      | 1   | 7     |
| 2. Architecture DSP             | 3        | 5      | 1   | 9     |
| 3. Plugin System                | 9        | 6      | 1   | 16    |
| 4. Performance                  | 1        | 3      | 2   | 6     |
| 5. Modulation / Automation      | 3        | 3      | 1   | 7     |
| 6. UX vs Engine mismatch        | 2        | 3      | 1   | 6     |
| 7. Latency / PDC                | 1        | 1      | 1   | 3     |
| 8. Crash safety                 | 4        | 1      | 2   | 7     |
| **Total**                       | **26**   | **25** | **10** | **61** |

---

## Concentration des problèmes (lecture transverse)

Trois zones concentrent la majorité des problèmes critical :

1. **Couche plugin externe (§3 + §6.3 + §7.1 + §8.1–8.4)** — pas de transport paramètres, pas d'état, pas de capacités, pas de PDC, pas de sandbox, double instance. Aujourd'hui un plugin chargé est un boîtier opaque 2 in / 2 out qui consomme de l'audio sans interaction possible.

2. **Modulation / Automation (§2.3 + §5.1 + §5.4 + §5.7 + §6.6)** — un seul type de source (LFO), pas d'automation temporelle, modulation cantonnée aux paramètres internes, MIDI prisonnier d'`InstrumentNode`. Le moteur a l'infrastructure (`ModulationRoute`, `LiveParams`) mais la majorité des usages musicaux standards manquent.

3. **Edition temps réel des paramètres d'effet (§1.3 + §1.4 + §2.8 + §4.6)** — chaque tour de bouton sur un EQ / Compressor / Plugin déclenche une recompile complète + `prepare()` sur tous les nodes. Risque de xrun direct et UX dégradante sur projets moyens.

Les fondations (DAG, ExecutionPlan, snapshot atomique, drop ring SPSC, FTZ/DAZ, PDC interne, worker pool) sont saines. La dette est concentrée à la **frontière** moteur ↔ plugin et moteur ↔ UI.
