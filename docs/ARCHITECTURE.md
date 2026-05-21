# Architecture — Echoes Studio

> Ce document décrit l'architecture de l'application, les flux de données entre couches, et les décisions d'architecture structurantes (ADR).

---

## Vue d'ensemble

Echoes Studio adopte une architecture en **trois couches distinctes** communicant via des interfaces explicites :

```
┌─────────────────────────────────────────────────────────────┐
│                        UI Layer                             │
│              SvelteKit 2 + TypeScript + SCSS                │
│   Components ──► Svelte Stores ──► Tauri IPC calls         │
└─────────────────────────┬───────────────────────────────────┘
                          │  IPC (JSON over Tauri commands)
┌─────────────────────────▼───────────────────────────────────┐
│                      Core Layer                             │
│                    Rust / Tauri 1.x                         │
│     Command Handlers ──► State ──► Audio Engine API         │
└─────────────────────────┬───────────────────────────────────┘
                          │  Rust API (direct function calls)
┌─────────────────────────▼───────────────────────────────────┐
│                    Audio Engine Layer                       │
│                  Rust + cpal + hound                        │
│    File I/O ──► Sample Processing ──► Output Stream        │
└─────────────────────────────────────────────────────────────┘
```

---

## Couche UI (Frontend)

**Technologies :** SvelteKit 2, TypeScript, SCSS, Web Workers

### Responsabilités
- Rendu de l'interface utilisateur (timeline, clips, transport)
- Gestion de l'état UI via Svelte stores
- Envoi des commandes vers le Core Layer via l'API Tauri
- Rendu des formes d'onde (canvas, Web Worker)

### Structure des composants

```
src/
├── routes/
│   └── +page.svelte          # Shell applicatif principal
├── components/
│   ├── MenuBar.svelte         # Transport (Play/Pause/Stop/Record)
│   ├── MainPannel.svelte      # Zone de travail principale
│   ├── Track.svelte           # Piste audio individuelle
│   ├── Clip.svelte            # Clip audio avec waveform canvas
│   ├── Knob.svelte            # Contrôle rotatif (volume, paramètres)
│   └── StatusBar.svelte       # Barre d'état inférieure
├── stores/                    # [TODO: À créer — Svelte writable stores]
│   ├── transport.ts           # État du transport (play, BPM, position)
│   ├── tracks.ts              # Liste des pistes et clips
│   └── ui.ts                  # État UI (zoom, sélection, panels)
└── lib/
    └── waveform.worker.ts     # Web Worker — rendu waveform off-thread
```

### Flux de données UI

```
User Interaction
      │
      ▼
Component (Svelte)
      │  dispatch / bind
      ▼
Svelte Store (reactive state)
      │  $store subscription
      ▼
Component re-render
      │  (when backend needed)
      ▼
invoke('tauri_command', args)  ──► Core Layer
      │
      ▼
Store update with response
```

---

## Couche Core (Rust / Tauri)

**Technologies :** Rust, Tauri 1.x

### Responsabilités
- Point d'entrée de l'application desktop
- Exposition des commandes Tauri (IPC handlers)
- Orchestration des appels à l'Audio Engine
- Gestion de l'état applicatif côté Rust (`tauri::State`)

### Structure

```
src-tauri/src/
├── main.rs           # Entry point, commandes Tauri, app state
└── audio_clip.rs     # Struct AudioClip, parsing WAV
```

### Commandes Tauri (IPC)

| Commande | Input | Output | Statut |
|----------|-------|--------|--------|
| `create_new_audio_clip` | `path: String` | `[Vec<f32>, u32]` | Implémenté |
| `[TODO] start_playback` | `position: f64` | `()` | À implémenter |
| `[TODO] stop_playback` | — | `()` | À implémenter |
| `[TODO] export_mixdown` | `path: String, format: String` | `()` | Futur |

### IPC — Convention de sérialisation

