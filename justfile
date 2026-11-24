set windows-shell := ["pwsh.exe", "-NoLogo", "-Command"]

# Frontend checks (what CLAUDE.md asks before closing a task).
check:
    pnpm check
    pnpm test

# Rust side, same gates as scripts/test-all.mjs.
[working-directory('src-tauri')]
rust:
    cargo fmt -- --check
    cargo clippy --workspace -- -D warnings
    cargo test --workspace

# Interactive dev: Tauri window with Vite HMR. Rust is not rebuilt on save: restart to pick up Rust changes.
dev:
    pnpm tauri:dev

# Same, but cargo rebuilds and relaunches the app on every Rust save.
dev-watch:
    pnpm tauri:dev:watch
