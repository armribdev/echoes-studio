# Current Engine Diagram — Echoes Studio

> Description **structurelle** du moteur audio tel qu'il existe aujourd'hui (2026-05-31).
> Source : audits [`audio-engine-audit-2026-05-29.md`](../audits/audio-engine-audit-2026-05-29.md), [`plugin-system-audit-2026-05-31.md`](../audits/plugin-system-audit-2026-05-31.md), [`plugin-gui-audit-2026-05-31.md`](../audits/plugin-gui-audit-2026-05-31.md).
> Ce document décrit **ce qui existe**, pas ce qui devrait exister. Aucune proposition d'architecture cible.

---

## 0. Vue d'ensemble

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                            FRONTEND (Svelte / Tauri)                         │
│  Timeline · Mixer · EffectChain · PluginPicker · Knob · Settings             │
└────────────────────────────────┬─────────────────────────────────────────────┘
                                 │ Tauri IPC (JSON)
                                 ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                       RUST — COMMAND / MAIN THREAD                           │
│                                                                              │
│  engine_gateway::api     →  AppProject (mutations)                           │
│         │                          │                                         │
│         │                          ▼                                         │
│         │                  AudioGraph::from_project                          │
│         │                          │                                         │
│         │                          ▼                                         │
│         │                  GraphCompiler::compile (Kahn topo-sort)           │
│         │                          │   build_node(NodeKind::*)               │
│         │                          ▼                                         │
│         │                  PluginHost::instantiate                           │
│         │                          │  (VST3 / CLAP → Box<dyn AudioNode>)     │
│         │                          ▼                                         │
│         │                  CompiledGraph { plan, processors, meter_cells,    │
│         │                                  route_cells }                     │
│         │                          ▼                                         │
│         ▼                  RuntimeSnapshot::enqueue                          │
│  LiveParams (atomics)              │   (AtomicPtr swap + drop ring SPSC)     │
│  shared with audio thread          │                                         │
│         │                          ▼                                         │
│         │              ╔═══════════════════════════════════════╗             │
│         │              ║  AUDIO THREAD (CPAL callback)         ║             │
│         │              ║                                       ║             │
│         └─────────────►║  RuntimeSnapshot::try_swap            ║             │
│                        ║  GraphScheduler::process_block        ║             │
│                        ║   ├─ pour chaque niveau topo :        ║             │
│                        ║   │    run_steps (séquentiel ou       ║             │
│                        ║   │                 worker_pool)      ║             │
│                        ║   ├─ PDC (delay rings par port)       ║             │
│                        ║   ├─ modulation block-rate            ║             │
│                        ║   ├─ FTZ/DAZ (denormal)               ║             │
│                        ║   └─ écrit f32 interleaved → CPAL     ║             │
│                        ╚═══════════════════════════════════════╝             │
│                                                                              │
│  Editor GUI plugin (open_plugin_editor) :                                    │
│    → instancie un SECOND ClapPlugin / IComponent dans une fenêtre Win32      │
│      séparée, qui n'est PAS l'instance utilisée par l'audio thread.          │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## 1. Modèle de tracks actuel

Source : `src-tauri/src/project.rs`.

```
AppProject
 ├ name: String
 ├ bpm: f32
 ├ numerator / denominator: u8
 └ tracks: Vec<AppTrack>
            │
            ├ id, name
            ├ volume, pan, muted, solo                ◄── tous lus aussi via LiveParams (atomics)
            ├ clips: Vec<AppClip>                     ◄── audio (Arc<Vec<f32>>)
            ├ midi_clips: Vec<AppMidiClip>            ◄── MIDI (notes en beats)
            ├ midi_notes: Vec<MidiNote>               ◄── legacy track-level (backward compat)
            ├ effects: Vec<TrackEffect>               ◄── chaîne d'inserts
            ├ modulators: Vec<ProjectModulator>       ◄── LFO uniquement aujourd'hui
            └ mod_routes: Vec<ProjectModulationRoute> ◄── modulator → target
```

**Constatation structurelle clé** : un `AppTrack` est *soit* audio (`clips` non vides), *soit* MIDI (`midi_clips` non vides). La distinction est faite **au moment de la construction du graphe** (cf. §2) :

```rust
// audio_graph/graph.rs
let track_node = if !track.midi_clips.is_empty() {
    graph.add_node(NodeKind::Instrument { track_id })   // → InstrumentNode + PolySynth interne
} else {
    graph.add_node(NodeKind::Track { … })               // → TrackNode (lecture clips audio)
};
```

