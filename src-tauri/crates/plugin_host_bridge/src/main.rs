//! Out-of-process plugin host (ADR-015, Phase 6c-ii — skeleton).
//!
//! In the finished sandbox (step 6d) the engine spawns this binary once per
//! sandboxed device, hands it the names of the shared-memory regions and the
//! target plugin, and the process runs a real-time loop: attach the rings,
//! load the real VST3/CLAP plugin, then per block drain params/events, call the
//! plugin's `process`, and publish the output — with a watchdog (6e) marking it
//! `Dead` on timeout.
//!
//! **This skeleton does none of that.** It exists in 6c-ii to (a) prove the
//! binary builds and links the shared [`plugin_bridge`] crate, and (b) pin the
//! command-line handshake the engine will use to spawn it. It performs the
//! protocol-version check and exits; it never maps a region, loads a plugin, or
//! processes audio. The hosting loop is added in 6d.
//!
//! ## CLI handshake (stable from here)
//!
//! ```text
//! plugin_host_bridge --protocol-version <N>
//!     N must equal plugin_bridge::protocol::BRIDGE_PROTOCOL_VERSION, else the
//!     engine and bridge disagree on the wire ABI → exit code 2.
//!
//! plugin_host_bridge --print-protocol-version
//!     Print the linked protocol version and exit 0 (used by the engine to probe
//!     a bridge binary before spawning it for real).
//! ```

use std::process::ExitCode;

use plugin_bridge::protocol::BRIDGE_PROTOCOL_VERSION;

/// Exit code returned when the engine's requested protocol version does not
/// match the version this binary was compiled against.
const EXIT_PROTOCOL_MISMATCH: u8 = 2;
/// Exit code for malformed / unsupported arguments.
const EXIT_USAGE: u8 = 64; // EX_USAGE (sysexits.h convention)

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.split_first() {
        Some((flag, rest)) if flag == "--print-protocol-version" && rest.is_empty() => {
            println!("{BRIDGE_PROTOCOL_VERSION}");
            ExitCode::SUCCESS
        }

        Some((flag, [value])) if flag == "--protocol-version" => match value.parse::<u32>() {
            Ok(v) if v == BRIDGE_PROTOCOL_VERSION => {
                eprintln!(
                    "[plugin_host_bridge] handshake OK (protocol v{BRIDGE_PROTOCOL_VERSION}). \
                     Hosting loop not implemented — ADR-015 step 6d."
                );
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
        },

        _ => {
            eprintln!(
                "usage: plugin_host_bridge --protocol-version <N>\n\
                 \x20      plugin_host_bridge --print-protocol-version\n\
                 (skeleton — ADR-015 step 6c-ii; hosting loop lands in 6d)"
            );
            ExitCode::from(EXIT_USAGE)
        }
    }
}
