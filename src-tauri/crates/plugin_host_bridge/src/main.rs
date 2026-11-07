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
//!                    [--plugin-format clap|vst3] [--plugin-role effect|instrument]
//!                    [--track-info <HEX>]
//! plugin_host_bridge --print-protocol-version
//! plugin_host_bridge --scan <clap|vst3> <FILE>
//! ```
//!
//! `--scan` is not a host: it lists the plugins of one file and prints them (ADR-033,
//! step 10), so that a file which crashes the loader ends this process and not the engine.

mod clap_adapter;
#[cfg(windows)]
mod editor;
mod scan;
mod vst3_adapter;

use std::collections::HashMap;
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use clap_adapter::{ClapAdapter, ClapBlockProcessor};
use clap_ffi::{HostTrackInfo, TimerSchedule};
use plugin_bridge::control::{self, Op, PayloadReader, STATUS_ERROR, STATUS_OK};
use plugin_bridge::protocol::BRIDGE_PROTOCOL_VERSION;
use plugin_bridge::records::{ScannedPluginRecord, TrackRecord, SCAN_EXIT_REFUSED};
use plugin_bridge::session::{BridgeLayout, BridgePeer, BridgeProcessor};
use vst3_adapter::{Vst3Adapter, Vst3BlockProcessor};

const EXIT_START_FAILED: u8 = 3;
const EXIT_USAGE: u8 = 64;
/// Main-loop wait between checks for control requests and shutdown.
const MAIN_LOOP_WAIT_MS: u32 = 5;

#[derive(Clone, Copy)]
enum PluginFormat {
    Clap,
    Vst3,
}

/// What the engine uses the plugin for.  A VST3 instrument gets the notes of the
/// block and is reset by releasing them, an effect is restarted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PluginRole {
    Effect,
    Instrument,
}

/// A plugin hosted by this process, as the control channel on the main thread
/// sees it.  The audio thread has its own processor per format.
trait HostedPlugin {
    fn handle(&self, op: Op, payload: &[u8]) -> Result<Vec<u8>, String>;

    /// Runs the timers the plugin registered; only CLAP plugins have any.
    fn run_due_timers(&self, _schedule: &mut TimerSchedule, _now: Instant) {}
}

