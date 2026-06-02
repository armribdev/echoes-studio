//! End-to-end spawn + handshake + audio round-trip against the real bridge
//! binary (ADR-015, Phase 6d-ii / 6d-iii).
//!
//! Windows-only: it exercises `SharedRegion`, which has a real implementation
//! only on Windows in 6d-i.  `env!("CARGO_BIN_EXE_plugin_host_bridge")` is
//! provided by Cargo to integration tests of the crate that defines the binary.
#![cfg(windows)]

use std::path::Path;
use std::time::{Duration, Instant};

use plugin_bridge::protocol::{
    BlockHeader, BLOCK_HEADER_BYTES, BLOCK_MAGIC, BRIDGE_PROTOCOL_VERSION,
};
use plugin_bridge::session::{BridgeLayout, BridgeSession};

fn unique_name(tag: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("echoes_bridge_it_{}_{}_{}", std::process::id(), tag, n)
}

fn bridge_exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_plugin_host_bridge"))
}

#[test]
fn spawn_reaches_ready_then_shuts_down() {
    let layout = BridgeLayout::new(64, 2, 8);
    let name = unique_name("ready");

    // `spawn` returning Ok means the full handshake completed: child launched,
    // opened the region, validated the control block, attached the rings, READY.
    let session = BridgeSession::spawn(bridge_exe(), &name, &layout, Duration::from_secs(5))
        .expect("bridge should reach READY");
    assert_eq!(session.name(), name);

    // Drop sets SHUTDOWN and reaps the child (grace, then kill). Clean return =
    // no hang / no leaked process.
    drop(session);
}

#[test]
fn process_block_round_trips_planar_audio() {
    use plugin_bridge::session::BlockOutcome;
    use std::time::Duration;

    let block_size = 32u32;
    let channels = 2u32;
    let layout = BridgeLayout::new(block_size, channels, 8);
    let name = unique_name("rt");

    let mut session = BridgeSession::spawn(bridge_exe(), &name, &layout, Duration::from_secs(5))
        .expect("bridge should reach READY");

    let n = (channels * block_size) as usize;
    let mut output = vec![0.0f32; n];

    for seq in 0..32u64 {
        // Planar input, distinct per block.
        let input: Vec<f32> = (0..n).map(|i| seq as f32 + i as f32 * 0.5).collect();
        // Generous budget: the passthrough bridge echoes well within it.
        let outcome =
            session.process_block(&input, &mut output, seq, 48_000, Duration::from_secs(1));
        assert_eq!(
            outcome,
            BlockOutcome::Processed,
            "block {seq} should round-trip"
        );
        assert_eq!(output, input, "block {seq} samples mismatch");
    }
    assert_eq!(session.timeout_count(), 0);
    assert!(!session.is_dead(), "a healthy bridge must not be condemned");

    drop(session);
}

#[test]
fn watchdog_condemns_bridge_after_consecutive_timeouts() {
    use plugin_bridge::session::{BlockOutcome, DEAD_AFTER_CONSECUTIVE_TIMEOUTS};

    let layout = BridgeLayout::new(16, 2, 8);
    let name = unique_name("watchdog");
    let mut session = BridgeSession::spawn(bridge_exe(), &name, &layout, Duration::from_secs(5))
        .expect("bridge should reach READY");

    let n = (2 * 16) as usize;
    let input = vec![1.0f32; n];
    let mut output = vec![1.0f32; n];

    // A zero spin budget guarantees a timeout every block (the bridge cannot echo
    // within ~nanoseconds), so the watchdog trips deterministically.
    for _ in 0..DEAD_AFTER_CONSECUTIVE_TIMEOUTS {
        assert!(!session.is_dead(), "not condemned before the threshold");
        let outcome = session.process_block(&input, &mut output, 0, 48_000, Duration::ZERO);
        assert_eq!(outcome, BlockOutcome::TimedOut);
        assert!(
            output.iter().all(|&s| s == 0.0),
            "timed-out block is silence"
        );
    }

    assert!(
        session.is_dead(),
        "condemned after the consecutive-timeout threshold"
    );

    // Once dead, even a generous budget short-circuits to silence immediately —
    // no more spinning on the audio thread.
    output.iter_mut().for_each(|s| *s = 1.0);
    let outcome = session.process_block(&input, &mut output, 0, 48_000, Duration::from_secs(1));
    assert_eq!(outcome, BlockOutcome::TimedOut);
    assert!(output.iter().all(|&s| s == 0.0));

    drop(session);
}

#[test]
fn audio_blocks_round_trip_through_the_bridge() {
    let block_size = 64u32;
    let channels = 2u32;
    let layout = BridgeLayout::new(block_size, channels, 8);
    let name = unique_name("audio");

    let session = BridgeSession::spawn(bridge_exe(), &name, &layout, Duration::from_secs(5))
        .expect("bridge should reach READY");

    let frames = (channels * block_size) as usize;
    let mut out = vec![0u8; session.audio_slot_size()];

    for seq in 0..16u64 {
        // Distinct samples per block so a swap/drop/reorder would be caught.
        let samples: Vec<f32> = (0..frames)
            .map(|i| seq as f32 * 1000.0 + i as f32)
            .collect();
        let sample_bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let header = BlockHeader {
            magic: BLOCK_MAGIC,
            version: BRIDGE_PROTOCOL_VERSION,
            block_size,
            channel_count: channels,
            sample_rate: 48_000,
            _reserved: 0,
            sequence: seq,
        };

        // Ping-pong: send one block, then wait (bounded) for it to come back.
        assert!(
            session.try_send_input(&header, &sample_bytes),
            "input ring full"
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let n = loop {
            if let Some(n) = session.try_recv_output(&mut out) {
                break n;
            }
            assert!(Instant::now() < deadline, "bridge did not echo block {seq}");
            std::hint::spin_loop();
        };

        // The identity stand-in echoes header + samples unchanged.
        let got_header = BlockHeader::from_bytes(&out[..n]).expect("header");
        assert_eq!(got_header.sequence, seq);
        assert_eq!(got_header.channel_count, channels);
        let got_samples: Vec<f32> = out[BLOCK_HEADER_BYTES..n]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(got_samples, samples, "block {seq} samples mismatch");
    }

    drop(session);
}