Toutes les données échangées sont sérialisées en JSON via `serde_json`. Les erreurs sont propagées comme `String` (à terme, migrer vers un type d'erreur structuré).

```rust
// Pattern actuel
#[tauri::command]
async fn my_command(arg: String) -> Result<ReturnType, String> {
    // ...
}
```

---

## Couche Audio Engine

**Technologies :** Rust, `cpal` 0.15, `hound` 3.5

### Responsabilités
- Chargement et décodage de fichiers audio (WAV, [TODO: MP3, FLAC])
- Gestion du stream audio de sortie (`cpal`)
- Gestion du stream audio d'entrée (enregistrement)
- [TODO] Graph de traitement audio (effets, mixage)
- [TODO] Synchronisation du transport (position de lecture)

### Modèle de données audio

```rust
// audio_clip.rs — état actuel
pub struct AudioClip {
    pub samples: Vec<f32>,   // Échantillons normalisés [-1.0, 1.0]
    pub sample_rate: u32,    // Fréquence d'échantillonnage (Hz)
}

// [TODO] Modèle cible
pub struct AudioClip {
    pub id: ClipId,
    pub samples: Arc<Vec<f32>>,  // Partagé entre threads via Arc
    pub sample_rate: u32,
    pub channels: u16,
    pub start_sample: u64,       // Position sur la timeline
}

pub struct Track {
    pub id: TrackId,
    pub clips: Vec<AudioClip>,
    pub volume: f32,             // 0.0 - 1.0
    pub pan: f32,                // -1.0 (L) à 1.0 (R)
    pub muted: bool,
    pub solo: bool,
}
```

### Graph de traitement (vision cible)

```
AudioClip (samples)
      │
      ▼
[Clip Gain]
      │
      ▼
[Insert FX Chain] ──► Equalizer ──► Compressor ──► [Custom FX]
      │
      ▼
[Track Fader + Pan]
      │
      ▼
[Master Bus]
      │
      ▼
[Master FX Chain]
      │
      ▼
Output Stream (cpal)
```

→ Spécifications détaillées : [AUDIO_ENGINE.md](AUDIO_ENGINE.md)

---

## Gestion de l'état global

### État actuel (problème)
L'état applicatif est localisé dans `MainPannel.svelte` comme variable locale Svelte. Cela entraîne un couplage fort et rend le partage d'état inter-composants difficile.

### Cible : Svelte Stores

```typescript
// stores/tracks.ts
import { writable } from 'svelte/store';
import type { Track } from '../types/global';

export const tracks = writable<Track[]>([]);

// stores/transport.ts
export const transport = writable({
  playing: false,
  recording: false,
  position: 0,       // en samples
  bpm: 120,
  looping: false,
});
```

Les stores sont réactifs et partagés entre tous les composants sans prop drilling.

---

## Threading

```
Main Thread (UI)
    │  Svelte reactivity, DOM events, Tauri IPC calls
    │
    ├──► Web Worker (Waveform Rendering)
    │        Rendu canvas off-thread pour éviter les jank UI
    │
    └──► [via Tauri IPC]
              │
              ▼
         Rust Main Thread
              │  Tauri command handlers
              │
              ├──► Audio Output Thread (cpal)
              │        Callback temps-réel, JAMAIS de blocking I/O
              │        Buffer size : [TODO: configurable, défaut 512]
              │
              └──► File I/O Thread (tokio async)
                       Chargement de fichiers audio en arrière-plan
```

**Règle critique :** Le callback audio (`cpal` output stream) ne doit **jamais** faire d'allocation mémoire ni d'appel bloquant. Toutes les données doivent être préparées en avance et échangées via des canaux lock-free (ex : `ringbuf`).

---

## Décisions d'architecture (ADR)

### ADR-001 : Tauri plutôt qu'Electron

**Contexte :** Choix du runtime desktop cross-platform.
**Décision :** Tauri 1.x.
**Raisons :**
- Binaires 10-20× plus petits qu'Electron
- Empreinte mémoire réduite (utilise le WebView natif de l'OS)
- Backend Rust natif = performances audio sans overhead Node.js
- Sécurité renforcée (modèle de permissions explicite)

**Trade-off :** API WebView différente selon l'OS (Chromium sur Windows/Linux, WebKit sur macOS) — nécessite des tests cross-platform.

---

### ADR-002 : SvelteKit pour le frontend

**Contexte :** Choix du framework UI.
**Décision :** SvelteKit 2 (mode SPA, SSR désactivé).
**Raisons :**
- Réactivité sans virtual DOM = performances UI fluides pour une DAW
- Bundle final plus léger que React/Vue pour une app desktop
- TypeScript natif
- Stores Svelte = state management sans librairie externe

**Trade-off :** Écosystème plus petit que React, moins de composants tiers disponibles.

---

### ADR-003 : Rust pour l'audio engine

**Contexte :** Choix du langage pour le traitement audio.
**Décision :** Rust + `cpal`.
**Raisons :**
- Garanties temps-réel : absence de GC, contrôle mémoire explicite
- `cpal` supporte ASIO (Windows), CoreAudio (macOS), ALSA/JACK (Linux)
- Sécurité mémoire sans garbage collector
- Interopérabilité native avec Tauri

**Trade-off :** Courbe d'apprentissage Rust, complexité du système de types pour le code audio concurrent.

---

### ADR-004 : [TODO] Format de projet

**Contexte :** Format de sauvegarde des projets.
**Options :**
- JSON texte — lisible, versionnable, mais verbeux pour les grandes timelines
- MessagePack binaire — compact, mais non-lisible
- SQLite — requêtes, historique, mais dépendance lourde
**Décision :** [TODO — à décider en Phase 2]

---

### ADR-005 : [TODO] Support de plugins

**Contexte :** Format de plugins audio tiers.
**Options :**
- VST3 — standard industrie, support Windows/macOS/Linux
- LV2 — open-source, Linux natif
- CLAP — nouveau standard open-source, prometteur
- Pas de plugins tiers — maintenir la simplicité
**Décision :** [TODO — à décider en Phase 4]

---

## Liens

- [TECH_STACK.md](TECH_STACK.md) — Justification détaillée des choix technologiques
- [AUDIO_ENGINE.md](AUDIO_ENGINE.md) — Specs du moteur audio
- [UI_GUIDELINES.md](UI_GUIDELINES.md) — Architecture des composants UI
