# Plugin GUI Hosting Audit — 2026-05-31

Auditeur : Claude (Sonnet 4.6)
Portée : `src-tauri/src/plugin_system/gui.rs`, `src-tauri/src/plugin_formats/clap/ffi.rs`, `src-tauri/src/plugin_formats/vst3/ffi.rs`, intégration via `src-tauri/src/main.rs::{open_plugin_editor, close_plugin_editor}`.

Déclencheur : crash `STATUS_ACCESS_VIOLATION (0xc0000005)` à l'ouverture d'un éditeur CLAP (Vital reproductible).

Légende : ✅ Corrigé — ⚠️ Non corrigé (dette documentée)

---

## Verdict

L'architecture actuelle « instance audio + instance GUI séparées » est **non-spec** pour VST3 comme pour CLAP. L'implémentation cumulait au moins **7 bugs de safety/lifecycle distincts** qui, isolément, suffisent à produire un access violation. La cause **directe** du crash CLAP était l'absence d'appel à `clap_entry.init()`.

Les bugs bloquants (C1, W1, W2, A2, A3, A4, V1, V2, V3, V5, CL1, CL2, CL4) ont tous été corrigés au commit de ce jour. V4 (dual-component) a été corrigé le 2026-06-04. Reste une dette d'architecture (A1) et des améliorations non bloquantes (CL3, CL5, W3).

---

## Cause directe du crash (C1) — ✅ Corrigé

### C1 — `clap_entry.init()` jamais appelé

Le raisonnement du commentaire original était inversé : Vital crashe parce qu'on appelait `init` une **seconde** fois (violation « can only be called once »), pas parce qu'il est strict. Sans `init()` les tables statiques globales du plugin ne sont pas initialisées → `get_factory` renvoie un vtable non patché → `create_plugin` saute dans le vide → `0xc0000005`.

**Correctif appliqué** ([clap/ffi.rs](../../../src-tauri/src/plugin_formats/clap/ffi.rs)) : cache process-wide `HashMap<PathBuf, Arc<ClapLibrary>>` protégé par `OnceLock<Mutex>`. `ClapLibrary::load()` appelle `init()` exactement une fois par chemin canonique. `deinit()` appelé au dernier `Drop`. Scan (`clap/host.rs`) et GUI n'appellent plus `init()` directement.

---

## Bugs structurels (architecture)

### A1 — Deux instances de plugin par effet ✅ Corrigé (ADR-016 : l'éditeur réutilise l'`Arc<PluginInstance>` audio)

Ni VST3 ni CLAP ne supportent ce modèle. Conséquences :

- Aucune sync paramètres ↔ audio : un knob bougé dans le GUI n'affecte pas l'audio
- Aucune sync d'état/preset
- Double allocation des ressources lourdes (wavetables, samples, IR)
- Plugins à enregistrement global (MTS-ESP, iLok) : corruption ou refus de licence

**Cible** : `PluginInstance` unique par effet, partagée entre `AudioNode` et `EditorWindow` via `Arc`. Refonte lourde à planifier séparément.

### A2 — `host_get_extension` retournait null pour tout ✅ Corrigé

**Correctif appliqué** ([gui.rs](../../../src-tauri/src/plugin_system/gui.rs)) : `gui_host_get_extension` route `clap.log` → `ClapHostLog` (imprime sur stderr) et `clap.gui` → `ClapHostGui` (resize, closed). Déclarations de types dans `clap/ffi.rs`.

### A3 — `request_callback` dispatché synchronement (race + violation thread) ✅ Corrigé

Spec CLAP : `request_callback` peut être appelée depuis n'importe quel thread. L'ancienne implémentation appelait `on_main_thread` synchronement → violation de contrat.

**Correctif appliqué** ([gui.rs](../../../src-tauri/src/plugin_system/gui.rs)) : `clap_host_request_callback` poste `WM_APP_DRAIN` via `PostMessageW` (thread-safe). Le WndProc draine `on_main_thread` sur le main thread dans les handlers `WM_TIMER` et `WM_APP_DRAIN`, en passant par le `tick` closure.

### A4 — `on_main_thread` appelé une seule fois après `show` ✅ Corrigé

**Correctif appliqué** : `SetTimer(hwnd, 1, 16ms, NULL)` posé dans `show_and_start_timer()`. `WM_TIMER` appelle `(ud.tick)()` → `plugin.on_main_thread`. `KillTimer` dans `WM_CLOSE` avant la cleanup.

---

## Bugs Win32 / lifecycle fenêtre

### W1 — WndProc actif pendant `attached`/`set_parent` sans USERDATA ✅ Corrigé

