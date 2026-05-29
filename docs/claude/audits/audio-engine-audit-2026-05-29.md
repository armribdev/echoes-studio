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

#### I4 — Pas de compensation de latence (PDC)
**Fichiers** : [audio_graph/node.rs:168](../../../src-tauri/src/audio_graph/node.rs#L168), [audio_graph/compiler.rs](../../../src-tauri/src/audio_graph/compiler.rs)
`latency_samples()` existe mais n'est jamais lu par le compiler. Aucune compensation de chemin. Indispensable pour tout plugin avec lookahead.

#### I5 — Mute/solo via recompile complet
**Fichiers** : [engine_gateway/api.rs:340](../../../src-tauri/src/engine_gateway/api.rs#L340), [audio_graph/graph.rs:243](../../../src-tauri/src/audio_graph/graph.rs#L243)
Devrait être un atomique de gain lu par-block. Cas particulier de C3.

#### I6 — Reset modulation conditionnel
**Fichiers** : [audio_graph/scheduler.rs:288](../../../src-tauri/src/audio_graph/scheduler.rs#L288), [audio_graph/nodes/gain.rs:60](../../../src-tauri/src/audio_graph/nodes/gain.rs#L60)
`reset_modulation()` n'est appelé que si `!actions.is_empty()`. Si une route est retirée en cours de session, le dernier `mod_delta` persiste. Toujours reset (ou toujours sommer depuis zéro).

#### I7 — `PluginHost` totalement déconnecté
**Fichiers** : [plugin_system/host.rs:101](../../../src-tauri/src/plugin_system/host.rs#L101), [audio_graph/compiler.rs:412](../../../src-tauri/src/audio_graph/compiler.rs#L412)
`NodeKind::Plugin` retourne toujours un `GainNode` placeholder. `PluginHost::instantiate` n'est jamais appelé. Les quatre formats (VST3/AU/CLAP/WASM) sont des stubs. Toute l'abstraction plugin est actuellement un no-op.

#### I8 — L'export bypass le moteur
**Fichiers** : [main.rs:455-613](../../../src-tauri/src/main.rs#L455)
`export_wav`/`export_mp3` reçoivent les samples depuis l'`OfflineAudioContext` du frontend. Web Audio est donc un deuxième moteur de rendu de facto. L'utilisateur peut entendre quelque chose de différent en live (moteur Rust) vs ce qui est exporté (Web Audio). Risque de divergence silencieuse sérieux.

#### I9 — Pas de protection anti-dénormales
**Fichiers** : [audio_graph/nodes/reverb.rs](../../../src-tauri/src/audio_graph/nodes/reverb.rs), [audio_graph/nodes/delay.rs](../../../src-tauri/src/audio_graph/nodes/delay.rs), [audio_graph/nodes/compressor.rs](../../../src-tauri/src/audio_graph/nodes/compressor.rs)
Les chemins de feedback (reverb, delay, enveloppe compresseur) ne flush pas les dénormales. Long silences → spikes CPU 100× sur x86. Ajouter `+ 1e-25` ou activer FTZ/DAZ.

#### I10 — Stacked-borrows / aliasing dans `run_steps`
**Fichiers** : [audio_graph/scheduler.rs:226-309](../../../src-tauri/src/audio_graph/scheduler.rs#L226)
Le pattern `pool_ptr.add(src)` (immutable) + `pool_ptr.add(step_idx)` (mutable) sur un même `Vec` est sujet à des problèmes de provenance sous les futures évolutions de `noalias`. Préférer `split_at_mut` pour séparer explicitement les régions processed/current.

#### I11 — Lock du moteur pendant tout le compile
**Fichiers** : [engine_gateway/api.rs:1442](../../../src-tauri/src/engine_gateway/api.rs#L1442)
`update_from_project` acquiert `audio_engine.lock()` puis fait tout le compile (avec clone de tous les samples). D'autres commandes bloquent. Risque de deadlock vu l'ordre opposé dans `apply_loaded_project`.

#### I12 — Compare de `String` sur le hot path modulation
**Fichiers** : [audio_graph/scheduler.rs:55](../../../src-tauri/src/audio_graph/scheduler.rs#L55), [audio_graph/nodes/gain.rs:68](../../../src-tauri/src/audio_graph/nodes/gain.rs#L68)
`CompiledModAction::param_name: String`, comparé sur chaque route chaque block. Remplacer par un `u32` interné.

---

### MINEURES / hygiène

#### M1 — Soft-clip master inconditionnel
**Fichier** : [audio_graph/nodes/master.rs:50](../../../src-tauri/src/audio_graph/nodes/master.rs#L50)
`tanh` n'est pas transparent en dessous de ±0.9. Mettre derrière un setting.

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

C1, C2, C3, C4, C5 et I1 sont corrigés. Reste **C6** (tempo MIDI désynchronisé) — fix trivial une fois identifié : appeler `recompile_if_needed` dans `UPDATE_TEMPO` ou stocker les notes en beats. Ensuite la priorité bascule vers les importantes (I2–I12), notamment I7 (`PluginHost` à brancher) et I8 (export bypass moteur) qui ouvrent le chemin v1.

**Note résiduelle pour C3** : `UPDATE_EFFECT` continue à recompiler. Pour fermer complètement la boucle hot-path, concevoir un `ParameterBlock` générique ou des cellules atomiques typées par effet (EQ, Compresseur, Reverb, Delay) — chaque effet expose alors ses params via `LiveParams` au même titre que volume/pan/mute/solo.