Il **n'existe pas** de track « hybride » MIDI+audio, ni de track sans clips utilisée uniquement comme bus.

### Effets supportés sur une track

`EffectParams` (enum tagué `kind`) :

| Variant       | Implémenté DSP | Source                                        |
|---------------|----------------|-----------------------------------------------|
| `Eq`          | ✅             | `nodes/eq.rs` (3-band, biquads cookbook RBJ, oversampling 2×) |
| `Compressor`  | ✅             | `nodes/compressor.rs` (oversampling 2×)       |
| `Reverb`      | ✅             | `nodes/reverb.rs` (Freeverb-like)             |
| `Delay`       | ✅             | `nodes/delay.rs` (ping-pong supporté)         |
| `Plugin`      | 🟡 partiel     | `nodes/…` non dédié — instancié par `PluginHost` (VST3/CLAP), traité comme `Box<dyn AudioNode>` 2 in / 2 out |

### Modulation

`ModulatorParams` enum :

```rust
pub enum ModulatorParams {
    Lfo(LfoParams),       // ← un SEUL variant existe
}
```

L'enum `ModulatorKind` côté `audio_graph/modulation.rs` énumère `Lfo | Envelope | Macro | AudioRate`, mais seul `Lfo` est buildé. `Envelope`, `Macro`, `AudioRate` sont des stubs silencieux.

`ProjectModulationRoute` cible :
- `ModulationTargetRef::TrackGain { track_id }`
- `ModulationTargetRef::EffectParam { track_id, effect_id, param }`

Pas de cible « paramètre plugin ». Pas de cible globale (master, return bus).

---

## 2. Chaîne de traitement audio

Pipeline en 4 étages :

```
AppProject ──(AudioGraph::from_project)──► AudioGraph (DAG)
                                              │
                                              │ build_node(NodeKind::…)
                                              ▼
                                          GraphCompiler::compile
                                              │  (Kahn topo-sort + compute_execution_levels)
                                              ▼
                                          ExecutionPlan (immuable)
                                              ├ steps: Vec<Step>
                                              ├ input_connections: Vec<InputConnection { delay_samples }>
                                              ├ step_levels / levels (vagues parallèles)
                                              ├ step_latencies / step_cumulative_latency
                                              └ ModulationMap → CompiledModActions
                                              │
                                              ▼
                                          CompiledGraph { plan, processors, meter_cells, route_cells }
                                              │
                                              ▼
                                          GraphScheduler (audio thread)
                                              │ process_block(transport_info, block_size)
                                              ▼
                                          AudioBuffer (planaire) → CPAL output
```

### Nœuds DSP existants

`audio_graph/nodes/` :

| Node                | Rôle                                                  | I/O ports                  |
|---------------------|-------------------------------------------------------|----------------------------|
| `TrackNode`         | Lit les clips audio d'une track (resample linéaire)   | 0 in / 1 out               |
| `TrackGainNode`     | volume × mute × solo × any_solo (lecture atomique)    | 1 in / 1 out               |
| `GainNode`          | Gain générique modulable                              | 1 in / 1 out               |
| `PanNode`           | Pan equal-power                                       | 1 in / 1 out               |
| `BusNode`           | Sommation                                             | N in / 1 out               |
| `MasterNode`        | Somme finale + soft-clip optionnel (tanh, OS 2×)      | N in / 1 out (terminal)    |
| `SendNode`          | Tap pass-through avec gain send                       | 1 in / 1 out (routage Return non implémenté) |
| `MeterNode`         | Peak metering vers `meter_cells`                      | 1 in / 1 out               |
| `InstrumentNode`    | Wrap `PolySynth` interne + flux MIDI (snapshot)       | 0 in / 1 out (stéréo)      |
| `PolySynth`         | Synthé polyphonique interne (8 voix, pan par pitch)   | enfant d'`InstrumentNode`  |
| `LfoNode`           | LFO (sine/triangle/saw/square), rate via atomique     | 0 in / 1 out (modulation)  |
| `EqNode`            | 3-band biquads (oversample 2×)                        | 1 in / 1 out               |
| `CompressorNode`    | Compresseur RMS + lookahead nul (oversample 2×)       | 1 in / 1 out               |
| `ReverbNode`        | Freeverb-like                                         | 1 in / 1 out               |
| `DelayNode`         | Stereo delay + ping-pong + feedback                   | 1 in / 1 out               |
| `Plugin` (NodeKind) | Wrap `Box<dyn AudioNode>` produit par `PluginHost`    | 2 in / 2 out (forcé)       |

### Compilation

