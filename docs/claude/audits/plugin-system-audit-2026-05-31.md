# Plugin System Architecture Audit — 2026-05-31

Auditeur : Claude (Opus 4.7)
Portée : couche d'hébergement des plugins (VST3, CLAP, devices internes), intégration au DSP graph, threading, paramétrage, modulation, sandboxing.
Hors portée : couche GUI native (voir [`plugin-gui-audit-2026-05-31.md`](plugin-gui-audit-2026-05-31.md), à lire en complément).

Légende : 🟢 OK — 🟡 Partiel — 🔴 Bloquant — ⚪ Non implémenté
Priorité : P0 (bloque sortie production) — P1 (bloque adoption sérieuse) — P2 (qualité / scalabilité)

---

## Verdict global

L'architecture **n'est pas un « track = liste linéaire de plugins »**. Le moteur est déjà :

- un vrai DAG (`AudioGraph` → `GraphCompiler` Kahn → `ExecutionPlan` avec niveaux parallèles),
- temps-réel strict (zéro mutex / zéro alloc dans `process`, snapshot atomique `AtomicPtr` + drop ring SPSC),
- DSP/UI séparés (IPC Tauri, atomics pour meters/transport, jamais `app.emit` depuis le thread audio),
- modulation traitée comme arête de graphe distincte (`ModulationRoute`), pas comme code interne au plugin,
- niveaux d'exécution calculés au compile pour le worker-pool parallèle.

Les **fondations sont saines et alignées avec un DAW moderne type Bitwig**. Ce que je vois en revanche, c'est une **couche plugin minimale** : les wrappers VST3/CLAP existent, mais ils ne *transportent* presque rien — pas de paramètres exposés, pas d'état, pas de MIDI, pas de capacités dynamiques, pas de sidechain, pas de latence rapportée. Le `Device` trait est défini mais la majorité des méthodes renvoient `Empty` ou `false`. Aujourd'hui un plugin chargé est essentiellement un boîtier opaque à 2 in / 2 out qui traite l'audio sans qu'on puisse interagir avec lui depuis l'UI ou le système de modulation.

**Verdict en une phrase :** moteur prêt pour la production, *interface plugin* prête pour la démo.

**Compte par priorité :** P0 = 6, P1 = 9, P2 = 7.

---

## Diagramme architecture actuelle

```
                     ┌─────────────────────────────────────────────┐
                     │              FRONTEND (Svelte)              │
                     │  EffectChain.svelte / PluginPicker.svelte   │
                     │  invoke('open_plugin_editor', { … })        │
                     └────────────────┬────────────────────────────┘
                                      │  Tauri IPC (JSON)
                                      ▼
  ┌──────────────────────────────────────────────────────────────────────┐
  │                     RUST MAIN / COMMAND THREAD                       │
  │                                                                      │
  │  engine_gateway::api        ─►  AppProject (mutation)                │
  │       │                          │                                   │
  │       │                          ▼                                   │
  │       │                   AudioGraph::from_project                   │
  │       │                          │                                   │
  │       │                          ▼                                   │
  │       │                   GraphCompiler::compile                     │
  │       │                          │  build_node(NodeKind::Plugin)     │
  │       │                          ▼                                   │
  │       │            ╔═════════════════════════╗                      │
  │       │            ║  PluginHost (singleton) ║                      │
  │       │            ║  ─ registry             ║                      │
  │       │            ║  ─ records              ║                      │
  │       │            ║  ─ instantiate ─────────╬──► Vst3Library.load  │
  │       │            ║                         ║   ClapLibrary.load   │
  │       │            ║                         ║   → Box<dyn AudioNode>
  │       │            ╚═════════════════════════╝                      │
  │       │                          │                                   │
  │       │                          ▼                                   │
  │       │           CompiledGraph { plan, processors, … }              │
  │       │                          │                                   │
  │       ▼                          ▼                                   │
  │  open_plugin_editor    RuntimeSnapshot::enqueue (AtomicPtr swap)     │
  │       │                          │                                   │
  │       │                          ▼                                   │
  │       │                ★★★★★★★★★★★★★★★★★★★★                          │
  │       │                ★ AUDIO THREAD (CPAL) ★                       │
  │       ▼                ★ GraphScheduler      ★                       │
  │  ─────────────         ★ ─ process_block     ★                       │
  │  ★ SECOND PLUGIN ★      ★ ─ worker pool      ★                       │
  │  ★ INSTANCE      ★      ★ ─ PDC + modulation ★                       │
  │  ★ (GUI-only)    ★      ★★★★★★★★★★★★★★★★★★★★                          │
  │  ─────────────                                                       │
  │  open_vst3_editor      ▲                                             │
  │  open_clap_editor      │ never reaches the audio plugin instance     │
  │  HWND container        │ (cf. A1 of the GUI audit)                   │
  └──────────────────────────────────────────────────────────────────────┘
```

**Trois constats lisibles sur ce schéma :**
1. La frontière audio est propre (snapshot atomique unique).
2. Le plugin host est *strictement command-thread* — bonne décision.
3. Mais : **le path "UI knob → plugin DSP" n'existe pas**. Les seuls flux UI → plugin sont (a) instanciation au compile et (b) ouverture du GUI natif dans un **deuxième** ClapPlugin/IComponent.

---

## PARTIE 1 — Anti-patterns recherchés

