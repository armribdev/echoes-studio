# Target Architecture — Echoes Studio (migration vers DAW moderne)

> Architecture **cible réaliste** pour faire évoluer Echoes Studio en DAW moderne (Bitwig/Ableton/Reaper-like), **sans rewrite**.
> Source des contraintes : [`critical_issues.md`](critical_issues.md), [`current_engine_diagram.md`](current_engine_diagram.md), audits sous [`../audits/`](../audits/).
> Date : 2026-05-31. Statut : proposition pour ratification.

Ce document décrit l'**état cible** et la **stratégie de migration**. Il ne décrit pas un système à construire de zéro : chaque brique cible est attachée à une brique actuelle conservée, étendue, ou remplacée en place.

---

## 0. Principes directeurs

1. **Le DAG reste le cœur du moteur**. Aucune réécriture du scheduler, de l'`ExecutionPlan`, du snapshot atomique, du drop ring, du worker pool, du PDC, du FTZ/DAZ. Ces briques sont validées par les audits.
2. **La dette est aux frontières**. Toutes les évolutions structurelles ciblent les **interfaces** : moteur ↔ plugin (transport paramètres, capabilities, lifecycle unique, sandbox), moteur ↔ UI (LiveParams étendus, automation, surface routing), moteur ↔ MIDI (ports MIDI de premier ordre).
3. **Coexistence permanente du chemin legacy et du chemin nouveau**. Les nouveaux mécanismes sont introduits **derrière** les types existants (`NodeKind`, `EffectParams`, `ModulatorParams`) — pas à la place. Migration variant par variant.
4. **Stabilité RT non négociable**. Aucune étape ne dégrade les invariants du thread audio (cf. §1.6 actuel). Les ajouts doivent passer par des cellules atomiques pré-allouées ou des files SPSC bornées, jamais par allocation ni mutex sur le path hot.
5. **Pas de greenfield**. Toute proposition ici doit pouvoir cohabiter avec la branche `master` actuelle au prix d'un seul feature flag ou d'une seule indirection.

---

## 1. Résumé structuré de l'architecture actuelle (depuis audit)

### 1.1 Ce qui fonctionne et reste

| Brique | Statut | Conservation cible |
|---|---|---|
| DAG audio (`AudioGraph` + `Edge` + Kahn topo-sort) | ✅ | conservé, étendu (PortKind, MIDI edges) |
| `ExecutionPlan` immuable + `levels` parallèles | ✅ | conservé tel quel |
| `RuntimeSnapshot` (`AtomicPtr<GraphScheduler>` + drop ring SPSC) | ✅ | conservé tel quel |
| `GraphScheduler::process_block` + worker pool | ✅ | conservé ; priorisation RT ajoutée (1.7) |
| PDC interne (delay rings par port + walk topo) | ✅ | conservé ; alimenté par latence plugin |
| FTZ/DAZ + pré-allocation totale | ✅ | conservé, propagé aux nouveaux nodes |
| `LiveParams` (atomics) pour volume/pan/mute/solo/mod rate/depth | ✅ | conservé, **étendu** (cf. §6) |
| `MidiNoteSnapshot` (capacité fixe, tri stable) | ✅ | conservé ; étendu en port MIDI graphe |
| Export offline (réutilise `compile_scheduler`) | ✅ | conservé tel quel |
| Thin-wrapper plugins (ADR-003) | ✅ | conservé ; complété (capabilities, params, state) |

### 1.2 Ce qui doit évoluer en place

| Brique actuelle | Limite (réf. critical_issues) | Évolution cible |
|---|---|---|
| `NodeKind { Track, Instrument, Plugin, Eq, … }` enum fermé | 2.2, 2.3, 2.5 | enum **élargi** progressivement (variants Container, ReturnBus, NoteFx, …) ; pas remplacé |
| Ports `usize` non typés | 2.2 | `PortKind { Audio, Midi, Modulation, Control }` ajouté ; convention legacy → `Audio` |
| `EffectParams` enum + recompile sur tout change | 1.4, 4.6 | `ParameterBlock` par effet (cellules atomiques) ; recompile réservé aux changements **structurels** |
| `InstrumentNode { inner: PolySynth }` codé en dur | 3.2 | `InstrumentNode { inner: Box<dyn AudioNode> }` polymorphe (PolySynth ou plugin instrument) |
| `PluginHost::instantiate` retourne `Box<dyn AudioNode>` opaque | 3.1, 3.8, 3.11 | retourne `Arc<PluginInstance>` partagé hôte/audio/GUI |
| `DeviceInfo` hardcodé 2/2 | 3.3 | `PluginCapabilities` introspecté au scan + au prepare |
| `ModulatorParams::Lfo` unique | 5.1 | variants `Envelope`, `Macro`, `Random`, `Follower` ajoutés côte à côte |
| `ModulationTargetRef { TrackGain, EffectParam }` | 5.4 | variant `PluginParam { instance_id, param_key }` ajouté |
| `AppTrack` = audio XOR MIDI | §1 diag | inchangé en UX ; les containers (Chain/Layer) permettent l'hybride sans casser `AppTrack` |
| `SendNode` sans `ReturnNode` | 2.4 | `ReturnBus` ajouté ; routing graphe interne ; UX inchangée |
| `LifecycleState` bicéphale | 3.7 | source de vérité unique côté `PluginInstance` |

### 1.3 Ce qui n'existe pas et doit être ajouté

| Manque (réf.) | Cible |
|---|---|
| Automation (lanes temporelles) — 5.7, 6.6 | `AutomationLane` au niveau projet ; baked en `AutomationCurve` au compile ; lue per-block par `AutomationEvaluator` (block-rate) |
| Sandbox plugin out-of-process — 8.1–8.4 | bridge optionnel `plugin_host_bridge` (process séparé) ; même API `Arc<PluginInstance>` |
| XRun télémétrie — 1.1 | wired via `OutputCallbackInfo` → SPSC → telemetry sink |
| MIDI live input — diag §3 | `MidiInputNode` + capture device |
| Note FX — 2.6 | nodes MIDI-in/MIDI-out greffés sur ports MIDI typés |
| Containers (Chain/Layer/Split) — 2.5 | `ContainerNode` métacompilé : non émis dans `ExecutionPlan`, déplié en sous-graphe |
| Bypass sans recompile — 6.4 | flag atomique sur step + court-circuit dans `process_one_step` |
| Tempo-sync LFO + phase reset — 5.5, 5.6 | extension `LfoParams` ; reset hook au transport restart |

