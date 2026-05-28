# Tech Stack — Echoes Studio

> Rationale for each technology choice, alternatives considered, and cross-platform constraints.

---

## Overview

| Layer | Technology | Version | Role |
|-------|------------|---------|------|
| Desktop runtime | Tauri | **2.x** | Native shell, IPC, permissions |
| UI Framework | SvelteKit | 2.0 | User interface |
| UI Language | TypeScript | 5.0 | Static typing (strict mode) |
| Styling | SCSS/Sass | 1.69 | Design system variables + component styles |
| Build tool | Vite | 5.0 | Frontend bundler + dev server |
| Backend language | Rust | 2021 edition | Audio engine, IPC handlers, file I/O |
| Audio I/O | cpal | 0.15 | Cross-platform audio stream |
| WAV parsing | hound | 3.5 | WAV read/write |
| Serialisation | serde / serde_json | 1.0 | IPC JSON, project file format |
| Package manager | pnpm | ≥ 8 | Node dependency management |

**Removed from previous versions:**
- `wavesurfer.js` — replaced by a custom `waveform.worker.ts` Web Worker with canvas rendering; no external dependency needed for waveform display.

---

## Tauri

### Why Tauri?

Tauri builds desktop applications with a web frontend backed by a native Rust process. It is the right choice for Echoes Studio for three reasons:

**Audio performance:** The Rust backend calls `cpal` directly, without a JavaScript runtime on the audio path. An Electron-based audio engine would route the callback through Node.js, introducing latency and GC jitter incompatible with real-time audio.

**Binary size:** Tauri uses the OS-native WebView (WebView2 on Windows, WebKit on macOS/Linux). The final binary is ~5–10 MB. Electron embeds Chromium — binaries start at ~100 MB.

**Security model:** Every native API capability must be declared explicitly in `tauri.conf.json`. The default is deny-all, which is the correct default for a desktop audio application.

### Tauri 2.x

The project runs on Tauri 2.x. The v2 rewrite introduced a redesigned permission system, better mobile API surface, and a cleaner plugin model. Migration from the original 1.x scaffold was completed in Phase 0.

### Cross-Platform Notes

| OS | Requirement | Notes |
|----|------------|-------|
| Windows | WebView2 (pre-installed on Windows 11) | Installer required on Windows 10 |
| Windows | MSVC Build Tools 2019+ (C++ workload) | Required to compile `cpal` |
| macOS | Xcode Command Line Tools | Provides linker + macOS SDK |
| Linux | `libwebkit2gtk-4.0-dev`, `libgtk-3-dev` | GTK + WebKit required by Tauri |

### Alternatives Considered

| Alternative | Reason rejected |
|-------------|----------------|
| Electron | ~100 MB binary, Node.js on audio path — unacceptable for real-time audio |
| Qt (C++) | No hot-reload, ageing UI ecosystem, complex licensing |
| JUCE | Excellent audio library but proprietary UI, C++ only |
| Flutter Desktop | Promising but Dart ecosystem less mature for professional audio |
| Native (Win32/Cocoa/GTK) | Per-platform maintenance cost unacceptable for a small team |

---

## SvelteKit

### Why SvelteKit?

**Compiled reactivity:** Svelte compiles reactivity to vanilla JavaScript — no virtual DOM, no diffing at runtime. For a DAW with hundreds of elements updating at 60 fps (waveforms, clip positions, VU meters), this means smoother rendering.

**Bundle size:** A compiled Svelte component is smaller than its React equivalent. For a desktop app served without a CDN, startup time is directly affected.

**Native stores:** Svelte's built-in `writable`, `readable`, and `derived` stores replace Redux/Zustand for DAW state management without additional dependencies.

**SvelteKit in SPA mode:** SSR is disabled (`prerender = true`). This gives access to SvelteKit's router (useful for future Settings and About pages) while remaining a pure SPA from Tauri's perspective.

### Alternatives Considered

| Alternative | Reason rejected |
|-------------|----------------|
| React | Virtual DOM unnecessary for desktop, heavier bundle |
| Vue 3 | Good candidate, but Svelte has better performance at equivalent bundle size |
| Solid.js | Very fast, but smaller ecosystem and fewer potential contributors |
| Vanilla TS | Too much boilerplate to manage DAW-level reactivity |

---

## TypeScript

TypeScript 5 strict mode is used throughout the frontend. No `.js` files exist in `src/`. All domain interfaces are defined in `src/types/global.d.ts` and `src/shared/protocol/`.

---

## SCSS

