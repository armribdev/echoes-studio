# Roadmap — Echoes Studio

> Ce document décrit les phases de développement d'Echoes Studio, du prototype actuel jusqu'à la v1.0 et au-delà. Les priorités reflètent la vision d'un DAW radicalement simple et fonctionnel.

---

## Phase 0 — Fondations ✅ *complète*

**Objectif :** Scaffolding technique, architecture initiale, proof of concept UI.

### Réalisé
- [x] Setup Tauri 2.x + SvelteKit 2 + Rust
- [x] Composants UI de base : Track, Clip, MenuBar, StatusBar, Knob
- [x] Parsing WAV via `hound` (Rust)
- [x] Bridge IPC Tauri : chargement de fichier audio
- [x] Rendu waveform canvas via Web Worker (`waveform.worker.ts`)
- [x] Zoom timeline
- [x] Nom de piste éditable
- [x] Thème sombre et système de couleurs
- [x] State management : Svelte stores + event sourcing (gateway → reducer → engineStore)
- [x] Nettoyage du scaffolding (worker JS déprécié supprimé, identifiant Tauri corrigé)

---

## Phase 1 — MVP : Lecture & Chargement ✅ *complète*

**Objectif :** Un utilisateur peut charger des fichiers audio, les placer sur la timeline, et les lire.

**Critère de passage :** Charger un WAV, le placer sur une piste, appuyer sur Play → le son sort.

### Réalisé
- [x] **Lecture audio** via Web Audio API (chemin export uniquement depuis la migration CPAL)
  - Transport : Play / Pause / Stop / Rewind unifié via dispatcher
  - Tête de lecture animée sur la timeline
  - Raccourci Espace pour play/pause
- [x] **Drag & drop fichiers** — glisser un `.wav` depuis l'explorateur vers une piste
- [x] **File picker natif** — menu « Importer audio… » (Ctrl+Shift+I) via `tauri-plugin-dialog`
- [x] **Positionnement des clips** — placer et déplacer un clip sur la timeline
- [x] **Règle temporelle** — affichage mesures + temps (BPM configurable, signature rythmique)
- [x] **Snap to grid** — accrochage à la grille temporelle (Alt pour désactiver)
- [x] **Volume par piste** — GainNode Web Audio, mise à jour en temps réel
- [x] **Save / Load projet** — format `.echoes` (JSON) via Ctrl+S / Ctrl+Shift+S / menu
- [x] **Undo / Redo** — Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z (event sourcing Rust)
- [x] **Feedback utilisateur** — status bar pour erreurs, confirmations save/load
- [x] **Gestion d'erreur IPC** — erreurs d'import et d'undo/redo visibles dans l'UI
- [x] **Moteur audio natif CPAL** (`audio_engine/`) — lecture temps-réel native
  - `SharedTransport` : état de transport atomique (play/pause/seek, position en samples)
  - `RuntimeSnapshot` : swap lock-free du `GraphScheduler` entre thread commande et callback audio
  - Callback RT-safe : buffer pré-alloué (4096 frames max), aucune allocation, aucun verrou bloquant, aucun I/O
  - Reporter de position (~30 fps) → événement `POSITION_UPDATED` → tête de lecture animée
  - Sélection du périphérique de sortie CPAL, taux d'échantillonnage (44.1–96 kHz), taille de buffer
  - Fallback automatique buffer fixé → buffer par défaut si le périphérique ne supporte pas la valeur demandée
  - Web Audio API relégué au seul chemin d'export offline (`OfflineAudioContext`)
- [x] **Moteur de graphe DSP** (`audio_graph/`) — exécution déterministe du graphe audio
  - Trait `AudioNode` : interface uniforme pour tous les nœuds DSP (prepare / process)
  - `AudioBuffer` : layout planaire f32, opérations sans allocation (clear, mix_from, apply_gain)
  - `GraphCompiler` : tri topologique (algorithme de Kahn BFS), plan d'exécution immuable
  - `GraphScheduler` : boucle d'exécution du plan, buffers pré-alloués par nœud et par port
  - `ExecutionPlan` : `Arc<>` partagé entre thread commande et thread audio, zéro copie
  - Nœuds intégrés : `TrackNode`, `GainNode`, `PanNode`, `MeterNode`, `BusNode`, `MasterNode`
  - Tri stable par `NodeId` → rendu bit-identique entre les compilations
  - Modèle de sécurité des buffers : invariant `src_step < current_step` garanti par le compilateur
- [x] **Event sourcing complet** (`engine_gateway/`) — mutations projet via log d'événements
  - `EventStore` : log immuable avec pointeur `head`, snapshots périodiques
  - Batch commands : plusieurs mutations en un seul aller-retour IPC, une seule recompilation
  - `AudioDataCache` : données WAV préservées à travers les undo/redo (pas de relecture disque)
  - Frontend : `ProjectEventStore` + reducer pur `applyEvent(state, event) → state`
  - Protocole typé (`CommandEnvelope` / `EngineEvent`) partagé Rust ↔ TypeScript
  - Waveform peaks précalculées à 3 résolutions (64 / 512 / 4096 samples) pour zoom rapide
