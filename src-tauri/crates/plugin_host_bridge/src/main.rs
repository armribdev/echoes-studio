//! Out-of-process plugin host (ADR-015, Phase 6d-ii).
//!
//! In the finished sandbox the engine spawns this binary once per sandboxed
//! device, hands it the shared-memory region and the target plugin, and the
//! process runs a real-time loop: attach the rings, load the real VST3/CLAP
//! plugin, then per block drain params/events, call the plugin's `process`, and
//! publish the output — with a watchdog (6e) marking it `Dead` on timeout.
//!
//! **As of 6d-ii** the binary performs the process-lifecycle **handshake** but no
//! audio: in `--region-name` mode it opens the shared region, validates the
//! control block, signals `READY`, and waits for the engine's `SHUTDOWN`. It
//! loads no plugin and processes no audio — the hosting loop is 6d-iii / 6d-iv.
//!
//! ## CLI (stable)
//!
//! ```text
//! plugin_host_bridge --region-name <NAME> --region-bytes <N>
//!     Serve mode: open the named shared region of N bytes, handshake to READY,
//!     run until the engine sets SHUTDOWN. Exit 0 on clean shutdown, 3 on a
//!     handshake/region error.
//!
//! plugin_host_bridge --protocol-version <N>
//!     N must equal plugin_bridge::protocol::BRIDGE_PROTOCOL_VERSION, else exit 2.
//!
//! plugin_host_bridge --print-protocol-version
//!     Print the linked protocol version and exit 0 (engine probe).
//! ```

use std::process::ExitCode;

use plugin_bridge::protocol::BRIDGE_PROTOCOL_VERSION;
use plugin_bridge::session;

/// Engine's requested protocol version does not match this binary's.
const EXIT_PROTOCOL_MISMATCH: u8 = 2;
/// Serve mode failed (region open / control-block validation).
const EXIT_SERVE_FAILED: u8 = 3;
/// Malformed / unsupported arguments.
const EXIT_USAGE: u8 = 64; // EX_USAGE (sysexits.h convention)

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();

    match argv.as_slice() {
        ["--print-protocol-version"] => {
            println!("{BRIDGE_PROTOCOL_VERSION}");
            ExitCode::SUCCESS
        }

        ["--protocol-version", value] => check_protocol(value),

        ["--region-name", name, "--region-bytes", bytes] => serve(name, bytes),

        _ => usage(),
    }
}

fn check_protocol(value: &str) -> ExitCode {
    match value.parse::<u32>() {
        Ok(v) if v == BRIDGE_PROTOCOL_VERSION => {
            eprintln!("[plugin_host_bridge] handshake OK (protocol v{BRIDGE_PROTOCOL_VERSION}).");
            ExitCode::SUCCESS
        }
        Ok(v) => {
            eprintln!(
                "[plugin_host_bridge] protocol mismatch: engine requested v{v}, \
                 bridge speaks v{BRIDGE_PROTOCOL_VERSION}"
            );
            ExitCode::from(EXIT_PROTOCOL_MISMATCH)
        }
        Err(_) => {
            eprintln!("[plugin_host_bridge] invalid --protocol-version value: {value:?}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn serve(name: &str, bytes: &str) -> ExitCode {
    let bytes: usize = match bytes.parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("[plugin_host_bridge] invalid --region-bytes value: {bytes:?}");
            return ExitCode::from(EXIT_USAGE);
        }
    };

    match session::serve(name, bytes) {
        Ok(()) => {
            eprintln!("[plugin_host_bridge] clean shutdown (region '{name}').");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[plugin_host_bridge] serve failed for region '{name}': {e}");
            ExitCode::from(EXIT_SERVE_FAILED)
        }
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: plugin_host_bridge --region-name <NAME> --region-bytes <N>\n\
         \x20      plugin_host_bridge --protocol-version <N>\n\
         \x20      plugin_host_bridge --print-protocol-version"
    );
    ExitCode::from(EXIT_USAGE)
}
