# Tech Stack — Echoes Studio

> Justification de chaque choix technologique, alternatives considérées, et contraintes cross-platform.

---

## Vue d'ensemble

| Couche | Technologie | Version | Rôle |
|--------|-------------|---------|------|
| Desktop runtime | Tauri | 1.5.4 | Shell natif, IPC, permissions |
| UI Framework | SvelteKit | 2.0.0 | Interface utilisateur |
| Langage UI | TypeScript | 5.0 | Typage statique frontend |
| Styling | SCSS/Sass | 1.69 | Styles avec variables |
| Waveform | wavesurfer.js | 7.7.1 | Rendu waveform canvas |
| Build tool | Vite | 5.0.3 | Bundler frontend |
| Backend | Rust | 2021 edition | Audio engine, I/O |
| Audio I/O | cpal | 0.15.2 | Stream audio cross-platform |
| WAV parsing | hound | 3.5.1 | Lecture/écriture WAV |
| Sérialisation | serde / serde_json | 1.0 | IPC JSON |
| Package manager | pnpm | ≥ 8 | Dépendances Node |

---

## Tauri

### Pourquoi Tauri ?

Tauri permet de construire des applications desktop avec un frontend web tout en exposant des APIs natives via un backend Rust. C'est le choix le plus adapté pour Echoes Studio pour plusieurs raisons :

**Performance audio :** Le backend Rust permet d'accéder directement à `cpal` et aux APIs audio bas niveau sans passer par un runtime JavaScript. Un callback audio Electron passerait par Node.js, introduisant une latence et un risque de jitter inacceptables.

**Empreinte système :** Tauri utilise le WebView natif de l'OS (WebView2 sur Windows, WebKit sur macOS/Linux) — le binaire final fait ~5-10 MB contre ~100+ MB pour Electron qui embarque Chromium.

**Sécurité :** Le modèle de permissions Tauri est explicite : chaque API native doit être déclarée dans `tauri.conf.json`. Par défaut tout est refusé — c'est le bon défaut pour une app audio qui n'a pas besoin d'accès réseau arbitraire.

### Alternatives considérées

| Alternative | Raison d'exclusion |
|-------------|-------------------|
| Electron | Trop lourd (Chromium embarqué), backend JS inadapté à l'audio temps-réel |
| Qt (C++) | Pas de hot-reload, écosystème UI moins moderne, licence complexe |
| JUCE | Excellente librairie audio mais UI propriétaire vieillissante, C++ uniquement |
| Flutter Desktop | Bon candidat mais dart ecosystem moins mature pour audio professionnel |
| Native (Win32/Cocoa/GTK) | Trop de code à maintenir pour chaque plateforme |

### Contraintes cross-platform Tauri

- **Windows :** Nécessite WebView2 (préinstallé sur Windows 11, installable sur 10). Build nécessite MSVC.
- **macOS :** Notarisation Apple nécessaire pour la distribution. WebKit peut avoir des différences CSS mineures.
- **Linux :** Nécessite `webkit2gtk`. Les distributions Linux varient — tester sur Ubuntu, Fedora, Arch.

### Tauri v2 — Note de migration

Tauri 2.0 est sorti en 2024 avec un système de permissions refondu et une meilleure API mobile. Une migration vers Tauri 2.x est recommandée avant la Phase 2 (MVP public). Voir [TODO: lien migration guide].

---

## SvelteKit

### Pourquoi SvelteKit ?

**Réactivité compilée :** Svelte compile les réactivités en JavaScript vanilla — il n'y a pas de virtual DOM, pas de diffing à l'exécution. Pour une DAW avec des centaines d'éléments UI qui changent rapidement (waveforms, positions de clips, VU-mètres), cela se traduit par des animations plus fluides.

**Taille du bundle :** Un composant Svelte compilé est plus léger qu'un composant React équivalent. Pour une app desktop sans CDN, chaque KB compte sur le temps de démarrage.

**Stores natifs :** Svelte inclut un système de stores réactifs (`writable`, `readable`, `derived`) qui remplace avantageusement Redux ou Zustand pour la gestion d'état d'une DAW.

**SvelteKit vs Svelte seul :** SvelteKit est utilisé en mode SPA (SSR désactivé via `prerender = true`). Cela donne accès au routeur SvelteKit (utile pour des pages futures : settings, about) tout en restant une SPA côté Tauri.

