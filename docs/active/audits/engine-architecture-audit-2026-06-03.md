# Engine Architecture Audit — 2026-06-03

Auditeur : Claude (Opus 4.8)
Portée : moteur audio complet — `src-tauri/src/audio_engine/`, `src-tauri/src/audio_graph/`,
`src-tauri/src/plugin_system/`, `src-tauri/src/plugin_formats/`, intégration via
`src-tauri/src/engine_gateway/`.
Axe : refactor / simplification / redesign / réécriture **avant** que la dette ne
devienne structurelle. Priorités : stabilité runtime, hard realtime safety,
déterminisme, scaling, maintenance long terme.

---

## Verdict global

L'architecture est **nettement au-dessus de la moyenne** pour ce stade : le chemin
temps réel est pensé (swap lock-free + drop ring, SPSC params, FTZ/DAZ, panic
isolation, PDC, déterminisme par tie-break `NodeId`). Le squelette des bonnes
décisions est là et documenté par ADR.

Mais **3 dettes structurelles deviendront bloquantes** et touchent précisément les
priorités (stabilité, RT, scaling), plus un cluster de problèmes RT/perf de second
rang. Aucune n'exige un rewrite ; toutes exigent une correction **avant** que
d'autres couches se figent par-dessus.

Grille de classement : cosmétique / dette acceptable / **dette structurelle** /
**architecture probablement mauvaise**.

---

## Synthèse priorisée

| # | Problème | Classe | Risque clé |
|---|----------|--------|-----------|
| 1 | Ré-instanciation plugins à chaque recompile | **structurelle** | glitch + perte état DSP + freeze |
| 2 | `records` clé = type plugin (R-2) | **structurelle** | param/état sur la mauvaise instance |
| 3 | Compile synchrone sous double verrou | **structurelle** | mur de réactivité à l'échelle |
| 4 | Workers non-RT + unpark/spin sur RT | **structurelle / archi-mauvaise** | underrun multicœur sous charge |
| 5 | Buffers brassés à 4096 frames | **structurelle (perf)** | anti-cache, plafond pistes |
| 6 | Modulation block-rate | acceptable→structurelle | escalier, pas d'audio-rate |
| 7 | Crash plugin = crash process | acceptable (boundary OK) | **bloquant shipping** |
| 8 | Ports plugin codés stéréo | structurelle (ambition pro) | pas de mono/surround/multi-out |

**Ordre d'attaque conseillé :** 2 → 1 (couplés : un cache d'instances correct exige
une clé d'instance correcte), puis 5 (gain perf transverse pas cher), puis 7 (avant
toute exposition publique), puis 3 et 4 quand parallélisme/scaling deviennent réels.

Aucun point ne justifie un **rewrite** : le découplage `Box<dyn AudioNode>` / ADR /
boundary sandbox tient. Ce sont des **refactors ciblés** — mais 1-2-3 doivent passer
**avant** que le gateway et le projet s'enracinent davantage sur l'adressage par type
de plugin.

---

## 🔴 Niveau 1 — Dette structurelle (à traiter avant d'aller plus loin)

### 1. Ré-instanciation de TOUS les plugins à chaque recompilation structurelle

