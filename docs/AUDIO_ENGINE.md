# Audio Engine — Echoes Studio

> Spécifications techniques du moteur audio : modèle de traitement, threading, formats supportés, et décisions de design.

---

## 1. Vue d'ensemble

Le moteur audio d'Echoes Studio est implémenté en Rust et s'interface avec l'UI via le bridge IPC de Tauri. Il repose sur `cpal` pour l'accès cross-platform aux streams audio et `hound` pour le parsing de fichiers WAV.

```
┌─────────────────────────────────────────────────────────┐
│                    Audio Engine                         │
│                                                         │
│  File I/O ──► AudioClip ──► Mixer ──► Output Stream    │
│                                │                        │
│                            Input Stream                 │
│                          (enregistrement)               │
└─────────────────────────────────────────────────────────┘
```

**État actuel :** Seul le chargement de fichiers WAV est fonctionnel. La lecture audio (`cpal` output stream) est en cours d'implémentation.

---

## 2. Spécifications techniques

### 2.1 Paramètres audio

| Paramètre | Valeur par défaut | Configurable | Notes |
|-----------|------------------|--------------|-------|
| Sample rate | 44 100 Hz | Oui (Phase 5) | Standard CD, compatible la majorité du matériel |
| Bit depth interne | f32 (32-bit float) | Non | Format de traitement interne normalisé [-1.0, 1.0] |
| Canaux de sortie | 2 (stéréo) | [TODO] | Surround en vision long terme |
| Buffer size | [TODO: 512 samples] | Oui (Phase 5) | Trade-off latence / stabilité |
| Latence cible | < 10 ms | — | Dépend du buffer size et du backend |

### 2.2 Formats de fichiers supportés

| Format | Statut | Bibliothèque |
|--------|--------|-------------|
| WAV (PCM 16/24/32-bit) | Implémenté | `hound` |
| WAV (float 32-bit) | Implémenté | `hound` |
| MP3 | Planifié (Phase 2) | `symphonia` |
| FLAC | Planifié (Phase 2) | `symphonia` |
| OGG Vorbis | Planifié (Phase 2) | `symphonia` |
| AIFF | [TODO] | `symphonia` |
| M4A/AAC | [TODO] | `symphonia` |

### 2.3 Backends audio par plateforme

| OS | Backend par défaut | Backend alternatif |
|----|-------------------|-------------------|
| Windows | WASAPI | ASIO (latence ultra-basse, [TODO]) |
| macOS | CoreAudio | — |
| Linux | ALSA | JACK, PipeWire |

---

## 3. Modèle de données

### 3.1 AudioClip (état actuel)

```rust
// src-tauri/src/audio_clip.rs
pub struct AudioClip {
    pub samples: Vec<f32>,   // Échantillons normalisés [-1.0, 1.0]
    pub sample_rate: u32,    // Fréquence d'échantillonnage source
}
```

### 3.2 Modèle cible (Phase 1-2)

```rust
pub struct AudioClip {
    pub id: ClipId,                  // Identifiant unique
    pub samples: Arc<[f32]>,         // Partagé entre threads via Arc, immutable
    pub sample_rate: u32,
    pub channels: u16,               // Canaux source (1=mono, 2=stéréo)
    pub start_sample: u64,           // Position sur la timeline (en samples)
    pub gain: f32,                   // Gain du clip (1.0 = neutre)
}

pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub clips: Vec<AudioClip>,
    pub volume: f32,                 // 0.0 - 1.0 (lin), affiché en dB à l'UI
    pub pan: f32,                    // -1.0 (L) à 1.0 (R)
    pub muted: bool,
    pub solo: bool,
    pub fx_chain: Vec<Box<dyn AudioEffect>>,  // [TODO: Phase 3]
}

pub struct AudioEngine {
    pub tracks: Vec<Track>,
    pub sample_rate: u32,
    pub transport: Transport,
    pub output_stream: Option<cpal::Stream>,  // Stream actif cpal
}

pub struct Transport {
    pub position: u64,       // Position actuelle en samples
    pub playing: bool,
    pub recording: bool,
    pub bpm: f64,
    pub loop_start: Option<u64>,
    pub loop_end: Option<u64>,
}
```

