# Build Guide — Echoes Studio

This document is the single source of truth for building and running Echoes Studio on any platform. The same three commands work on Windows, macOS and Linux — no platform-specific instructions to remember.

---

## Table of contents

1. [TL;DR](#tldr)
2. [Prerequisites](#prerequisites)
3. [Installation per OS](#installation-per-os)
4. [Available commands](#available-commands)
5. [Environment checker](#environment-checker)
6. [Debugging](#debugging)
7. [How the build pipeline works](#how-the-build-pipeline-works)
8. [Troubleshooting](#troubleshooting)

---

## TL;DR

```bash
git clone https://github.com/echoes-studio/echoes-studio.git
cd echoes-studio
pnpm install        # installs JS deps; rustup auto-installs Rust toolchain on first cargo run
pnpm tauri:dev      # starts dev build (Vite + Tauri, hot-reload)
```

That's it. If something fails, run `pnpm check-env` for a structured diagnosis.

---

## Prerequisites

Five tools must be present before the first build. Every other dependency is fetched automatically by `pnpm install`, `cargo build`, or `rustup` (driven by `rust-toolchain.toml`).

| Tool                  | Minimum version   | Purpose                               | How `pnpm tauri:dev` notices if missing                   |
| --------------------- | ----------------- | ------------------------------------- | --------------------------------------------------------- |
| **Node.js**           | 18.0.0            | Runs Vite + SvelteKit                 | `engine-strict=true` blocks `pnpm install`                |
| **pnpm**              | 8.0.0             | Package manager (do not use npm/yarn) | Engine check fails                                        |
| **Rust** (via rustup) | 1.60.0            | Compiles the Tauri backend            | `cargo` is unavailable, build aborts                      |
| **cargo**             | bundled with Rust | Rust build tool                       | as above                                                  |
| **Tauri CLI**         | 2.x               | Orchestrates dev/build                | Installed locally with `pnpm install` (`@tauri-apps/cli`) |

The Tauri CLI is **not** installed globally — it lives in `node_modules` and is invoked via `pnpm tauri ...`, which means the version is pinned per-checkout.

### Platform-specific additions

| OS          | Extra requirement                                                                       | Why                                           |
| ----------- | --------------------------------------------------------------------------------------- | --------------------------------------------- |
| **Windows** | WebView2 Runtime (pre-installed on Win 11)                                              | Tauri renders the UI inside a WebView2 window |
| **Windows** | MSVC Build Tools 2019+ ("Desktop development with C++" workload)                        | Compiles C dependencies (e.g. `cpal`)         |
| **macOS**   | Xcode Command Line Tools                                                                | Provides the linker and macOS SDK             |
| **Linux**   | `libwebkit2gtk-4.0-dev`, `libgtk-3-dev`, `librsvg2-dev`, `libayatana-appindicator3-dev` | Tauri requires GTK + WebKit on Linux          |

`pnpm check-env` verifies every line of these tables on your machine.

---

## Installation per OS

### Windows (PowerShell)

```powershell
# 1. Node.js LTS
winget install OpenJS.NodeJS.LTS

# 2. pnpm — pick ONE of:
npm install -g pnpm@latest
# or:
corepack enable
corepack prepare pnpm@latest --activate

# 3. Rust — download rustup-init.exe from https://rustup.rs and run it.
#    Accept the defaults (stable toolchain, MSVC host).
#    rust-toolchain.toml in this repo will auto-install the right components.

# 4. Visual Studio Build Tools — download and run:
#    https://visualstudio.microsoft.com/visual-cpp-build-tools/
#    Select the "Desktop development with C++" workload.

# 5. WebView2 (only Windows 10 — Windows 11 has it pre-installed):
#    https://developer.microsoft.com/en-us/microsoft-edge/webview2/
#    Download the Evergreen Bootstrapper and run it.
```

### macOS

```bash
# 1. Xcode Command Line Tools
xcode-select --install

# 2. Node.js
brew install node@20
# or download from https://nodejs.org

# 3. pnpm
npm install -g pnpm@latest

# 4. Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

### Linux (Debian / Ubuntu)

```bash
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.0-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  build-essential \
  curl wget libssl-dev patchelf

# Node.js via nvm (avoids root-owned npm):
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.7/install.sh | bash
source ~/.bashrc
nvm install 20

# pnpm
npm install -g pnpm@latest

# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

### Linux (Fedora / RHEL)

```bash
sudo dnf install -y \
  webkit2gtk3-devel \
  gtk3-devel \
  libappindicator-gtk3-devel \
  librsvg2-devel \
  openssl-devel \
  curl wget patchelf

# Node, pnpm and Rust: identical to the Debian instructions above.
```

---

## Available commands

Every command below is identical on Windows, macOS and Linux.

| Command                | What it does                                                                                                                                                             |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `pnpm install`         | Install all JS dependencies from `pnpm-lock.yaml`                                                                                                                        |
| `pnpm check-env`       | Verify every required tool is present and at the right version                                                                                                           |
| `pnpm tauri:dev`       | **Start the dev build** — Vite hot-reload for the UI; Rust is not rebuilt on save (`--no-watch`), restart to pick up Rust changes. Runs `check-env` automatically first. |
| `pnpm tauri:dev:watch` | Same, but cargo rebuilds and relaunches the app on every Rust save.                                                                                                      |
| `pnpm tauri:build`     | Compile a release build and create platform installers (bundles `plugin_host_bridge` as a sidecar via `src-tauri/tauri.bundle.conf.json`; without it an installed app lists no plugin). Runs `check-env` first.                                                                                          |
| `pnpm dev`             | Vite dev server only (frontend, no Tauri window)                                                                                                                         |
| `pnpm build`           | SvelteKit build only (`build/`)                                                                                                                                          |
| `pnpm test`            | Run the full validation suite: `pnpm check` + `pnpm lint` + `pnpm test:frontend` + `cargo fmt --check` + `cargo clippy` + `cargo test`                                   |
| `pnpm test:frontend`   | Vitest — frontend unit/integration tests (pure TS modules + project event reducer)                                                                                       |
| `pnpm check`           | TypeScript + Svelte validation                                                                                                                                           |
| `pnpm check:watch`     | Same, in watch mode                                                                                                                                                      |
| `pnpm lint`            | Prettier + ESLint check                                                                                                                                                  |
| `pnpm format`          | Prettier auto-format                                                                                                                                                     |
| `pnpm clean`           | Remove `build/`, `.svelte-kit/`, `src-tauri/target/`                                                                                                                     |
| `pnpm clean:deep`      | Same as above + `node_modules/` (re-run `pnpm install` afterwards)                                                                                                       |

### Rust-only commands

These still work for fine-grained workflows. Run them from `src-tauri/`:

```bash
cd src-tauri
cargo check           # fast type-check without producing binaries
cargo clippy          # lint
cargo test            # unit tests
cargo fmt             # auto-format
```

---

## Environment checker

`pnpm check-env` reads — never writes — and prints a structured report:

```
Echoes Studio — Environment Check
Platform : win32 x64
Node     : v20.11.0
Date     : 2026-05-26

Node.js
────────────────────────────────────────────────────
  ✓ OK  Node.js ≥ 18.0.0  found 20.11.0

pnpm
────────────────────────────────────────────────────
  ✓ OK  pnpm ≥ 8.0.0  found 9.1.0

Rust toolchain
────────────────────────────────────────────────────
  ✓ OK  rustc ≥ 1.60.0  found 1.79.0
  ✓ OK  cargo  found 1.79.0

Project state
────────────────────────────────────────────────────
  ✓ OK  pnpm-lock.yaml present  found
  ✓ OK  node_modules installed  found
  ✓ OK  node_modules up to date  in sync with lock file
  ✓ OK  @tauri-apps/cli (local)  found 1.6.3

Platform-specific requirements
────────────────────────────────────────────────────
  ✓ OK  Microsoft WebView2 Runtime  found in registry
  ✓ OK  MSVC Build Tools (C++ workload)  found VS Build Tools 17.9.34622.214

────────────────────────────────────────────────────
All checks passed — ready to build!
```

Flags:

- `--quiet` — print only failures (used internally by `predev:tauri` / `prebuild:tauri`)

The script exits with code `1` if any required tool is missing, making it safe to chain in CI or pre-commit hooks.

### What gets checked

| Category    | Check                                                                                               |
| ----------- | --------------------------------------------------------------------------------------------------- |
| Node.js     | version ≥ 18.0.0                                                                                    |
| pnpm        | installed, version ≥ 8.0.0                                                                          |
| Rust        | `rustc` ≥ 1.60.0                                                                                    |
| Cargo       | present                                                                                             |
| Project     | `pnpm-lock.yaml` exists, `node_modules` present and not older than lockfile                         |
| Tauri CLI   | installed locally in `node_modules`                                                                 |
| **Windows** | WebView2 runtime, MSVC C++ workload (via `vswhere.exe` or `cl.exe` fallback)                        |
| **macOS**   | Xcode CLI Tools, macOS SDK path                                                                     |
| **Linux**   | `webkit2gtk-4.0`, `gtk+-3.0`, `librsvg-2.0`, `ayatana-appindicator3`, `patchelf` (via `pkg-config`) |

---

## Debugging

**F5** dans VS Code exécute `pnpm tauri:dev` dans un terminal intégré — pas d'extension de debug requise. La configuration est dans `.vscode/launch.json` (type `node-terminal`).

Pour poser des breakpoints Rust, la voie la plus simple reste d'utiliser `pnpm tauri:dev` en ligne de commande avec les logs Rust activés :

```powershell
$env:RUST_LOG="debug"; pnpm tauri:dev
```

### Debug tips

- The frontend is debugged separately with the **DevTools** panel — right-click inside the Tauri window → **Inspect Element**. Source-map support is on by default (`sourceMap: true` in `tsconfig.json`).
- For more verbose Rust logs, run `pnpm tauri:dev` with `RUST_LOG=debug` (PowerShell: `$env:RUST_LOG="debug"; pnpm tauri:dev`).
- To debug the audio engine in isolation, run `cargo run --manifest-path src-tauri/Cargo.toml` — this skips Tauri entirely.

---

## How the build pipeline works

```
pnpm tauri:dev
│
├── predev:tauri hook runs `node scripts/check-env.mjs --quiet`
│   └── aborts the build if a required tool is missing
│
├── tauri CLI reads src-tauri/tauri.conf.json
│   ├── runs beforeDevCommand: "pnpm run dev"  →  Vite dev server (port 5173)
│   └── compiles src-tauri/ with cargo (debug profile)
│
├── Tauri opens a native window pointed at http://localhost:5173
│
└── File watchers run in parallel:
    ├── Vite  →  HMR for .svelte / .ts changes (instant)
    └── cargo →  incremental recompile for .rs changes (seconds)
```

```
pnpm tauri:build
│
├── prebuild:tauri hook runs `node scripts/check-env.mjs --quiet`
│
├── runs beforeBuildCommand: "node scripts/prepare-bridge.mjs && pnpm run build"
│     ├── plugin_host_bridge (release) → src-tauri/binaries/plugin_host_bridge-<triple>  (sidecar)
│     └── SvelteKit → build/
│
└── cargo build --release
    └── bundles installer for the current OS:
        ├── Windows  →  .msi + .exe (NSIS)
        ├── macOS    →  .dmg + .app
        └── Linux    →  .deb + .AppImage
```

**Output locations:**

| Artifact            | Path                                           |
| ------------------- | ---------------------------------------------- |
| Frontend build      | `build/`                                       |
| Rust debug binary   | `src-tauri/target/debug/echoes-studio[.exe]`   |
| Rust release binary | `src-tauri/target/release/echoes-studio[.exe]` |
| Platform bundles    | `src-tauri/target/release/bundle/`             |

### Why these specific design choices

- **Vite port is pinned (`strictPort: true`).** Vite refuses to fall back to 5174 if 5173 is taken. Without this, `tauri.conf.json#devPath` (which hardcodes `http://localhost:5173`) would point at nothing and the window would open empty.
- **`rust-toolchain.toml` pins the channel.** Rustup respects this file silently — first `cargo` invocation installs the right channel + components.
- **`adapter-static` instead of `adapter-auto`.** Tauri serves files via a custom protocol; SvelteKit doesn't need a server adapter. `adapter-auto` worked but emitted a noisy warning at every dev launch.
- **No global Tauri CLI.** The CLI is in `devDependencies`, so the version is pinned per checkout and `pnpm install` is the only setup step.

---

## Troubleshooting

### `Port 5173 is already in use`

The Vite config sets `strictPort: true` on purpose — it surfaces the conflict instead of silently picking a different port (which would leave the Tauri window blank). Either kill the other process or change the port in **two places**:

1. `vite.config.ts` → `server.port`
2. `src-tauri/tauri.conf.json` → `build.devPath`

### `Environment check failed: MSVC Build Tools` on Windows

`pnpm check-env` looks for the C++ workload via `vswhere.exe` (shipped by every VS 2017+ installer) rather than relying on `cl.exe` being on the PATH (which only happens inside a Developer Command Prompt). If the check fails, install the **"Desktop development with C++"** workload from https://visualstudio.microsoft.com/visual-cpp-build-tools/.

### First `cargo build` is very slow

Normal — Cargo compiles the full dependency tree from scratch. The `target/` directory caches compiled artefacts; subsequent builds are incremental. Do **not** delete `target/` unless debugging a stale-cache issue (`pnpm clean` does it correctly).

### `error: linking with cc failed` on Linux

Missing C toolchain:

```bash
sudo apt install build-essential    # Debian/Ubuntu
sudo dnf install gcc                # Fedora
```

### `webkit2gtk-4.0 not found` on Linux

```bash
sudo apt install libwebkit2gtk-4.0-dev    # Debian/Ubuntu
sudo dnf install webkit2gtk3-devel        # Fedora
```

### "WebView2 is not installed" on Windows

Download the Evergreen Bootstrapper from [Microsoft's WebView2 page](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) and run it. Windows 11 has it pre-installed.

### F5 ne fait rien ou erreur de type inconnu

La configuration F5 utilise `node-terminal`, intégré à VS Code sans extension. Si ça échoue, vérifier que `.vscode/launch.json` contient bien `"type": "node-terminal"` et non une ancienne config `lldb` ou `cppvsdbg`.

### Stale dependencies after `git pull`

`pnpm check-env` detects when `pnpm-lock.yaml` is newer than the last install and prints a warning. Run `pnpm install` to resync.

### TypeScript errors after switching branches

```bash
pnpm install        # update node_modules
pnpm check          # confirm the error is in your code, not generated types
```

### Rust binary not found after package rename

If you have a `target/` directory from before the package was renamed to `echoes-studio`, run:

```bash
pnpm clean          # removes src-tauri/target/ cleanly on every OS
```

### `pnpm tauri:dev` runs the env check every time — can I skip it?

Yes — the check is wired as a `predev:tauri` hook, so `pnpm run tauri:dev --ignore-scripts` bypasses it. Alternatively, invoke the Tauri CLI directly: `pnpm exec tauri dev`. Run a full `pnpm check-env` whenever something feels off.
