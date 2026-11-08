set windows-shell := ["pwsh.exe", "-NoLogo", "-Command"]

# Frontend checks (what CLAUDE.md asks before closing a task).
check:
    pnpm check
    pnpm test

# Rust side, run from src-tauri/.
rust:
    cd src-tauri; cargo fmt --check; cargo clippy --all-targets; cargo test