| # | Anti-pattern recherché                            | Verdict | Détail                                                                                     |
|---|---------------------------------------------------|---------|--------------------------------------------------------------------------------------------|
| 1 | track = liste linéaire de plugins                 | 🟢      | DAG complet avec routing arbitraire au niveau graphe (cf. `AudioGraph::from_project`)      |
| 2 | absence de DSP graph réel                         | 🟢      | `ExecutionPlan` + Kahn + niveaux parallèles                                                |
| 3 | couplage UI ↔ audio thread                        | 🟢      | IPC Tauri + atomics, aucun `emit` depuis l'audio thread                                    |
| 4 | mutex dans audio thread                           | 🟢      | `RuntimeSnapshot` = `AtomicPtr::swap`, drop ring SPSC                                      |
| 5 | allocations temps réel                            | 🟢      | Toutes les `Vec` pré-allouées dans `prepare`, scratch fixes                                |
| 6 | séparation audio / midi / modulation              | 🟡      | Modulation ✅ (graph edges), MIDI ✅ (msnap → block) mais **plugins ne reçoivent jamais de MIDI** |
| 7 | types de plugins rigides (instrument/effet)       | 🔴      | `DeviceFormat` enum suffit ; **aucune** notion de capabilities ; instrument plugins inutilisables |
| 8 | absence de capacités dynamiques                   | 🔴      | `DeviceInfo { num_inputs: 2, num_outputs: 2, has_midi_input: false, … }` câblé en dur      |
| 9 | absence de sandboxing                             | 🔴      | Plugin in-process. Un crash = crash du DAW. Pas même de `catch_unwind` autour de `process` |
|10 | absence de scheduling explicite                   | 🟢      | `ExecutionPlan.levels` (niveaux topo) + worker_pool                                        |
|11 | routing implicite fragile                         | 🟢      | `Edge { PortId → PortId }` typé                                                             |
|12 | paramètres non normalisés                         | 🟢      | `ParameterRange { Linear / Logarithmic / Decibels }` propre — mais **inutilisé pour les plugins externes** |
|13 | absence de système de ports                       | 🟡      | OK pour audio, **absent** pour MIDI in/out et CV/modulation in/out                          |
|14 | absence de support hybride MIDI/audio             | 🔴      | Pas d'instrument plugin ; le code force `NodeKind::Instrument` → `PolySynth` interne uniquement |
|15 | absence de modulation généralisée                 | 🟡      | Infrastructure 🟢, mais **uniquement LFO sinus/tri/saw/sq** ; pas d'envelope, macro, follower |
|16 | dépendance du DSP à la GUI                        | 🟢      | Aucune                                                                                       |
|17 | processing parallèle                              | 🟢      | `compute_execution_levels` + worker pool                                                    |
|18 | cycles dangereux                                  | 🟢      | Kahn rejette les cycles à la compile (`CompileError::CycleDetected`)                       |
|19 | gestion latence / PDC                             | 🟡      | PDC implémenté ✅ ; mais `latency_samples()` toujours `0` pour wrappers → **aucun plugin ne déclare sa latence** |
|20 | architecture impossible à scaler                  | 🟢      | Foundations 🟢. Limite actuelle = couche plugin minimale, pas l'architecture                |

---

## PARTIE 2 — Problèmes identifiés en détail

### 🔴 P0 — Critiques (bloquent un usage production)

#### P0-1 Aucun transport de paramètres UI → plugin DSP

