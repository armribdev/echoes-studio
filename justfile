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