**Constat (vérifié).** [host.rs:176-241](../../../src-tauri/src/plugin_system/host.rs#L176-L241)
`instantiate()` ne consulte jamais `records` : il crée systématiquement un **nouveau**
`PluginInstance` (chargement DLL, `create_plugin`/`instantiate_processor`, `activate`,
re-query latence, `setState`) et écrase l'entrée. Or le compilateur appelle
`build_node`→`instantiate` pour **chaque** nœud `Plugin` à **chaque** compile
([compiler.rs:232-254](../../../src-tauri/src/audio_graph/compiler.rs#L232-L254),
[compiler.rs:676-703](../../../src-tauri/src/audio_graph/compiler.rs#L676-L703)). Donc
ajouter une piste, un clip ou une route ré-instancie l'intégralité des plugins du
graphe.

**Pourquoi c'est dangereux dans un vrai DAW.** Chaque édition structurelle :
- **détruit l'état DSP interne** des plugins (queues de reverb/delay, enveloppes de
  compresseur, historiques d'analyseur) — sauf l'état sérialisé FFI, rejoué *que* au
  chargement de projet via `pending_state_blobs`, pas sur recompile ;
- provoque un **pic CPU** (load DLL + `activate`) sur le thread commande, **verrous
  tenus** (cf. #3) → micro-freeze UI ;
- recalcule la PDC (latence re-interrogée) → **clic/glitch** audible, voire xrun.

Contradiction directe avec le commentaire de `retain_devices` qui promet « a plugin
that survives a recompile is never disturbed (no FFI churn, bit-identical) »
([host.rs:460-476](../../../src-tauri/src/plugin_system/host.rs#L460-L476)) — la
promesse est fausse car `instantiate` a déjà écrasé le record juste avant.

**Coût futur.** Linéaire avec le nombre de plugins : un projet réaliste (20-50
plugins) rend *chaque* clic d'édition coûteux et glitchy. Le retrofit d'un cache
d'instances devra toucher host + compiler + chemin de restauration d'état — d'autant
plus douloureux que le projet/gateway s'enracinent sur l'adressage par `device_id`.

**Classement : dette structurelle** (frôle l'architecture-mauvaise).
**Correctif :** dans `instantiate`, réutiliser le record existant si l'instance
(idéalement par `instance_id`, cf. #2) est déjà vivante et que le manifeste n'a pas
changé — ne reconstruire que les nœuds réellement nouveaux.

---

### 2. `records` indexé par `device_id` = collision « même plugin sur 2 pistes » (R-2)

**Constat.** [host.rs:83](../../../src-tauri/src/plugin_system/host.rs#L83)
`records: HashMap<String, DeviceRecord>` clé = `device_id` = **type** de plugin.
[host.rs:232](../../../src-tauri/src/plugin_system/host.rs#L232) `instantiate` insère
par `plugin_id`. Deux usages du même plugin → la seconde insertion écrase la première.
`push_plugin_param`/`save_device_state`/`suspend` adressent tous par `device_id`
([host.rs:405-416](../../../src-tauri/src/plugin_system/host.rs#L405-L416)). Le champ
`instance_id` ([instance.rs:75-82](../../../src-tauri/src/plugin_system/instance.rs#L75-L82))
a été ajouté mais **n'est pas propagé** dans project/graph/gateway — demi-mesure.

**Pourquoi c'est dangereux.** Les plugins dupliqués sont la norme absolue dans un DAW
(même EQ sur 10 pistes). Ici : tourner un potard sur l'instance A part vers la
dernière instance insérée ; save/restore d'état mélange les instances. C'est un
**bug de correction**, pas seulement de perf.

**Coût futur.** Le modèle d'adressage doit changer de bout en bout (le projet/graphe
doivent porter un handle d'instance, pas un type). Chaque mois passé à empiler des
appels `by device_id` ajoute des sites à migrer.

**Classement : dette structurelle** (déjà identifiée R-2 dans le code). Prioritaire
car elle bloque #1 (un vrai cache d'instances suppose une clé d'instance correcte).

---

### 3. Compilation synchrone sous double verrou sur le thread IPC

**Constat.** [api.rs:1696-1711](../../../src-tauri/src/engine_gateway/api.rs#L1696-L1711)
`recompile_if_needed` prend `project.lock()` **puis** `engine.lock()` et appelle
`update_from_project` (compile complet, incl. instanciation plugins FFI) **dans le
handler de commande**, de façon synchrone.

**Pourquoi c'est dangereux.** Pas une violation RT (hors thread audio), mais un **mur
de scalabilité/réactivité** : toute commande structurelle bloque le verrou projet
pendant toute la durée du compile ; combinée à #1, chaque édition paie le coût
d'instanciation de tous les plugins, sérialisé. L'ordre de verrou (`project → engine`)
est correctement documenté pour éviter l'AB-BA avec l'offline, mais c'est fragile.

**Coût futur.** Au-delà de quelques dizaines de nœuds/plugins, latence d'édition
perceptible. Le passage à un compile **asynchrone hors verrou** (figer un snapshot
d'intention projet, compiler off-thread, enqueue) est un refactor non trivial une fois
que tout le gateway suppose un compile synchrone.

**Classement : dette structurelle** (acceptable court terme, douloureuse à l'échelle).

---

## 🟠 Niveau 2 — Problèmes RT / perf réels

### 4. Worker pool : `unpark` par niveau sur le thread audio + spin-wait sur workers non-RT

**Constat.** [worker_pool.rs:148-168](../../../src-tauri/src/audio_graph/worker_pool.rs#L148-L168) :
à chaque niveau de chaque bloc, le thread audio `unpark` chaque worker (un
**futex/syscall** par worker) puis **spin-loop** sur `workers_done`. Les workers
héritent de la **priorité normale** du process (commentaire l.24-25 : « raising them
to RT is left to a separate change »).

**Pourquoi c'est dangereux.** Deux choses se cumulent sur le chemin hard-RT :
- `thread::unpark` = appel système, exécuté *depuis le callback audio*, autant de fois
  que de workers × niveaux. L'esprit « no syscalls on the audio thread » est violé
  pour les graphes profonds.
- le thread audio (haute prio implicite CPAL) **attend en spin** la complétion de
  workers qui ne sont **pas** prioritaires → **inversion de priorité** : sous charge
  système, un worker descheduled stalle le callback → underrun. Exactement la
  « fragilité multicœur » ciblée.

Mitigations présentes : `POOL_MIN_LEVEL_WIDTH=2` et le chemin inline single-step
évitent le pool pour les graphes étroits. Bien, mais insuffisant pour un gros graphe
large.

**Coût futur.** Latent tant que le parallélisme n'est pas activé en prod. Dès qu'il
l'est, il faut : (a) priorité RT sur les workers, (b) éviter `unpark` par niveau (pool
persistant en work-stealing, ou réveil unique par bloc), (c) borne sur le spin.

**Classement : dette structurelle** pour priorité/spin ; **architecture probablement
mauvaise** pour le `unpark`-par-niveau si le compte de niveaux grandit.

> Note de sûreté connexe : `Workspace` expose des pointeurs bruts dont la soundness
> repose **entièrement** sur l'invariant de niveau du compilateur
> ([scheduler.rs:158-180](../../../src-tauri/src/audio_graph/scheduler.rs#L158-L180)).
> Correct aujourd'hui mais **extrêmement fragile** : toute future lecture d'un buffer
> du même niveau = UB. À garder sous test/assertion stricte.

---

### 5. Buffers traités à `MAX_BLOCK_FRAMES` (4096) quelle que soit la taille de bloc réelle

**Constat.** Les buffers sont alloués à `max_block` = 4096
([scheduler.rs:247-258](../../../src-tauri/src/audio_graph/scheduler.rs#L247-L258)), et
`num_frames` **est** la capacité, pas la longueur vive. `clear/copy_from/mix_from/
apply_gain` opèrent sur `data.len()` = `num_channels*4096`
([buffer.rs:96-129](../../../src-tauri/src/audio_graph/buffer.rs#L96-L129)). Le
scheduler `clear()` les sorties à chaque step
([scheduler.rs:524-527](../../../src-tauri/src/audio_graph/scheduler.rs#L524-L527)).
Les nœuds traitent bien `transport.block_size` (correction OK), mais les **opérations
de buffer du scheduler et des bus/master** brassent 4096 frames même avec un bloc
device de 128-256.

**Pourquoi c'est dangereux.** 16-32× de trafic mémoire inutile par opération de
buffer, **multiplié par le nombre de nœuds** → pollution cache, bande passante
gaspillée, plafond de polyphonie/pistes abaissé. **Anti-cache** pur.

**Coût futur.** Pervasif : corriger = ajouter une « longueur active » à `AudioBuffer`
et la propager dans toutes les ops de bloc. Bien moins cher maintenant qu'après avoir
écrit 30 nœuds contre l'hypothèse « longueur fixe ».

**Classement : dette structurelle** (perf), correctif localisé mais transverse.

---

### 6. Modulation au rythme du bloc (block-rate), un échantillon par bloc

**Constat.** [scheduler.rs:536-545](../../../src-tauri/src/audio_graph/scheduler.rs#L536-L545)
lit `src_bufs[port].channel(0)[0]` — premier échantillon du bloc — marqué PROVISIONAL.
Jusqu'à un bloc de latence ; inadapté à la modulation audio-rate (FM, ring-mod).

**Pourquoi c'est dangereux.** Escalier audible sur LFO rapides, pas de mod audio-rate.
Pour un DAW qui veut vendre la modulation, c'est un plafond fonctionnel ; le retrofit
sample-accurate implique un dispatch entrelacé → **redesign du scheduler**.

**Classement : dette acceptable** aujourd'hui (honnêtement marquée), **devient
structurelle** si la modulation est un argument produit.

---

### 7. Stabilité : un plugin natif qui crashe abat le process

**Constat.** [scheduler.rs:572](../../../src-tauri/src/audio_graph/scheduler.rs#L572)
`catch_unwind` n'attrape que les panics Rust ; un SEH/segfault C++ d'un plugin
in-process **abort** le process (commenté honnêtement). Le sandbox out-of-process
existe en squelette opt-in (`BridgedNode`, ADR-015) mais est **inerte**
([host.rs:182-183](../../../src-tauri/src/plugin_system/host.rs#L182-L183)).

**Pourquoi c'est dangereux.** Dans un vrai DAW, **un** plugin instable tue la session
entière (perte de travail). **Risque #1 de stabilité** d'un produit qui shippe avec
hosting natif.

**Bonne nouvelle.** La frontière de sandbox est *conçue d'avance* (opt-in,
`Box<dyn AudioNode>` côté graphe) — donc le sandbox n'est **pas** « impossible à
ajouter plus tard » : seul point « impossible à rajouter » de la liste initiale qui
est déjà couvert architecturalement. Il reste à le rendre réel.

**Classement : dette acceptable** structurellement (le boundary existe), mais
**bloquant pour shipping**. À prioriser avant toute V1 publique.

---

## 🟡 Niveau 3 — Rigidités d'API à surveiller

### 8. Comptes de ports plugin codés en dur (stéréo)

[compiler.rs:496-541](../../../src-tauri/src/audio_graph/compiler.rs#L496-L541) :
`Plugin => 2 in / 2 out`, `Bus => 8`, `num_channels` figé à 2
([engine.rs:283](../../../src-tauri/src/audio_engine/engine.rs#L283)). Les plugins
mono, >2ch, multi-out, et le surround ne routent pas correctement. Le sidechain aux
est scaffolded mais compte 0.

**Pourquoi c'est dangereux à terme.** Un DAW « pro » finit toujours par devoir router
du mono/surround/multi-out. L'`AudioBuffer` mono-`Vec<f32>` planar à N canaux fixes par
buffer suit la même rigidité.

**Classement : dette structurelle** pour l'ambition « pro » ; **acceptable** tant que
le scope est stéréo. À ne pas enfouir sous trop de code.

---

## ✅ Ce qui est bon (à ne PAS toucher)

- **Swap snapshot lock-free + drop ring**
  ([snapshot.rs](../../../src-tauri/src/audio_graph/snapshot.rs)) : `drop` hors RT,
  raisonnement de provenance correct. Modèle de référence.
- **SPSC param queue**
  ([param_queue.rs](../../../src-tauri/src/plugin_system/param_queue.rs)) :
  Release/Acquire correct, overflow = drop-newest. Sound.
- **Déterminisme** : tie-break `NodeId` au topo-sort + ordre d'itération des ports
  d'entrée **fixe** → la somme flottante intra-nœud est ordre-stable indépendamment de
  l'ordonnancement des workers ; la barrière de niveau garantit le reste. Le test
  `parallel_pool_produces_same_output_as_sequential` le verrouille. **Invariant
  fragile** mais correct — le protéger par tests.
- **Live params atomiques**
  ([live_params.rs](../../../src-tauri/src/audio_graph/live_params.rs)) :
  potards/solo/depth sans recompile — exactement le bon shape. (Ironique vu #1 : la
  même rigueur n'est pas appliquée aux plugins.)
- **PDC** ([compiler.rs:305-340](../../../src-tauri/src/audio_graph/compiler.rs#L305-L340))
  avec test d'alignement multi-bloc.
- **Reporter via try_lock**
  ([engine.rs:834](../../../src-tauri/src/audio_engine/engine.rs#L834)) : jamais
  bloquant côté UI.

---

## Liens

- ADR concernés : 003, 004, 007, 008, 009, 010, 011, 012, 015 —
  [decisions/000-index.md](../decisions/000-index.md).
- Audits antérieurs : [audio-engine-audit-2026-05-29.md](audio-engine-audit-2026-05-29.md),
  [plugin-system-audit-2026-05-31.md](plugin-system-audit-2026-05-31.md),
  [plugin-gui-audit-2026-05-31.md](plugin-gui-audit-2026-05-31.md).
