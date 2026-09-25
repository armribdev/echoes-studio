//! Out-of-process plugin host (ADR-015).
//!
//! The engine spawns one bridge per sandboxed plugin.  The bridge loads the
//! plugin, then runs two threads until the engine asks it to stop:
//! - an audio thread that processes the blocks the engine sends over shared
//!   memory ([`plugin_bridge::session::BridgePeer::serve_audio`]);
//! - the main thread, which answers control requests on stdin/stdout and runs
//!   the plugin's editor window.
//!
//! A crash of the plugin kills only this process; the engine notices the
//! missing replies and silences the device.
//!
//! ```text
//! plugin_host_bridge --region-name <N> --max-frames <F> --channels <C>
//!                    --event-capacity <E> --plugin-dll <PATH> --plugin-id <ID>
//! plugin_host_bridge --print-protocol-version
//! ```

#[cfg(windows)]
mod editor;
mod plugin;

use std::collections::HashMap;
use std::io::{self, BufReader, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{mpsc, Arc};

use plugin::{ClapBlockProcessor, LoadedPlugin};
use plugin_bridge::control::{self, Op, PayloadReader, STATUS_ERROR, STATUS_OK};
use plugin_bridge::protocol::BRIDGE_PROTOCOL_VERSION;
use plugin_bridge::session::{BridgeLayout, BridgePeer};

const EXIT_START_FAILED: u8 = 3;
const EXIT_USAGE: u8 = 64;
/// Main-loop wait between checks for control requests and shutdown.
const MAIN_LOOP_WAIT_MS: u32 = 5;

struct Options {
    region_name: String,
    layout: BridgeLayout,
    plugin_dll: PathBuf,
    plugin_id: String,
}

fn parse_options(args: &[String]) -> Option<Options> {
    let values: HashMap<&str, &str> = args
        .chunks(2)
        .map(|pair| match pair {
            [flag, value] => Some((flag.as_str(), value.as_str())),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let number = |flag: &str| values.get(flag)?.parse::<u32>().ok();
    Some(Options {
        region_name: values.get("--region-name")?.to_string(),
        layout: BridgeLayout::new(
            number("--max-frames")?,
            number("--channels")?,
            number("--event-capacity")?,
        ),
        plugin_dll: PathBuf::from(values.get("--plugin-dll")?),
        plugin_id: values.get("--plugin-id")?.to_string(),
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--print-protocol-version"] {
        println!("{BRIDGE_PROTOCOL_VERSION}");
        return ExitCode::SUCCESS;
    }
    let Some(options) = parse_options(&args) else {
        eprintln!("[plugin_host_bridge] invalid arguments: {args:?}");
        return ExitCode::from(EXIT_USAGE);
    };
    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[plugin_host_bridge] {error}");
            ExitCode::from(EXIT_START_FAILED)
        }
    }
}

fn run(options: Options) -> Result<(), String> {
    let peer = Arc::new(
        BridgePeer::open(&options.region_name, options.layout).map_err(|e| e.to_string())?,
    );
    #[cfg(windows)]
    editor::enable_per_monitor_dpi();
    let plugin = match LoadedPlugin::load(&options.plugin_dll, &options.plugin_id) {
        Ok(plugin) => Arc::new(plugin),
        Err(error) => {
            peer.signal_error();
            return Err(format!("cannot load '{}': {error}", options.plugin_id));
        }
    };

    let audio_thread = {
        let peer = Arc::clone(&peer);
        let mut processor =
            ClapBlockProcessor::new(Arc::clone(&plugin), options.layout.channels as usize);
        std::thread::Builder::new()
            .name("bridge-audio".into())
            .spawn(move || {
                raise_thread_priority();
                peer.serve_audio(&mut processor);
            })
            .map_err(|e| e.to_string())?
    };
    let requests = spawn_request_reader();
    peer.signal_ready();

    serve_requests(&peer, &plugin, &requests);

    #[cfg(windows)]
    editor::close();
    peer.request_shutdown();
    let _ = audio_thread.join();
    drop(plugin);
    Ok(())
}

/// Reads control requests from stdin on a helper thread; `None` = stdin closed.
fn spawn_request_reader() -> mpsc::Receiver<Option<(u8, Vec<u8>)>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = BufReader::new(io::stdin());
        loop {
            let request = control::read_frame(&mut stdin).ok();
            let closed = request.is_none();
            if sender.send(request).is_err() || closed {
                return;
            }
        }
    });
    receiver
}

/// Main thread: answer requests and run the editor until shutdown.
fn serve_requests(
    peer: &BridgePeer,
    plugin: &LoadedPlugin,
    requests: &mpsc::Receiver<Option<(u8, Vec<u8>)>>,
) {
    let mut stdout = io::stdout().lock();
    while !peer.shutdown_requested() {
        loop {
            match requests.try_recv() {
                Ok(Some((op, payload))) => {
                    let (status, reply) = match handle(plugin, op, &payload) {
                        Ok(reply) => (STATUS_OK, reply),
                        Err(error) => (STATUS_ERROR, error.into_bytes()),
                    };
                    if control::write_frame(&mut stdout, status, &reply).is_err() {
                        return;
                    }
                }
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => return,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        let _ = stdout.flush();
        wait_for_work();
    }
}

fn handle(plugin: &LoadedPlugin, op: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
    let op = Op::from_u8(op).ok_or_else(|| format!("unknown request {op}"))?;
    let mut reader = PayloadReader::new(payload);
    let invalid = |e: io::Error| e.to_string();
    match op {
        Op::Activate => {
            let sample_rate = reader.f64().map_err(invalid)?;
            let max_frames = reader.u32().map_err(invalid)?;
            plugin
                .activate(sample_rate, max_frames)
                .map(|latency| latency.to_le_bytes().to_vec())
        }
        Op::SaveState => plugin.save_state(),
        Op::LoadState => plugin.load_state(payload).map(|()| Vec::new()),
        Op::Params => Ok(plugin.params()),
        Op::ParamValue => {
            let native_id = reader.u32().map_err(invalid)?;
            plugin
                .param_value(native_id)
                .map(|value| value.to_le_bytes().to_vec())
        }
        Op::FlushValues => {
            let mut values = Vec::new();
            while !reader.is_empty() {
                values.push((
                    reader.u32().map_err(invalid)?,
                    reader.f64().map_err(invalid)?,
                ));
            }
            plugin.flush_values(&values);
            Ok(Vec::new())
        }
        Op::OpenEditor => open_editor(plugin, &String::from_utf8_lossy(payload)),
    }
}

#[cfg(windows)]
fn open_editor(plugin: &LoadedPlugin, title: &str) -> Result<Vec<u8>, String> {
    editor::open(plugin, title).map(|hwnd| hwnd.to_le_bytes().to_vec())
}

#[cfg(not(windows))]
fn open_editor(_: &LoadedPlugin, _: &str) -> Result<Vec<u8>, String> {
    Err("plugin editors are only available on Windows".into())
}

#[cfg(windows)]
fn wait_for_work() {
    editor::pump_messages(MAIN_LOOP_WAIT_MS);
}

#[cfg(not(windows))]
fn wait_for_work() {
    std::thread::sleep(std::time::Duration::from_millis(MAIN_LOOP_WAIT_MS as u64));
}

#[cfg(windows)]
fn raise_thread_priority() {
    const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThread() -> *mut std::ffi::c_void;
        fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
    }
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
}

#[cfg(not(windows))]
fn raise_thread_priority() {}
