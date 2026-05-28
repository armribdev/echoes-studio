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
- [x] **Moteur audio natif CPAL** (`audio_engine/engine.rs`) — lecture temps-réel native
  - `SharedTransport` : état de transport atomique (play/pause/seek, position en samples)
  - `RuntimeSnapshot` : swap lock-free du `GraphScheduler` entre thread commande et callback
  - Callback RT-safe : buffer pré-alloué (4096 frames max), aucune allocation, aucun verrou bloquant, aucun I/O
  - Reporter de position (~30 fps) → événement `POSITION_UPDATED` → tête de lecture animée
  - `AudioGraphCompiler` : compile le projet en `GraphScheduler` à la volée (topological sort, Kahn BFS)
  - Sélection du périphérique de sortie CPAL, taux d'échantillonnage (44.1–96 kHz), taille de buffer
  - Fallback automatique buffer fixé → buffer par défaut si le périphérique ne supporte pas la valeur demandée
  - Web Audio API relégué au seul chemin d'export offline (`OfflineAudioContext`)
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

### Restant
- [ ] **Multi-format audio** — support MP3, FLAC, OGG en plus de WAV (via `symphonia`)
- [ ] **Export MP3** — encodage après mixdown WAV
- [ ] **Couper / Copier / Coller des clips** — sélection et suppression OK, clipboard absent
- [ ] **Zoom vertical** de la timeline — hauteur de piste fixe (100 px)
- [ ] **Mute / Solo undoable** — non intégré à l'event sourcing pour l'instant

---

## Phase 3 — v0.5 : Enregistrement & Effets

**Objectif :** Enregistrer depuis un microphone et appliquer des effets basiques.

**Critère de passage :** Enregistrer une voix, ajouter une réverb, exporter.

### Fonctionnalités
- [ ] **Enregistrement audio** — arm + record sur une piste via `cpal` input (device déjà sélectionnable dans les préférences)
- [ ] **Monitoring** — écoute en temps réel pendant l'enregistrement
- [ ] **Effets intégrés** : égaliseur, compresseur, réverbération, délai
- [ ] **Insert chain** — chaîner plusieurs effets par piste
- [ ] **Automation** — courbes d'automation pour volume et paramètres d'effets
- [ ] **Marqueurs / Régions** — points de repère sur la timeline
- [ ] **Métronome** — clic synchronisé au BPM

---

## Phase 4 — v0.8 : MIDI & Instruments

**Objectif :** Composition MIDI avec instruments virtuels.

**Critère de passage :** Jouer un pattern MIDI sur un instrument virtuel intégré.

### Fonctionnalités
- [ ] **Piano roll** — éditeur MIDI intégré
- [ ] **Support MIDI** — entrée/sortie MIDI (clavier, contrôleur)
- [ ] **Instruments virtuels natifs** : sampler simple, synthétiseur basique
- [ ] **[TODO: Décision] Support VST3** — plugins tiers (architecture `plugin_formats/vst3/` scaffoldée, implique gestion de dépendances lourdes)
- [ ] **[TODO: Décision] Support AU** (macOS) — architecture `plugin_formats/au/` scaffoldée
- [ ] **[TODO: Décision] Support WASM interne** — `plugin_formats/internal/` avec runtime WASM scaffoldé
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
- [ ] **Tests** — unitaires Rust + tests d'intégration Svelte
- [ ] **Documentation utilisateur** — guide d'utilisation interactif
- [ ] **Auto-update** — mise à jour intégrée via Tauri updater
- [ ] **Signature des binaires** — Windows (Authenticode), macOS (notarisation)
- [ ] **Internationalisation** — EN + FR minimum

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
