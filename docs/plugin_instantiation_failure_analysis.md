# Plugin Instantiation — Reality & Failure Analysis

> Focus #1 de l'audit de réalité. Question posée : *le plugin est-il créé /
> inséré dans le graphe / processé / activé dans le scheduler / son lifecycle
> est-il respecté — et où ça casse exactement ?*
>
> Fichiers analysés : `plugin_system/{instance,lifecycle,host,registry,bridge,bridged_node}.rs`,
> `plugin_formats/{clap,vst3}/{host,wrapper,ffi}.rs`, `audio_graph/compiler.rs`,
> `crates/clap_ffi/src/lib.rs`.

---

## 0. Réponse directe

Dans le **happy-path**, **rien ne casse à la création** : le plugin est créé,
inséré, préparé, schedulé et processé. La défaillance réelle est dans la
**gestion d'identité et de cycle de vie** des instances à travers les recompiles.

| Étape | Verdict | Où |
|-------|---------|----|
| Plugin créé ? | ✅ Oui | `PluginHost::instantiate` → `instantiate_vst3`/`instantiate_clap` |
| Inséré dans le DSP graph ? | ✅ Oui | `EffectParams::Plugin` → `NodeKind::Plugin` → `build_node` → `processors[]` |
| Reçoit `process()` ? | ✅ Oui | `scheduler::process_one_step` appelle `processor.process(...)` |
| Activé dans le scheduler ? | ✅ Oui | wrapper `prepare()` fait `activate`/`start_processing` (CLAP) / `setActive`/`setProcessing` (VST3) |
| Lifecycle complet respecté ? | ⚠️ Non | saut direct à `Running` ; `suspend/resume/destroy` morts |

---

## 1. Le chemin de création (ce qui marche)