`GraphCompiler::compile` :
1. Topo-sort (Kahn) ; rejette les cycles (`CompileError::CycleDetected`).
2. `build_node` instancie chaque processor ; pour `NodeKind::Plugin`, appelle `PluginHost::instantiate` (qui charge la lib VST3/CLAP ou retourne un `PassThroughDevice`).
3. `prepare()` appelé sur chaque processor (sample_rate, max_block_size).
4. `compute_latency_compensation` : walk topo → calcule `delay_samples` par `InputConnection`, `cumulative_latency` par step. **Note** : tous les plugins externes déclarent `latency_samples() = 0` (override jamais implémentée).
5. `compute_execution_levels` : groupe les steps par vague topologique en pliant les arêtes audio **et** modulation. Stocké dans `plan.levels`.
6. Résout chaque `ModulationRoute` → `CompiledModAction { src_step, dst_step, param_id (u32), depth_cells }`. `param_name` résolu une seule fois en `u32` via `AudioNode::param_id`.

### Ordonnancement

`GraphScheduler::run_levels` :
- Pour chaque niveau de `plan.levels` :
  - Si 1 seul step ou pool absent → exécution inline.
  - Sinon → dispatch sur `WorkerPool` (work-stealing par `AtomicUsize`, barrière Release/Acquire). Le thread audio participe à la pool (rôle worker N+1).
- Pour chaque step :
  - Gather des inputs via `input_ptr_scratch` (pré-alloué, taille `max_input_ports`).
  - PDC : push à travers une `DelayLine` ring buffer par port si `delay_samples > 0`.
  - `reset_modulation()` (inconditionnel) puis dispatch des `CompiledModAction` du block courant.
  - Appel `processor.process(ctx)` enveloppé dans un `catch_unwind` (P0-5 court terme).

### Worker pool

`audio_graph/worker_pool.rs` :
- N threads pré-spawnés et parkés, taille fixée par env `ECHOES_AUDIO_WORKERS` (défaut 0 = séquentiel).
- Dispatch par niveau ; pas de migration RT-priority sur les workers (ils héritent de la priorité du process).
- Aucune mesure de coût par step (pas d'EWMA).

---

## 3. Chaîne MIDI

```
AppTrack.midi_clips: Vec<AppMidiClip { start_s, end_s, notes: Vec<MidiNote (beats) } >
        │
        │  AT COMPILE TIME (GraphCompiler)
        │   notes converties en MidiNoteSnapshot (samples ABSOLUS, BPM courant)
        ▼
InstrumentNode {
    notes: Vec<MidiNoteSnapshot>,
    inner: Box<dyn AudioNode> = PolySynth     ◄── HARD-CODÉ (compiler.rs)
}
        │
        │  AT PROCESS TIME (audio thread, par block)
        │   1. détecte discontinuité transport (seek / reprise) → reset + NoteOn synthétiques
        │   2. filtre les notes actives dans le block → MidiEventList (capacité fixe, triée)
        │   3. inner.process_midi(events, audio_out)
        ▼
PolySynth (interne)
        │
        ▼
AudioBuffer stéréo
```

### Propriétés / limitations structurelles

- **Le MIDI ne quitte jamais `InstrumentNode`.** Pas de `MidiPort` séparé sur le graphe ; pas de routing MIDI vers un `NodeKind::Plugin`.
- **Bake samples-absolus au compile.** `MidiNoteSnapshot.start_sample / end_sample` calculés avec le BPM courant. Cf. §C6 dans `critical_issues.md`.
- **MidiEventList à capacité fixe.** Pré-allouée (zéro alloc RT). Tri stable par offset croissant, NoteOn avant NoteOff à offset égal.
- **Pas d'input MIDI live** (R3 dans l'audit) : ni clavier USB, ni MIDI réseau. Tout est joué depuis les clips.
- **Pas de Note FX** : aucun nœud ne transforme un flux MIDI avant l'instrument.

---

## 4. Routing audio / MIDI

```
Edge { from: PortId, to: PortId }
       │
       │  PortId = (NodeId, port_index)
       │  Les ports sont NON TYPÉS (audio, MIDI, modulation indistingués au niveau type)
       ▼
AudioGraph { nodes, edges, modulation_routes: Vec<ModulationRoute> }
```

### Constatations

