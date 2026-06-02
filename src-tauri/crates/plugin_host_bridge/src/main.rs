//! Out-of-process plugin host (ADR-015, Phase 6d-ii).
//!
//! In the finished sandbox the engine spawns this binary once per sandboxed
//! device, hands it the shared-memory region and the target plugin, and the
//! process runs a real-time loop: attach the rings, load the real VST3/CLAP
//! plugin, then per block drain params/events, call the plugin's `process`, and
//! publish the output — with a watchdog (6e) marking it `Dead` on timeout.
//!
//! **As of 6d-iii** the serve mode runs the ring-driven **process loop** with an
//! identity pass-through stand-in: it opens the shared region, attaches the audio
//! rings, signals `READY`, and copies each input block to the output ring until
//! the engine sets `SHUTDOWN`.  It loads **no real plugin yet** — the VST3/CLAP
//! FFI load that replaces the identity copy is 6d-iii-b.
//!
//! ## CLI (stable)
//!
//! ```text
//! plugin_host_bridge --region-name <N> --block-size <B> --channels <C> --audio-capacity <A>
//!     Serve mode: open the region for this layout, run the process loop until
//!     the engine sets SHUTDOWN. Exit 0 on clean shutdown, 3 on a region error,
//!     64 on bad arguments.
//!
//! plugin_host_bridge --protocol-version <N>
//!     N must equal plugin_bridge::protocol::BRIDGE_PROTOCOL_VERSION, else exit 2.
//!
//! plugin_host_bridge --print-protocol-version
//!     Print the linked protocol version and exit 0 (engine probe).
//! ```

mod clap_processor;

use std::path::Path;
use std::process::ExitCode;

use clap_processor::ClapProcessor;
use plugin_bridge::protocol::BRIDGE_PROTOCOL_VERSION;
use plugin_bridge::session::{self, BridgeLayout};

/// Engine's requested protocol version does not match this binary's.
const EXIT_PROTOCOL_MISMATCH: u8 = 2;
/// Serve mode failed (region open / control-block validation / plugin load).
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

        ["--region-name", name, "--block-size", bs, "--channels", ch, "--audio-capacity", cap] => {
            serve(name, bs, ch, cap)
        }

        ["--region-name", name, "--block-size", bs, "--channels", ch, "--audio-capacity", cap, "--sample-rate", sr, "--plugin-dll", dll, "--plugin-id", pid] => {
            serve_plugin(name, bs, ch, cap, sr, dll, pid)
        }

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

fn serve(name: &str, block_size: &str, channels: &str, audio_capacity: &str) -> ExitCode {
    let layout = match (block_size.parse(), channels.parse(), audio_capacity.parse()) {
        (Ok(bs), Ok(ch), Ok(cap)) => BridgeLayout::new(bs, ch, cap),
        _ => {
            eprintln!(
                "[plugin_host_bridge] invalid layout args: \
                 block-size={block_size:?} channels={channels:?} audio-capacity={audio_capacity:?}"
            );
            return ExitCode::from(EXIT_USAGE);
        }
    };

    match session::serve(name, &layout) {
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

/// Serve mode driving a **real loaded CLAP plugin** (ADR-015 6d-iii-b-2).  Loads
/// the `.clap`, wires it as the processor, and runs the same loop as [`serve`].
fn serve_plugin(
    name: &str,
    block_size: &str,
    channels: &str,
    audio_capacity: &str,
    sample_rate: &str,
    plugin_dll: &str,
    plugin_id: &str,
) -> ExitCode {
    let (Ok(bs), Ok(ch), Ok(cap), Ok(sr)) = (
        block_size.parse::<u32>(),
        channels.parse::<u32>(),
        audio_capacity.parse::<usize>(),
        sample_rate.parse::<f64>(),
    ) else {
        eprintln!("[plugin_host_bridge] invalid plugin serve args");
        return ExitCode::from(EXIT_USAGE);
    };
    let layout = BridgeLayout::new(bs, ch, cap);

    let mut processor = match ClapProcessor::load(
        Path::new(plugin_dll),
        plugin_id,
        sr,
        ch as usize,
        bs as usize,
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[plugin_host_bridge] plugin load failed ({plugin_id}): {e}");
            return ExitCode::from(EXIT_SERVE_FAILED);
        }
    };

    match session::serve_with(name, &layout, &mut processor) {
        Ok(()) => {
            eprintln!(
                "[plugin_host_bridge] clean shutdown (region '{name}', plugin '{plugin_id}')."
            );
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
        "usage: plugin_host_bridge --region-name <N> --block-size <B> --channels <C> --audio-capacity <A>\n\
         \x20      [--sample-rate <SR> --plugin-dll <PATH> --plugin-id <ID>]   (load a real CLAP plugin)\n\
         \x20      plugin_host_bridge --protocol-version <N>\n\
         \x20      plugin_host_bridge --print-protocol-version"
    );
    ExitCode::from(EXIT_USAGE)
}