struct Options {
    region_name: String,
    format: PluginFormat,
    role: PluginRole,
    layout: BridgeLayout,
    plugin_dll: PathBuf,
    plugin_id: String,
    /// The plugin's track at launch, so `init` already finds it.
    track_info: Option<HostTrackInfo>,
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
    let format = match values.get("--plugin-format").copied() {
        None | Some("clap") => PluginFormat::Clap,
        Some("vst3") => PluginFormat::Vst3,
        Some(_) => return None,
    };
    let role = match values.get("--plugin-role").copied() {
        None | Some("effect") => PluginRole::Effect,
        Some("instrument") => PluginRole::Instrument,
        Some(_) => return None,
    };
    Some(Options {
        region_name: values.get("--region-name")?.to_string(),
        format,
        role,
        layout: BridgeLayout::new(
            number("--max-frames")?,
            number("--channels")?,
            number("--event-capacity")?,
        ),
        plugin_dll: PathBuf::from(values.get("--plugin-dll")?),
        plugin_id: values.get("--plugin-id")?.to_string(),
        track_info: values
            .get("--track-info")
            .and_then(|hex| HostTrackInfo::from_hex(hex)),
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--print-protocol-version"] {
        println!("{BRIDGE_PROTOCOL_VERSION}");
        return ExitCode::SUCCESS;
    }
    if let [flag, format, file] = args.as_slice() {
        if flag == "--scan" {
            return run_scan(format, Path::new(file));
        }
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

fn run_scan(format: &str, file: &Path) -> ExitCode {
    scan::silence_crash_dialogs();
    let scanned = match format {
        "vst3" => scan::scan_vst3(file),
        "clap" => scan::scan_clap(file),
        _ => Err(format!("unknown plugin format {format}")),
    };
    match scanned {
        Ok(plugins) => {
            println!("{}", ScannedPluginRecord::result_line(&plugins));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[plugin_host_bridge] scan of {}: {error}", file.display());
            ExitCode::from(SCAN_EXIT_REFUSED)
        }
    }
}

fn run(options: Options) -> Result<(), String> {
    let peer = Arc::new(
        BridgePeer::open(&options.region_name, options.layout).map_err(|e| e.to_string())?,
    );
    #[cfg(windows)]
    editor::enable_per_monitor_dpi();
    let (plugin, mut processor) = match load_plugin(&options) {
        Ok(loaded) => loaded,
        Err(error) => {
            peer.signal_error();
            return Err(format!("cannot load '{}': {error}", options.plugin_id));
        }
    };

    let audio_thread = {
        let peer = Arc::clone(&peer);
        std::thread::Builder::new()
            .name("bridge-audio".into())
            .spawn(move || {
                raise_thread_priority();
                peer.serve_audio(&mut *processor);
            })
            .map_err(|e| e.to_string())?
    };
    let requests = spawn_request_reader();
    peer.signal_ready();

    serve_requests(&peer, &*plugin, &requests);

    #[cfg(windows)]
    editor::close();
    peer.request_shutdown();
    let _ = audio_thread.join();
    drop(plugin);
    Ok(())
}

type Loaded = (Arc<dyn HostedPlugin>, Box<dyn BridgeProcessor + Send>);

fn load_plugin(options: &Options) -> Result<Loaded, String> {
    let channels = options.layout.channels as usize;
    match options.format {
        PluginFormat::Clap => {
            let plugin = Arc::new(ClapAdapter::load(
                &options.plugin_dll,
                &options.plugin_id,
                options.track_info.as_ref(),
            )?);
            let processor = ClapBlockProcessor::new(Arc::clone(&plugin), channels);
            Ok((plugin, Box::new(processor)))
        }
        PluginFormat::Vst3 => {
            initialise_com()?;
            let plugin = Vst3Adapter::load(&options.plugin_dll, &options.plugin_id, options.role)?;
            let processor = Vst3BlockProcessor::new(Arc::clone(&plugin));
            Ok((plugin, Box::new(processor)))
        }
    }
}

/// VST3 plugins expect the apartment of an application's main thread, and their
/// editor needs OLE for drag and drop.  The thread that loads the
/// plugin is the one the control requests run on.
#[cfg(windows)]
fn initialise_com() -> Result<(), String> {
    #[link(name = "ole32")]
    extern "system" {
        fn OleInitialize(reserved: *mut std::ffi::c_void) -> i32;
    }
    // SAFETY: called once, on the main thread, before any plugin is loaded.
    let result = unsafe { OleInitialize(std::ptr::null_mut()) };
    if result < 0 {
        return Err(format!("COM could not be initialised (0x{result:08x})"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn initialise_com() -> Result<(), String> {
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
    plugin: &dyn HostedPlugin,
    requests: &mpsc::Receiver<Option<(u8, Vec<u8>)>>,
) {
    let mut stdout = io::stdout().lock();
    let mut timers = TimerSchedule::default();
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
        plugin.run_due_timers(&mut timers, Instant::now());
        wait_for_work();
    }
}

fn handle(plugin: &dyn HostedPlugin, op: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
    let op = Op::from_u8(op).ok_or_else(|| format!("unknown request {op}"))?;
    plugin.handle(op, payload)
}

impl HostedPlugin for ClapAdapter {
    fn handle(&self, op: Op, payload: &[u8]) -> Result<Vec<u8>, String> {
        handle_clap(self, op, payload)
    }

    fn run_due_timers(&self, schedule: &mut TimerSchedule, now: Instant) {
        ClapAdapter::run_due_timers(self, schedule, now);
    }
}

fn handle_clap(plugin: &ClapAdapter, op: Op, payload: &[u8]) -> Result<Vec<u8>, String> {
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
        Op::PresetLoad => plugin.load_preset(payload).map(|()| Vec::new()),
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
        Op::ParamText => {
            let native_id = reader.u32().map_err(invalid)?;
            let value = reader.f64().map_err(invalid)?;
            Ok(plugin.param_text(native_id, value))
        }
        Op::OpenEditor => open_editor(plugin, &String::from_utf8_lossy(payload)),
        Op::NoteNames => Ok(plugin.note_names()),
        Op::LoadedPreset => Ok(plugin.loaded_preset()),
        Op::TrackInfo => {
            let track = TrackRecord::decode(payload).map_err(|_| "invalid track info")?;
            plugin.set_track_info(&HostTrackInfo {
                name: track.name,
                color: track.color,
            });
            Ok(Vec::new())
        }
        Op::Handshake => Ok(plugin.handshake()),
        Op::PresetList | Op::PresetLoadState => {
            Err("a CLAP plugin has no program lists or preset states".into())
        }
    }
}

#[cfg(windows)]
fn open_editor(plugin: &ClapAdapter, title: &str) -> Result<Vec<u8>, String> {
    editor::open(plugin, title).map(|hwnd| hwnd.to_le_bytes().to_vec())
}

#[cfg(not(windows))]
fn open_editor(_: &ClapAdapter, _: &str) -> Result<Vec<u8>, String> {
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