---

## 4. Graph de traitement audio

### 4.1 Signal path (Phase 1 — simple)

```
Pour chaque track :
  Pour chaque clip sur la track dont la fenêtre couvre la position actuelle :
    1. Lire samples[position - clip.start_sample]
    2. Appliquer clip.gain
    3. Appliquer track.volume + track.pan → L/R
    4. Accumuler dans le buffer de mixage (additionner les tracks)

Sortie :
  5. Appliquer master volume
  6. Clipping soft (limiter à [-1.0, 1.0])
  7. Envoyer au stream cpal
```

### 4.2 Signal path (Phase 3 — avec effets)

```
Clip.samples
     │
     ▼
[Clip Gain]
     │
     ▼
[Insert FX Chain] ─── effect1 ─── effect2 ─── ...
     │
     ▼
[Track Fader]  ──── volume (linéaire)
     │
     ▼
[Track Panner] ──── loi de panoramique constant-power
     │
     ▼
[Bus / Master]
     │
     ▼
[Master FX]
     │
     ▼
[Limiter] ──── protection contre la saturation numérique
     │
     ▼
Output Buffer (cpal callback)
```

---

## 5. Modèle de threading

### 5.1 Architecture des threads

```
Thread principal Tauri (UI thread)
    │
    ├──► Thread Tokio async (I/O)
    │        - Chargement de fichiers audio
    │        - Communication IPC Tauri
    │        - Jamais de traitement audio temps-réel
    │
    └──► Audio Output Thread (cpal callback)
             - Haute priorité OS (RT priority si possible)
             - Callback périodique : buffer_size / sample_rate (ex: 512/44100 ≈ 11ms)
             - CONTRAINTES STRICTES :
               * Zéro allocation mémoire (pas de Vec::new, Box::new...)
               * Zéro mutex (peut causer priority inversion)
               * Zéro I/O disque ou réseau
               * Zéro panic (unwrap interdit)
```

### 5.2 Communication entre threads

Pour passer des données entre le thread UI et le thread audio sans mutex :

```rust
// Pattern recommandé : ring buffer lock-free
// Bibliothèque : ringbuf (à ajouter en Phase 1)

use ringbuf::RingBuffer;

// Producteur (thread UI/IO) : envoie les nouvelles données audio
// Consommateur (thread audio) : lit les données dans le callback

let (mut producer, mut consumer) = RingBuffer::<f32>::new(BUFFER_SIZE * 4).split();
```

**Pour les changements de paramètres (volume, pan) :** utiliser `std::sync::atomic::AtomicU32` (lecture atomique sans lock).

```rust
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub struct TrackParams {
    // Stocker volume comme bits f32 dans un AtomicU32
    pub volume: Arc<AtomicU32>,
    pub muted: Arc<AtomicBool>,
}
```

### 5.3 Séquence de démarrage du stream audio

```
1. Initialiser cpal : énumérer les devices de sortie disponibles
2. Sélectionner le device par défaut (ou celui configuré par l'utilisateur)
3. Négocier le format audio supporté (sample rate, buffer size)
4. Allouer les buffers de mixage (taille fixe, zéro allocation à l'exécution)
5. Démarrer le stream avec le callback de rendu audio
6. Le callback est appelé périodiquement par le driver audio OS
```

---

## 6. Lecture audio — Implémentation cible (Phase 1)

