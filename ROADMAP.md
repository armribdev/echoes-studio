# Roadmap — Echoes Studio

> Feuille de route **produit** : ce qui est livré et ce qui vient ensuite. Mise à jour le 2026-09-30.
> Le détail moteur (plugins, modulation, sandbox) est dans [`docs/active/migration/migration_plan.md`](docs/active/migration/migration_plan.md) ; la bibliothèque de sons dans [`docs/library-browser-roadmap.md`](docs/library-browser-roadmap.md). Les statuts par sous-système sont dans [`.claude/state_index.md`](.claude/state_index.md).

---

## Livré

**Fondations et moteur (Phases 0–1)**
- Tauri 2 + SvelteKit 2 + Rust ; event sourcing complet (undo/redo, sauvegarde `.echoes`).
- Moteur audio natif CPAL, graphe DSP déterministe compilé, swap de snapshot sans verrou, rendu offline identique au live (harnais C-2).
- Import audio multi-format (WAV, MP3, FLAC, OGG), pics de forme d'onde, PCM progressif à l'import (ADR-024), export WAV/MP3.

**Mixage et arrangement (Phase 2)**
- Pistes : volume, pan, mute/solo, couleurs, vumètres, menus contextuels ; clips audio et MIDI (couper/copier/coller, trim, duplication Alt+glisser, répétition, accrochage magnétique, boucle de lecture).
- Préférences audio/projet/interface, périphériques CPAL, latence estimée.

**Effets et modulation (Phase 3, partielle)**
- Égaliseur, compresseur, réverbération, délai ; compensation de latence (PDC).
- Modulation sous-bloc : LFO (formes, synchro tempo), enveloppe, suiveur audio, macro ; assignation par glisser-déposer.
- Automation : moteur + premier éditeur dans l'arrangement (pistes, points, crayon, cibles absolues ADR-025).

**MIDI et instruments (Phase 4, largement livrée)**
- Piano roll, synthé intégré (PolySynth), entrée MIDI matérielle en direct (ADR-022).
- Hébergement de plugins CLAP et VST3 (effets et instruments CLAP), paramètres, état, éditeur natif, sidechain et sorties multiples CLAP.
- Isolation des plugins CLAP dans un processus séparé (opt-in, ADR-015).

**Bibliothèque de sons** : phase 1 en cours (panneau, glisser-déposer, préécoute).

---

## Prochaines étapes (ordre recommandé)

- [ ] **Bibliothèque**
  - [ ] Clôturer la phase 1 (hand-test des critères de sortie ; panneau / DnD / préécoute déjà en place)
  - [ ] Phase 2 (pop-up d'insertion)
  - [ ] Phase 3 (accords / MIDI)
- [x] **Interface d'automation** : courbes dans l'arrangement, choix du paramètre (livré 2026-09-29 ; hand-test partiel)
- [x] **Enregistrement MIDI** dans un clip depuis le clavier (ADR-022 amendé ; pas encore hand-testé à l'oreille)
- [ ] **Enregistrement audio** : armement + capture via l'entrée CPAL, monitoring. Rien n'existe dans le code aujourd'hui.
- [ ] **CI**
  - [x] CI minimale (`.github/workflows/ci.yml` : fmt, clippy, tests Rust + harnais C-2, svelte-check, build frontend)
  - [ ] Rendre Prettier/ESLint bloquants, après un passage de formatage sur tout le dépôt
- [ ] **Transport auxiliaire**
  - [x] Métronome + count-in (ADR-026 ; pas encore hand-testé)
  - [ ] Marqueurs / régions d'arrangement
- [ ] **Compléter les plugins** : VST3 (bus auxiliaires, latence dynamique, sandbox), AU (macOS, stub), synthés natifs supplémentaires (sampler)

---

## Version 1.0 — Stabilité et finition

**Critère de passage :** tests couvrant le moteur et le frontend, aucun crash connu, prise en main rapide.

- [ ] Onboarding au premier lancement
- [ ] Raccourcis clavier configurables
- [ ] Thèmes UI (clair / sombre / personnalisé)
- [ ] Profilage et optimisation (worker pool, suréchantillonnage, télémétrie XRun dans l'interface)
- [ ] Tests : environ 270 tests Rust aujourd'hui ; ajouter des tests d'intégration frontend
- [ ] Documentation utilisateur
- [ ] Mise à jour automatique (Tauri updater) et signature des binaires (Authenticode, notarisation)
- [ ] Internationalisation (EN + FR)
- [ ] Format de projet v2 — décision ouverte (voir « ADR-004 — Project file format » dans [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), à ne pas confondre avec l'ADR-004 de `docs/active/decisions/` sur les snapshots du moteur)
- [ ] Nettoyage du code hérité (phase 7 du plan de migration)

---

## Au-delà de la 1.0

- Répétitions de clips MIDI liées (alias à la Logic) — ADR préalable requis
- Warp / étirement de loops à la dépose — ADR préalable requis
- Collaboration en temps réel, marketplace de presets, scripting, application compagnon

---

## Suivi

Issues GitHub taguées par phase (`phase/1`, `phase/2`…) et par nature (`feature`, `bug`, `tech-debt`, `design`) :
[Issues ouvertes](https://github.com/echoes-studio/echoes-studio/issues)