---

## 2. Contraintes de migration

Ces contraintes encadrent toute proposition ci-dessous. Elles ne sont pas négociables.

**C-1 — Pas de rupture du callback audio.** Aucune étape ne peut introduire de mutex, d'allocation, de drop, ou de syscall dans `process_block`. Toute évolution se branche via `LiveParams`, SPSC, ou snapshot atomique.

**C-2 — Bit-identique sur le chemin existant.** Tant qu'un projet n'utilise pas une nouvelle feature (automation, plugin instrument, container), son rendu offline doit rester octet-pour-octet identique. C'est la garantie de non-régression.

**C-3 — Migration variant par variant.** Aucune phase ne migre tous les effets / tous les modulateurs / tous les wrappers d'un coup. On élargit les enums, on traite un cas, on valide, on passe au suivant.

**C-4 — Compatibilité ascendante des projets.** Les projets sauvegardés en `master` doivent se charger sans conversion manuelle. Les nouveaux champs ont des defaults sûrs (`Option`, version `serde(default)`).

**C-5 — Lock-free entre command et audio.** Tout nouveau point d'écriture cmd→audio passe par : (a) cellules atomiques pré-allouées (`LiveParams`-like), (b) file SPSC bornée pré-allouée, ou (c) snapshot atomique pour changements structurels. Jamais Mutex.

**C-6 — Une seule instance plugin par device.** Cible non négociable issue de A1/CL3/CL5 : l'audio et le GUI partagent le **même** `Arc<PluginInstance>`. Pas de seconde instance pour l'éditeur.

**C-7 — Pas de réécriture du compilateur.** `GraphCompiler::compile` est étendu (PortKind validation, déploiement containers, baking automation). Pas réécrit.

**C-8 — UX track-linéaire préservée.** Le modèle UX `Track + EffectChain` reste l'interface par défaut. Les containers et le routing arbitraire s'ajoutent **par-dessus**, comme « advanced view ». Pas de migration forcée des projets vers une vue graphe.

**C-9 — Sandbox optionnelle, pas par défaut.** Le bridge out-of-process est une option (latence + complexité) ; les plugins fiables continuent in-process pour la perf. Le choix est par-device.

**C-10 — Pas de feature flag pour C-2.** Les changements bit-identiques s'appliquent au master sans toggle. Les features risquées (sandbox, containers, sample-accurate modulation) sont derrière un flag jusqu'à validation.

---

## 3. Architecture cible hybride

### 3.1 Vue d'ensemble

```
┌────────────────────────────────────────────────────────────────────────────┐
│  FRONTEND (Svelte)                                                         │
│   Timeline · Mixer · EffectChain · Automation Lane · Modulation Matrix     │
│   PluginPicker (filtré par capabilities) · Routing View (advanced)         │
└──────────────────────────────────┬─────────────────────────────────────────┘
                                   │ Tauri IPC
                                   ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  COMMAND THREAD                                                            │
│                                                                            │
│   engine_gateway::api                                                      │
│        │                                                                   │
│        ├── AppProject (legacy structure : Tracks, EffectChain, Modulators) │
│        │                                                                   │
│        ├── AppProject++ (extensions backward-compat) :                     │
│        │     · automation_lanes: Vec<AutomationLane>                       │
│        │     · containers: Vec<Container> (optional, advanced)             │
│        │     · returns: Vec<ReturnBus>                                     │
│        │     · plugin_state_blobs: HashMap<DeviceId, Vec<u8>>              │
│        │                                                                   │
│        ▼                                                                   │
│   AudioGraph::from_project  ─── élargi : émet PortKind, MIDI edges,        │
│        │                              ContainerNode, ReturnBusNode         │
│        ▼                                                                   │
│   GraphCompiler::compile  ─── élargi : valide PortKind, déplie containers, │
│        │                              bake AutomationCurves                │
│        │                                                                   │
│        ├──► PluginHost.instantiate ──► Arc<PluginInstance>                 │
│        │      (host conserve référence ; wrapper référence Arc clone)      │
│        │                                                                   │
│        ▼                                                                   │
│   CompiledGraph { plan, processors, meter_cells, route_cells,              │
│                   automation_cells (NEW) }                                 │
│        │                                                                   │
│        ▼                                                                   │
│   RuntimeSnapshot::enqueue ── inchangé                                     │
│                                                                            │
│   LiveParams (étendu)                                                      │
│    ├ tracks (existant)                                                     │
│    ├ modulators (existant)                                                 │
│    ├ effect_params: HashMap<EffectId, Arc<ParameterBlock>>   ← NEW         │
│    ├ plugin_params: HashMap<DeviceId, Arc<PluginParamBlock>> ← NEW         │
│    ├ automation_state: Arc<AutomationCells>                  ← NEW         │
│    └ bypass: HashMap<StepId, Arc<AtomicBool>>                ← NEW         │
│                                                                            │
│   PluginHost (étendu)                                                      │
│    ├ instances: HashMap<DeviceId, Arc<PluginInstance>>       ← NEW         │
│    ├ param_queues: HashMap<DeviceId, SpscQueue<ParamChange>> ← NEW         │
│    └ midi_queues:  HashMap<DeviceId, SpscQueue<MidiEvent>>   ← NEW         │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
                                   │
   ╔═══════════════════════════════│════════════════════════════════════════╗
   ║                               ▼                                        ║
   ║  AUDIO THREAD (CPAL callback) — invariants inchangés                   ║
   ║                                                                        ║
   ║   try_swap snapshot ── inchangé                                        ║
   ║                                                                        ║
   ║   process_block :                                                      ║
   ║    ├ AutomationEvaluator.advance(transport)  ← NEW (block-rate)        ║
   ║    │   → écrit dans automation_cells (atomics)                         ║
   ║    │                                                                   ║
   ║    ├ pour chaque step / level :                                        ║
   ║    │   if bypass.load() → court-circuit input→output (NEW)             ║
   ║    │   drain ParamChange SPSC → push to plugin instance (NEW)          ║
   ║    │   drain MidiEvent SPSC  → push to plugin instance (NEW)           ║
   ║    │   modulation block-rate (existant)                                ║
   ║    │   PDC delay rings (existant — alimenté par latence plugin)        ║
   ║    │   processor.process(ctx)  enveloppé catch_unwind (existant)       ║
   ║    │   ┌─ in-process path : appel direct (legacy)                      ║
   ║    │   └─ sandbox path : ring shared-mem → bridge process (NEW, opt)   ║
   ║    │                                                                   ║
   ║    └ master buffer planaire → CPAL                                     ║
   ╚════════════════════════════════════════════════════════════════════════╝
                                   │
                                   ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  PLUGIN PROCESS (bridge sandbox — OPTIONNEL)                               │
│   plugin_host_bridge.exe                                                   │
│    ├ ring shared-mem audio in / audio out / param / event                  │
│    ├ wrapper VST3/CLAP réel                                                │
│    └ watchdog (timeout process → mark Dead, return silence)                │
└────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Cinq couches conceptuelles

Le moteur cible est lu en cinq couches, du plus stable au plus volatile :

```
                                         volatilité  isolation RT
