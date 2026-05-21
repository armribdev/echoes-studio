# Build Guide — Echoes Studio

This document is the single source of truth for building and running Echoes Studio on any platform. If something is missing or outdated, please open a PR.

---

## Table of contents

1. [Prerequisites](#prerequisites)
2. [Installation per OS](#installation-per-os)
3. [Quick start](#quick-start)
4. [Available commands](#available-commands)
5. [Environment checker](#environment-checker)
6. [How the build pipeline works](#how-the-build-pipeline-works)
7. [Troubleshooting](#troubleshooting)

---

## Prerequisites

All three layers of the stack must be present before you can build.

| Tool | Minimum version | What it's for |
|---|---|---|
| **Node.js** | 18.0.0 | Runs the frontend toolchain (Vite, SvelteKit) |
| **pnpm** | 8.0.0 | Package manager (replaces npm/yarn in this project) |
| **Rust** (via rustup) | 1.60.0 | Compiles the Tauri backend |
| **cargo** | ships with Rust | Rust build tool |
| **Tauri CLI** | 1.5.x | Orchestrates the full dev/build cycle (installed locally via pnpm) |

### Platform-specific additions

| OS | Extra requirement | Why |
|---|---|---|
| **Windows** | WebView2 Runtime (pre-installed on Win 11) | Tauri renders the UI inside a WebView2 window |
| **Windows** | MSVC Build Tools 2019+ | Compiles C dependencies (e.g. `cpal` for audio) |
| **macOS** | Xcode Command Line Tools | Provides the linker and macOS SDK |
| **Linux** | `libwebkit2gtk-4.0-dev` + `libgtk-3-dev` | Tauri requires GTK + WebKit on Linux |

---

## Installation per OS

### Windows

```powershell
# 1. Node.js — download the LTS installer from https://nodejs.org
#    Or use winget:
winget install OpenJS.NodeJS.LTS

# 2. pnpm
npm install -g pnpm@latest

# 3. Rust
# Download and run rustup-init.exe from https://rustup.rs
# Accept the defaults (stable toolchain, MSVC host)
rustup toolchain install stable

# 4. Visual Studio Build Tools
# Download from https://visualstudio.microsoft.com/visual-cpp-build-tools/
# Select the "Desktop development with C++" workload

# WebView2 is pre-installed on Windows 11.
# On Windows 10, download the Evergreen Bootstrapper from:
# https://developer.microsoft.com/en-us/microsoft-edge/webview2/
```

### macOS

```bash
# 1. Xcode Command Line Tools (if not already installed)
xcode-select --install

# 2. Node.js — via Homebrew or the official installer
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
# 1. System dependencies for Tauri
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.0-dev \
  build-essential \
  curl \
  wget \
  libssl-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf

# 2. Node.js (use nvm to avoid permission issues)
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.7/install.sh | bash
source ~/.bashrc
nvm install 20
nvm use 20

# 3. pnpm
npm install -g pnpm@latest

# 4. Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

### Linux (Fedora / RHEL)

```bash
# 1. System dependencies
sudo dnf install -y \
  webkit2gtk3-devel \
  openssl-devel \
  curl \
  wget \
  gtk3-devel \
  libappindicator-gtk3-devel \
  librsvg2-devel

# 2–4. Node.js, pnpm, and Rust: same as Debian instructions above
```

---

## Quick start

These four commands get you from a fresh clone to a running dev build on any OS:

```bash
# 1. Install JS dependencies (reads pnpm-lock.yaml — fully reproducible)
pnpm install

# 2. (Optional but recommended) Verify your environment before building
pnpm check-env

# 3. Launch the full dev build — Vite + Rust compiled together
pnpm tauri:dev
```

The first `tauri:dev` takes several minutes while Cargo compiles dependencies. Subsequent runs are much faster thanks to the Cargo incremental build cache.

---

## Available commands

| Command | What it does |
|---|---|
| `pnpm install` | Install all JS dependencies from the lock file |
| `pnpm check-env` | Verify all required tools are present and at the right version |
| `pnpm tauri:dev` | **Start the app in dev mode** — hot-reload for Svelte, incremental rebuild for Rust |
| `pnpm tauri:build` | Compile a release build and create platform installers |
| `pnpm dev` | Start the Vite dev server only (frontend, no Tauri desktop shell) |
| `pnpm build` | Build the frontend only (SvelteKit → `build/`) |
| `pnpm check` | TypeScript type-check + Svelte validation |
| `pnpm check:watch` | Same as above, in watch mode |
| `pnpm lint` | Check formatting (Prettier) and linting (ESLint) |
| `pnpm format` | Auto-format all files in-place |

### Rust-specific commands

Run these from inside `src-tauri/`:

```bash
cd src-tauri

cargo check           # Fast type-check without compiling binaries
cargo clippy          # Lint (catches common Rust mistakes)
cargo test            # Run unit tests
cargo fmt             # Auto-format Rust code
```

---

## Environment checker

`pnpm check-env` (or `node scripts/check-env.mjs`) prints a structured report of every required tool:

```
Echoes Studio — Environment Check
Platform : win32 x64
Date     : 2026-05-21

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

Node dependencies
────────────────────────────────────────────────────
  ✓ OK  node_modules installed  found
  ✓ OK  @tauri-apps/cli (local)  found 1.6.3

Platform-specific requirements
────────────────────────────────────────────────────
  ✓ OK  Microsoft WebView2 Runtime  found in registry
  ✓ OK  MSVC Build Tools (cl.exe)  found at C:\Program Files\...

────────────────────────────────────────────────────
All checks passed — ready to build!
Run: pnpm tauri:dev
```

The script exits with code `1` when any error is found, making it CI-friendly.

---

## How the build pipeline works

```
pnpm tauri:dev
│
├── tauri CLI reads src-tauri/tauri.conf.json
│   ├── runs beforeDevCommand: "pnpm run dev"  →  Vite dev server (port 5173)
│   └── compiles src-tauri/ with cargo (debug profile)
│
├── Tauri opens a native window pointed at http://localhost:5173
│
└── File watchers run in parallel:
    ├── Vite  →  hot-module replacement for .svelte / .ts changes (instant)
    └── cargo →  incremental recompile for .rs changes (seconds)
```

```
pnpm tauri:build
│
├── runs beforeBuildCommand: "pnpm run build"  →  SvelteKit → build/
│
└── cargo build --release
    └── bundles installer for the current OS:
        ├── Windows  →  .msi + .exe (NSIS)
        ├── macOS    →  .dmg + .app
        └── Linux    →  .deb + .AppImage
```

**Output locations:**

| Artifact | Path |
|---|---|
| Frontend build | `build/` |
| Rust debug binary | `src-tauri/target/debug/echoes-studio[.exe]` |
| Rust release binary | `src-tauri/target/release/echoes-studio[.exe]` |
| Platform bundles | `src-tauri/target/release/bundle/` |

---

## Troubleshooting

### `pnpm tauri:dev` fails with "npm not found" or "pnpm not found"

Tauri reads `beforeDevCommand` from `tauri.conf.json`. Make sure your shell has `pnpm` in its PATH. After installing pnpm, open a **new** terminal session so the PATH is refreshed.

### First `cargo build` is very slow

Normal — Cargo compiles the full dependency tree from scratch. The `target/` directory caches compiled artifacts; subsequent builds are incremental and much faster. Do **not** delete `target/` unless debugging a stale-cache issue.

### `error: linking with cc failed` on Linux

Missing C toolchain. Run:
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

### `error[E0463]: can't find crate for std` (wrong Rust target)

Run:
```bash
rustup target add x86_64-pc-windows-msvc   # Windows
rustup target add x86_64-apple-darwin      # macOS (Intel)
rustup target add aarch64-apple-darwin     # macOS (Apple Silicon)
rustup target add x86_64-unknown-linux-gnu # Linux
```

### SvelteKit `adapter-auto` warning in dev

This is expected in a local Tauri context — adapter-auto emits a warning because it cannot detect the deployment platform. The build still works correctly; the Tauri Vite plugin overrides the adapter behaviour.

### TypeScript errors after `git pull`

Run `pnpm install` first — a package update may have changed the types. Then `pnpm check` to confirm the error is in your code and not in stale generated types.

### Port 5173 already in use

Either stop the other process or change the dev port in `vite.config.ts`:
```ts
server: {
  port: 5174,   // pick any free port
  strictPort: true
}
```
Then update `devPath` in `src-tauri/tauri.conf.json` to match.

### Rust binary not found after rename

The Cargo package was renamed from `app` to `echoes-studio`. If you have an old `target/` directory from before this rename, run `cargo clean` inside `src-tauri/` to clear stale artifacts:
```bash
cd src-tauri && cargo clean
```