### 1.1 Scan (démarrage)
`AudioEngine::new` appelle `host.scan_plugins()`
([`engine.rs:258`](../src-tauri/src/audio_engine/engine.rs#L258)). CLAP : scan des
dossiers, `ClapLibrary::load` (qui appelle `clap_entry.init` 1×/DLL, cf. §4),
puis énumération via la factory → `PluginManifest` dans la registry.

### 1.2 Topologie
`AudioGraph::from_project` insère un `NodeKind::Plugin { device_id, sandboxed }`
pour chaque `EffectParams::Plugin` activé dans la chaîne d'effets d'une piste
([`graph.rs:382-385`](../src-tauri/src/audio_graph/graph.rs#L382)), câblé en série :
`track → … → Plugin(in 0) … Plugin(out 0) → … → gain → meter → master`.

### 1.3 Instanciation
`GraphCompiler::build_node` pour `NodeKind::Plugin`
([`compiler.rs:675-712`](../src-tauri/src/audio_graph/compiler.rs#L675)) :
1. si `sandboxed` → tente `BridgedNode::try_new` (CLAP + bridge exe présents),
   sinon fall-through ;
2. `plugin_host.instantiate(id, device_id, sandboxed)`.

`PluginHost::instantiate` ([`host.rs:152`](../src-tauri/src/plugin_system/host.rs#L152)) :
- VST3 : `Vst3Library::load` (LoadLibraryW) → `instantiate_processor` →
  `PluginInstance::new_vst3` → `VstWrapperNode::new(node_id, Arc::clone)`.
- CLAP : `create_clap_instance` → `create_plugin` → `PluginInstance::new_clap` →
  `ClapWrapperNode::new(node_id, Arc::clone)`.
- Le host **garde un clone de l'`Arc`** dans `records[device_id]`, le wrapper en
  garde un autre. Tampon `set_lifecycle(Running)` immédiat.
- Internal/AU/WASM → `PassThroughDevice`.

En cas d'échec (registry miss, FFI null) → **fallback unity-gain `GainNode`**
([`compiler.rs:709`](../src-tauri/src/audio_graph/compiler.rs#L709)) : le graphe
compile toujours, le plugin est silencieusement remplacé par un pass-through.
Idem instrument → fallback `PolySynth` ([`compiler.rs:580`](../src-tauri/src/audio_graph/compiler.rs#L580)).

### 1.4 Préparation & activation
`compile()` appelle `node.prepare(ctx)` pour chaque node
([`compiler.rs:257`](../src-tauri/src/audio_graph/compiler.rs#L257)), **sur le
thread commande**. Le wrapper y exécute la séquence FFI d'activation :
- CLAP : `init` → `activate(sr, 1, max_block)` → (latence + énum params) →
  `start_processing` ([`clap/wrapper.rs:154-201`](../src-tauri/src/plugin_formats/clap/wrapper.rs#L154)).
- VST3 : `set_bus_arrangements` → `activate_bus`×2 → `setActive(1)` →
  `setup_processing` → `setProcessing(1)` → latence
  ([`vst3/wrapper.rs:144-194`](../src-tauri/src/plugin_formats/vst3/wrapper.rs#L144)).

### 1.5 Exécution
Le scheduler appelle `processor.process(inputs, outputs, transport)` enveloppé
dans `catch_unwind` ([`scheduler.rs:630`](../src-tauri/src/audio_graph/scheduler.rs#L630)).
Le wrapper draine son ring SPSC de params, remplit les pointeurs de canaux
(pré-alloués en `prepare`), et appelle `process` FFI. **Aucune alloc/lock** sur
le chemin. RT-safe.

➡️ **Le plugin joue, et ses paramètres UI l'atteignent.**

---

## 2. Où ça casse réellement

### 2.1 ⛔ R-1 — Ré-instanciation à chaque recompile (perte d'état)
`build_node`→`instantiate` est rappelé à **chaque** `compile()`. Or :
- Tout changement structurel recompile.
- **Tout tour de bouton d'un param d'effet ou de plugin recompile aussi**
  (`critical_issues.md` §1.4 — chemin hot non dévié pour les effets/plugins).

Conséquence runtime : à chaque édition, le plugin natif est **détruit puis
recréé** (`create_plugin`/`instantiate_processor` neufs), `init`/`activate`/
`start_processing` rejoués, et **l'état interne du plugin repart au défaut**.
ADR-007 §"Refcount transitions" documente ce cycle (t2 = nouvelle
`PluginInstance`, ancien record remplacé) — c'est *intentionnel* mais ses effets
(perte d'état + double instance transitoire) sont une **dégradation réelle**.

> Le partage `Arc<PluginInstance>` d'ADR-007 garantit *une* instance **par
> compile**, pas la **réutilisation** d'une instance **entre** compiles. L'objectif
> « single live plugin per device » est donc tenu *dans* un graphe mais
> **churné** entre graphes.

### 2.2 ⛔ R-4 — État non restauré après ré-instanciation
`PluginHost::load_device_state` n'est appelé qu'au **chargement de projet**
([`main.rs:252`](../src-tauri/src/main.rs#L252)). Quand R-1 recrée l'instance suite à
un tour de bouton, le `saved_state` n'est **pas** re-poussé dans la nouvelle
instance. Le plugin perd presets/état à chaque édition d'un autre paramètre.

### 2.3 🔶 R-2 — `records` indexé par `device_id` (collision multi-instance)
[`host.rs:206`](../src-tauri/src/plugin_system/host.rs#L206) :
`records.insert(plugin_id.to_string(), …)`. Deux effets utilisant le **même
plugin** (même `device_id`) sur deux pistes produisent deux nodes wrapper
distincts dans le graphe, mais **un seul** `DeviceRecord` host (le dernier
compilé écrase le premier). Conséquences :
- `push_plugin_param(device_id, …)` ne sait viser qu'**une** des deux instances.
- `open_plugin_editor` → `host.instance(plugin_id)` ouvre l'éditeur sur cette
  même unique instance, pas forcément celle de la piste cliquée.
- Le wrapper « écrasé » tourne quand même dans le graphe mais devient
  **inadressable** par le host.

### 2.4 🔶 R-3 — Teardown host mort (fuite)
`destroy_device` / `suspend_device` / `resume_device`
([`host.rs:403-437`](../src-tauri/src/plugin_system/host.rs#L403)) n'ont **aucun
appelant** (vérifié par grep sur tout `src/`). Or ADR-007 §"If the user removes
the plugin" **suppose** `host.destroy_device("id")` appelé à la suppression.
Réalité : quand un plugin est retiré d'une piste, le recompile produit un graphe
sans ce node ; `instantiate` n'est donc pas rappelé pour lui, et son
`DeviceRecord` **reste indéfiniment** dans `records`. L'`Arc` host maintient
l'instance vivante → **le plugin natif n'est jamais détruit** (DLL + ressources
retenues) jusqu'à la fermeture de l'app. La machine à états de `lifecycle.rs` est
donc en pratique **morte** (seul `Running`/`Destroyed` forcé sont atteints).

### 2.5 ⛔ R-5 — Crash C++ (SEH) non isolé en in-process
`catch_unwind` ([`scheduler.rs:630`](../src-tauri/src/audio_graph/scheduler.rs#L630))
ne capte que les panics Rust. Une access-violation / exception C++ d'un plugin
(Vital, Diva sur certains presets) **abat le process**. Seule mitigation : le
sandbox out-of-process (`BridgedNode`, ADR-015), **opt-in et désactivé par
défaut** (C-9). En mode normal, un plugin qui crashe = DAW qui crashe.

### 2.6 🔶 R-6 — Params d'instrument CLAP jetés
`ClapInstrument::process_midi` draine puis **jette** les params
(`drain_params(|_| {})`, [`clap_instrument.rs:146`](../src-tauri/src/audio_graph/nodes/clap_instrument.rs#L146)).
Un CLAP utilisé comme **instrument** reçoit ses notes mais **pas** ses
automations de paramètres (contrairement au même CLAP utilisé en **effet**, qui
les applique). Documenté comme follow-up dans le code.

---

## 3. Faux problèmes (résolus, contrairement à la doc / mémoire)

- **« Deux instances audio + GUI »** (mémoire `plugin-gui-audit-pointer`, A1) :
  **résolu** en Phase 2c. `gui.rs::open_vst3_editor`/`open_clap_editor`
  réutilisent l'instance audio via `instance.vst3_handles()` /
  `instance.clap_plugin()` ([`main.rs:771`](../src-tauri/src/main.rs#L771) passe
  l'`Arc` partagé). Plus de second `IComponent`/`ClapPlugin`.
- **« `clap_entry.init` sauté = crash Vital »** (mémoire) : **résolu**.
  `ClapLibrary::load` appelle `((*entry).init)(path)` exactement une fois par DLL
  ([`crates/clap_ffi/src/lib.rs:815`](../src-tauri/crates/clap_ffi/src/lib.rs#L815)),
  caché dans `CLAP_LIBRARIES: OnceLock<Mutex<HashMap<PathBuf, Arc<ClapLibrary>>>>`.
- **« Pas de transport de params UI→plugin »** : **résolu** (ADR-008, ring SPSC
  drainé par les wrappers).

---

## 4. Détails FFI vérifiés

- **CLAP** : init DLL une fois (cache), `create_plugin` avec un `Box<ClapHost>`
  heap-stable, `Drop` de `PluginInstance` fait `destroy()`, le wrapper fait
  `stop_processing`/`deactivate` (gated `active`). Séquence conforme spec.
- **VST3** : `Vst3Library::load` **ne cache pas** la DLL (contrairement à CLAP) —
  chaque `instantiate_vst3` refait `LoadLibraryW` (l'OS refcount le module) et
  crée une factory + composant neufs ; `FreeLibrary` au `Drop` de chaque
  `Vst3Library`. `PluginInstance::Drop` fait
  `setProcessing(0)→setActive(0)→terminate→release×2`. Conforme.
- **Internal/AU/WASM** : `PassThroughDevice` (AU/WASM = stubs `eprintln` +
  pass-through, [`host.rs:182-194`](../src-tauri/src/plugin_system/host.rs#L182)).

---

## 5. Synthèse — « où ça casse exactement »

> **Pas à la création.** Ça casse dans le **maintien de l'identité de l'instance
> à travers le temps** :
>
> 1. l'instance est **recréée** au lieu d'être réutilisée à chaque recompile (R-1),
>    avec **perte d'état** (R-4) ;
> 2. l'indexation par `device_id` (R-2) empêche de distinguer deux usages du même
>    plugin ;
> 3. le **teardown n'est jamais déclenché** (R-3), donc les instances retirées
>    fuient et la machine lifecycle est morte ;
> 4. en in-process (défaut), un **crash natif tue tout** (R-5).
>
> Les symptômes utilisateur attendus : presets de plugin qui « sautent » en
> tournant un autre bouton, éditeur GUI ouvrant la mauvaise instance avec plugins
> dupliqués, mémoire qui monte à chaque ajout/retrait de plugin, et crash total
> sur plugin instable.
