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

**Bibliothèque de sons** (phases 1–3, 2026-09-30) : dock + préécoute WebAudio + DnD `ADD_CLIP` ; pop-up Bitwig dual-surface (`+` lane) ; factory accords/MIDI/drums + `ADD_MIDI_CLIP` + reject audio↔MIDI. Critères de sortie code-vérifiés ; drums hybrides livrés le 2026-10-13 (16 one-shots générés par code, pas encore testés à l'oreille) ; packs → phase 4. Voir [`docs/library-browser-roadmap.md`](docs/library-browser-roadmap.md).

---

## Prochaines étapes (ordre recommandé)

- [x] **Interface d'automation** : courbes dans l'arrangement, choix du paramètre (livré 2026-09-29 ; hand-test partiel)
- [x] **Enregistrement MIDI** dans un clip depuis le clavier (ADR-022 amendé ; pas encore hand-testé à l'oreille)
  - [x] Pitch bend, CC (pédale, molette…) et aftertouch enregistrés dans le clip et rejoués vers les instruments CLAP, lignes de contrôle dans la piano roll (ADR-029, 2026-10-13 ; pas encore testé avec un vrai clavier)
  - [ ] **Expressions par note** (style Bitwig / MPE) : courbes de pression, timbre et micro-pitch dessinées sur chaque note, contrôleurs MPE (Seaboard, Linnstrument…). Nouveau modèle de données porté par la note, événements CLAP note-expression, capture qui garde le canal MIDI — ADR dédié requis ; s'ajoute aux contrôles de canal d'ADR-029 sans les remplacer. **En cours (ADR-039, 2026-11-26)** : modèle et lecture vers les synthés CLAP livrés (étape A, pas encore entendu sur un vrai synthé) ; éditeur dans la piano roll livré (étape C, 2026-11-26 : lignes pression / timbre / micro-pitch qui éditent la note sélectionnée, pas encore essayé dans l'app) ; sandbox livré (étape B, 2026-11-27, protocole du bridge v5) ; entrée MPE par appareil (zone basse) et sa capture livrées (cycle 2, 2026-11-27, testées sur des messages simulés ; reste à valider avec un vrai contrôleur MPE)
  - [x] Suites des lignes de contrôle (2026-11-26, pas encore essayé dans l'app) : aftertouch polyphonique éditable (une ligne par touche jouée), courbes douces par Ctrl + Maj + glisser (cuites en paliers, sans changer le format), contrôles dessinés dans le clip de l'arrangement. Une forme de courbe par point, interpolée par le moteur, reste écartée (touche au budget d'événements RT et au harnais C-2)
- [x] **Enregistrement audio** : armement + capture via l'entrée CPAL, monitoring (ADR-027 ; pas encore hand-testé)
- [x] **CI**
  - [x] CI minimale (`.github/workflows/ci.yml` : fmt, clippy, tests Rust + harnais C-2, lint Prettier/ESLint, svelte-check, build frontend)
  - [x] Rendre Prettier/ESLint bloquants, après un passage de formatage sur tout le dépôt
- [x] **Transport auxiliaire**
  - [x] Métronome + count-in (ADR-026 ; pas encore hand-testé)
  - [x] Marqueurs / régions d'arrangement (event-sourcés en beats ; piste sous la règle ; boucle depuis une région via `SET_LOOP` ; 2026-09-30)
- [ ] **Compléter les plugins** : VST3 (bus auxiliaires, latence dynamique, sandbox), AU (macOS, stub), synthés natifs supplémentaires — sampler à pads livré (ADR-028, 2026-10-09, pas encore testé à l'oreille) ; kit d'usine de 16 sons générés par code, chargé à la sélection du Sampler et listé dans la bibliothèque (2026-10-13, pas encore testé à l'oreille)

---

## Version 1.0 — Stabilité et finition

**Critère de passage :** tests couvrant le moteur et le frontend, aucun crash connu, prise en main rapide.

- [ ] Onboarding au premier lancement
- [x] Raccourcis clavier configurables
- [ ] Profilage et optimisation (worker pool, suréchantillonnage). Fait : télémétrie XRun dans l'interface (2026-11-27, compteur dans la barre d'état dès le premier XRun, même store que les Préférences audio ; pas encore essayé dans l'app). Mesuré le 2026-11-27 (rapports `#[ignore]` du harnais) : les workers donnent 2 à 4× en rendu offline, sortie bit-identique ; le suréchantillonnage coûte 0,05 % du budget, rien à optimiser. Reste : décider d'activer les workers par défaut, après un essai live avec le compteur XRun (reste à valider par toi)
- [x] Tests : ~270 tests Rust ; suite Vitest frontend (reducer événementiel ADR-005, arrangement, library, piano roll, automation, sélection) — `pnpm test:frontend` / CI
- [ ] Documentation utilisateur
- [ ] Mise à jour automatique (Tauri updater) et signature des binaires (Authenticode, notarisation). Fait (2026-11-28) : plugin updater, dialogue de mise à jour qui refuse d'installer avec des modifs non sauvegardées, workflow `release.yml` (installateur NSIS signé pour l'updater, release en brouillon), clé publique dans `tauri.conf.json`, procédure dans `docs/RELEASE.md`. Chaîne essayée le 2026-11-28 : `v0.1.0` installée, mise à jour vers `v0.1.1` constatée par toi (dépôt public, secrets posés). Pas vérifié : le refus d'installer avec des modifications non sauvegardées. Reste hors périmètre : certificat Authenticode (acheté, puis secrets `WINDOWS_CERTIFICATE*`) et notarisation macOS (pas de build macOS)
- [x] Internationalisation (EN + FR) — `svelte-i18n` ; menus contextuels, préférences projet, library, piano roll, dialogs, menus natifs OS
- [ ] Format de projet v2 — décision ouverte (voir « ADR-004 — Project file format » dans [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), à ne pas confondre avec l'ADR-004 de `docs/active/decisions/` sur les snapshots du moteur)
- [ ] Nettoyage du code hérité (phase 7 du plan de migration). Audit du 2026-11-27 : le fallback recompile et le double matcher d'instrument sont déjà retirés, `DeviceState` et `ModulationMap` sont vivants, les `dead_code` restants sont des variantes réservées du contrat du graphe ; rien de prouvé mort à supprimer. Reste : attendre ≥ 2 cycles de release en production (`migration_plan.md` § 7.4)

---

## Au-delà de la 1.0

- Répétitions de clips MIDI liées (alias à la Logic) — ADR préalable requis
- Warp / étirement de loops à la dépose — ADR préalable requis
- Collaboration en temps réel, marketplace de presets, scripting, application compagnon

---

## Suivi

Issues GitHub taguées par phase (`phase/1`, `phase/2`…) et par nature (`feature`, `bug`, `tech-debt`, `design`) :
[Issues ouvertes](https://github.com/echoes-studio/echoes-studio/issues)
