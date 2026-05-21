# Roadmap — Echoes Studio

> Ce document décrit les phases de développement d'Echoes Studio, du prototype actuel jusqu'à la v1.0 et au-delà. Les priorités reflètent la vision d'un DAW radicalement simple et fonctionnel.

---

## Phase 0 — Fondations *(état actuel)*

**Objectif :** Scaffolding technique, architecture initiale, proof of concept UI.

### Réalisé
- [x] Setup Tauri 1.x + SvelteKit 2 + Rust
- [x] Composants UI de base : Track, Clip, MenuBar, StatusBar, Knob
- [x] Parsing WAV via `hound` (Rust)
- [x] Bridge IPC Tauri : chargement de fichier audio
- [x] Rendu waveform canvas via Web Worker
- [x] Zoom timeline
- [x] Nom de piste éditable
- [x] Thème sombre et système de couleurs

### En cours
- [ ] Nettoyage du scaffolding (suppression worker JS déprécié, chemins hardcodés)
- [ ] Mise en place du state management (Svelte stores)

---

## Phase 1 — MVP : Lecture & Chargement *(priorité absolue)*

**Objectif :** Un utilisateur peut charger des fichiers audio, les placer sur la timeline, et les lire.

**Critère de passage :** Charger un WAV, le placer sur une piste, appuyer sur Play → le son sort.

### Fonctionnalités
- [ ] **Lecture audio** — implémentation `cpal` playback (débloquant pour tout le reste)
  - Transport : Play / Pause / Stop / Rewind
  - Tête de lecture animée sur la timeline
- [ ] **File picker natif** — dialog Tauri pour sélectionner des fichiers `.wav`
- [ ] **Règle temporelle** — affichage temps (secondes) et mesures (BPM configurable)
- [ ] **State management** — migration vers Svelte stores (`tracks`, `playhead`, `transport`)
- [ ] **Drag & drop fichiers** — glisser un fichier depuis l'explorateur vers une piste
- [ ] **Positionnement des clips** — placer un clip à un instant précis sur la timeline

### Technique
- [ ] Supprimer `waveform-worker.js` (remplacé par `waveform.worker.ts`)
- [ ] Remplacer l'identifiant Tauri `com.tauri.dev` par `com.echoes-studio.app`
- [ ] Corriger le rendu waveform (canvas trop petit, 100×100)
- [ ] Gestion d'erreur IPC : feedback utilisateur sur les erreurs de chargement

---

## Phase 2 — v0.3 : Timeline Pro & Mixage basique

**Objectif :** Une timeline exploitable avec gestion basique du mix.

**Critère de passage :** Composer un arrangement multi-piste, ajuster les volumes, exporter.

### Fonctionnalités
- [ ] **Multi-format audio** — support MP3, FLAC, OGG en plus de WAV
- [ ] **Volume & Pan par piste** — fader + panoramique
- [ ] **Mute / Solo** par piste
- [ ] **Snap to grid** — alignement sur la grille temporelle
- [ ] **Couper / Copier / Coller des clips**
- [ ] **Undo / Redo** — historique d'actions (Ctrl+Z / Ctrl+Y)
- [ ] **Export audio** — mixdown stéréo vers WAV/MP3
- [ ] **Save / Load projet** — format JSON ou binaire custom
- [ ] **Zoom horizontal et vertical** de la timeline

---

## Phase 3 — v0.5 : Enregistrement & Effets

**Objectif :** Enregistrer depuis un microphone et appliquer des effets basiques.

**Critère de passage :** Enregistrer une voix, ajouter une réverb, exporter.

### Fonctionnalités
- [ ] **Enregistrement audio** — arm + record sur une piste via `cpal` input
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
- [ ] **[TODO: Décision] Support VST3** — plugins tiers (implique gestion de dépendances lourdes)
- [ ] **[TODO: Décision] Support LV2** (Linux) / **AU** (macOS)
- [ ] **Quantisation MIDI**
- [ ] **Velocity, pitch bend, modulation**

---

## Phase 5 — v1.0 : Stabilité & Polish

**Objectif :** Version publique stable, documentée, testée.

**Critère de passage :** Couverture de tests ≥ 70%, zéro crash connu, onboarding ≤ 30 secondes.

### Fonctionnalités
- [ ] **Onboarding interactif** — tutoriel en contexte au premier lancement
- [ ] **Gestion des préférences** — fenêtre settings (audio device, buffer size, thème)
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

→ [Issues ouvertes](https://github.com/[TODO:org]/echoes-studio/issues)