SCSS is used for design-system variables (`src/variables.scss`) and component-scoped styles. Every `.svelte` component uses `<style lang="scss">`.

Core design tokens:
```scss
$color-bg-base:     #1a1a1f;
$color-bg-panel:    #222229;
$color-border:      #3c3c43;
$color-text-primary: #f7f7f9;
$color-accent:      #1571ff;
```

---

## Rust (Audio Engine)

### Why Rust?

Real-time audio imposes hard constraints: zero allocation in the audio callback, zero GC pauses, deterministic latency. Rust is the only modern language that delivers these guarantees without sacrificing memory safety.

Tauri integration is native — the Rust backend compiles directly into the Tauri binary with no interop overhead.

### Audio Graph System (`audio_graph/`)

The DSP engine is implemented as a directed acyclic graph compiled ahead of time into a flat execution plan. Key modules:

| Module | Role |
|--------|------|
| `audio_graph/node.rs` | `AudioNode` trait — uniform interface for all DSP elements |
| `audio_graph/buffer.rs` | `AudioBuffer` — planar f32, allocation-free operations |
| `audio_graph/compiler.rs` | `GraphCompiler` — topological sort, node instantiation, `prepare()` |
| `audio_graph/scheduler.rs` | `GraphScheduler` — drives execution plan in the audio callback |
| `audio_graph/execution_plan.rs` | `ExecutionPlan` — immutable, `Arc`-shared between threads |
| `audio_graph/nodes/` | Built-in nodes: Track, Gain, Pan, Meter, Bus, Master |

See [AUDIO_ENGINE.md](AUDIO_ENGINE.md) and `docs/claude/architecture/audio-graph.md` for details.

### cpal

Cross-platform audio library. Supports WASAPI (Windows), CoreAudio (macOS), ALSA/JACK/PipeWire (Linux). The CPAL callback is the only place where the audio thread runs — everything else is on the command thread.

**Alternative considered:** PortAudio via FFI — cpal is more idiomatic Rust and better maintained.

### hound

WAV file parsing and writing. Lightweight and reliable. Multi-format decoding (MP3, FLAC, OGG) will be added via `symphonia` in Phase 2.

---

## Vite

SvelteKit's default bundler and dev server. Provides the fastest HMR available. No reason to change.

---

## pnpm

Required via `.npmrc` (`engine-strict=true`). Faster and more disk-efficient than npm through hard-links. `pnpm-lock.yaml` is committed and versioned.

---

## Waveform Rendering

Waveform display uses a custom implementation in `src/lib/waveform.worker.ts` — a Web Worker that computes peak arrays and renders them to a canvas off the UI thread. No external waveform library.

Pre-computed peaks at three resolutions are stored on each `AppClip`:
- `peaks_64` — 1 peak per 64 samples (zoomed-in view)
- `peaks_512` — 1 peak per 512 samples (medium zoom)
- `peaks_4096` — 1 peak per 4096 samples (overview)

This eliminates re-computation on zoom changes.

---

## Planned Dependencies

| Dependency | Phase | Purpose |
|------------|-------|---------|
| `symphonia` (Rust) | Phase 2 | MP3/FLAC/OGG decoding |
| `rubato` (Rust) | Phase 3 | Sample rate conversion for clips not at project SR |
| `dasp` (Rust) | Phase 3 | DSP primitives (filters, envelopes) for built-in effects |
| VST3 SDK / `vst3-sys` (Rust) | Phase 4 | VST3 plugin hosting (scaffolding done in `plugin_formats/vst3/`) |

**Explicitly not used:**
- `ringbuf` — lock-free ring buffer. Evaluated for the command→audio path, replaced by `RuntimeSnapshot` (a simpler `Mutex<Option<GraphScheduler>>` with `try_lock`) which fits the snapshot-swap model better.

---

## Build

### Development
```bash
pnpm tauri:dev
# Starts Vite dev server (localhost:5173) + Tauri in watch mode
```

### Production
```bash
pnpm tauri:build
# Builds Vite → dist/, then Tauri bundles the app
# Output: src-tauri/target/release/bundle/
#   Windows  → .msi + .exe (NSIS)
#   macOS    → .app + .dmg
#   Linux    → .deb + .AppImage
```

### CI/CD
[TODO: Configure GitHub Actions to build on all 3 platforms]

---

## Links

- [ARCHITECTURE.md](ARCHITECTURE.md) — How the layers fit together
- [AUDIO_ENGINE.md](AUDIO_ENGINE.md) — Audio engine implementation details
- [BUILD.md](BUILD.md) — Setup and build instructions