`CreateWindowExW` envoie WM_NCCREATE synchronement avant même de retourner le HWND. L'ancienne implémentation ne posait USERDATA qu'après `seal_and_show`, après `attached`/`set_parent` → UAF si WM_CLOSE arrivait pendant cette fenêtre.

**Correctif appliqué** ([gui.rs](../../../src-tauri/src/plugin_system/gui.rs)) : `WindowUserData` boxé passé via `lpParam` de `CreateWindowExW`. `WM_NCCREATE` l'installe immédiatement dans `GWLP_USERDATA` via `CREATESTRUCTW.lpCreateParams`. Aucun message ne peut plus arriver avec USERDATA = 0.

### W2 — WndProc ne validait pas l'identité de USERDATA ✅ Corrigé

**Correctif appliqué** : magic sentinel `USERDATA_MAGIC = 0xEC_E0_5E_50_15_00_F1_00` en tête de `WindowUserData`, validé par `userdata()` avant chaque déréférencement. `Box::from_raw` sur GWLP_USERDATA ne se fait qu'après vérification du magic.

### W3 — Pas de gestion DPI ✅ Corrigé (2026-09-25)

Le plugin n'était jamais informé de l'échelle de l'écran (`gui.set_scale` / `setContentScaleFactor` jamais appelés) et le cadre de fenêtre était calculé au DPI système (`AdjustWindowRectEx`). Sur écran 4K @ 200% : fenêtre minuscule, hit-testing décalé.

`SetThreadDpiAwarenessContext` s'est révélé inutile : tao (Tauri) met déjà le processus en `PER_MONITOR_AWARE_V2` au démarrage, donc le conteneur l'est aussi.

**Correctif appliqué** ([gui.rs](../../../src-tauri/src/plugin_system/gui.rs)) :