- [x] **UI « Ajouter une piste »** — bouton dans le panneau gauche, menu natif et menu contextuel
  - Bouton **"+ Nouvelle piste"** dans le footer du panneau gauche
  - Sous-menu natif **Piste** : "Nouvelle piste" (`Ctrl+Shift+T`) + "Supprimer la piste sélectionnée"
  - État vide de la timeline avec illustration, description et CTA "Nouvelle piste"
  - Suppression de piste depuis le menu natif ou le menu contextuel

---

## Phase 2 — v0.3 : Timeline Pro & Mixage basique *(en cours)*

**Objectif :** Une timeline exploitable avec gestion basique du mix.

**Critère de passage :** Composer un arrangement multi-piste, ajuster les volumes, exporter.

### Réalisé
- [x] **Pan par piste** — `StereoPannerNode` Web Audio, Knob UI, persistance via `UPDATE_TRACK`
- [x] **Mute / Solo** par piste — boutons M/S, commands `MUTE_TRACK` / `SOLO_TRACK`, solo exclusif côté Rust
- [x] **Export audio WAV** — `OfflineAudioContext` mixdown + `export_wav` Rust, bouton « ↓ WAV » (Ctrl+Shift+E)
- [x] **Panneau Préférences** (`SettingsPanel.svelte`) — interface multi-sections complète *(livré en avance de Phase 5)*
  - Section *Audio* : sélecteurs périphérique IN/OUT via CPAL (`listAudioDevices`), fréquence d'échantillonnage (44.1 / 48 / 88.2 / 96 kHz), taille de buffer (64–2048 frames), profondeur de bits (16/24/32), canaux IN/OUT configurables, routage Master L/R et Casque, jauge de latence estimée avec code couleur (vert < 10 ms / jaune < 25 ms / rouge ≥ 25 ms)
  - Section *Projet* : dossier de projets par défaut, sauvegarde automatique (intervalle + nombre max de backups), BPM et signature rythmique pour les nouveaux projets
  - Section *Interface* : mini-carte de timeline (toggle)
  - Application à chaud : redémarre le flux CPAL + recompile et pousse le graphe audio sans interruption visible
  - Bridge TypeScript `audioBackend.ts` : `listAudioDevices()` / `applyAudioSettings()`
  - Commandes Rust : `get_audio_devices`, `apply_audio_settings` (persistance + redémarrage stream)
- [x] **Menu contextuel piste** (`TrackContextMenu.svelte`) — clic droit sur une piste
  - Actions : Nouvelle piste, Renommer, Supprimer
  - Renommage inline via `contentEditable` directement sur le label de piste
- [x] **Sélection de piste** — surlignage visuel de la piste active (clic), propagation aux commandes "Supprimer"
- [x] **Couleurs par piste** — barre latérale colorée (palette de 8 couleurs cycliques)
- [x] **Refonte composant `Knob`** — rendu SVG arc (piste grise + arc accent)
  - Géométrie 298.8° (7:30 → 4:30 dans le sens horaire)
  - Double-clic pour réinitialiser à `defaultValue`
  - Labels `VOL` / `PAN` au-dessus des knobs
- [x] **Référentiel architectural `docs/claude/`** — mémoire long terme pour les sessions Claude Code
  - `architecture/` : audio-graph, plugin-system, threading-model, state-management
  - `rules/` : realtime-safety, anti-patterns, modulation-rules, architecture-decision-rules
  - `systems/` : graph-compiler, plugin-wrapper-system, dsp-runtime
  - `decisions/` : 5 ADR (graph-first, event-sourcing scope, plugin wrapper, snapshots immuables, UI/engine séparation)
- [x] **Système de modulation — structures de données** (`audio_graph/modulation.rs`)
  - `ModulationRoute`, `ModulationSource`, `ModulationTarget`, `ModulationMap` implémentés
  - Intégré dans `ExecutionPlan` ; zéro routes actives pour l'instant (runtime à implémenter)
- [x] **Multi-format audio** — WAV, MP3, FLAC, OGG décodés via `symphonia` (`audio_clip.rs`)
- [x] **Export MP3** — encodage `mp3lame-encoder` (commande `export_mp3`), bitrate configurable depuis les préférences
- [x] **Couper / Copier / Coller des clips** — Ctrl+X / Ctrl+C / Ctrl+V + menu contextuel clic droit sur clip
- [x] **Vumètres UI** — `MeterNode` → `meter_cells` → reporter (`POSITION_UPDATED` / `LEVELS_UPDATED`) → store `trackLevels` → `FaderMeter` par piste (L/R avec décroissance logicielle)

