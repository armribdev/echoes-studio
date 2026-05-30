# Audio Engine Audit — 2026-05-29

Auditeur : Claude (Opus 4.7)
Portée : `src-tauri/src/audio_engine/`, `src-tauri/src/audio_graph/`, `src-tauri/src/plugin_system/`, `src-tauri/src/plugin_formats/`, intégration via `src-tauri/src/engine_gateway/`.

---

## Verdict

Architecture du graphe **bien structurée au niveau des types** : séparation editor / compile / execute propre, plans immuables, swap atomique du scheduler, trait de nœud agnostique du format de plugin. Les fondations sont saines.

L'implémentation contient **deux violations sérieuses de la sécurité temps-réel** (drop de l'allocateur sur le thread audio, mutex sur le thread audio) et un **problème de pattern de performance fondamental** (reconstruction complète du graphe + clone des samples de clips à chaque changement de paramètre). Le sous-système MIDI a des bugs de timing audibles sur du matériel réel. Plusieurs nœuds DSP ont des problèmes de correction. Plugins, sends/returns, compensation de latence, exécution parallèle, MIDI live et rendu offline via le moteur ne sont pas implémentés.

C'est un v0.1 compétent avec des fondations solides, mais **pas encore au niveau "DAW professionnel"**. La plupart des problèmes critiques sont localisés et corrigeables sans re-architecturer.

---

## Cartographie de la codebase