### Alternatives considérées

| Alternative | Raison d'exclusion |
|-------------|-------------------|
| React | Virtual DOM inutile pour app desktop, bundle plus lourd |
| Vue 3 | Bon candidat, mais Svelte offre de meilleures perfs à bundle équivalent |
| Solid.js | Très performant mais écosystème plus petit, moins de contributeurs potentiels |
| Vanilla TS | Trop de boilerplate pour gérer la réactivité d'une DAW |

---

## TypeScript

TypeScript 5 est le standard pour tout le code frontend. Pas de fichiers `.js` dans `src/` (sauf héritage à supprimer).

**Configuration (`tsconfig.json`) :** mode strict activé. Toutes les interfaces métier sont définies dans `src/types/global.d.ts`.

---

## SCSS

SCSS est utilisé pour les variables de thème (`src/variables.scss`) et la structure des styles composants. Chaque composant `.svelte` utilise `<style lang="scss">` pour scoper ses styles.

**Variables globales (`variables.scss`) :**
```scss
$bg-base: #1a1a1f;
$bg-panel: #222229;
$border: #3c3c43;
$text: #f7f7f9;
$accent: #1571ff;
```

**Alternative considérée :** CSS custom properties uniquement — écarté car les variables SCSS permettent plus de composition (mixins, fonctions) utile pour un design system DAW.

---

## Rust (Audio Engine)

### Pourquoi Rust ?

L'audio temps-réel impose des contraintes strictes : zéro allocation dans le callback audio, zéro pause GC, latence prévisible. Rust est le seul langage moderne qui offre ces garanties sans sacrifier la sécurité mémoire.

De plus, l'intégration Tauri est native — le backend Rust compile directement dans le binaire Tauri.

### cpal

`cpal` (Cross-Platform Audio Library) est la bibliothèque audio bas niveau standard de l'écosystème Rust. Elle supporte :

| OS | Backend audio |
|----|--------------|
| Windows | WASAPI (défaut), ASIO (optionnel) |
| macOS | CoreAudio |
| Linux | ALSA, JACK, PipeWire |

**Alternative considérée :** PortAudio via FFI — cpal est plus idiomatique Rust et mieux maintenu.

### hound

Parsing et écriture de fichiers WAV. Léger et fiable. Le support d'autres formats (MP3, FLAC, OGG) sera ajouté en Phase 2 via `symphonia` (décodeur audio multi-format pur Rust).

---

## Vite

Build tool et dev server. Choix par défaut de SvelteKit, offre le hot-module replacement le plus rapide disponible. Aucune raison de changer.

---

## pnpm

Gestionnaire de paquets Node. Requis via `.npmrc` (`engine-strict=true`). Plus rapide et économe en espace disque que npm grâce aux hard-links. Le fichier `pnpm-lock.yaml` est commité et versionné.

---

## Dépendances à venir (planifiées)

| Dépendance | Phase | Usage |
|------------|-------|-------|
| `symphonia` (Rust) | Phase 2 | Décodage MP3/FLAC/OGG |
| `ringbuf` (Rust) | Phase 1 | Échange de données lock-free entre threads audio |
| `rubato` (Rust) | Phase 2 | Conversion de sample rate |
| `dasp` (Rust) | Phase 3 | DSP primitives (filtres, enveloppes) |
| `[TODO]` | Phase 4 | Support VST3 ou CLAP |

---

## Gestion du build

### Dev
```bash
pnpm tauri dev
# Lance Vite dev server (localhost:5173) + Tauri en watch mode
```

### Production
```bash
pnpm tauri build
# Build Vite → dist/, puis Tauri bundle l'app avec le runtime
# Output : src-tauri/target/release/bundle/
#   - Windows : .msi + .exe
#   - macOS : .app + .dmg
#   - Linux : .deb + .AppImage
```

### CI/CD
[TODO: Configurer GitHub Actions pour builder sur les 3 plateformes]

---

## Liens

- [ARCHITECTURE.md](ARCHITECTURE.md) — Comment les couches s'assemblent
- [AUDIO_ENGINE.md](AUDIO_ENGINE.md) — Détails de l'implémentation audio