- [x] **Mute / Solo undoable** — `TRACK_MUTED` / `TRACK_SOLOED` intégrés à l'event sourcing (`append_and_maybe_snapshot`) ; undo/redo via replay du log

---

## Phase 3 — v0.5 : Enregistrement & Effets

**Objectif :** Enregistrer depuis un microphone et appliquer des effets basiques.

**Critère de passage :** Enregistrer une voix, ajouter une réverb, exporter.

### Fonctionnalités
- [ ] **Enregistrement audio** — arm + record sur une piste via `cpal` input (device déjà sélectionnable dans les préférences)
- [ ] **Monitoring** — écoute en temps réel pendant l'enregistrement
- [x] **Effets intégrés** : égaliseur, compresseur, réverbération, délai
  - Chaque effet = un `AudioNode` dans le graphe (graph-first, pas de chaîne d'inserts spéciale)
- [x] **Routing d'effets via le graphe** — chaîne d'inserts par piste, wired dans le graphe DSP
- [x] **Runtime de modulation** — `ModulationRoute` dispatché dans le scheduler ; LFO sine/triangle/saw/square
- [ ] **Automation** — courbes d'automation pour volume et paramètres d'effets
  - Mécanisme unifié avec la modulation : `ParameterBlock` par frame
- [ ] **Marqueurs / Régions** — points de repère sur la timeline
- [ ] **Métronome** — clic synchronisé au BPM
- [ ] **Thread compilateur dédié** — déplacer la compilation du graphe hors du thread commande pour éviter les blocages IPC sur les gros projets
- [ ] **Compensation de latence** — collecte des `latency_samples()` des nœuds, délais d'alignement au master

---

## Phase 4 — v0.8 : MIDI & Instruments

**Objectif :** Composition MIDI avec instruments virtuels.

**Critère de passage :** Jouer un pattern MIDI sur un instrument virtuel intégré.

### Fonctionnalités
- [x] **Piano roll** — éditeur MIDI intégré (canvas, optimistic UI, event-sourced via `SET_MIDI_NOTES`)
- [ ] **Support MIDI** — entrée/sortie MIDI (clavier, contrôleur)
- [ ] **Instruments virtuels natifs** : sampler simple, synthétiseur basique
- [ ] **Support VST3** — `plugin_formats/vst3/` scaffoldé (`ffi.rs`, `host.rs`, `wrapper.rs`)
  - Trait `Device` (extends `AudioNode`) et `PluginHost` prêts (`plugin_system/`)
  - `ParameterMap`, `LifecycleState`, `PluginRegistry`, `DeviceState` implémentés
  - À faire : bindings COM VST3, traduction buffers, mirroring paramètres, transport sync
- [ ] **Support AU** (macOS) — `plugin_formats/au/` scaffoldé
- [ ] **Support WASM interne** — `plugin_formats/internal/wasm_runtime.rs` scaffoldé
- [ ] **Transfert d'état DSP** à la recompilation — préserver reverb tails, delay lines entre les swaps de graphe
- [ ] **Hébergement hors-processus (best effort)** — isolation crash VST3 via subprocess + shared-memory ring buffers
- [ ] **Quantisation MIDI**
- [ ] **Velocity, pitch bend, modulation**

---

## Phase 5 — v1.0 : Stabilité & Polish

**Objectif :** Version publique stable, documentée, testée.

**Critère de passage :** Couverture de tests ≥ 70%, zéro crash connu, onboarding ≤ 30 secondes.

### Fonctionnalités
- [ ] **Onboarding interactif** — tutoriel en contexte au premier lancement
- [x] **Gestion des préférences** — panneau Préférences complet (audio, projet, interface) *(implémenté dès la Phase 2)*
- [ ] **Raccourcis clavier** configurables
- [ ] **Thèmes UI** — clair / sombre / custom
- [ ] **Performances** — profiling et optimisation audio engine
- [ ] **Tests** — unitaires Rust (`audio_graph/`, `engine_gateway/`) + tests d'intégration Svelte
- [ ] **Documentation utilisateur** — guide d'utilisation interactif
- [ ] **Auto-update** — mise à jour intégrée via Tauri updater
- [ ] **Signature des binaires** — Windows (Authenticode), macOS (notarisation)
- [ ] **Internationalisation** — EN + FR minimum
- [ ] **Format de projet v2** — décision ouverte : JSON texte vs MessagePack vs SQLite (voir ADR-004 dans `docs/ARCHITECTURE.md`)

---

## Au-delà de la v1.0

- Collaboration en temps réel *(vision long terme)*
- Marketplace de presets et samples
- Scripting / extension system
- Support AudioUnit v3 (macOS)
- Application mobile compagnon (capture d'idées)

---

## Suivi des priorités

Les issues GitHub sont taguées avec le numéro de phase (`phase/1`, `phase/2`, etc.) et la nature (`feature`, `bug`, `tech-debt`, `design`).

→ [Issues ouvertes](https://github.com/echoes-studio/echoes-studio/issues)