### Points d'entrée / cycle de vie
- [src-tauri/src/main.rs:623](../../../src-tauri/src/main.rs#L623) `start_native_audio` — bootstrap du stream CPAL au lancement.
- [src-tauri/src/audio_engine/engine.rs:215](../../../src-tauri/src/audio_engine/engine.rs#L215) `AudioEngine::start_stream_with_settings` — possède le stream CPAL, swap atomique des schedulers.
- [src-tauri/src/audio_engine/engine.rs:488](../../../src-tauri/src/audio_engine/engine.rs#L488) `build_stream<T>` — closure du callback audio.

### Cœur DSP temps-réel (`src-tauri/src/audio_graph/`)
- `node.rs` — trait `AudioNode` (ports, `prepare`, `process`, hooks de modulation).
- `buffer.rs` — `AudioBuffer` planaire, pré-alloué.
- `graph.rs` — `AudioGraph` côté éditeur (nœuds, arêtes, modulation map). `from_project()` construit la topologie complète.
- `compiler.rs` — `GraphCompiler` (Kahn topo-sort) → `CompiledGraph { plan, processors, meter_cells }`.
- `scheduler.rs` — `GraphScheduler::process_block`, le driver côté thread RT.
- `snapshot.rs` — `RuntimeSnapshot` (slot unique `Mutex<Option<GraphScheduler>>`).
- `execution_plan.rs` — `ExecutionPlan` immuable (steps + input connections + ModulationMap).
- `modulation.rs` — types `ModulationRoute` / `ModulationMap`.
- `midi.rs` — `MidiEvent` / `MidiEventList` à capacité fixe / `MidiNoteSnapshot`.
- `nodes/*` — `AudioNode`s concrets : track, gain, pan, bus, master, send, meter, instrument, poly_synth, modulator (LFO), eq, compressor, reverb, delay.

### Bridge / state
- [src-tauri/src/audio_engine/graph.rs:40](../../../src-tauri/src/audio_engine/graph.rs#L40) `AudioGraphCompiler::compile_scheduler`.
- [src-tauri/src/audio_engine/transport.rs](../../../src-tauri/src/audio_engine/transport.rs) `SharedTransport` (atomiques partagés command ↔ audio).
- [src-tauri/src/engine_gateway/api.rs](../../../src-tauri/src/engine_gateway/api.rs) `send_command` / `send_batch` — appelle `recompile_if_needed` après presque chaque commande.
- [src-tauri/src/plugin_system/](../../../src-tauri/src/plugin_system/) — `PluginHost`, `Device`, etc. **Actuellement déconnecté du compiler.**
- [src-tauri/src/plugin_formats/](../../../src-tauri/src/plugin_formats/) — stubs VST3/AU/CLAP/WASM, non implémentés.

---

## Issues — par priorité

### CRITIQUES (à corriger avant v1)

#### C1 — `GraphScheduler` droppé sur le thread audio ✅ **CORRIGÉ**
**Fichiers** : [audio_engine/engine.rs:493](../../../src-tauri/src/audio_engine/engine.rs#L493), [audio_graph/snapshot.rs](../../../src-tauri/src/audio_graph/snapshot.rs)
Quand `snapshot.try_take()` retournait `Some(new)`, l'ancien scheduler était remplacé et droppé sur place. Cela désallouait chaque nœud, chaque output buffer, chaque `Arc<Vec<f32>>` de samples (si dernière ref), chaque buffer interne de Reverb. **C'était une désallocation tas sur le thread audio**, en contradiction directe avec l'invariant déclaré du fichier.
**Fix appliqué** : `RuntimeSnapshot` réécrit avec `AtomicPtr<GraphScheduler>` + ring SPSC borné. Nouvelle API `try_swap(&mut Box<GraphScheduler>)` : remplace in-place le scheduler du thread audio et pousse le déchu dans le drop ring. Le thread de commande draine l'anneau à chaque `enqueue` et fait les `drop` hors RT.

#### C2 — `std::sync::Mutex::try_lock()` sur le thread audio ✅ **CORRIGÉ**
**Fichiers** : [audio_graph/snapshot.rs](../../../src-tauri/src/audio_graph/snapshot.rs)
Sur Windows passait par `AcquireSRWLockExclusive` — pas strictement lock-free. Le contrat "aucune primitive bloquante sur le thread audio" n'était pas tenu. Plus aucun `Mutex` dans `RuntimeSnapshot` : la slot pending est un `AtomicPtr<GraphScheduler>`, lu via `swap(null, AcqRel)` sur le thread audio. Résolu en même temps que C1.

#### C3 — Recompilation complète sur chaque changement de paramètre ✅ **CORRIGÉ**
**Fichiers** : [engine_gateway/api.rs](../../../src-tauri/src/engine_gateway/api.rs) (tous les handlers), [audio_graph/compiler.rs:368](../../../src-tauri/src/audio_graph/compiler.rs#L368)
`UPDATE_TRACK` (volume/pan), `UPDATE_EFFECT`, `MUTE_TRACK`, `SOLO_TRACK`, `SET_MODULATION_DEPTH`, rate de modulateur — tous appellent `recompile_if_needed()`. Chaque recompile :
- Reconstruit l'`AudioGraph` complet.
- Instancie chaque nœud (`build_node`).
- Appelle `prepare` sur chaque nœud (Reverb réalloue tous ses 24 comb+allpass, EQ recalcule ses coefs, compressor reset l'enveloppe).
- **Clone chaque `Vec<f32>` de samples de clip** (`c.raw_samples.clone()` à [compiler.rs:368](../../../src-tauri/src/audio_graph/compiler.rs#L368)).

Pour un projet stéréo de 5 minutes : ~115 MB clonés par tour de bouton, plus des queues de reverb/delay cassées. Le pattern DAW correct = atomiques par-paramètre lus par `process_block`, recompile réservée aux changements **structurels**.

**Fix appliqué** :
- Nouveau registre [`LiveParams`](../../../src-tauri/src/audio_graph/live_params.rs) : `HashMap<String, Arc<…>>` sous `Mutex` côté commande, exposant des `AtomicU32` / `AtomicBool` lus lock-free par le thread audio. Une instance partagée `Arc<LiveParams>` vit dans `AppState` et l'`AudioEngine`.
- Nouveau nœud [`TrackGainNode`](../../../src-tauri/src/audio_graph/nodes/track_gain.rs) qui lit `volume × mute × solo × any_solo` depuis ses atomiques par-block (remplace `GainNode` pour `NodeKind::TrackGain`).
- [`LfoNode`](../../../src-tauri/src/audio_graph/nodes/modulator.rs) accepte un `Arc<ModulatorCells>` via `with_rate_cells()` et lit `rate_hz` per-block.
- `CompiledModAction.depth` devient `depth_cells: Option<Arc<RouteCells>>` lu par block dans [scheduler.rs:317](../../../src-tauri/src/audio_graph/scheduler.rs#L317). Le compilateur résout les cellules par `route_id` (`ModulationRoute.route_id: Option<String>` ajouté) et les transporte via `CompiledGraph.route_cells`.
- Handlers gateway hot-path (`UPDATE_TRACK`, `MUTE_TRACK`, `SOLO_TRACK`, `UPDATE_MODULATOR.rate`, `SET_MODULATION_DEPTH`) : écrivent le projet **et** l'atomique correspondant ; **plus de `recompile_if_needed`**.
- `LiveParams::prune_to_project` à chaque recompile structurelle jette les entrées orphelines et rafraîchit `any_solo` sans clobberiser les atomiques existants — un knob turn en vol survit à une recompile concurrente.
- I1 réglé en parallèle (voir ci-dessous) : la recompile fait `Arc::clone` au lieu de cloner les samples.

**Restes hors scope (à traiter séparément)** :
- `UPDATE_EFFECT` continue à recompiler — chaque type d'effet a son propre lot de params ; route séparée à concevoir (atomiques typés par effet ou `ParameterBlock` générique).
- `UPDATE_TEMPO` traité indépendamment (C6).

#### C4 — Limite dure de 32 pistes (panic OOB) ✅ **CORRIGÉ**
**Fichiers** : [audio_graph/scheduler.rs:42](../../../src-tauri/src/audio_graph/scheduler.rs#L42), [audio_graph/scheduler.rs:241](../../../src-tauri/src/audio_graph/scheduler.rs#L241), [audio_graph/graph.rs:237](../../../src-tauri/src/audio_graph/graph.rs#L237)
`MAX_NODE_INPUT_PORTS = 32` mais le master a `num_inputs = project.tracks.len()`. Au-delà de 32 pistes, `self.input_ptr_scratch[port]` panic. Plafond produit dur.
**Fix appliqué** : `ExecutionPlan::max_input_ports()` + dimensionnement de `input_ptr_scratch` à `max(MIN_INPUT_PTR_SCRATCH, plan.max_input_ports())` dans `GraphScheduler::new`.

#### C5 — Événements MIDI non triés + perte de notes au seek ✅ **CORRIGÉ**
**Fichiers** : [audio_graph/nodes/instrument.rs:74](../../../src-tauri/src/audio_graph/nodes/instrument.rs#L74), [audio_graph/nodes/poly_synth.rs:216](../../../src-tauri/src/audio_graph/nodes/poly_synth.rs#L216), [audio_graph/midi.rs](../../../src-tauri/src/audio_graph/midi.rs)
- `InstrumentNode::process` itère `self.notes.iter()` sans tri. Si deux notes se chevauchent dans un block, l'ordre des events est arbitraire. `PolySynth::process_midi` exige un tri par offset croissant (`while events[ev_idx].offset <= frame`) — sinon des NoteOn sont déclenchés en retard, voire perdus.
- `note.start_sample >= block_start && note.start_sample < block_end` : si on seek le playhead **après** le start d'une note encore active, le NoteOn n'est jamais joué → silence.

**Fix appliqué** : `MidiEventList::sort_by_offset()` (tri in-place, NoteOn avant NoteOff à offset égal). `InstrumentNode` détecte les discontinuités transport (seek, reprise après pause/stop) via `next_expected_block_start` + `was_playing` ; reset du synthé + NoteOn synthétiques à offset 0 pour les notes actives (`start_sample < pos < end_sample`), puis tri avant `process_midi`.

#### C6 — Changement de tempo désynchronise silencieusement le MIDI
**Fichiers** : [engine_gateway/api.rs:564](../../../src-tauri/src/engine_gateway/api.rs#L564), [audio_graph/compiler.rs:330](../../../src-tauri/src/audio_graph/compiler.rs#L330)
`MidiNoteSnapshot` est baké en samples absolus à la compilation avec le BPM courant. `UPDATE_TEMPO` n'appelle pas `recompile_if_needed`. Changer le BPM après avoir placé des notes les laisse jouer à l'ancien tempo jusqu'à ce qu'autre chose force un recompile.
**Fix** : appeler `recompile_if_needed(state)` dans le handler `UPDATE_TEMPO`, ou stocker les notes en beats et convertir par block.

---

### IMPORTANTES (qualité / scaling)

#### I1 — Clone des `raw_samples` à chaque recompile ✅ **CORRIGÉ**
**Fichiers** : [audio_graph/compiler.rs:368](../../../src-tauri/src/audio_graph/compiler.rs#L368), [project.rs:198](../../../src-tauri/src/project.rs#L198)
`AppClip::raw_samples: Vec<f32>` au lieu de `Arc<Vec<f32>>`. À changer en `Arc<Vec<f32>>` partagé entre projet et `ClipData`. (Lié à C3.)

**Fix appliqué** : `AppClip::raw_samples`, `CachedAudioData::raw_samples`, `AudioClipData::raw_samples` et `ClipDto::raw_samples` passent à `Arc<Vec<f32>>` (sérialisation JSON via helper `serialize_arc_vec_f32`). `GraphCompiler::build_node` fait `StdArc::clone(&c.raw_samples)` au lieu de `StdArc::new(c.raw_samples.clone())` — plus aucun deep-clone des samples décodés à la recompile.

#### I2 — Coefficients shelving EQ buggés ✅ **CORRIGÉ**
**Fichiers** : [audio_graph/nodes/eq.rs:28](../../../src-tauri/src/audio_graph/nodes/eq.rs#L28), [audio_graph/nodes/eq.rs:56](../../../src-tauri/src/audio_graph/nodes/eq.rs#L56)
`alpha = w.sin() / 2.0 * ((a + 1.0/a) * (1.0/1.0 - 1.0) + 2.0).sqrt()` — `(1.0/1.0 - 1.0) == 0.0`, donc le terme "shelf-S" disparaît. Réponse incorrecte. Comparer au cookbook RBJ.
**Fix appliqué** : helper `Biquad::shelf_alpha(w, a, shelf_slope)` conforme au cookbook RBJ (`(1/S - 1)`), pente par défaut `S = 1.0`. Tests unitaires sur gain DC, demi-gain à f₀ et dépendance à S.

#### I3 — Resampling nearest-neighbour dans `TrackNode` ✅ **CORRIGÉ**
**Fichiers** : [audio_graph/nodes/track.rs:113](../../../src-tauri/src/audio_graph/nodes/track.rs#L113)
Mélanger des clips 44.1 kHz à 48 kHz produit un aliasing audible.
**Fix appliqué** : interpolation linéaire entre `samples[idx0]` et `samples[idx0+1]` avec fraction `frac = src_pos - floor(src_pos)`. Quand les taux sont identiques (`sr_ratio == 1.0`), `frac == 0.0` et le comportement est identique à l'avant. Reste idéalement à remplacer par un resampler polyphase pour une qualité broadcast.

#### I4 — Pas de compensation de latence (PDC) ✅ **CORRIGÉ**
**Fichiers** : [audio_graph/node.rs:168](../../../src-tauri/src/audio_graph/node.rs#L168), [audio_graph/execution_plan.rs](../../../src-tauri/src/audio_graph/execution_plan.rs), [audio_graph/compiler.rs](../../../src-tauri/src/audio_graph/compiler.rs), [audio_graph/scheduler.rs](../../../src-tauri/src/audio_graph/scheduler.rs)
`latency_samples()` existait mais n'était jamais lu. Sur un graphe avec un chemin contenant un nœud à latence (lookahead, linear-phase EQ), les chemins parallèles arrivaient à la sommation décalés.
**Fix appliqué** :
- `InputConnection` gagne un champ `delay_samples: u32`. `ExecutionPlan` expose `step_latencies` et `step_cumulative_latency`, plus `total_latency_samples()`.
- `GraphCompiler::compute_latency_compensation` (appelé après l'instanciation des nœuds) marche le plan en ordre topologique : pour chaque step, `arrival = max(cumulative_latency[c.src_step])` sur les inputs, chaque `delay_samples = arrival - cumulative_latency[c.src_step]`, puis `cumulative_latency[i] = arrival + latency_samples()`.
- `GraphScheduler` pré-alloue une `DelayLine` (ring buffer par canal + `AudioBuffer` de sortie scratch) pour chaque `(step, port)` avec `delay_samples > 0`. `run_steps` pousse l'input gathered à travers la delay line et redirige le pointeur de scratch vers le buffer de sortie. Aucune allocation sur le thread audio.
- Tests : `pdc_aligns_two_parallel_paths`, `pdc_zero_latency_is_passthrough`, `pdc_delay_carries_across_block_boundary` dans [scheduler.rs](../../../src-tauri/src/audio_graph/scheduler.rs) valident l'alignement d'impulsions, le pass-through quand aucun nœud n'a de latence, et la persistance correcte du delay au travers des frontières de block.

**Restes** : la modulation lit toujours le block-rate du source step sans alignement PDC (concession Phase 3 / R2). Acceptable tant que la modulation reste block-rate ; à revoir quand la modulation sample-accurate sera implémentée.

#### I5 — Mute/solo via recompile complet ✅ **CORRIGÉ** (avec C3)
**Fichiers** : [engine_gateway/api.rs:348](../../../src-tauri/src/engine_gateway/api.rs#L348), [engine_gateway/api.rs:378](../../../src-tauri/src/engine_gateway/api.rs#L378), [audio_graph/nodes/track_gain.rs](../../../src-tauri/src/audio_graph/nodes/track_gain.rs)
Cas particulier de C3, résolu par le même fix : `TrackGainNode` lit `volume × mute × solo × any_solo` depuis ses atomiques par-block. Les handlers `MUTE_TRACK` et `SOLO_TRACK` flippent ces atomiques (`LiveParams::set_track_muted`, `set_track_solo_exclusive`) **sans appeler `recompile_if_needed`**. Le solo est exclusif (un seul track à la fois) et met à jour le flag global `any_solo` consulté par toutes les pistes.

#### I6 — Reset modulation conditionnel ✅ **CORRIGÉ**
**Fichiers** : [audio_graph/scheduler.rs:428](../../../src-tauri/src/audio_graph/scheduler.rs#L428), [audio_graph/nodes/gain.rs:60](../../../src-tauri/src/audio_graph/nodes/gain.rs#L60)
`reset_modulation()` n'était appelé que si `!actions.is_empty()`. Si une route est retirée en cours de session (ou si la liste d'actions effective devient vide entre blocks pour une autre raison), le dernier `mod_delta` du block précédent persistait.
**Fix appliqué** : `reset_modulation()` est désormais appelé inconditionnellement pour chaque step avant la dispatch des actions. L'impl par défaut du trait étant un no-op, le surcoût est nul pour les nœuds sans paramètres modulés ; pour les nœuds modulables, la dépense est une simple écriture à zéro d'un `f32` par paramètre.

#### I7 — `PluginHost` totalement déconnecté ✅ **CORRIGÉ**
**Fichiers** : [plugin_system/host.rs:101](../../../src-tauri/src/plugin_system/host.rs#L101), [audio_graph/compiler.rs](../../../src-tauri/src/audio_graph/compiler.rs), [audio_engine/graph.rs](../../../src-tauri/src/audio_engine/graph.rs), [audio_engine/engine.rs](../../../src-tauri/src/audio_engine/engine.rs)
`NodeKind::Plugin` retournait toujours un `GainNode` placeholder. `PluginHost::instantiate` n'était jamais appelé. Toute l'abstraction plugin était un no-op.

**Fix appliqué** :
- `AudioEngine` possède désormais un `PluginHost` (champ `plugin_host: PluginHost`), construit dans `AudioEngine::new`. Vit sur le thread commande ; jamais touché par le thread audio (les `Box<dyn AudioNode>` sont déplacés dans le `GraphScheduler` à la compilation).
- `AudioGraphCompiler::compile_scheduler` et `GraphCompiler::compile` acceptent `&mut PluginHost`, threaded jusqu'à `build_node`.
- Le bras `NodeKind::Plugin` de `build_node` appelle `plugin_host.instantiate(node_id, device_id)`. Sur succès : le `Box<dyn AudioNode>` retourné par le host (interne → `PassThroughDevice` ; VST3/AU/CLAP/WASM → encore `PassThroughDevice` en attendant les wrappers réels) est inséré dans le graphe ; un `DeviceRecord` est enregistré côté host pour le suivi de lifecycle et l'undo des paramètres. Sur échec (registry miss / instanciation échouée) : fallback `GainNode` unitaire pour que le graphe compile quand même, plus log d'erreur.
- `update_from_project` et `start_stream_with_settings` passent `&mut self.plugin_host` au compile.

**Restes hors scope** :
- Les wrappers VST3/AU/CLAP/WASM eux-mêmes restent des stubs : `PluginHost::instantiate` retourne un `PassThroughDevice` pour ces formats. Le chemin est désormais branché — il suffit d'implémenter chaque wrapper et de remplacer le fallback dans `host.rs`.
- `num_inputs_for_kind(NodeKind::Plugin)` est encore figé à 2/2 ; à terme il faudra lire `PluginManifest.num_audio_inputs/outputs` au moment de la compilation pour respecter les configurations mono / 5.1 / etc.
- L'écriture des paramètres (`PluginHost::set_parameter_normalised`) n'est pas encore routée depuis le gateway. Idem pour la sérialisation d'état au save/load projet.

#### I8 — L'export bypass le moteur ✅ **CORRIGÉ**
**Fichiers** : [audio_engine/offline.rs](../../../src-tauri/src/audio_engine/offline.rs), [main.rs](../../../src-tauri/src/main.rs), [src/lib/exportMixdown.ts](../../../src/lib/exportMixdown.ts)
`export_wav`/`export_mp3` recevaient les samples depuis l'`OfflineAudioContext` du frontend. Web Audio devenait un deuxième moteur de rendu de facto — risque de divergence silencieuse entre live (Rust) et export (Web Audio).

**Fix appliqué** :
- Nouveau module [`audio_engine::offline`](../../../src-tauri/src/audio_engine/offline.rs) : `render_project_offline(project, live_params, plugin_host, sample_rate)` compile un `GraphScheduler` éphémère via `AudioGraphCompiler::compile_scheduler` (même chemin que la live) et le pilote en boucle avec un `TransportInfo` synthétique jusqu'à la durée du projet. Aucune interaction avec le stream CPAL, aucune `enqueue` sur le snapshot. Tourne sur le thread commande — peut allouer librement.
- `export_wav` / `export_mp3` (dans `main.rs`) reçoivent maintenant uniquement `path`, `sample_rate` + format params. Ils lockent `AppState`, appellent `render_project_offline` avec le `LiveParams` partagé et le `PluginHost` de l'engine (donc l'export reflète volumes / mute / solo / depth de modulation courants ainsi que l'état des plugins), puis encodent via `hound` / `mp3lame-encoder` comme avant.
- [`src/lib/exportMixdown.ts`](../../../src/lib/exportMixdown.ts) ne crée plus d'`OfflineAudioContext`, ne convertit plus les clips, n'envoie plus de `Float32Array`s à travers IPC. Il vérifie juste qu'il y a du contenu à exporter (`totalDuration > 0`) puis invoque la commande Tauri avec le chemin et les params d'encodage.

**Bénéfices** : un seul moteur DSP, donc l'export entend ce que le user entend (compresseur, EQ, reverb, modulation, PDC, plugins) ; économie IPC majeure (plus de transfert MB de samples) ; ouvre la voie au rendu hors-ligne plus rapide-que-temps-réel par-stem.

**Restes hors scope** : pas de fichier de notification de progression pour les longs renders (l'UI reste figée sur "Export en cours…"). À ajouter quand le rendu deviendra long (gros projets, plugins). Le tempo passé au `TransportInfo` est le tempo statique du projet — quand C6 sera traité avec stockage en beats, l'export héritera automatiquement.

#### I9 — Pas de protection anti-dénormales ✅ **CORRIGÉ**
**Fichiers** : [audio_graph/denormal.rs](../../../src-tauri/src/audio_graph/denormal.rs), [audio_graph/nodes/reverb.rs](../../../src-tauri/src/audio_graph/nodes/reverb.rs), [audio_graph/nodes/delay.rs](../../../src-tauri/src/audio_graph/nodes/delay.rs), [audio_engine/engine.rs](../../../src-tauri/src/audio_engine/engine.rs), [audio_engine/offline.rs](../../../src-tauri/src/audio_engine/offline.rs)
Les chemins de feedback (reverb comb/allpass, delay) ne flushaient pas les dénormales : sur x86, les sub-normales coûtent 10×–100× plus de cycles, donc un long silence dans la queue de reverb pouvait provoquer un XRun.

**Fix appliqué** :
- Nouveau module [`audio_graph::denormal`](../../../src-tauri/src/audio_graph/denormal.rs) avec `enable_flush_to_zero()` qui flippe FTZ + DAZ (bits 15/6 de MXCSR sur x86/x86_64, bit FZ de FPCR sur aarch64). Per-thread, donc appelé sur le thread audio CPAL via un flag `denormal_guard_set` à la première invocation du callback dans [`engine::build_stream`](../../../src-tauri/src/audio_engine/engine.rs#L618), et au début de [`render_project_offline`](../../../src-tauri/src/audio_engine/offline.rs) pour le rendu offline.
- Belt-and-suspenders pour les architectures sans FTZ : constante `ANTI_DENORMAL = 1e-25` ajoutée aux écritures de feedback du `CombFilter`, `AllpassFilter` et `DelayNode`. ≈ −500 dBFS — inaudible, mais largement au-dessus de la frontière sub-normale (~1.18e-38).
- L'enveloppe du compresseur converge vers 1.0 pendant les silences (pas vers 0), donc pas de risque de sub-normale en pratique ; FTZ couvre tout cas pathologique sans surcoût.

Tests existants (reverb / scheduler / PDC) inchangés (10 passent).

#### I10 — Stacked-borrows / aliasing dans `run_steps` ✅ **CORRIGÉ**
**Fichiers** : [audio_graph/scheduler.rs](../../../src-tauri/src/audio_graph/scheduler.rs)
Le pattern `pool_ptr.add(src)` (immutable) + `pool_ptr.add(step_idx)` (mutable) sur un même `Vec` était sujet à des problèmes de provenance sous les futures évolutions de `noalias`.

**Fix appliqué** :
- `run_steps` destructure `self` au sommet (`let Self { plan, processors, output_buffers, … } = self`), ce qui matérialise les emprunts disjoints sur chaque champ et permet au borrow-checker de prouver la non-aliasing sans raw pointer.
- À chaque step, `output_buffers.split_at_mut(step_idx)` sépare explicitement `processed` (sources, reborrowed shared) de `outputs = &mut tail[0]` (cible du step courant). Les pointeurs d'inputs et la lecture de modulation passent par `&processed[src_step]` — plus aucune aliasing raw pointer sur le pool.
- Les anciens raw pointers `dl_outer_ptr` (PDC) et `actions_ptr` (modulation) sont supprimés ; remplacés par des emprunts directs `&mut delay_lines[step_idx]` et `&mod_actions_by_step[step_idx]`, possibles maintenant que les champs sont destructurés.
- Le seul `unsafe` restant dans `run_steps` est le transmute `input_ptr_scratch → &[&AudioBuffer]` pour le scratch pré-alloué (inhérent à la structure data-oriented ; la SAFETY note est mise à jour pour pointer vers les nouvelles dérivations via `split_at_mut`).

Tests PDC inchangés : `pdc_aligns_two_parallel_paths`, `pdc_zero_latency_is_passthrough`, `pdc_delay_carries_across_block_boundary` passent. Suite `audio_graph` complète verte (12 tests).

#### I11 — Lock du moteur pendant tout le compile ✅ **CORRIGÉ (partiel — deadlock résolu)**
**Fichiers** : [engine_gateway/api.rs](../../../src-tauri/src/engine_gateway/api.rs), [main.rs](../../../src-tauri/src/main.rs)
`recompile_if_needed` (api.rs) et `recompile_audio_graph` (main.rs) verrouillaient **engine → project**, alors que `render_project_to_stereo` (main.rs:466) verrouille **project → engine** : AB-BA deadlock latent.
**Fix appliqué** : ordre canonique unifié **project → engine** dans les deux helpers de recompile. Doc-comments mis à jour pour expliquer la convention.
*Reste à faire* : la durée de détention du lock engine pendant le compile (qui bloque les autres commandes engine) demanderait un refactor de `PluginHost` en `Arc<Mutex<>>` pour compiler hors du lock engine — repoussé (out of scope d'I11 strict, candidat pour un futur R).

#### I12 — Compare de `String` sur le hot path modulation
**Fichiers** : [audio_graph/scheduler.rs:55](../../../src-tauri/src/audio_graph/scheduler.rs#L55), [audio_graph/nodes/gain.rs:68](../../../src-tauri/src/audio_graph/nodes/gain.rs#L68)
`CompiledModAction::param_name: String`, comparé sur chaque route chaque block. Remplacer par un `u32` interné.

---

### MINEURES / hygiène

#### M1 — Soft-clip master inconditionnel ✅ **CORRIGÉ**
**Fichiers** : [audio_graph/nodes/master.rs](../../../src-tauri/src/audio_graph/nodes/master.rs), [audio_graph/live_params.rs](../../../src-tauri/src/audio_graph/live_params.rs), [audio_settings.rs](../../../src-tauri/src/audio_settings.rs), [main.rs](../../../src-tauri/src/main.rs), [preferences.ts](../../../src/stores/preferences.ts), [SettingsPanel.svelte](../../../src/components/SettingsPanel.svelte)
`tanh` n'était pas transparent en dessous de ±0.9 (ex. `tanh(0.5) ≈ 0.46`) mais était appliqué sur chaque échantillon du master.
**Fix appliqué** : `LiveParams::master_soft_clip` (`Arc<AtomicBool>`, défaut `false`) lu par `MasterNode` per-block — pas de recompile au toggle. `apply_audio_settings` synchronise le flag depuis les préférences. Case à cocher « Limiteur master (soft clip) » dans Paramètres → Audio. Master linéaire par défaut (somme + peak metering uniquement).

#### M2 — Types legacy `audio_engine/snapshot.rs` morts
**Fichier** : [audio_engine/snapshot.rs](../../../src-tauri/src/audio_engine/snapshot.rs)
Supprimer ou réutiliser.

#### M3 — Commentaire `unsafe impl Send` erroné
**Fichier** : [audio_graph/scheduler.rs:91](../../../src-tauri/src/audio_graph/scheduler.rs#L91)
Justification incorrecte (le scheduler EST envoyé entre threads). Corriger le doc-comment.

#### M4 — Pas de détection XRun
**Fichier** : [audio_engine/engine.rs:511](../../../src-tauri/src/audio_engine/engine.rs#L511)
`OutputCallbackInfo` ignoré. Ajouter détection underrun + télémétrie UI.

#### M5 — Reporter de position alloue à 30 Hz
**Fichier** : [audio_engine/engine.rs:607](../../../src-tauri/src/audio_engine/engine.rs#L607)
`serde_json::json!` + `Map::new()` par tick. Utiliser une struct `Serialize` typée réutilisable.

#### M6 — Bounds check par frame dans `TrackNode`
**Fichier** : [audio_graph/nodes/track.rs:107](../../../src-tauri/src/audio_graph/nodes/track.rs#L107)
Calculer l'intersection `[start_frame, end_frame]` une fois, itérer seulement dessus.

#### M7 — Incohérence port count Send
**Fichiers** : [audio_graph/compiler.rs:292](../../../src-tauri/src/audio_graph/compiler.rs#L292), [audio_graph/nodes/send.rs:26](../../../src-tauri/src/audio_graph/nodes/send.rs#L26)
Compiler déclare `num_outputs = 0`, le nœud déclare `num_output_ports = 1`. Aligner.

#### M8 — PolySynth mono dupliqué
**Fichier** : [audio_graph/nodes/poly_synth.rs:233](../../../src-tauri/src/audio_graph/nodes/poly_synth.rs#L233)
Pas de spread stéréo des voix. OK pour v0.

#### M9 — Pas d'oversampling
EQ, compresseur (genou), soft-clip — aliasing sur transitoires.

---

### REFACTORS (à planifier)

#### R1 — Exécution parallèle du graphe
Le `ExecutionPlan` a déjà les dépendances par-step. Un scheduler par level-set + pool de workers débloquerait le multi-core. Important dès que les nœuds plugin coûtent cher.

#### R2 — Modulation sample-accurate
Requiert soit dispatch par-sample (lent), soit "programmes de modulation" compilés qui remplissent un buffer de paramètre par-block lu inline par le nœud. À implémenter quand FM/audio-rate sera nécessaire.

#### R3 — Entrée MIDI live
Sous-système d'input séparé (midir / midi-msg), file lock-free MIDI alimentant `InstrumentNode`.

#### R4 — Compresser le gateway
1458 lignes de handlers presque identiques dans [engine_gateway/api.rs](../../../src-tauri/src/engine_gateway/api.rs). Macro ou table-driven.

---

## Ordre d'attaque suggéré

C1, C2, C3, C4, C5, I1, I2, I3, I4, I5, I6, I7, I8, I9, I10, I11 (deadlock) et M1 sont corrigés. Reste **C6** (tempo MIDI désynchronisé) — fix trivial une fois identifié : appeler `recompile_if_needed` dans `UPDATE_TEMPO` ou stocker les notes en beats. Ensuite la priorité bascule vers **I12** (String hot-path modulation), le volet "engine lock pendant compile" d'I11 (refactor `PluginHost` en `Arc<Mutex<>>`), puis les mineures M2–M9.

**Note résiduelle pour C3** : `UPDATE_EFFECT` continue à recompiler. Pour fermer complètement la boucle hot-path, concevoir un `ParameterBlock` générique ou des cellules atomiques typées par effet (EQ, Compresseur, Reverb, Delay) — chaque effet expose alors ses params via `LiveParams` au même titre que volume/pan/mute/solo.
