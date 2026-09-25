//! Engine-side client of a sandboxed plugin's bridge process (ADR-015).
//!
//! [`BridgedPlugin`] is the native handle of a sandboxed `PluginInstance`: it
//! owns the `plugin_host_bridge` child and turns the instance's off-RT calls
//! (activation, state, parameters, editor) into control requests.  Audio goes
//! through [`BridgedPlugin::audio`] from the graph node.
//!
//! Every request runs on the command or UI thread and is bounded by a timeout,
//! so a hung plugin cannot freeze the app; it then fails like a refused call.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use plugin_bridge::control::Op;
use plugin_bridge::session::{AudioPort, BridgeLayout, BridgeProcess};

use crate::plugin_formats::clap::ffi::{read_pod, ClapParamInfo};
use crate::plugin_formats::clap::params::param_info;
use crate::plugin_system::parameter::PluginParamInfo;

/// Largest block a sandboxed plugin processes (the engine's `MAX_BLOCK_FRAMES`).
pub const MAX_FRAMES: u32 = 4096;
pub const CHANNELS: u32 = 2;
/// Room for a block's events in each direction: a full `ClapEventBuffer`
/// (1280 events of ≤ 56 bytes) plus the transport.
pub const EVENT_CAPACITY: u32 = 80 * 1024;

/// Loading a large plugin (wavetables, samples) can take seconds.
const START_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// A condemned bridge may be gone; do not keep the caller waiting.
const DEAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

/// `plugin_host_bridge` next to the running engine binary.
pub fn locate_bridge_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe()
        .ok()?
        .with_file_name(format!("plugin_host_bridge{}", std::env::consts::EXE_SUFFIX));
    exe.exists().then_some(exe)
}

/// `(sample_rate bits, max_frames, latency)` of the current activation.
type Activation = (u32, u32, u32);

pub struct BridgedPlugin {
    process: BridgeProcess,
    activation: Mutex<Option<Activation>>,
}

impl BridgedPlugin {
    /// Spawn a bridge hosting CLAP plugin `clap_id` from `dll`.  Returns once
    /// the plugin is loaded and initialised in the child.
    pub fn spawn(bridge_exe: &Path, dll: &Path, clap_id: &str) -> io::Result<Self> {
        let args: Vec<OsString> = vec![
            "--plugin-dll".into(),
            dll.into(),
            "--plugin-id".into(),
            clap_id.into(),
        ];
        let layout = BridgeLayout::new(MAX_FRAMES, CHANNELS, EVENT_CAPACITY);
        Ok(BridgedPlugin {
            process: BridgeProcess::spawn(bridge_exe, layout, &args, START_TIMEOUT)?,
            activation: Mutex::new(None),
        })
    }

    pub fn audio(&self) -> &AudioPort {
        self.process.audio()
    }

    /// The watchdog condemned the bridge: its plugin crashed or hung.
    pub fn is_dead(&self) -> bool {
        self.audio().is_dead()
    }

    fn request(&self, op: Op, payload: &[u8]) -> io::Result<Vec<u8>> {
        let timeout = if self.is_dead() { DEAD_REQUEST_TIMEOUT } else { REQUEST_TIMEOUT };
        let reply = self.process.request(op, payload, timeout);
        if let Err(error) = &reply {
            eprintln!("[bridge] {op:?} failed: {error}");
        }
        reply
    }

    /// Activate at `(sample_rate, max_frames)` unless already active there.
    /// Returns the plugin's latency, or `None` when activation failed.
    pub fn activate(&self, sample_rate: f32, max_frames: u32) -> Option<u32> {
        let mut activation = self.activation.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((rate, frames, latency)) = *activation {
            if rate == sample_rate.to_bits() && frames == max_frames {
                return Some(latency);
            }
        }
        let mut payload = (sample_rate as f64).to_le_bytes().to_vec();
        payload.extend_from_slice(&max_frames.to_le_bytes());
        let reply = self.request(Op::Activate, &payload).ok()?;
        let latency = u32::from_le_bytes(reply.get(..4)?.try_into().ok()?);
        *activation = Some((sample_rate.to_bits(), max_frames, latency));
        Some(latency)
    }

    pub fn save_state(&self) -> Option<Vec<u8>> {
        self.request(Op::SaveState, &[]).ok()
    }

    pub fn load_state(&self, bytes: &[u8]) -> bool {
        self.request(Op::LoadState, bytes).is_ok()
    }

    pub fn params(&self) -> Vec<PluginParamInfo> {
        let Ok(reply) = self.request(Op::Params, &[]) else {
            return Vec::new();
        };
        let record = std::mem::size_of::<ClapParamInfo>();
        reply
            .get(4..)
            .unwrap_or_default()
            .chunks_exact(record)
            .filter_map(read_pod::<ClapParamInfo>)
            .map(|info| param_info(&info))
            .collect()
    }

    pub fn param_value(&self, native_id: u32) -> Option<f64> {
        let reply = self.request(Op::ParamValue, &native_id.to_le_bytes()).ok()?;
        Some(f64::from_le_bytes(reply.get(..8)?.try_into().ok()?))
    }

    /// Set plain parameter values outside `process`.
    pub fn flush_values(&self, values: &[(u32, f64)]) {
        let payload: Vec<u8> = values
            .iter()
            .flat_map(|(native_id, value)| native_id.to_le_bytes().into_iter().chain(value.to_le_bytes()))
            .collect();
        let _ = self.request(Op::FlushValues, &payload);
    }

    /// Open the plugin's editor in the bridge process, or bring it to the front.
    pub fn open_editor(&self, title: &str) -> Result<(), String> {
        allow_foreground_window(self.process.process_id());
        self.request(Op::OpenEditor, title.as_bytes()).map(|_| ()).map_err(|e| e.to_string())
    }
}

/// Lets the bridge bring its editor window to the front: Windows only grants
/// that to a process the foreground process allows.
#[cfg(windows)]
fn allow_foreground_window(process_id: u32) {
    #[link(name = "user32")]
    extern "system" {
        fn AllowSetForegroundWindow(process_id: u32) -> i32;
    }
    unsafe { AllowSetForegroundWindow(process_id) };
}

#[cfg(not(windows))]
fn allow_foreground_window(_: u32) {}