- **Pas de `PortKind`** : `num_input_ports() / num_output_ports() -> usize`. Le compileur n'a aucun moyen de rejeter une connexion « modulation → audio in ».
- **Modulation routée hors-bande** : `ModulationRoute` vit dans un vecteur parallèle, pas comme `Edge` du graphe. Le compileur la **fold** dans le calcul des niveaux mais l'exécution lit directement le buffer du source step.
- **MIDI non routé** : le flux MIDI est purement interne à `InstrumentNode`. Le graphe n'a pas d'arête MIDI.
- **Sidechain** : aucune notion d'« input auxiliaire ». Un compresseur ducker classique n'a pas de chemin pour recevoir la sidechain.
- **Returns** : `SendNode` existe (1 in / 1 out) mais le routage vers un `ReturnNode` ou bus auxiliaire n'est pas implémenté — le send tap est pass-through.

---

## 5. Système de plugins actuel

```
                       ┌───────────────────────────────────┐
                       │  PluginHost (command-thread only) │
                       │   ├ registry: Vec<PluginInfo>     │
                       │   ├ records:  HashMap<id, DeviceRecord>
                       │   └ instantiate(device_id) → Box<dyn AudioNode>
                       └────────┬──────────────────────────┘
                                │
       ┌────────────────────────┼─────────────────────────────────┐
       ▼                        ▼                                 ▼
  Vst3Library             ClapLibrary                       PassThroughDevice
  (mod plugin_formats/    (mod plugin_formats/clap/         (fallback : utilisé pour
   vst3/)                  + cache global init/deinit         AU et WASM, et même
       │                   par chemin)                        pour VST3/CLAP en cas
       │                        │                             d'instanciation échouée)
       ▼                        ▼
  VstWrapperNode          ClapWrapperNode
   ├ DeviceInfo { num_inputs:2, num_outputs:2,
   │              has_midi_input:false, … }   ← HARD-CODÉ
   ├ params: ParameterMap::new()              ← JAMAIS PEUPLÉE
   ├ latency_samples() → 0                    ← override jamais faite
   ├ process()                                ← appelle plugin.process avec
   │                                            input_param_changes = null
   │                                            input_events = null
   └ save_state / load_state → no-op
```

### Cycle de vie d'un effet plugin

```
1. User charge un plugin via PluginPicker
2. UPDATE_EFFECT (engine_gateway) ajoute un TrackEffect { params: Plugin { plugin_id, … } }
3. recompile_if_needed → GraphCompiler::compile
4. build_node(NodeKind::Plugin) → PluginHost::instantiate(node_id, device_id)
5. Wrapper construit, prepare() appelé → injecté dans le scheduler via snapshot
6. Audio thread appelle process() à chaque block
```

### Lifecycle bicéphale

```
PluginHost                     VstWrapperNode (ou Clap)
  └ DeviceRecord                 └ self.lifecycle
      └ lifecycle:                  ├ Created → Loaded → Initialized → Running
        LifecycleState::Running     │  (machine à états dans plugin_system/lifecycle.rs)
        ← saute directement         │
          à Running                 └ jamais synchronisé avec le host
```

### GUI plugin natif

`open_plugin_editor` / `open_vst3_editor` / `open_clap_editor` (cf. `main.rs`) :

```
Click sur "ouvrir éditeur"
   │
   ▼
Win32 CreateWindowExW container (HWND owned par Echoes)
   │
   ▼
NOUVELLE instance plugin (deuxième ClapPlugin / IComponent)
   │  ← séparée de celle utilisée par l'audio thread
   ▼
plugin.gui.create / IPlugView::attached (HWND)
   │
   ▼
SetTimer 16ms → WM_TIMER → plugin.on_main_thread()
WM_APP_DRAIN (PostMessageW depuis n'importe quel thread)
```

**Conséquence structurelle** : il existe **deux instances de plugin par effet ouvert** — une qui traite l'audio, une qui anime la fenêtre. Aucune synchronisation entre les deux (paramètres, presets, état).

---

## 6. Threading model existant