┌────────────────────────────────────┐
│  L5 — Surface UX                    │   haute      n/a
│  (Tracks, EffectChain, Lanes UI,    │
│   Modulation Matrix, Routing View)  │
├────────────────────────────────────┤
│  L4 — Project Model                 │   moyenne    n/a
│  (AppProject + AppProject++         │
│   ; serialisable ; source de vérité)│
├────────────────────────────────────┤
│  L3 — Graph Build                   │   moyenne    cmd thread
│  (AudioGraph::from_project,         │
│   PortKind validation,              │
│   container expansion)              │
├────────────────────────────────────┤
│  L2 — Compile & Snapshot            │   basse      cmd thread → swap
│  (ExecutionPlan, automation bake,   │
│   PluginInstance setup, drop ring)  │
├────────────────────────────────────┤
│  L1 — Runtime (audio thread)        │   GELÉE      audio thread
│  (GraphScheduler, WorkerPool,       │   = invariants RT non négociables
│   PDC, LiveParams, SPSC drains)     │
└────────────────────────────────────┘
```

**Règle d'or** : toute évolution se justifie par sa couche. Une feature L5 ne doit toucher L1 que par cellules atomiques préallouées ou SPSC. Si elle exige une modif L1, c'est un signal d'ADR.

---

## 4. Diagrammes — legacy / transition / cible

### 4.1 Legacy path (existe aujourd'hui)

```
UPDATE_EFFECT (Eq freq)
     │
     ▼
AppProject mutate
     │
     ▼
recompile_if_needed  ─── lock engine (50–200 ms sur gros projets)
     │
     ▼
GraphCompiler::compile (rebuild complet)
     │
     ▼
prepare() sur TOUS les nodes  ── réalloc comb/allpass Reverb, reset Compressor
     │
     ▼
RuntimeSnapshot::enqueue → swap
     │
     ▼
                                       audio thread : nouveau scheduler
                                       glitch potentiel sur queues reverb/delay
```

Conséquences : 1.3 + 1.4 + 4.6 (critical_issues.md).

### 4.2 Transition path (cible court terme, sans casser legacy)

Pour les paramètres **à path hot** (Eq freq, Compressor threshold, Reverb wet, Delay feedback, et **chaque param plugin enregistré**) :

```
UPDATE_EFFECT (Eq freq)
     │
     ▼
AppProject mutate
     │
     ▼