```rust
// Squelette du callback audio (src-tauri/src/audio_engine.rs)

fn audio_callback(
    data: &mut [f32],
    engine_state: &Arc<EngineState>,
) {
    let transport = engine_state.transport.load(Ordering::Relaxed);

    if !transport.playing {
        // Silence
        data.fill(0.0);
        return;
    }

    let position = transport.position;
    let buffer_len = data.len() / 2; // stéréo interleaved

    // Remplir avec silence
    data.fill(0.0);

    // Mixer toutes les tracks actives
    for track in engine_state.tracks.iter() {
        if track.muted.load(Ordering::Relaxed) {
            continue;
        }

        let volume = f32::from_bits(track.volume.load(Ordering::Relaxed));

        for clip in track.clips.iter() {
            // Vérifier si le clip est dans la fenêtre de lecture
            let clip_start = clip.start_sample;
            let clip_end = clip_start + clip.samples.len() as u64;

            if position >= clip_end || position + buffer_len as u64 <= clip_start {
                continue; // Clip hors fenêtre
            }

            // Copier les samples du clip dans le buffer de sortie
            for i in 0..buffer_len {
                let sample_pos = position + i as u64;
                if sample_pos < clip_start || sample_pos >= clip_end {
                    continue;
                }
                let sample_idx = (sample_pos - clip_start) as usize;
                let sample = clip.samples[sample_idx] * volume;

                // Sortie stéréo interleaved [L, R, L, R, ...]
                data[i * 2] += sample;
                data[i * 2 + 1] += sample;
            }
        }
    }

    // Avancer la position du transport
    // [Mise à jour atomique de transport.position]
}
```

---

## 7. Gestion de la latence

### 7.1 Sources de latence

| Source | Latence typique | Contrôle |
|--------|----------------|----------|
| Buffer audio driver | 5-20ms (512 samples @ 44.1kHz = 11.6ms) | Configurable |
| Conversion sample rate | Négligeable si évitée | Forcer même SR que le device |
| Décodage fichier | Async (préchargé en mémoire) | Préchargement complet |
| WASAPI exclusive mode | 1-3ms | [TODO: Phase 5] |

### 7.2 Stratégie

Pour la Phase 1-2 : chargement complet des fichiers audio en RAM avant la lecture. Évite tout I/O disque en temps réel.

Pour les projets très longs (Phase 3+) : [TODO: streaming depuis le disque avec buffer double].

---

## 8. Export / Mixdown

[TODO: Phase 2]

Fonctionnement prévu :
1. Rendre le projet depuis la position 0 jusqu'à la fin du dernier clip
2. Exécuter le même graph de traitement que la lecture temps-réel
3. Écrire les samples dans un fichier WAV via `hound` (ou via `symphonia` pour MP3/FLAC)
4. Ne pas utiliser le stream cpal — rendu offline à la vitesse maximale du CPU

---

## 9. Plugins audio

### 9.1 Effets intégrés (Phase 3)

Interface commune pour tous les effets :

```rust
pub trait AudioEffect: Send + Sync {
    fn process(&mut self, buffer: &mut [f32], sample_rate: u32);
    fn reset(&mut self);
    fn name(&self) -> &str;
    // [TODO: paramètres exposés à l'UI]
}
```

Effets prévus : `Gain`, `Pan`, `EQ`, `Compressor`, `Reverb`, `Delay`.

### 9.2 Plugins tiers (Phase 4)

La décision sur le format de plugin (VST3, CLAP, LV2) est en attente. Voir [ADR-005 dans ARCHITECTURE.md](ARCHITECTURE.md#adr-005--todo-support-de-plugins).

**Contrainte :** Les plugins tiers s'exécutent dans le thread audio — ils doivent respecter les mêmes règles (zéro allocation, zéro blocking). Un sandbox process pourrait être nécessaire pour les plugins non fiables.

---

## 10. Monitoring & Debug

[TODO: Phase 2]

- Affichage CPU audio (% du budget callback utilisé)
- VU-mètres par piste (pic + RMS)
- Indicateur d'underrun (xrun) — dropout audio
- Log structuré des événements audio (désactivable en production)

---

## Liens

- [ARCHITECTURE.md](ARCHITECTURE.md) — Architecture globale et threading
- [TECH_STACK.md](TECH_STACK.md) — Justification cpal, hound, symphonia
- [src-tauri/src/audio_clip.rs](../src-tauri/src/audio_clip.rs) — Implémentation actuelle
- [src-tauri/src/main.rs](../src-tauri/src/main.rs) — Commandes Tauri et entry point