```
┌─────────────────┐    ┌────────────────────┐    ┌──────────────────┐    ┌──────────────────┐
│  GUI thread     │    │  Command thread    │    │  Audio thread    │    │  Worker pool     │
│  (Tauri main /  │    │  (tokio off-main + │    │  (CPAL callback) │    │  (N threads,     │
│   WndProc Win32)│    │   Tauri commands)  │    │                  │    │   ECHOES_AUDIO_  │
│                 │    │                    │    │                  │    │   WORKERS)       │
└────────┬────────┘    └──────────┬─────────┘    └────────┬─────────┘    └────────┬─────────┘
         │                        │                       │                       │
         │   IPC (JSON)           │                       │                       │
         │ ────────────────────►  │                       │                       │
         │                        │  AppProject mutation  │                       │
         │                        │  AudioGraph rebuild   │                       │
         │                        │  GraphCompiler::compile                       │
         │                        │  RuntimeSnapshot::enqueue (AtomicPtr push)    │
         │                        │ ─────────────────►    │                       │
         │                        │                       │ try_swap (AtomicPtr   │
         │                        │                       │   .swap(null, AcqRel))│
         │                        │                       │ process_block         │
         │                        │                       │  ├ par niveau :       │
         │                        │                       │  │   dispatch ───────►│ work-stealing
         │                        │                       │  │                    │ (AtomicUsize)
         │                        │                       │  ◄────────────────────│ barrière done
         │                        │                       │                       │
         │                        │ ◄─── drop ring SPSC ──│ (ancien scheduler     │
         │                        │  drain on next enqueue│  poussé dans le ring) │
         │                        │                       │                       │
         │                        │  LiveParams (Arc<HashMap<…, Arc<…>>>)         │
         │                        │  ── écrit atomiques ──┤  lit atomiques        │
         │                        │  (Acq/Rel ou Relaxed) │  per-block            │
         │                        │                       │                       │
         │   Plugin editor :      │                       │                       │
         │   Win32 WndProc        │                       │                       │
         │   ├ WM_TIMER → tick    │                       │                       │
         │   ├ WM_APP_DRAIN       │                       │                       │
         │   │  (PostMessageW)    │                       │                       │
         │   └ plugin.on_main_thread (instance GUI)       │                       │
         │                        │                       │                       │
         │   (instance GUI ≠ instance audio — A1 GUI audit)
```

### Synchronisation cross-thread — primitives utilisées

| Primitive                                      | Usage                                                    |
|------------------------------------------------|----------------------------------------------------------|
| `RuntimeSnapshot { AtomicPtr<GraphScheduler> }`| Swap scheduler vivant ↔ scheduler en attente             |
| Ring SPSC borné (drop ring)                    | Pousse l'ancien scheduler du thread audio vers cmd thread pour drop hors-RT |
| `LiveParams` : `Arc<Mutex<HashMap<…, Arc<…>>>>`| Côté cmd ; le `HashMap` est protégé Mutex                |
| `Arc<AtomicU32 / AtomicBool / AtomicF32>`      | Cellules par-paramètre lues lock-free par audio thread   |
| `Transport` : atomiques partagés (`SharedTransport`) | Transport (play/pause/seek/bpm) entre cmd et audio |
| `WorkerPool { AtomicUsize next_idx, AtomicBool workers_done }` | Work-stealing par niveau          |
| `ClapHostShared { plugin: AtomicPtr<ClapPlugin>, host_hwnd: AtomicPtr<c_void> }` | Coord GUI ↔ tick |

### Invariants temps-réel respectés (post-fix C1/C2)

- Aucun `Mutex` dans le path `process_block`.
- Aucune allocation tas dans `process_block` (scratch, delay rings, output buffers tous pré-alloués dans `prepare` / `compile`).
- Aucun `Drop` de scheduler / nœud sur le thread audio (ring SPSC déporte le drop sur le thread commande).
- FTZ/DAZ activé sur le thread audio (`enable_flush_to_zero` à la première invocation).
- `catch_unwind` autour de `processors[i].process` (Rust panic only — pas de SEH C++).

---

## 7. Param system actuel

### LiveParams (atomiques partagés)

```
LiveParams (Arc partagé command thread ↔ audio thread)
 ├ tracks: HashMap<TrackId, Arc<TrackCells>>
 │           ├ volume:  AtomicU32   (bits-of-f32)
 │           ├ pan:     AtomicU32
 │           ├ muted:   AtomicBool
 │           └ solo:    AtomicBool
 ├ any_solo: Arc<AtomicBool>
 ├ modulators: HashMap<ModId, Arc<ModulatorCells>>
 │           └ rate_hz: AtomicU32   (lu per-block par LfoNode)
 ├ routes:     HashMap<RouteId, Arc<RouteCells>>
 │           └ depth:   AtomicU32
 └ master_soft_clip: Arc<AtomicBool>
```

### Path d'écriture (UI knob → DSP)

```
UI knob → Tauri invoke → engine_gateway handler
    │
    ├─ UPDATE_TRACK / MUTE_TRACK / SOLO_TRACK :
    │     écrit AppProject ET LiveParams.set_track_volume / set_track_muted / …
    │     PAS de recompile_if_needed
    │
    ├─ UPDATE_MODULATOR (rate / tempo_sync / beat_division) / SET_MODULATION_DEPTH :
    │     idem (cellule atomique)
    │
    ├─ UPDATE_MODULATOR (waveform LFO, réglages AudioFollower) :
    │     écrit AppProject + recompile_if_needed (valeurs figées dans le nœud)
    │
    ├─ UPDATE_EFFECT (Eq, Compressor, Reverb, Delay, Plugin) :
    │     écrit AppProject + recompile_if_needed → COMPILE COMPLET
    │     (pas de cellules par-effet ; tous les effets passent par recompile)
    │
    └─ UPDATE_TEMPO :
          écrit AppProject + applique sur SharedTransport
          MAIS PAS de recompile → désync MIDI (C6)
```