classify_param(effect_id, param) :
     ├── if HOT (params atomiques alloués) :
     │      LiveParams.effect_params[effect_id].set(param, value)
     │      → audio thread lit l'atomic au prochain block, smooth ramp
     │      → AUCUN recompile, AUCUN lock
     │
     └── if STRUCTURAL (ex: changement de type de filtre, ajout d'effet) :
            recompile_if_needed  ── chemin legacy préservé pour ce cas seul
```

Idem pour plugins :

```
UPDATE_PLUGIN_PARAM (device_id, param_key, value)
     │
     ▼
PluginHost.param_queues[device_id].push(ParamChange { param_key, value, sample_offset: 0 })
     │
     ▼
                                       audio thread : drain SPSC
                                       push to plugin via VST3 IParameterChanges / CLAP clap_event_param_value
                                       AUCUN recompile
```

### 4.3 Future graph model (cible long terme, additionne sans remplacer)

```
                                     AppProject++
                                          │
                                          ▼
                       ┌─────────────────────────────────────┐
                       │   AudioGraph (DAG)                   │
                       │   Edge { from, to, kind: PortKind }  │
                       │   ─── PortKind ∈ { Audio,            │
                       │            Midi, Modulation,         │
                       │            Control }                 │
                       └──────────────┬──────────────────────┘
                                      │
                                      ▼
                       ┌──────────────────────────────────────┐
                       │  Containers (Chain / Layer / Split / │
                       │   Multiband / ReturnBus)             │
                       │                                      │
                       │  AT COMPILE :                        │
                       │   container_expand(container) →      │
                       │   sous-graphe inline dans le DAG     │
                       │   (les containers n'apparaissent     │
                       │    PAS dans ExecutionPlan)           │
                       └──────────────┬───────────────────────┘
                                      │
                                      ▼
                       GraphCompiler::compile  (inchangé en structure,
                                                étendu en validation)
                                      │
                                      ▼
                       ExecutionPlan  ── identique au modèle actuel
                                      │
                                      ▼
                       GraphScheduler  ── identique
                                      │
              ┌───────────────────────┴────────────────────────┐
              ▼                                                ▼
       Effets internes                                Plugin instances
       (Eq, Reverb, …)                                Arc<PluginInstance>
              │                                                │
              ▼                                                ▼
       ParameterBlock                                 PluginParamBlock
       (atomics par param)                            (atomics par param découvert
                                                       au prepare via introspection)
              │                                                │
              └─────────────► LiveParams ◄────────────────────┘
                                  ▲
                                  │
                       Automation (lanes)
                       AutomationEvaluator.advance(transport)
                       écrit dans LiveParams par block
```

### 4.4 Diagramme de coexistence

```
                  ┌───────────────────┐
                  │   AppProject      │
                  │                   │
                  │ tracks: Vec<…>    │  ← inchangé
                  │ effects: Vec<…>   │  ← variants legacy fonctionnent
                  │ modulators: …     │  ← variant Lfo legacy fonctionne
                  │                   │
                  │ automation_lanes: Vec<AutomationLane>          ← NEW (opt)
                  │ containers: Vec<Container>                     ← NEW (opt)
                  │ returns: Vec<ReturnBus>                        ← NEW (opt)
                  └─────────┬─────────┘
                            │
                            ▼
                  AudioGraph::from_project
                            │
            ┌───────────────┴────────────────┐
            ▼                                ▼
      Path legacy                      Path nouveau
      (track / effect chain)           (containers, returns,
            │                           plugins instruments,
            │                           modulators étendus)
            │                                │
            └────────────────┬───────────────┘
                             ▼
                      AudioGraph (mixte)
                      ── tous les nodes legacy ET nouveaux
                          partagent le même DAG, PortKind, scheduler
```

**Aucun « ancien moteur » et « nouveau moteur » ne coexistent**. Il y a **un seul moteur** qui supporte progressivement plus de variants.

---

## 5. Stratégie de coexistence ancien/nouveau moteur

### 5.1 Pas de fork du moteur

Refus explicite : `EngineV1` + `EngineV2` séparés. Coût de maintenance prohibitif, risque de divergence comportementale. Tout passe par **un seul** `GraphScheduler`.

### 5.2 Mécanique d'extension

Quatre techniques sont utilisées en combinaison :

**T1 — Enum élargi avec default safe.**
```rust
enum NodeKind {
    Track { … },              // legacy
    Instrument { … },         // legacy
    Plugin { … },             // legacy
    Eq { … }, Compressor { … }, Reverb { … }, Delay { … },
    LfoModulator { … },

    // === extensions ===
    Container(ContainerKind),       // dépaqueté au compile
    ReturnBus { … },
    NoteFx(NoteFxKind),
    EnvelopeModulator { … },
    MacroModulator { … },
    AudioRateModulator { … },
}
```
Tout match sur `NodeKind` reste exhaustif et explicite (pas de catch-all). Un projet legacy ne crée aucun des nouveaux variants.

**T2 — Champs `Option<…>` / `#[serde(default)]` sur `AppProject`.**
```rust
struct AppProject {
    name: String, bpm: f32, tracks: Vec<AppTrack>,  // inchangés
    #[serde(default)] automation_lanes: Vec<AutomationLane>,
    #[serde(default)] containers: Vec<Container>,
    #[serde(default)] returns: Vec<ReturnBus>,
    #[serde(default)] plugin_state_blobs: HashMap<DeviceId, Vec<u8>>,
}
```
Un projet sauvegardé en `master` se recharge avec ces champs vides → comportement strictement identique.

**T3 — Feature flag par fonctionnalité risquée.**
- `containers` — désactivé par défaut, activé via toggle settings.
- `plugin_sandbox` — désactivé par défaut, activé par device (cf. C-9).
- `sample_accurate_modulation` — désactivé jusqu'à validation perf (cf. 5.2 critical_issues).

Pas de feature flag pour les fixes critiques (3.1, 3.2, 3.4, 3.5, 8.1). Ils s'appliquent au master.

**T4 — Code path dual sur le path hot.**
Pour les paramètres effet : tant qu'un `ParameterBlock` n'est pas alloué pour un effet donné, l'écriture passe par recompile (legacy). Au fur et à mesure qu'on alloue les blocs, l'écriture bascule sur LiveParams. Mécanisme local, pas global :
```rust
fn update_effect_param(effect_id, key, value) {
    if let Some(block) = live_params.effect_params.get(effect_id) {
        block.set(key, value);    // chemin nouveau
    } else {
        // chemin legacy
        project.mutate(...); recompile_if_needed();
    }
}
```

### 5.3 Garanties de non-régression

- **Bit-identique offline** : suite de tests CI qui rejoue N projets de référence en export et compare octet par octet (cf. C-2).
- **Tests legacy gelés** : tous les tests pré-migration restent verts. Aucun changement de baseline accepté sans ADR.
- **PluginHost stub-mode** : tant que les wrappers ne déclarent pas leurs capabilities introspectées, ils tombent sur le comportement actuel (2/2, no params, no state). Pas de régression pour les projets actuels qui utilisent un plugin externe en mode opaque.

---

## 6. Mapping explicite — ancien → nouveau

### 6.1 Modèle de données

| Ancien | Nouveau | Stratégie |
|---|---|---|
| `AppTrack { clips, midi_clips, … }` (audio XOR MIDI) | `AppTrack` inchangé | inchangé ; le mode hybride passe par container |
| `AppTrack.effects: Vec<TrackEffect>` | idem | inchangé ; l'effet peut maintenant être un `Plugin` instrument-capable |
| `EffectParams::Plugin { plugin_id }` | idem | enrichi : `device_id` référence un `PluginInstance` partagé |
| `ModulatorParams::Lfo(…)` | + `Envelope`, `Macro`, `Random`, `Follower` | enum élargi |
| `ModulationTargetRef::{TrackGain, EffectParam}` | + `PluginParam { device_id, key }` | variant ajouté |
| pas d'automation | `AutomationLane { target, points: Vec<Point>, curve }` | champ nouveau |
| pas de container | `Container { kind, children: Vec<NodeId>, routing }` | champ nouveau (opt) |
| pas de return bus | `ReturnBus { id, name, effects, fader }` | champ nouveau (opt) |

### 6.2 Graphe

| Ancien | Nouveau | Stratégie |
|---|---|---|
| `Edge { from: PortId, to: PortId }` non typé | `Edge { from, to, kind: PortKind }` | `PortKind::Audio` par défaut → bit-identique |
| `num_input_ports() -> usize` | `input_ports() -> &[PortDescriptor]` (kind + channel count) | méthode ajoutée ; ancienne conservée comme `usize` aggregat |
| Modulation hors-bande (`Vec<ModulationRoute>`) | idem + arêtes `PortKind::Modulation` possibles | les deux coexistent ; le compileur normalise |
| MIDI dans `InstrumentNode` interne | + arêtes `PortKind::Midi` entre nodes | path interne préservé pour PolySynth ; path graphe pour NoteFx / plugins instruments |
| `SendNode` pass-through | `SendNode` + `ReturnBusNode` connectés via aux edges | rétrocompat : un `SendNode` sans return existant continue en pass-through |

### 6.3 Plugins

| Ancien | Nouveau | Stratégie |
|---|---|---|
| `PluginHost::instantiate → Box<dyn AudioNode>` | `instantiate → (Arc<PluginInstance>, Box<dyn AudioNode>)` | le node wrap un clone `Arc` ; host garde un clone ; GUI réutilise le même Arc |
| `DeviceInfo { num_inputs: 2, num_outputs: 2, has_midi_*: false }` hardcodé | `PluginCapabilities` lue depuis `IComponent::getBusInfo` / `clap.audio-ports` au prepare | hardcode reste comme fallback ; capabilities probées en plus |
| `ParameterMap::new()` vide | énumération via `IEditController::getParameter*` / `clap.params.*` au prepare | si vide (ancien plugin) : comportement inchangé |
| `save_state → empty / load_state → no-op` | `IComponent::getState/setState` / `clap_plugin_state.{save,load}` | implémentation via FFI ; fallback empty pour wrappers non encore migrés |
| `latency_samples() → 0` | override avec `getLatencySamples` / `clap.get_latency` | nouvelle valeur consommée par PDC (déjà implémenté côté interne) |
| `set_parameter_normalised → JSON blob` jamais lu | SPSC `ParamChange` drainée dans `process()` ; injectée via `IParameterChanges` / `clap_input_events` | path actuel reste pour devices fallback |
| Instance GUI ≠ instance audio | une seule `Arc<PluginInstance>` ; le GUI appelle des méthodes sur la même | `open_plugin_editor` refactorisé pour réutiliser l'instance existante |
| `LifecycleState` dans host ET wrapper | un seul état dans `PluginInstance` | wrapper et host lisent via l'`Arc` |
| `NodeKind::Plugin` pour effets uniquement | `NodeKind::Plugin` peut être instrument (compileur regarde `capabilities.is_instrument`) | `InstrumentNode::inner` accepte un `Arc<PluginInstance>` à la place de `PolySynth` |

### 6.4 Paramètres & modulation

| Ancien | Nouveau | Stratégie |
|---|---|---|
| `LiveParams.tracks / modulators` | + `effect_params: HashMap<EffectId, Arc<ParameterBlock>>` | extension du struct |
| Recompile à chaque change param effet | écriture dans `ParameterBlock` (atomic) lu par node | path conservé pour les effets non encore migrés ; migration variant par variant |
| Pas de queue paramètre plugin | `PluginHost.param_queues: HashMap<DeviceId, SpscQueue<ParamChange>>` | nouvelle file |
| Modulation cible TrackGain/EffectParam | + `PluginParam { device_id, param_key }` | source écrit dans la SPSC du plugin au lieu d'appeler `apply_modulation` |
| Pas d'automation | `AutomationEvaluator` consomme `AutomationLane` baked en `AutomationCurve` ; écrit dans `LiveParams` block-rate | block-rate aligne avec modulation existante |

### 6.5 MIDI

| Ancien | Nouveau | Stratégie |
|---|---|---|
| `MidiNoteSnapshot` bake samples au compile (BPM courant) | bake en **beats** + résolution en samples per-block via `TransportInfo` | corrige 2.1 (UPDATE_TEMPO désync) |
| MIDI cantonné à `InstrumentNode` | flux MIDI via `PortKind::Midi` edges | `InstrumentNode` accepte une entrée MIDI ; NoteFx insère des nodes en amont |
| Pas d'input live | `MidiInputNode` (capture device → SPSC → audio thread) | ajout source MIDI ; PortKind unifié |

### 6.6 Threading

| Ancien | Nouveau | Stratégie |
|---|---|---|
| Workers héritent priorité process | `SetThreadPriority(THREAD_PRIORITY_TIME_CRITICAL)` / `SCHED_FIFO` | applied au spawn |
| WorkerPool par scheduler (respawn à chaque recompile) | WorkerPool partagé via `Arc<WorkerPool>` dans `AudioEngine` | refactor isolé ; transparent pour le scheduler |
| `catch_unwind` Rust seulement | + `__try/__except` Win32 OU bridge sandbox out-of-process | sandbox via flag par device |

---

## 7. Points de rupture (breaking points)

Liste des choix qui **doivent** être tranchés avant migration, parce qu'ils invalident une option ailleurs.

**BP-1 — `PluginInstance` partagé via `Arc` casse l'ABI interne actuelle.**
`PluginHost::instantiate` ne retourne plus un `Box<dyn AudioNode>` mais `(Arc<PluginInstance>, Box<dyn AudioNode>)`. Toute fonction qui assume une signature en `Box` doit être adaptée. Pas de fallback possible : la double instance audio/GUI est cassée par construction.

**BP-2 — `PortKind` impose une migration des matchers exhaustifs.**
Tous les `match port.kind` deviennent exhaustifs. Le compileur valide les connexions. Les anciens `Edge { from, to }` se chargent avec `kind: PortKind::Audio` (default). Pas de fallback runtime — un projet qui aurait une arête « modulation routée comme audio » (pas possible aujourd'hui) doit être normalisé.

**BP-3 — `MidiNoteSnapshot` en beats au lieu de samples.**
Casse la sérialisation interne (pas le format projet — les `MidiNote` projet sont déjà en beats). Le compileur produit désormais des snapshots en beats, le scheduler résout en samples per-block. Test bit-identique exigé en CI.

**BP-4 — `NodeKind::Instrument.inner` accepte plugin OU PolySynth.**
Casse l'hypothèse codée en dur `inner = Box<PolySynth>`. Le compileur choisit en fonction de `capabilities.is_instrument` et du `device_id`. Migration en un patch isolé.

**BP-5 — Sandbox plugin introduit une latence par-block.**
Un device sandboxed coûte ~1 block de latence supplémentaire (round-trip shared-mem). PDC doit l'absorber → impose que la latence rapportée par le bridge soit correcte. Choix par-device, pas global.

**BP-6 — Containers sont métacompilés.**
Un `Container` n'existe **pas** dans `ExecutionPlan` ; il est déplié en sous-graphe inline au compile. Conséquence : pas d'optimisation possible « process container atomically ». Si on veut plus tard du « bypass container » sans recompile, il faut un `bypass: AtomicBool` lu par chaque step interne (mécanique 6.4).

**BP-7 — Automation écrit dans `LiveParams` block-rate.**
Choix explicite block-rate (~10 ms à 48 kHz / 512). Conséquence : automation rapide (rampes sub-block) produit du stepping audible. Le sample-accurate est repoussé (feature flag, cf. T3).

**BP-8 — `LiveParams.effect_params` allocation par effet à la compilation.**
Casse l'invariant « LiveParams immuable après init engine ». Stratégie : `effect_params` est un `Arc<RwLock<HashMap<…>>>` côté cmd, **uniquement** étendu (jamais rétréci en lecture). Lecture audio thread via clone d'`Arc<ParameterBlock>` au compile, stocké dans le node — donc lock-free au runtime.

---

## 8. Zones à migrer en dernier (risques RT)

Ordonnées du **moins risqué** (à attaquer tôt) au **plus risqué** (à différer).

### 8.1 Risque faible — fixes plugin in-process (à attaquer en premier)

- **3.1 transport paramètres UI → plugin** : SPSC `ParamChange`, drain dans `process`. Pas de modif scheduler. Risque RT nul.
- **3.4 énumération `ParameterMap`** : appel au prepare (off-RT). Risque nul.
- **3.5 save/load state** : appels au save/load projet (off-RT). Risque nul.
- **3.6 latency_samples** : valeur lue au prepare, alimente le PDC existant. Risque nul.
- **3.3 capabilities introspectées** : prepare-time. Risque nul.
- **3.11 `PluginInstance` unique** : refactor structurel (BP-1) ; pas de modif RT. Risque modéré (refactor large).

### 8.2 Risque modéré — extensions param système (après 8.1)

- **1.4 `ParameterBlock` par effet** : ajoute des atomiques par param, lu par node. Path hot inchangé. Risque RT faible si bien implémenté ; risque conception (par-effet API) modéré.
- **6.4 bypass sans recompile** : `AtomicBool` lu en début de step. Coût négligeable. Risque RT faible.
- **5.5/5.6 LFO phase reset + tempo-sync** : hook au transport restart, calcul block-rate. Risque faible.
- **5.1 ajout `Envelope`/`Macro`** : nouveaux nodes, isolés. Risque faible.

### 8.3 Risque élevé — extension graphe (différer)

- **2.2 `PortKind`** : touche le compileur et chaque node. Test bit-identique critique (BP-2). Risque conception élevé.
- **2.3 MIDI ports + 2.6 Note FX** : refactor de la chaîne MIDI. Casse l'hypothèse `InstrumentNode` monolithique. Test critique.
- **2.5 containers** : métacompilation. Risque conception élevé. Feature flag obligatoire.
- **5.7 / 6.6 automation** : nouvelle évaluation block-rate dans le scheduler. Coût CPU mesurable. Risque RT modéré.

### 8.4 Risque critique — à différer le plus possible

- **5.2 sample-accurate modulation** : refactor du scheduler pour découpage sub-block. Casse l'invariant 1-process-call-par-block. Coût CPU significatif. Feature flag obligatoire ; pas avant validation perf complète (cf. R2 audit).
- **8.1 sandbox out-of-process** : nouveau process, IPC shared-mem, watchdog, PDC adapté (BP-5). Couvre 1.5/1.6/3.11/8.1–8.4. Module critique, taille de PR contenue : prototyper sur **un seul plugin** d'abord (Vital ou Diva). Bloquant : pas avant que 8.1 in-process soit stable.
- **Worker pool priorité RT (1.7)** : platform-specific (Linux rtprio rlimit, Windows priority). Risque deadlock si mal configuré (priority inversion sur le lock du pool). À couvrir par tests d'intégration sur les 3 OS.

---

## 9. ADRs proposées

Cinq décisions critiques, formulées dans le style des ADR existants (`../decisions/`). Chacune est ratifiable indépendamment.

### ADR-006 — Un seul `PluginInstance` partagé via `Arc` (audio + GUI + host)

**Contexte.** L'architecture actuelle instancie deux copies d'un plugin pour un même effet (cf. 3.11) : une pour l'audio thread, une pour le GUI Win32. Conséquences documentées : pas de sync params, double allocation, plugins à enregistrement global cassés.

**Décision.** `PluginHost::instantiate` produit un `Arc<PluginInstance>`. Le wrapper node, le panneau GUI, et le `PluginHost` détiennent chacun un clone d'Arc. Le wrapper expose un `Box<dyn AudioNode>` qui délègue à l'instance partagée. Toutes les méthodes plugin (params, state, lifecycle, GUI) passent par ce point unique.

**Conséquence.** BP-1 et BP-4. Refactor du wrapper, du PluginHost, et de `open_plugin_editor`. Couvre 3.1, 3.4, 3.5, 3.7, 3.8, 3.11, 6.3, 8.6 (par construction).

**Alternative rejetée.** Maintenir deux instances + sync via canal de paramètres. Rejeté : double consommation CPU/RAM, plugins à licence globale incompatibles (MTS-ESP), violation du contrat VST3 et CLAP.

---

### ADR-007 — Param transport plugin via SPSC `ParamChange` drainée dans `process`

**Contexte.** Aujourd'hui (3.1) `set_parameter_normalised` écrit dans un blob JSON jamais lu par le plugin. L'automation, la modulation, le MIDI learn et le morph sont muets.

**Décision.** Chaque `PluginInstance` détient une `SpscQueue<ParamChange>` (taille préallouée, ex. 1024). L'UI / l'automation / la modulation poussent dedans côté commande ; le wrapper draine en début de `process()` et convertit en `IParameterChanges` (VST3) ou `clap_event_param_value` (CLAP). `sample_offset: 0` au premier jet (block-rate), `sample_offset: 0..block_size` quand le sample-accurate sera activé (cf. 8.4).

**Conséquence.** Couvre 3.1, 3.9, 5.4. Pas d'impact sur les invariants RT (file SPSC bornée, pas de mutex). Migration variant par variant.

**Alternative rejetée.** Recompile à chaque change paramètre plugin. Rejeté : voir 1.3/1.4/4.6 — déjà la cause de xruns sur effets internes.

---

### ADR-008 — `PortKind` typé sur le graphe, défaut `Audio` pour rétrocompat

**Contexte.** Ports non typés (2.2) → impossible de router MIDI/Modulation dans le DAG, impossible de valider les connexions au compile, sidechain inexpressible.

**Décision.** Introduction de `PortKind { Audio, Midi, Modulation, Control }` sur les `PortDescriptor`. Le compileur rejette les connexions hétérogènes au compile. Migration : tous les ports actuels deviennent `Audio` par défaut, ce qui produit un graphe strictement identique. Les modulations restent **aussi** routées hors-bande via `Vec<ModulationRoute>` (compatibilité) ; les nouvelles modulations peuvent être encodées en arête. MIDI : `InstrumentNode` accepte une entrée `Midi` ; PolySynth interne reste alimenté par snapshot tant que pas d'arête MIDI explicite.

**Conséquence.** BP-2. Couvre 2.2, 2.3, 2.6, sidechain (P0-3 / 3.3 wired). Test bit-identique en CI critique.

**Alternative rejetée.** Sous-graphes séparés audio/MIDI. Rejeté : duplique le scheduler, double les invariants, casse l'unicité de l'`ExecutionPlan`.

---

### ADR-009 — `ParameterBlock` par effet pour path hot, recompile réservé aux changements structurels

**Contexte.** 1.4 + 4.6 — `UPDATE_EFFECT` recompile le graphe complet à chaque tour de bouton sur un EQ, Compressor, Reverb, Delay, ou paramètre plugin. Cause majeure de xruns et de blocage UI.

**Décision.** Chaque node effet enregistre au `prepare` un `Arc<ParameterBlock>` dans `LiveParams.effect_params`. Les paramètres « hot » (continus, modulables) sont représentés en cellules atomiques lues par le node à chaque block. La recompile est réservée aux changements **structurels** : ajout/suppression d'effet, changement de type d'effet, changement de topologie, changement de tempo (cf. 2.1 → recompile MIDI). La classification hot/structural est explicite par effet.

**Conséquence.** Couvre 1.3, 1.4, 2.8, 4.6. BP-8 (allocation par effet à la compilation). Migration variant par variant : tant qu'un effet n'a pas son `ParameterBlock` câblé, son UPDATE continue à recompiler (legacy fallback).

**Alternative rejetée.** Diff-compile (recompiler uniquement les nodes touchés). Rejeté : invalide le snapshot atomique, complexifie le compilateur, gain marginal vs `ParameterBlock`.

---

### ADR-010 — Sandbox plugin out-of-process en opt-in par-device, jamais par défaut

**Contexte.** 1.5 + 1.6 + 8.1–8.4 — `catch_unwind` Rust ne capte pas les SEH C++ ; un crash plugin = crash du DAW. Aucun moyen d'interrompre un plugin qui boucle.

**Décision.** Un bridge `plugin_host_bridge.exe` est implémenté comme **option par-device**. La communication passe par shared-memory ring buffers (audio in/out, params, events). Watchdog en cas de timeout : le device est marqué `Dead`, le wrapper renvoie du silence, l'utilisateur est notifié. La latence supplémentaire (~1 block) est rapportée par le bridge et absorbée par le PDC interne.

Le bridge **n'est jamais activé par défaut** : les plugins fiables continuent in-process pour la perf. Le toggle est sur le device, configurable côté UI, persisté dans `AppProject++`.

**Conséquence.** Couvre 8.1–8.4. BP-5 (latence). Prototyper d'abord sur un seul plugin connu pour crasher (Vital), valider, étendre.

**Alternative rejetée.** Bridge global obligatoire (modèle Bitwig « All Plug-ins in Bridge »). Rejeté : trop coûteux par défaut, casse le bit-identique des projets existants, latence non absorbable pour live monitoring sur small buffer.

---

### ADR-011 — Automation : lanes baked en `AutomationCurve`, évaluation block-rate, écriture dans `LiveParams`

**Contexte.** 5.7 + 6.6 — pas d'automation au sens DAW. Les seules variations temporelles sont LFO (modulation) et lecture clip (transport).

**Décision.** `AutomationLane { target: ModulationTargetRef, points: Vec<AutomationPoint>, curve_kind: CurveKind }` au niveau projet. Au compile, chaque lane est bakée en `AutomationCurve` (lookup table ou polynôme par segment) stockée dans `CompiledGraph.automation_cells`. À chaque block, un `AutomationEvaluator` (appelé par `process_block` avant les nodes) avance le curseur selon `TransportInfo` et écrit la valeur dans la cellule atomique correspondante de `LiveParams`.

L'écriture par automation **partage** la cellule avec l'UI et la modulation : la dernière écriture gagne. Ordre canonique : automation puis modulation puis UI (la modulation s'ajoute à l'automation).

**Conséquence.** Couvre 5.7, 6.6. BP-7 (block-rate). Sample-accurate repoussé sous feature flag (8.4).

**Alternative rejetée.** Automation évaluée par node lui-même (pas par l'evaluator central). Rejeté : duplication de la logique de lecture transport dans chaque node, contention sur les cellules d'écriture, complexité de coordination avec la modulation.

---

### Décisions optionnelles (à ratifier selon priorité)

- **ADR-012 (proposable)** — Tempo en beats sur `MidiNoteSnapshot`, résolution en samples per-block (corrige 2.1, BP-3).
- **ADR-013 (proposable)** — `WorkerPool` partagé via `Arc<WorkerPool>` sur `AudioEngine`, priorité RT par-OS (couvre 1.7 + 4.2).
- **ADR-014 (proposable)** — Containers comme métacompilation (dépaqueté au compile en sous-graphe), pas comme nodes runtime (BP-6).

---

## 10. Vérification — chaque pièce peut être introduite sans casser l'existant

Inventaire de validation, partie par partie.

| Évolution cible | Compatible avec compile actuel ? | Compatible avec scheduler RT ? | Compatible projets sauvegardés ? | Couverture |
|---|---|---|---|---|
| `PluginInstance` Arc partagé (ADR-006) | oui — `instantiate` retourne aussi un `Box<dyn AudioNode>` au compileur | oui — wrapper délègue, pas de modif scheduler | oui — `device_id` inchangé | 3.1, 3.4–3.11, 6.3, 8.6 |
| SPSC `ParamChange` (ADR-007) | oui — file allouée au prepare | oui — drain en début de `process()`, borné, lock-free | oui — projets sans plugin = pas de file | 3.1, 3.9, 5.4 |
| `PortKind` (ADR-008) | oui — défaut `Audio` produit le DAG identique | oui — aucune modif runtime | oui — `#[serde(default)]` | 2.2, 2.3, 2.6, sidechain |
| `ParameterBlock` (ADR-009) | oui — alloué par node au prepare | oui — atomic read per-block | oui — fallback recompile tant que pas câblé | 1.3, 1.4, 2.8, 4.6 |
| Sandbox out-of-process (ADR-010) | oui — bridge derrière l'`AudioNode` | oui — ring shared-mem borné | oui — opt-in par-device, défaut in-process | 1.5, 1.6, 8.1–8.4 |
| Automation lanes (ADR-011) | oui — bake au compile | oui — evaluator block-rate écrit dans `LiveParams` | oui — `Vec::new()` par défaut | 5.7, 6.6 |
| MIDI bake en beats (ADR-012) | oui — refactor isolé du baker | oui — résolution per-block dans `process` | oui — `AppMidiClip` déjà en beats | 2.1 (UPDATE_TEMPO) |
| WorkerPool partagé (ADR-013) | oui — `Arc<WorkerPool>` sur `AudioEngine` | oui — disponible sans respawn | oui — comportement par défaut inchangé | 1.7, 4.2 |
| Containers métacompilés (ADR-014) | oui — dépaquetage avant `compute_execution_levels` | oui — invisibles à l'exécution | oui — `Vec::new()` par défaut | 2.5 |
| `Envelope` / `Macro` / `Random` / `Follower` | oui — nouveaux variants enum | oui — nodes isolés | oui — variants non utilisés ignorés | 5.1 |
| LFO tempo-sync + phase reset | oui — extension `LfoParams` | oui — calcul block-rate existant | oui — `Option<TempoSync>` | 5.5, 5.6 |
| `ReturnBus` + routage Send | oui — node ajouté, edges aux | oui — DAG standard | oui — pas de return = comportement actuel | 2.4 |
| Bypass step (`AtomicBool`) | oui — alloué au compile | oui — read 1 atomic + court-circuit | oui — défaut false | 6.4 |
| XRun télémétrie | oui — branché sur `OutputCallbackInfo` | oui — push SPSC borné | oui — opt-in via reporter existant | 1.1 |

**Conclusion.** Aucune des évolutions cibles n'exige de modification destructrice des invariants validés (DAG, snapshot atomique, scheduler RT, drop ring, pré-allocation). Toutes peuvent être introduites par variant additif, fallback legacy, ou feature flag. La migration est **incrémentale et réversible** à chaque PR.

---

## 11. Ordonnancement de migration (synthèse, hors scope détaillé)

Suggestion d'ordre, fondée sur la matrice §8 (risque) × valeur utilisateur :

1. **Vague 1 — frontière plugin (4–6 PRs)** : ADR-006 (Arc unique), ADR-007 (SPSC params), capabilities introspectées, save/load state, latency reporting. Débloque 80% du critical du §3 sans toucher au scheduler.
2. **Vague 2 — path hot effets (2–3 PRs)** : ADR-009 (`ParameterBlock` Eq → Compressor → Reverb → Delay → plugins). Supprime les xruns à l'édition.
3. **Vague 3 — tempo + MIDI cohérence (2 PRs)** : ADR-012 (beats), wiring `UPDATE_TEMPO` → recompile uniquement si nécessaire.
4. **Vague 4 — extensions modulation/automation (3–4 PRs)** : Envelope/Macro/Follower, ADR-011 (automation lanes), LFO sync.
5. **Vague 5 — extension graphe (4–5 PRs)** : ADR-008 (PortKind), MIDI edges, NoteFx, ReturnBus, ADR-014 (containers).
6. **Vague 6 — résilience (2–3 PRs)** : ADR-010 (sandbox opt-in, prototype sur Vital), ADR-013 (workers RT), XRun télémétrie.
7. **Vague 7 (différé)** — sample-accurate modulation, AU/WASM formats, oversampling sélectable, polyphase resampling.

Chaque vague est indépendamment ratifiable et livrable. Le master reste utilisable à chaque étape.
