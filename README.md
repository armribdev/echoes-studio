# Echoes Studio

> A radically simple open-source DAW — designed from scratch to get out of your way.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Build](https://img.shields.io/github/actions/workflow/status/armribdev/echoes-studio/ci.yml?branch=master)](https://github.com/armribdev/echoes-studio/actions/workflows/ci.yml)
[![Version](https://img.shields.io/badge/version-0.1.0--alpha-orange)](ROADMAP.md)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)](#installation)

---

<!-- [TODO: Remplacer par une vraie capture d'écran] -->
```
┌──────────────────────────────────────────────────────┐
│                                                      │
│            Echoes Studio — screenshot                │
│                                                      │
└──────────────────────────────────────────────────────┘
```

---

## Philosophie

La plupart des DAW ont été construits par accumulation de fonctionnalités sur vingt ans. Le résultat : des interfaces de cockpit d'avion, une courbe d'apprentissage abrupte, et des workflows qui forcent l'utilisateur à penser comme le logiciel — pas comme un musicien.

Echoes Studio part d'un postulat inverse : **chaque interaction doit être évidente au premier regard.** Pas de menus profonds, pas de boîtes de dialogue inutiles, pas de paramètres cachés derrière des onglets. L'outil doit disparaître pour que la musique apparaisse.

**Principes fondateurs :**
- **Zéro friction d'entrée** — ouvrir l'app et enregistrer en moins de 10 secondes
- **Cohérence absolue** — le même geste fait toujours la même chose, partout
- **Densité maîtrisée** — ne montrer que ce qui est pertinent au contexte
- **Accessible à tous les niveaux** — du débutant au producteur professionnel

→ Philosophie UX complète : [docs/UI_GUIDELINES.md](docs/UI_GUIDELINES.md)

---

## Stack Technique

| Couche | Technologie |
|--------|-------------|
| Desktop runtime | [Tauri](https://tauri.app) 1.x |
| UI | [SvelteKit](https://kit.svelte.dev) 2 + TypeScript |
| Audio engine | Rust + [cpal](https://github.com/RustAudio/cpal) |
| Styling | SCSS |

→ Détail et justifications : [docs/TECH_STACK.md](docs/TECH_STACK.md)

---

## Installation

### Prérequis

- [Node.js](https://nodejs.org) ≥ 18
- [pnpm](https://pnpm.io) ≥ 8
- [Rust](https://rustup.rs) stable (≥ 1.60 — installé automatiquement via `rust-toolchain.toml`)
- Dépendances système Tauri :
  - **Linux** : `libwebkit2gtk-4.0-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`
  - **macOS** : Xcode Command Line Tools
  - **Windows** : Microsoft C++ Build Tools + WebView2 Runtime

Guide détaillé d'installation par OS : [docs/BUILD.md](docs/BUILD.md).

```bash
# Cloner le dépôt
git clone https://github.com/[TODO:org]/echoes-studio.git
cd echoes-studio

# Installer les dépendances frontend
pnpm install

# (Optionnel mais recommandé) Vérifier l'environnement
pnpm check-env

# Lancer en mode développement (Tauri + Vite hot-reload)
pnpm tauri:dev
```

`pnpm tauri:dev` lance automatiquement `check-env` en mode silencieux avant le build — il échoue tôt avec un message clair si un outil manque.

### Build de production

```bash
pnpm tauri:build
# Bundles générés dans src-tauri/target/release/bundle/
```

---

## Premiers pas

1. **Lancer Echoes Studio** via `pnpm tauri:dev`
2. **Charger un fichier audio** — glisser-déposer un `.wav` dans la zone de pistes *(à venir)*
3. **Lire le projet** — `Espace` pour play/pause *(à venir)*
4. **Enregistrer** — `R` pour armer l'enregistrement *(à venir)*

---

## Roadmap

Echoes Studio est en **alpha précoce**. Le moteur de lecture audio est en cours d'implémentation.

→ Phases de développement détaillées : [ROADMAP.md](ROADMAP.md)

---

## Contribuer

Contributions bienvenues : code, design, documentation, retours utilisateurs.

→ Guide complet : [CONTRIBUTING.md](CONTRIBUTING.md)

---

## Documentation

| Document | Description |
|----------|-------------|
| [ROADMAP.md](ROADMAP.md) | Phases MVP → v1.0 et priorités |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Guide de contribution |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Architecture et décisions techniques |
| [docs/TECH_STACK.md](docs/TECH_STACK.md) | Stack et justifications des choix |
| [docs/UI_GUIDELINES.md](docs/UI_GUIDELINES.md) | Principes UX et système de design |
| [docs/AUDIO_ENGINE.md](docs/AUDIO_ENGINE.md) | Moteur audio : specs et design |

---

## Licence

MIT © [TODO: Nom / Organisation]