### Path de lecture (DSP)

`TrackGainNode::process` lit `volume × mute × solo × any_solo` une fois par block.
`LfoNode::process` lit `rate_hz` une fois par block.
`MasterNode::process` lit `master_soft_clip` une fois par block.
Pour les autres effets (`Eq`, `Compressor`, …) et **tous les plugins externes** : les paramètres sont gelés dans le `Box<dyn AudioNode>` au moment du `build_node` ; toute modification passe par recompile.

### Paramètres plugins externes

```
PluginHost::set_parameter_normalised(device_id, param_key, value)
   │
   ▼
record.saved_state = DeviceState::from_json(…)   ◄── stocké dans un blob JSON
   │                                                 jamais re-lu par le wrapper
   ▼
                                                 ◄── le plugin ne reçoit RIEN

Wrapper::process()
   │
   ▼
ProcessData { input_param_changes: null, input_events: null, … }
```

→ **Aucun path UI → plugin DSP n'existe**. Cf. critical_issues §3.1.

---

## 8. Automation actuelle

**Il n'existe aucun système d'automation au sens DAW** (lanes par paramètre, points temporels, courbes interpolées). Les seules variations temporelles sont :

1. **LFO** : `LfoNode` génère un signal `[-1, +1]` lu par les routes de modulation.
2. **Routes de modulation** : `ProjectModulationRoute` connecte un modulator à une cible (`TrackGain` ou `EffectParam` interne).
3. **Transport-driven** : changement de position de playhead joue les clips audio / déclenche les notes MIDI.

```
ProjectModulator (LFO)
       │
       │  compile → ModulationRoute → CompiledModAction { src_step, dst_step, param_id, depth_cells }
       ▼
GraphScheduler.run_steps :
       │
       │  par block, pour chaque action :
       │   1. lit le buffer de sortie du src_step (block-rate, première frame)
       │   2. multiplie par depth (atomic) → delta
       │   3. dst_node.apply_modulation(param_id, delta)
       ▼
Node modulé : applique le delta sur son paramètre interne le temps d'un block
```

### Limitations structurelles