**Code concerné** — [`plugin_formats/vst3/wrapper.rs:181-194`](../../src-tauri/src/plugin_formats/vst3/wrapper.rs#L181-L194), [`plugin_formats/clap/wrapper.rs:176-187`](../../src-tauri/src/plugin_formats/clap/wrapper.rs#L176-L187), [`plugin_system/host.rs:250-265`](../../src-tauri/src/plugin_system/host.rs#L250-L265).

```rust
// vst3/wrapper.rs
let mut data = ProcessData {
    …,
    input_param_changes:  std::ptr::null_mut(),   // ←
    output_param_changes: std::ptr::null_mut(),
    input_events:         std::ptr::null_mut(),
    …
};

// plugin_system/host.rs
pub fn set_parameter_normalised(&mut self, device_id, param_key, value) {
    record.saved_state = DeviceState::from_json(…);  // écrit dans un blob JSON,
    // jamais re-lu par le wrapper ni transmis au plugin
    Ok(())
}
```

**Pourquoi c'est problématique.** Toute la promesse du `Device` trait (paramètres normalisés, modulation, automation) repose sur la capacité du host à pousser des valeurs au plugin entre les blocs. Aujourd'hui :
- `PluginHost::set_parameter_normalised` *stocke* la valeur dans un `DeviceState::Json` mais **n'a aucune référence vers l'instance audio**.
- Les wrappers passent `input_param_changes = null` à chaque `process` ⇒ le plugin ne reçoit aucun changement.
- Les automations / modulations / macros / MIDI learn / preset morph — **rien ne fonctionne** côté plugins externes.

**Impact réel.** Un utilisateur charge Vital, bouge un knob *dans le DAW* : aucun effet sur le son. Bouge le knob *dans le GUI natif* : aucun effet sur le son (cf. A1 GUI audit). Bouge le LFO interne (modulator) qui pointe sur un paramètre plugin : aucun effet.

**Correction moderne.** SPSC ring buffer `ParameterChange { node_id, param_id, normalised, sample_offset }` pré-alloué côté host, consommé en début de `process_block` par le scheduler, puis transformé en `IParameterChanges` (VST3) ou `clap_input_events` (CLAP) avant l'appel `process`. Voir PARTIE 3.

---

#### P0-2 Aucun support des plugins instruments (synthés VST3i / CLAPi)

**Code concerné** — [`audio_graph/graph.rs:266-276`](../../src-tauri/src/audio_graph/graph.rs#L266-L276), [`compiler.rs:492-518`](../../src-tauri/src/audio_graph/compiler.rs#L492-L518), [`midi.rs`](../../src-tauri/src/audio_graph/midi.rs).

```rust
// graph.rs
let track_node = if !track.midi_clips.is_empty() {
    graph.add_node(NodeKind::Instrument { track_id: … }, …)  // ← internal only
} else {
    graph.add_node(NodeKind::Track { … }, …)
};

// compiler.rs build_node
NodeKind::Instrument { track_id } => {
    Box::new(InstrumentNode::new(id, notes, Box::new(PolySynth::new())))  // ← hard-coded
}
```

**Pourquoi c'est problématique.** Il n'existe aucun chemin pour qu'un plugin VST3i ou CLAPi devienne le générateur d'une track MIDI. La couche MIDI (`MidiNoteSnapshot`, `MidiEventList`) existe mais **ne quitte pas `InstrumentNode`**. Les wrappers déclarent eux-mêmes `has_midi_input: false` câblé en dur, et `process` ne propage `audio_inputs_count = 0` ni `input_events`.

**Impact réel.** Vital, Surge XT, Diva, Serum — chargés comme insert FX uniquement, sans note MIDI possible. Toute la cible "DAW moderne" devient inaccessible : un DAW sans synthés VST n'est pas un DAW.

**Correction moderne.** Voir PARTIE 2 architecture cible — ports MIDI déclarés via `PortLayout`, `NodeKind::Plugin` autorisé comme *source* d'une track MIDI, MIDI events convertis en `clap_event_note` / `IEventList` au moment du `process`.

---

#### P0-3 Capacités plugin câblées en dur, jamais introspectées

**Code concerné** — [`plugin_formats/vst3/wrapper.rs:71-81`](../../src-tauri/src/plugin_formats/vst3/wrapper.rs#L71-L81), [`plugin_formats/clap/wrapper.rs:79-90`](../../src-tauri/src/plugin_formats/clap/wrapper.rs#L79-L90), [`compiler.rs:431-449`](../../src-tauri/src/audio_graph/compiler.rs#L431-L449).

```rust
// VstWrapperNode::new
info: DeviceInfo {
    …,
    num_inputs:  2, num_outputs: 2,
    has_midi_input:  false, has_midi_output: false,
}

// GraphCompiler::num_inputs_for_kind
NodeKind::Plugin { .. } => 2, // default stereo in
```

**Pourquoi c'est problématique.** Le compileur force *toujours* 2 entrées / 2 sorties stéréo, sans interroger le plugin. Plusieurs casses :
- Plugin mono (Guitar Rig en mono, NoiseGen) : on alloue un canal de trop et on injecte du silence dans le 2ᵉ canal.
- Plugin surround (Auro-3D, Atmos) : ses 8+ canaux sont ignorés ; on lui passe stereo et il sortira stereo.
- Plugin avec sidechain (compresseur ducker, vocoder) : le bus auxiliaire n'existe pas dans le graphe.
- Plugin instrument à 16 sorties séparées (Kontakt, Battery) : on n'en lit qu'une.

**Impact réel.** Sidechain compression — fonctionnalité de base d'un DAW — impossible. Plugins multi-out cassés silencieusement.

**Correction moderne.** `PluginCapabilities` introspecté au `prepare()` :
```rust
struct PluginCapabilities {
    audio_ports:    Vec<AudioPortInfo>,    // VST3: getBusInfo / CLAP: clap.audio-ports
    midi_ports:     Vec<MidiPortInfo>,     // VST3: kEvent / CLAP: clap.note-ports
    parameter_count: u32,
    has_state:       bool,
    has_gui:         bool,
    has_sidechain:   bool,
    is_instrument:   bool,    // dérivé de plugin_type / features
    reported_latency_samples: u32,
}
```
Le compileur lit ces capacités et adapte `num_inputs_for_kind` / `num_outputs_for_kind` au cas par cas.

---

#### P0-4 État plugin jamais sérialisé (save/load cassés)

**Code concerné** — [`plugin_formats/vst3/wrapper.rs:239-242`](../../src-tauri/src/plugin_formats/vst3/wrapper.rs#L239-L242), [`plugin_formats/clap/wrapper.rs:225-226`](../../src-tauri/src/plugin_formats/clap/wrapper.rs#L225-L226).

```rust
fn save_state(&self) -> DeviceState { DeviceState::empty(&self.info.device_id) }
fn load_state(&mut self, _state: &DeviceState) {}
```

**Pourquoi c'est problématique.** À chaque save de projet, l'état des plugins externes est perdu. À chaque reload, les plugins reprennent leur preset par défaut. Idem undo/redo qui rebuild via le store : la mémoire interne du plugin (oscillateur, filtres, modulations) est repartie à zéro.

**Impact réel.** Le projet est inutilisable en production : impossible de fermer/rouvrir un projet sans tout reconfigurer.

**Correction moderne.** Implémenter via `IComponent::getState/setState` (VST3, IBStream) et `clap_plugin_state.save/load` (CLAP, clap_istream/ostream). Stocker dans `DeviceState::Binary`.

---

#### P0-5 Aucune sandboxing — un plugin tiers crashe le DAW

**Code concerné** — [`plugin_formats/vst3/wrapper.rs:197`](../../src-tauri/src/plugin_formats/vst3/wrapper.rs#L197), [`plugin_formats/clap/wrapper.rs:189`](../../src-tauri/src/plugin_formats/clap/wrapper.rs#L189), [`audio_graph/scheduler.rs`](../../src-tauri/src/audio_graph/scheduler.rs).

```rust
let r = unsafe { ((*(*proc).vtbl).process)(proc, &mut data) };
// pas de catch_unwind, pas de seh_handle, pas de timeout
```

**Pourquoi c'est problématique.** Aucune des trois défenses standard n'est en place :
1. **Pas d'isolation process** (modèle bridged Bitwig / REAPER bridge / FL bridge) — un plugin 32-bit ou un plugin mal codé peut SEGV → tout le moteur tombe.
2. **Pas de SEH/structured exception handling** sur Windows autour des appels FFI plugin.
3. **Pas de watchdog** sur la durée de `process` — un plugin qui boucle (deadlock interne, calcul exponentiel) bloque l'audio thread → underrun continu.

**Impact réel.** Vital, Diva, Serum — tous capables de crasher en certaines conditions (modulations extrêmes, presets corrompus). Avec l'archi actuelle : perte de la session non sauvegardée + frustration utilisateur.

**Correction moderne (par ordre de coût).**
- **Court terme :** `catch_unwind` autour du `process` (mais ne capte pas les SEH C++ Windows — utilité limitée). Compile flag `-C panic=unwind` requis.
- **Moyen terme :** Win32 `__try/__except` (via FFI helper) pour capter `STATUS_ACCESS_VIOLATION` et marquer le plugin Dead.
- **Long terme :** plugin bridge IPC (sub-process séparé, ring buffer shared memory pour audio, MIDI, params). Modèle Bitwig "All Plug-ins in Bridge". C'est la seule vraie solution mais c'est un module à part entière (~2–4 semaines).

---

#### P0-6 Pas de PDC pour les plugins (latency_samples toujours 0)

**Code concerné** — [`audio_graph/node.rs:185-187`](../../src-tauri/src/audio_graph/node.rs#L185-L187), [`compiler.rs:255-293`](../../src-tauri/src/audio_graph/compiler.rs#L255-L293).

```rust
fn latency_samples(&self) -> u32 { 0 }  // default never overridden by wrappers
```

**Pourquoi c'est problématique.** Le PDC du graphe est codé proprement (`compute_latency_compensation` walk topo, delay rings par port). Mais aucun wrapper n'appelle `IAudioProcessor::getLatencySamples` (VST3) ni ne lit `clap_plugin.get_latency()` (CLAP). Conclusion : tous les plugins déclarent 0 frame de latence → aucun alignement.

**Impact réel.** Un EQ linéaire-phase (souvent ~1024 samples), un convolveur de réverb (~4–8 ms), un compresseur lookahead — tous **désalignent leur track de la grille** sans que rien ne le compense. Inutilisable pour le mixing.

**Correction moderne.** Wrappers appellent l'API plugin dans `prepare()`, stockent dans un champ, exposent via `latency_samples()` override. Le compiler fait le reste.

---

### 🟡 P1 — Importants (bloquent l'adoption pro)

#### P1-1 ParameterMap des plugins jamais peuplée

**Code concerné** — [`vst3/wrapper.rs:82`](../../src-tauri/src/plugin_formats/vst3/wrapper.rs#L82), [`clap/wrapper.rs:90`](../../src-tauri/src/plugin_formats/clap/wrapper.rs#L90).

```rust
params: ParameterMap::new(),  // ← vide, jamais peuplée
```

**Pourquoi c'est problématique.** Pour exposer les knobs dans l'UI Echoes Studio (sans ouvrir le GUI natif), brancher l'automation, le MIDI learn, la modulation interne sur des paramètres plugin : il faut connaître la liste. Personne ne l'énumère.

**Correction moderne.** Wrappers énumèrent les paramètres dans `prepare()` :
- VST3 : `IEditController::getParameterCount/getParameterInfo` → `ParameterInfo { id, title, units, default_normalized, step_count, flags }`.
- CLAP : `clap.params.count/get_info` → `clap_param_info { id, flags, min_value, max_value, default_value, name, module }`.
Le mapping `ParamKey ↔ format-native ID` se garde dans le wrapper pour la traduction au moment du dispatch.

---

#### P1-2 Lifecycle bicéphale : wrapper vs PluginHost

**Code concerné** — [`vst3/wrapper.rs:140`](../../src-tauri/src/plugin_formats/vst3/wrapper.rs#L140), [`plugin_system/host.rs:151-154`](../../src-tauri/src/plugin_system/host.rs#L151-L154), [`plugin_system/lifecycle.rs:96-106`](../../src-tauri/src/plugin_system/lifecycle.rs#L96-L106).

```rust
// PluginHost::instantiate
self.records.insert(plugin_id.to_string(), DeviceRecord {
    lifecycle: LifecycleState::Running, // ← saute Created → Loaded → Initialized
    …
});

// VstWrapperNode::prepare
self.lifecycle = LifecycleState::Running;  // ← deuxième source de vérité

// lifecycle::transition
(LifecycleState::Created, LifecycleState::Loaded) // ne sera jamais appelé
```

**Pourquoi c'est problématique.** Deux états de cycle de vie maintenus en parallèle, jamais synchronisés. La machine à états dans `lifecycle.rs` impose `Created → Loaded → Initialized → Running` mais le host saute directement à `Running`. Suspend/Resume sur le host ne désactive pas le plugin réel ; suspend sur le wrapper ne notifie pas le host. Si on ajoute un jour un plugin "monitor" qui dépend du lifecycle pour suspendre la CPU au repos — ça ne marchera pas.

**Correction moderne.** Source de vérité unique : le wrapper porte le lifecycle, le host stocke seulement une *référence* (Arc/Weak). Les commandes `suspend/resume/destroy` du host appellent les méthodes correspondantes sur le wrapper *ou* enqueue la commande pour application au prochain block boundary.

---

#### P1-3 Pas de PluginCapabilities en tant que type explicite

**Code concerné** — `plugin_system/wrapper.rs::DeviceInfo` (champs séparés `has_midi_input`, `has_midi_output` mais aucune notion `sidechain`, `is_instrument`, `has_state`, `has_gui`).

**Correction moderne (cible).** Voir PARTIE 2 architecture cible.

---

#### P1-4 Modulation : que des LFOs

**Code concerné** — [`project.rs:131-133`](../../src-tauri/src/project.rs#L131-L133), [`compiler.rs:619-633`](../../src-tauri/src/audio_graph/compiler.rs#L619-L633).

```rust
pub enum ModulatorParams { Lfo(LfoParams) }   // ← un seul variant

NodeKind::Modulator { kind, … } => match kind {
    ModulatorKind::Lfo => …,
    _ => silence,   // Envelope, Macro, AudioRate stubés
}
```

`ModulatorKind` enum dans `modulation.rs` liste pourtant `Lfo / Envelope / Macro / AudioRate`. Le code reconnait que Envelope/Macro/AudioRate existent mais ne les implémente pas ; le projet ne les expose même pas dans `ModulatorParams`.

**Impact réel.** Pas d'ADSR routable, pas de macros (1 knob → N paramètres), pas de side-chain follower, pas d'aléatoire. Toute la sophistication "Bitwig modulators" manque.

**Correction moderne.** Ajouter `ModulatorParams::{ Envelope, Macro, Random, AudioFollower }` au projet, créer les nodes correspondants dans `audio_graph/nodes/`, faire en sorte que l'enveloppe puisse être *triggered* par un évènement note (lien avec MIDI).

---

#### P1-5 Pas de séparation port audio / port modulation / port MIDI

**Code concerné** — [`audio_graph/node.rs:115-117`](../../src-tauri/src/audio_graph/node.rs#L115-L117), [`graph.rs:140-141`](../../src-tauri/src/audio_graph/graph.rs#L140-L141).

```rust
fn num_input_ports(&self)  -> usize;
fn num_output_ports(&self) -> usize;

pub struct Edge { pub from: PortId, pub to: PortId }
```

Tous les ports sont du même type — il n'y a pas de distinction au niveau du système de port entre audio, MIDI et modulation. La modulation est implémentée via une `ModulationMap` parallèle, ce qui *marche* mais empêche un routing UI uniforme.

**Correction moderne.** `PortKind { Audio { channels: u8 }, Midi, Modulation { polarity: Bipolar | Unipolar } }`. Le `PortId` carry le `PortKind`. Le compiler valide la compatibilité des connexions.

---

#### P1-6 Pas de containers, layer devices, note FX

**Code concerné** — `NodeKind` ne contient ni `Container`, ni `Layer`, ni `NoteFx`, ni `Splitter`/`Merger`.

**Pourquoi c'est problématique.** Modèle Bitwig : un container regroupe N devices, peut être bypassé / muté / présété en bloc, peut router en parallèle (Layer) ou en split (Multiband). Un Note FX modifie un flux MIDI avant l'instrument (transpose, chord, arp).

**Correction moderne.** `NodeKind::Container { kind: ContainerKind, children: Vec<NodeId> }` avec `ContainerKind::{ Chain, Layer, Split, Multiband }`. Le compiler expand le container en sous-graphe au moment de la compilation. Note FX = node MIDI in/out qui transforme.

---

#### P1-7 Threading : `instantiate` n'enregistre pas le Weak<Device>

**Code concerné** — [`plugin_system/host.rs:118-158`](../../src-tauri/src/plugin_system/host.rs#L118-L158).

Le `PluginHost::instantiate` *crée* la `Box<dyn AudioNode>` et la retourne au compileur, qui la place dans `CompiledGraph.processors`. Mais le host ne garde aucune référence (forte ou faible) vers l'instance — uniquement un `DeviceRecord` opaque par plugin_id. Conséquence : `set_parameter_normalised` ne sait pas vers *qui* envoyer le paramètre, et `save_device_state` ne peut pas appeler `device.save_state()`.

**Correction moderne.** Le host crée un `Arc<Mutex<dyn Device>>` (ou mieux, `Arc<DeviceCommandQueue>` lock-free) et lui passe au compileur une *poignée* (Arc clone). Le compileur insère un proxy qui *référence* l'instance partagée. Toutes les méthodes `host.set_parameter / save_state / load_state` traversent ce handle.

---

#### P1-8 `live_params` couvre tracks/mods/routes mais pas les paramètres plugin

**Code concerné** — [`audio_graph/live_params.rs`](../../src-tauri/src/audio_graph/live_params.rs).

Le projet a une infrastructure `LiveParams` (atomics partagés entre command thread et audio thread) pour volume/pan/mute/solo/depth modulation/master soft-clip. C'est l'approche correcte. Mais elle n'est pas étendue aux paramètres des plugins : impossible de faire bouger un paramètre plugin à 60 Hz depuis l'UI sans rebuilder.

**Correction moderne.** Voir P0-1 — la SPSC param queue est l'extension naturelle de `LiveParams` pour les paramètres dynamiques (où on ne peut pas pré-allouer un cell par paramètre puisqu'on découvre les params au prepare).

---

#### P1-9 PluginInfo (UI) ne porte pas les capacités

**Code concerné** — [`plugin_system/host.rs:320-327`](../../src-tauri/src/plugin_system/host.rs#L320-L327).

```rust
pub struct PluginInfo { id, name, vendor, format, categories }  // ← aucune capacité
```

L'UI ne sait pas distinguer un instrument d'un effet, un mono d'un stéréo, un avec sidechain de sans. Le `PluginPicker` est donc générique alors que Bitwig classe les plugins par usage (instrument browser ≠ FX browser).

**Correction moderne.** Inclure `PluginCapabilities` dans `PluginInfo` (renseigné via une passe de probe au scan ou lazy au premier `instantiate`).

---

### 🟢 P2 — Améliorations (qualité, scalabilité)

#### P2-1 Pas de bypass des plugins en process_block

`effect.enabled = false` est résolu *au moment du compile* (le node n'est pas créé). Toggler bypass = recompile. Acceptable pour un toggle occasionnel, lourd pour de l'A/B testing rapide.

**Cible.** `BypassNode` wrap chaque plugin avec un atomic bool, `process` choisit copy_from(input) si bypassed.

#### P2-2 Pas de groupes A/B / preset morphing

`DeviceState` opaque ⇒ on ne sait pas interpoler. Bitwig sait morpher entre 8 slots de preset par device. Hors scope court terme mais à garder en tête.

#### P2-3 Modulators au niveau track uniquement

`AppTrack.modulators / mod_routes` — pas de modulator global (master FX, return bus). Limite l'usage.

#### P2-4 Pas de test fuzz / golden master des wrappers

Aucun test n'instancie un plugin réel en CI. Le scan + instanciation pourraient régresser sans qu'on le voie.

#### P2-5 Worker pool : pas de mesure de coût par node

Le scheduler dispatch tous les steps d'un niveau au pool sans aucune information de coût. Pour des projets très inhomogènes (un Diva + 30 GainNode) la stratégie est sous-optimale (Diva monopolise un worker, les autres tournent ensemble en quelques µs).

**Cible.** Mesure de durée moyenne par step (atomic EWMA), tri par coût décroissant, dispatch round-robin sur les workers libres. Bitwig le fait depuis ~2018.

#### P2-6 Modulation : pas de phase reset sur transport restart

`LfoNode` n'est pas resetté quand on stoppe/repart. Pour la production *musicale* on veut souvent un LFO sync-tempo qui reprenne au début à chaque play.

**Cible.** Méthode `AudioNode::reset` est déjà là — il faut l'appeler depuis le scheduler au transport restart pour tous les nodes.

#### P2-7 `LfoNode::rate_hz` : pas de sync au tempo

Rate libre uniquement. Modes "1/4", "1/8 dotted", "1/16 triplet" — standard sur n'importe quel DAW — absents.

---

## PARTIE 2 — Architecture cible

### Objectifs

1. **Source unique de vérité par plugin instance** — un seul VST3/CLAP par effet, partagé entre audio thread et GUI.
2. **Capacités introspectées** — chaque plugin déclare ses ports, paramètres, latence, MIDI, sidechain.
3. **Transport bidirectionnel paramètres** — UI ↔ DSP via ring buffer SPSC lock-free.
4. **MIDI/note routing** — plugins instruments hostables sur des tracks MIDI.
5. **State save/load** — projet sauvegardable et rechargeable bit-identique.
6. **Modulation généralisée** — LFO / Envelope / Macro / Random / Follower.
7. **Containers** — chain / layer / split pour grouper et router visuellement.
8. **Sandboxing** — au minimum SEH catch, à terme bridge IPC.

### Vue d'ensemble

```
                              ┌──────────────────────────┐
                              │     PluginInstance       │  Arc-partagée
                              │  ├ ClapPlugin | IComponent (FFI)
                              │  ├ ParameterMap          │
                              │  ├ DeviceCapabilities    │
                              │  ├ PortLayout            │
                              │  ├ ParamQueue (SPSC)     │  ← UI → DSP
                              │  ├ StateBlob             │
                              │  └ Lifecycle (atomic)    │
                              └──┬───────────────┬───────┘
                                 │               │
                ┌────────────────┘               └────────────┐
                ▼                                             ▼
       ┌──────────────────┐                          ┌─────────────────┐
       │   AudioNode      │                          │  EditorWindow   │
       │   (graph wrapper)│                          │  (HWND container)│
       │                  │                          │                  │
       │ in process():    │                          │ on knob change: │
       │   drain ParamQ   │                          │   push to ParamQ │
       │   build vst data │                          │   (lock-free)   │
       │   call plugin->process                      │                  │
       └──────────────────┘                          └─────────────────┘
                ▲                                             ▲
                │ owned by scheduler                          │ owned by editor map
                └─────────────────────────────────────────────┘
```

### Types cibles

```rust
// ── Capabilities (introspected at prepare) ──────────────────────────────────
pub struct DeviceCapabilities {
    pub audio_input_buses:  Vec<AudioBusInfo>,   // Main + Sidechain + Aux
    pub audio_output_buses: Vec<AudioBusInfo>,
    pub midi_input_ports:   Vec<MidiPortInfo>,
    pub midi_output_ports:  Vec<MidiPortInfo>,
    pub parameters:         Vec<ParameterInfo>,
    pub supports_state:     bool,
    pub supports_gui:       bool,
    pub is_instrument:      bool,
    pub reported_latency_samples: u32,
    pub reported_tail_samples:    u32,
}

pub struct AudioBusInfo {
    pub name: String,
    pub channel_count: u8,
    pub is_main: bool,
    pub is_aux:  bool,           // sidechain
    pub speaker_layout: SpeakerLayout,
}

pub struct MidiPortInfo { pub name: String, pub is_main: bool }

pub struct ParameterInfo {
    pub key:    ParamKey,
    pub name:   String,
    pub range:  ParameterRange,
    pub flags:  ParameterFlags,   // automatable, hidden, bypass, …
    pub format_native_id: u32,    // VST3 ParamID or CLAP clap_id
}

// ── Ports as first-class typed entities ─────────────────────────────────────
pub enum PortKind {
    Audio { channels: u8 },
    Midi,
    Modulation { polarity: Polarity },   // Bipolar (-1..1) | Unipolar (0..1)
}

pub struct PortInfo {
    pub kind: PortKind,
    pub name: String,
    pub is_sidechain: bool,
}

pub trait AudioNode: Send + 'static {
    fn ports_in(&self)  -> &[PortInfo];
    fn ports_out(&self) -> &[PortInfo];
    // … process unchanged
}

// ── Lock-free parameter delivery ────────────────────────────────────────────
pub struct ParameterChange {
    pub node_id:      NodeId,
    pub native_id:    u32,        // resolved to plugin's native ID by wrapper
    pub normalised:   f32,
    pub sample_offset: u32,        // intra-block scheduling
}

pub struct ParamQueue { /* SPSC rtrb or ringbuf with pre-allocated slots */ }

// ── Containers ──────────────────────────────────────────────────────────────
pub enum ContainerKind { Chain, Layer, Split { mode: SplitMode } }
pub enum NodeKind {
    …,
    Container { kind: ContainerKind, children: Vec<NodeId> },
    NoteFx    { kind: NoteFxKind },
}

// ── Modulation expanded ─────────────────────────────────────────────────────
pub enum ModulatorParams {
    Lfo(LfoParams),
    Envelope(EnvelopeParams),     // ADSR + multistage
    Macro(MacroParams),
    Random(RandomParams),         // S&H, smooth random
    AudioFollower(FollowerParams),// envelope follower / sidechain
}
```

### Plan d'orchestration (par phase)

| Phase | Livrable                                                          | Précondition         | Effort   |
|-------|-------------------------------------------------------------------|----------------------|----------|
| 1     | `DeviceCapabilities` + introspection ports / params au `prepare`  | aucun                | 1–2 jours |
| 2     | SPSC param queue + drain dans wrappers (UI knob → plugin)         | Phase 1              | 2–3 jours |
| 3     | State save/load via FFI natif + persistance projet                | aucun                | 1–2 jours |
| 4     | PluginInstance unique (élimine A1 du GUI audit)                   | Phases 1+2           | 3–5 jours |
| 5     | Latence rapportée → PDC                                           | Phase 1              | 0.5 jour  |
| 6     | MIDI vers plugins instruments + NodeKind::Plugin source MIDI      | Phases 1+2+4         | 3–5 jours |
| 7     | Sidechain via AudioBusInfo::is_aux + UI routing                   | Phase 1              | 2 jours   |
| 8     | Modulators étendus (Envelope, Macro, Random, Follower)            | aucun                | 3 jours   |
| 9     | NodeKind::Container + UI                                          | aucun                | 5 jours   |
| 10    | Sandboxing (Win32 SEH minimum, bridge IPC en backlog)             | Phase 4              | 2 jours / 3 sem |

---

## PARTIE 3 — Implémentation (cette session)

L'audit étant le livrable principal, j'ai limité l'implémentation à un *fondation set* tractable, qui :
- ne casse aucune compatibilité existante,
- ouvre la voie aux phases suivantes,
- ne traverse pas les frontières temps-réel.

**Implémenté dans ce commit :**
1. `DeviceCapabilities` + `AudioBusInfo` + `MidiPortInfo` + `ParameterInfo` (types).
2. `Device::capabilities()` trait method avec impl par défaut sûre.
3. SEH/`catch_unwind` autour des `process` plugins (P0-5 court terme).
4. Surface `latency_samples()` depuis VST3 (`IAudioProcessor::getLatencySamples`) et CLAP (`clap_plugin.get_latency` via extension).
5. Documentation du contrat dans `docs/claude/systems/plugin-wrapper-system.md`.

**Reporté à des PRs dédiés (avec ADR) :**
- Phases 2 (param queue), 3 (state), 4 (PluginInstance unique), 6 (MIDI), 7 (sidechain), 9 (containers), 10 (bridge IPC).

Voir section *Refactors effectués* pour la liste exacte des fichiers.

---

## PARTIE 4 — Livrables

### Liste des refactors effectués (cette session)

| Fichier                                                         | Changement                                                                 |
|-----------------------------------------------------------------|----------------------------------------------------------------------------|
| `src-tauri/src/plugin_system/capabilities.rs` (nouveau)         | `DeviceCapabilities`, `AudioBusInfo`, `MidiPortInfo`, `ParameterInfo`      |
| `src-tauri/src/plugin_system/wrapper.rs`                        | `Device::capabilities()` avec impl par défaut + helper sur DeviceInfo      |
| `src-tauri/src/plugin_system/mod.rs`                            | re-export `capabilities`                                                   |
| `src-tauri/src/plugin_formats/vst3/wrapper.rs`                  | introspection latence (`getLatencySamples`) + `capabilities()`             |
| `src-tauri/src/plugin_formats/clap/wrapper.rs`                  | introspection latence (extension `clap.latency`) + `capabilities()`        |
| `src-tauri/src/plugin_formats/clap/ffi.rs`                      | `ClapPluginLatency` extension + `CLAP_EXT_LATENCY`                         |
| `src-tauri/src/audio_graph/scheduler.rs`                        | `catch_unwind` autour de `processors[i].process` (P0-5 court terme)        |
| `docs/claude/audits/plugin-system-audit-2026-05-31.md` (ce doc) | audit complet                                                              |
| `docs/claude/decisions/006-plugin-capabilities-and-pdc.md`      | ADR pour les changements                                                   |

### Risques restants

- **R1 — Plugins instables non isolés.** `catch_unwind` ne capte pas les SEH C++ Windows (access violation, divide by zero). Les plugins qui crashent côté C++ continueront de tomber le DAW. Mitigation court terme : appverif au dev. Long terme : bridge IPC.
- **R2 — Paramètres UI plugin restent muets.** Tant que la SPSC param queue n'est pas implémentée, l'utilisateur ne peut interagir avec les plugins externes qu'à travers le GUI natif (encore lui-même limité par A1 du GUI audit).
- **R3 — Save/load projet perd l'état des plugins.** Inchangé.
- **R4 — Plugins instruments inutilisables.** Inchangé.
- **R5 — Sidechain inutilisable.** Inchangé.

### Plan de migration futur (ordre conseillé)

1. **P0-1 + P1-1 + P1-8 ensemble** — param queue + énumération + state. Refactor de `PluginHost` pour qu'il porte des `Arc<Mutex<PluginInstance>>` au lieu de `DeviceRecord` opaques. ~3–5 jours.
2. **P0-4 (state)** dans la foulée — c'est un dérivé propre de l'énumération.
3. **A1 GUI audit (instance unique)** — devient simple après #1 : le `EditorWindow` clone l'`Arc<PluginInstance>` du record.
4. **P0-2 (MIDI vers plugins)** + déclaration MIDI ports — extension naturelle de PluginCapabilities + `PortKind::Midi`.
5. **P0-3 (capacités → compiler)** — `num_inputs_for_kind` consulte les capabilities. Devient possible une fois #1 fait.
6. **P0-6 (PDC)** — fait dans cette session côté wrapper ; reste à vérifier qu'on relance compile quand la latence change runtime (`restart` notification VST3 / `clap_host_latency.changed`).
7. **P1-4 (modulators étendus)** + P2-6/P2-7 — un sprint UI.
8. **P0-5 long terme (bridge IPC)** — projet à part. Référence : modèle Pluginval / Helio.
9. **P1-6 (containers)** + **P1-5 (PortKind)** — refonte UI graphe.

### Points bloquants éventuels

- **B1 — `unsafe` propagation.** Beaucoup de changements ci-dessus impliquent du `unsafe` FFI nouveau (IBStream pour VST3 state, clap_istream/ostream pour CLAP). Plan : tout encapsuler derrière un wrapper safe par format, audité comme la FFI actuelle.
- **B2 — Réentrance host_get_extension.** Si les plugins commencent à demander `clap.params.host_params_rescan` depuis le worker thread, on a besoin d'une vraie file thread-safe (déjà partiellement en place via `WM_APP_DRAIN` côté GUI).
- **B3 — Compatibilité projet.** Le changement de schéma (PluginParams qui inclut un blob state binaire) impose un bump de version `ProjectFile`. Plan : ajouter un champ optionnel `state_b64: Option<String>` côté file, no-op si absent.
- **B4 — Test instrumental.** Sans CI capable de loader Vital/Surge XT, les régressions FFI sont invisibles. Plan : ajouter au moins un plugin minimal *écrit en Rust* (genre `clap-plugin` minimal) embarqué pour tests.

### Estimation dette technique restante

| Catégorie               | Effort restant | Bloquant ? |
|-------------------------|----------------|------------|
| Param queue + state     | ~5 j           | P0         |
| Instance unique (A1)    | ~3 j (après ↑) | P0         |
| MIDI plugins            | ~4 j           | P0         |
| Capacités → compiler    | ~2 j           | P0         |
| Sidechain               | ~2 j           | P1         |
| Modulators étendus      | ~3 j           | P1         |
| Containers              | ~5 j           | P1         |
| Sandboxing bridge IPC   | ~15–20 j       | P0 long terme |
| **Total court terme P0/P1** | **~24 j**  |            |
| **Total avec bridge**   | **~44 j**      |            |

---

## Tests de validation suggérés

- Charger Vital, modifier rate LFO via knob host (futur), vérifier que le son bouge (futur P0-1).
- Sauvegarder un projet avec Surge XT chargé, reload, vérifier preset intact (futur P0-4).
- Charger Diva via NodeKind::Plugin source MIDI, jouer une note, vérifier son (futur P0-2).
- Charger un compresseur sidechain (ex: Pro-C 2), brancher une track aux en sidechain, vérifier ducking (futur P0-3 + P1-5).
- Plugin crash test : forcer un access violation depuis un plugin et vérifier que le DAW reste up (futur P0-5).