- DPI lu par fenêtre (`GetDpiForWindow`), cadre calculé avec `AdjustWindowRectExForDpi` (création + tout redimensionnement).
- CLAP : `set_scale(dpi/96)` puis `get_size` → resize, **avant** `set_parent` (ordre de la spec). Un plugin qui lit le DPI lui-même renvoie `false` et l'ignore.
- VST3 : QI de `IPlugViewContentScaleSupport` ([vst3/ffi.rs](../../../src-tauri/src/plugin_formats/vst3/ffi.rs)) → `setContentScaleFactor` après `attached`, puis `getSize` → resize. Interface optionnelle : absente = pas d'appel.
- `WM_DPICHANGED` (fenêtre déplacée vers un écran d'une autre échelle) : placement suggéré par Windows, puis même séquence échelle → taille.

À valider à la main : Vital (CLAP) et un VST3 JUCE sur écran 150–200 %, et déplacement entre deux écrans d'échelles différentes.

---

## Bugs VST3

### V1 — `comp.initialize` ignoré silencieusement ✅ Corrigé

**Correctif appliqué** ([gui.rs](../../../src-tauri/src/plugin_system/gui.rs)) : retour de `initialize` vérifié explicitement (erreur non-`kResultOk` / non-`kResultFalse` → bail). `HOST_APP` (IHostApplication statique minimal) passé comme contexte host : implémente `getName("Echoes Studio")` et renvoie `kResultFalse` pour `createInstance`.

### V2 — Aucun `IComponentHandler` setté ✅ Corrigé

**Correctif appliqué** : `set_component_handler` typé en slot 16 de `IEditControllerVtbl`. Statique `COMPONENT_HANDLER` (no-op `begin/perform/endEdit`, `restartComponent`) câblé immédiatement après QI de l'IEditController. Les plugins qui appellent `componentHandler->beginEdit(...)` sans null-check ne crashent plus.

### V3 — Aucun `IPlugFrame` setté ✅ Corrigé

**Correctif appliqué** : `PlugFrame` alloué par éditeur (`make_plug_frame()`), `set_frame` appelé avant `attached()`. `frame_resize_view` route vers `resize_to_client(container_hwnd, ...)` via `AtomicPtr<c_void>` câblé après `CreateWindowExW`. `set_frame(null)` dans le cleanup avant `removed()`.

### V4 — Modèle « single-component » exclusif ✅ Corrigé (2026-06-04)

Plugins dual-component (Steinberg pro, NI, **TDR Nova**) rejetés à l'ouverture de
l'éditeur : `IEditController not available via QI (single-component only)`.

**Correctif appliqué** :

- [vst3/ffi.rs](../../../src-tauri/src/plugin_formats/vst3/ffi.rs) : `set_component_state`
  typé (slot 5 de `IEditControllerVtbl`), `IConnectionPoint` (+IID), `Vst3Library::factory()`
  (réobtient la factory depuis la DLL déjà mappée), et `create_separate_controller` /
  `teardown_separate_controller`. La création suit la spec : `getControllerClassId` →
  `factory.createInstance(cid, IEditController)` → `initialize(host)` →
  `getState`→`setComponentState` (sync état processeur) → `IConnectionPoint.connect` ×2.
- [instance.rs](../../../src-tauri/src/plugin_system/instance.rs) : accesseur `vst3_library()`
  (clone l'`Arc<Vst3Library>` pour garder la DLL mappée).
- [gui.rs](../../../src-tauri/src/plugin_system/gui.rs) : `open_vst3_editor` tente d'abord le
  QI single-component ; en échec, crée un contrôleur séparé via `host_app_as_funknown()`
  comme contexte host. Le teardown correspondant (disconnect + terminate + release) est
  routé selon le chemin emprunté.

### V5 — Vtable `IEditController` / `IPlugView` sans assert de taille ✅ Corrigé

**Correctif appliqué** ([vst3/ffi.rs](../../../src-tauri/src/plugin_formats/vst3/ffi.rs)) :

```rust
const _: () = assert!(size_of::<IPlugViewVtbl>()      == 15 * size_of::<usize>(), "IPlugViewVtbl layout drift");
const _: () = assert!(size_of::<IEditControllerVtbl>() == 18 * size_of::<usize>(), "IEditControllerVtbl layout drift");
```

---

## Bugs CLAP

### CL1 — `plugin_slot: *mut *const ClapPlugin` non atomic ✅ Corrigé

**Correctif appliqué** : remplacé par `ClapHostShared { plugin: AtomicPtr<ClapPlugin>, host_hwnd: AtomicPtr<c_void> }` derrière `Arc`. Toutes les lectures/écritures inter-thread passent par `load/store(Ordering::Acquire/Release)`.

### CL2 — Ordre de drop incorrect dans cleanup ✅ Corrigé

**Correctif appliqué** : ordre dans la closure `cleanup` :

1. `shared.plugin.store(null)` → désactive le `tick` et tout callback entrant
2. `gui.hide` / `gui.destroy` → relâche les ressources fenêtre du plugin
3. `plugin.destroy` → garantit l'absence de callbacks après cette ligne
4. Drop des captures (`shared_for_cleanup`, `host_owned`, `lib_owned`, `clap_id_owned`) — ordre de drop Rust (inverse de déclaration)

### CL3 — `make_host` réinventé à chaque ouverture ✅ Corrigé (2026-09-25)

Après A1 (instance partagée), le problème restant était l'inverse : le plugin
gardait le host « nu » de l'instance audio, donc `request_resize` / `clap.log`
n'avaient aucune destination, et `request_callback` passait par un HWND
**global** unique (deux éditeurs CLAP ouverts → le premier ne recevait plus
son `on_main_thread`).

**Correctif appliqué** : `ClapHostContext` par instance derrière `host_data`
([clap_ffi](../../../src-tauri/crates/clap_ffi/src/lib.rs)), HWND de l'éditeur
enregistré par `open_clap_editor` ([gui.rs](../../../src-tauri/src/plugin_system/gui.rs)),
extensions host `clap.gui` et `clap.log` exposées. Le code « parqué » de gui.rs
a été supprimé.

### CL4 — `clap_id_c` liberé avant `destroy` ✅ Corrigé

**Correctif appliqué** : `clap_id_c` renommé `clap_id_owned`, capturé par move dans la closure `cleanup` → vivant jusqu'après `plugin.destroy`.

### CL5 — `Box<ClapHost>` déplacée sans Pin ⚠️ Non corrigé

Adresse heap stable par contrat de `Box` — OK en pratique, mais fragile. À résoudre avec `Pin<Box<ClapHost>>` lors de la refonte A1.

---

## Threading

### T1 — `run_on_main_thread` + `mpsc::recv` ⚠️ Non bloquant

En pratique Tauri exécute les commandes sur tokio off-main. Aucun deadlock observé.

### T2 — Double `WM_CLOSE` neutralisé ✅ OK

`SetWindowLongPtrW(…, 0)` + vérification du magic dans WM_CLOSE rendent le double-post idempotent.

---

## Plan de correction résiduel

1. ~~**W3**~~ corrigé (2026-09-25).
2. ~~**A1**~~ (ADR-016) et ~~**CL3**~~ (2026-09-25) corrigés. CL5 reste cosmétique : l'adresse d'une `Box` est stable tant qu'elle n'est pas réallouée.

---

## Tests de validation

- 50× ouvrir/fermer un éditeur sans fuite mémoire (Process Explorer)
- Vital (CLAP, strict thread) — crash initial résolu par C1
- Surge XT (CLAP de référence open-source)
- ValhallaSupermassive (VST3, exerce IComponentHandler)
- `appverif.exe` + flags Basics
- WinDbg `!gflag +hpa` (heap page allocator)
