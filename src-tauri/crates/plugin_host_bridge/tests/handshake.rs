//! End-to-end spawn + control handshake against the real bridge binary
//! (ADR-015, Phase 6d-ii).
//!
//! Windows-only: it exercises `SharedRegion`, which has a real implementation
//! only on Windows in 6d-i.  `env!("CARGO_BIN_EXE_plugin_host_bridge")` is
//! provided by Cargo to integration tests of the crate that defines the binary.
#![cfg(windows)]

use std::path::Path;
use std::time::Duration;

use plugin_bridge::session::{BridgeSession, CONTROL_BYTES};

fn unique_name(tag: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("echoes_bridge_it_{}_{}_{}", std::process::id(), tag, n)
}

#[test]
fn spawn_reaches_ready_then_shuts_down() {
    let exe = Path::new(env!("CARGO_BIN_EXE_plugin_host_bridge"));
    let name = unique_name("ready");

    // `spawn` returning Ok means: child launched, opened the region, validated
    // the control block, and flipped status to READY — the full handshake.
    let session = BridgeSession::spawn(exe, &name, CONTROL_BYTES, Duration::from_secs(5))
        .expect("bridge should reach READY");
    assert_eq!(session.name(), name);

    // Dropping the session sets SHUTDOWN and reaps the child (grace, then kill).
    // A clean return here means no hang / no leaked process.
    drop(session);
}

#[test]
fn spawn_is_repeatable() {
    // Two sessions back-to-back must not collide (unique names, clean teardown).
    let exe = Path::new(env!("CARGO_BIN_EXE_plugin_host_bridge"));
    for _ in 0..2 {
        let name = unique_name("repeat");
        let session = BridgeSession::spawn(exe, &name, CONTROL_BYTES, Duration::from_secs(5))
            .expect("bridge should reach READY");
        drop(session);
    }
}