- **Block-rate uniquement** : la modulation est échantillonnée 1× par block (~10 ms à 48 kHz / 512). Pas de sample-accurate (R2 dans l'audit).
- **Aucune cible plugin** : `ModulationTargetRef` ne sait pointer que vers `TrackGain` ou `EffectParam` d'un effet interne. Pas de path vers un paramètre VST3/CLAP.
- **PDC ignoré pour la modulation** : le scheduler lit le buffer du source step sans alignement PDC (concession explicite, cf. I4 reste).
- **Pas d'enveloppe** : `Envelope` est dans l'enum `ModulatorKind` mais le variant n'existe pas dans `ModulatorParams`. Pas d'ADSR triggerable par note.
- **Pas de macro / random / follower** : idem, énumérés mais non implémentés.
- **Pas de modulator global** : les modulators vivent au niveau track (`AppTrack.modulators`), pas au niveau projet / master / return.

---

## 9. Sidechain actuel

**Inexistant** au niveau architecture.

```
Structure actuelle d'un nœud Compressor :
   CompressorNode { 1 input port (audio), 1 output port (audio) }
                                ▲
                                │
                  pas de notion d'input « aux »

Structure actuelle d'un wrapper plugin :
   VstWrapperNode { num_inputs: 2 (forcé), num_outputs: 2 (forcé) }
                                ▲
                                │
                  pas d'introspection des bus aux (sidechain)
                  pas de bus auxiliaire dans le graphe
```

Conséquences structurelles :

- **Pas de port sidechain typé** : aucune notion `is_sidechain: bool` ni `is_aux: bool` sur les ports.
- **Pas de bus auxiliaire dans le graphe** : `Edge` ne sait pas distinguer une connexion main d'une connexion sidechain.
- **Plugins avec sidechain** : leur deuxième bus n'est jamais câblé. Compresseurs ducker, vocoders, gates triggered — tous inutilisables en mode sidechain.
- **Compresseur interne** : `CompressorNode` détecte l'enveloppe sur sa propre entrée uniquement (pas de detection externe).

---

## 10. Gestion buffers / audio callback

### Pré-allocation

Tout est pré-alloué à la compilation du `GraphScheduler` :

```
GraphScheduler {
  plan: Arc<ExecutionPlan>,                              ← immuable
  processors: Vec<Box<dyn AudioNode>>,                   ← un par step
  output_buffers: Vec<AudioBuffer>,                      ← un par step, alloué dans prepare()
  delay_lines: HashMap<(step, port), DelayLine>,         ← un par InputConnection à delay > 0
                                                            (DelayLine = ring buffer par canal +
                                                             AudioBuffer scratch de sortie)
  input_ptr_scratch: Vec<*const AudioBuffer>,            ← taille = max(MIN_INPUT_PTR_SCRATCH,
                                                                       plan.max_input_ports())
  mod_actions_by_step: Vec<Vec<CompiledModAction>>,      ← un Vec par step
  worker_input_rows: Vec<Vec<*const AudioBuffer>>,       ← (num_workers + 1) lignes de scratch
                                                            d'input (pas de contention cache)
  meter_cells: Vec<Arc<AtomicU32>>,                      ← partagé avec le PositionReporter
}
```

`AudioBuffer` est **planaire** (channels séparés), capacité fixée à `max_block_size`.

### Audio callback (CPAL)

```
build_stream<T> closure :
  on first call : enable_flush_to_zero() (FTZ + DAZ via MXCSR / FPCR)
  on each block :
    1. snapshot.try_swap(&mut scheduler)
       ── si pending : prend l'AtomicPtr, scheduler remplacé in-place,
                       ancien poussé dans le drop ring SPSC
    2. transport.snapshot() → TransportInfo (atomiques)
    3. scheduler.process_block(transport_info, frames)
    4. master buffer planaire → interleaving manuel → &mut [T] CPAL
    5. position reporter (~30 Hz throttled, payloads typés réutilisés)
```

### Scheduler `process_block`

```
process_block(transport, frames) :
  for level in plan.levels :
    if level.len() == 1 || worker_pool absent :
      for step in level : process_one_step(step, worker_id=N)
    else :
      worker_pool.dispatch(level)
      participate as worker N
      barrier wait
  return master output buffer

process_one_step(step) :
  ── gather inputs :
    for c in plan.input_connections[step.input_range] :
      if c.delay_samples > 0 :
        push input through delay_lines[(step, c.dst_port)]
        input_ptr_scratch[port] = &delay_line.output_scratch
      else :
        input_ptr_scratch[port] = &processed[c.src_step]
  ── reset modulation (inconditionnel) + dispatch :
    processor[step].reset_modulation()
    for action in mod_actions_by_step[step] :
      let delta = read(processed[action.src_step].channel(0)[0]) * action.depth.load()
      processor[step].apply_modulation(action.param_id, delta)
  ── process avec catch_unwind :
    catch_unwind(|| processor[step].process(ProcessContext { inputs: input_ptr_scratch, output: &mut outputs[0], transport, … }))
```

### Export offline

`audio_engine::offline::render_project_offline` :
- Compile un `GraphScheduler` éphémère via le **même** `AudioGraphCompiler::compile_scheduler` que la live.
- Pas d'interaction avec le stream CPAL.
- Tourne sur le thread commande → peut allouer librement.
- `TransportInfo` synthétique avec tempo statique du projet (note : héritera de la correction de C6 si appliquée).

---

## 11. Synthèse — ce qui existe vs ce qui n'existe pas

| Sous-système                             | État                                                                |
|------------------------------------------|---------------------------------------------------------------------|
| DAG audio (nodes + edges, Kahn)          | ✅ implémenté                                                       |
| ExecutionPlan immuable                   | ✅                                                                  |
| Snapshot atomique (AtomicPtr + drop ring)| ✅                                                                  |
| LiveParams (atomics) pour track basics   | ✅ (volume, pan, mute, solo, any_solo, mod rate/depth, soft clip)   |
| Worker pool parallèle                    | ✅ (off par défaut, `ECHOES_AUDIO_WORKERS`)                         |
| PDC (delay rings par port)               | ✅ pour nodes internes — ❌ ignoré par plugins (latence toujours 0) |
| Modulation block-rate (LFO)              | ✅                                                                  |
| Envelope / Macro / Random / Follower     | ❌ (énumérés, non implémentés)                                      |
| FTZ/DAZ + anti-denormal                  | ✅                                                                  |
| MIDI : clips, snapshot, PolySynth interne| ✅                                                                  |
| MIDI live input                          | ❌ (R3 backlog)                                                     |
| MIDI vers plugins externes               | ❌ (`InstrumentNode` hardcode `PolySynth`)                          |
| Note FX (transpose, arp, chord, …)       | ❌                                                                  |
| Plugins VST3/CLAP : chargement audio     | 🟡 (instanciés, traités, mais opaques)                              |
| Plugins : paramètres UI → DSP            | ❌ (`input_param_changes = null`)                                   |
| Plugins : énumération des paramètres     | ❌ (`ParameterMap::new()` vide)                                     |
| Plugins : save / load state              | ❌ (no-op)                                                          |
| Plugins : ports introspectés (mono/sur)  | ❌ (forcé 2/2)                                                      |
| Plugins : sidechain                      | ❌                                                                  |
| Plugins : latence rapportée              | ❌                                                                  |
| Plugins : instance unique audio + GUI    | ❌ (deux instances séparées, A1)                                    |
| Plugins : sandboxing process             | ❌ (`catch_unwind` Rust seulement, pas SEH C++)                     |
| Returns / Sends routés                   | 🟡 (`SendNode` existe, routage Return non câblé)                    |
| Containers (Chain, Layer, Split)         | ❌                                                                  |
| Automation (lanes temporelles)           | ❌                                                                  |
| Export offline via moteur unifié         | ✅                                                                  |
| Détection XRun + télémétrie              | ❌ (`OutputCallbackInfo` ignoré)                                    |

---

## 12. Points d'entrée code (références rapides)

| Concern                       | Fichier                                              |
|-------------------------------|------------------------------------------------------|
| Bootstrap stream CPAL         | [src-tauri/src/main.rs:623](../src-tauri/src/main.rs#L623) `start_native_audio` |
| Audio callback                | [src-tauri/src/audio_engine/engine.rs:488](../src-tauri/src/audio_engine/engine.rs#L488) `build_stream` |
| Snapshot swap (RT)            | [src-tauri/src/audio_graph/snapshot.rs](../src-tauri/src/audio_graph/snapshot.rs) |
| Compilation graphe            | [src-tauri/src/audio_graph/compiler.rs](../src-tauri/src/audio_graph/compiler.rs) |
| Exécution graphe              | [src-tauri/src/audio_graph/scheduler.rs](../src-tauri/src/audio_graph/scheduler.rs) |
| Worker pool                   | [src-tauri/src/audio_graph/worker_pool.rs](../src-tauri/src/audio_graph/worker_pool.rs) |
| ExecutionPlan                 | [src-tauri/src/audio_graph/execution_plan.rs](../src-tauri/src/audio_graph/execution_plan.rs) |
| Modulation                    | [src-tauri/src/audio_graph/modulation.rs](../src-tauri/src/audio_graph/modulation.rs) |
| LiveParams                    | [src-tauri/src/audio_graph/live_params.rs](../src-tauri/src/audio_graph/live_params.rs) |
| MIDI types                    | [src-tauri/src/audio_graph/midi.rs](../src-tauri/src/audio_graph/midi.rs) |
| Instrument / PolySynth        | [src-tauri/src/audio_graph/nodes/instrument.rs](../src-tauri/src/audio_graph/nodes/instrument.rs), [src-tauri/src/audio_graph/nodes/poly_synth.rs](../src-tauri/src/audio_graph/nodes/poly_synth.rs) |
| Plugin host                   | [src-tauri/src/plugin_system/host.rs](../src-tauri/src/plugin_system/host.rs) |
| Plugin capabilities (Phase 1) | [src-tauri/src/plugin_system/capabilities.rs](../src-tauri/src/plugin_system/capabilities.rs) |
| Wrapper VST3                  | [src-tauri/src/plugin_formats/vst3/wrapper.rs](../src-tauri/src/plugin_formats/vst3/wrapper.rs) |
| Wrapper CLAP                  | [src-tauri/src/plugin_formats/clap/wrapper.rs](../src-tauri/src/plugin_formats/clap/wrapper.rs) |
| GUI plugin (Win32)            | [src-tauri/src/plugin_system/gui.rs](../src-tauri/src/plugin_system/gui.rs) |
| Engine gateway (IPC)          | [src-tauri/src/engine_gateway/api.rs](../src-tauri/src/engine_gateway/api.rs) |
| Transport partagé             | [src-tauri/src/audio_engine/transport.rs](../src-tauri/src/audio_engine/transport.rs) |
| Export offline                | [src-tauri/src/audio_engine/offline.rs](../src-tauri/src/audio_engine/offline.rs) |
