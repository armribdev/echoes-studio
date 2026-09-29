//! `AudioEngine` — owns the CPAL stream and the current `GraphScheduler`.
//!
//! ## Architecture
//!
//! ```text
//! Command thread                      Audio thread (CPAL callback)
//! ──────────────                      ────────────────────────────
//! AudioGraphCompiler::compile()
//! GraphScheduler::new(compiled)
//! RuntimeSnapshot::enqueue(sched)  ──► try_swap(&mut current) → atomic swap
//!
//! SharedTransport::play()          ──► transport.snapshot() → TransportInfo
//!                                       scheduler.process_block()
//!                                       write to CPAL output buffer
//!                                       position_samples advanced per chunk (loop wrap)
//! ```
//!
//! ## Real-time safety contract
//!
//! The audio callback:
//!   ✓ Never allocates (mix_buf and scheduler Box live for the stream's lifetime)
//!   ✓ Never acquires a blocking lock (`try_swap` uses `AtomicPtr`)
//!   ✓ Never deallocates (displaced schedulers are handed off to the command
//!     thread via `RuntimeSnapshot`'s SPSC drop ring)
//!   ✓ Never performs I/O
//!   ✓ Never calls `async`/`await`
//!   ✓ Only reads from `GraphScheduler` (owned for the callback's duration)

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, SizedSample,
};
use tauri::Emitter;

use crate::audio_engine::graph::AudioGraphCompiler;
use crate::audio_engine::transport::{playback_chunk, SharedTransport};
use crate::audio_graph::denormal::enable_flush_to_zero;
use crate::audio_graph::midi::LiveInputClock;
use crate::audio_graph::node::TimeSignature;
use crate::audio_graph::{CompileConfig, GraphScheduler, LiveParams, MeterCell, RuntimeSnapshot};
use crate::audio_settings::AudioSettings;
use crate::engine_gateway::events::{
    ev, LevelsUpdatedPayload, PositionUpdatedPayload, TelemetryEngineEvent,
};
use crate::plugin_system::PluginHost;
use crate::project::AppProject;

/// Wraps `cpal::Stream` to implement `Send`.
///
/// On Windows, CPAL marks `Stream` as `!Send` via `PhantomData<*mut ()>`.
/// This is a conservative platform annotation. In practice the stream is safe
/// to move between threads because:
///   - It is exclusively accessed behind `Mutex<AudioEngine>` (single owner).
///   - CPAL's audio callback runs on CPAL's own internal thread, not through
///     Rust's `Send` boundary.
///
/// All mainstream CPAL applications use this pattern; see also
/// https://github.com/RustAudio/cpal/issues/818.
// Field 0 is never read: holding it is what keeps the CPAL stream playing.
struct SendStream(#[allow(dead_code)] cpal::Stream);
// SAFETY: see doc comment above.
unsafe impl Send for SendStream {}

/// Maximum number of frames per audio block.
///
/// Pre-allocates the mix buffer.  Must be ≥ the device's actual block size.
/// 4096 frames @ 48 kHz = ~85 ms — well above any typical WASAPI buffer.
const MAX_BLOCK_FRAMES: usize = 4096;

/// Position-reporter poll interval in milliseconds.
const REPORTER_INTERVAL_MS: u64 = 33;

/// Maximum number of audio channels tracked by the level meters.
const MAX_METER_CHANNELS: usize = 16;

const _: () = assert!(MAX_METER_CHANNELS == crate::engine_gateway::events::METER_CHANNEL_COUNT);

const ZERO_METER_LEVELS: [f32; MAX_METER_CHANNELS] = [0.0; MAX_METER_CHANNELS];

const DEFAULT_COMPILE_NUM_CHANNELS: usize = 2;

/// Minimum slack added to the expected block duration before declaring an XRun.
///
/// Real backends jitter slightly even under normal load; this avoids false positives.
const XRUN_SLACK_MIN: Duration = Duration::from_micros(2_000);

/// Wall-clock window after stream start during which XRun detection is suppressed.
///
/// The backend pre-fills its ring buffer and the first non-empty graph is swapped
/// in cold (cold caches, first `process_block`), so the earliest callbacks have
/// irregular timing that would otherwise count as false XRuns. A fixed block count
/// is fragile across buffer sizes; a short time window covers the transient.
const XRUN_WARMUP: Duration = Duration::from_millis(250);

/// Lock-free underrun counter shared between the audio callback (writer) and
/// the position reporter (reader → UI telemetry).
pub struct XrunStats {
    total: AtomicU64,
}

impl XrunStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            total: AtomicU64::new(0),
        })
    }

    #[inline]
    pub fn record(&self) {
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.total.store(0, Ordering::Relaxed);
    }
}

/// WASAPI delivers the callback on a normal-priority thread. A synth GUI (Vital)
/// preempts it and the block arrives late, heard as crackling. One call, no alloc.
fn raise_audio_thread_priority() {
    #[cfg(windows)]
    unsafe {
        unsafe extern "system" {
            fn GetCurrentThread() -> *mut std::ffi::c_void;
            fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
        }
        const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }
}

/// Returns `true` when the gap between consecutive callbacks exceeds the
/// expected block duration by more than the allowed slack.
fn is_callback_xrun(elapsed: Duration, expected_block: Duration) -> bool {
    let slack = XRUN_SLACK_MIN.max(expected_block / 4);
    elapsed > expected_block + slack
}

/// Opt-in XRun tracing on the reporter thread (never from the audio callback).
///
/// Set `ECHOES_DEBUG_XRUN=1` before launch, or build in debug mode where it
/// defaults to enabled.
fn xrun_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("ECHOES_DEBUG_XRUN")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(cfg!(debug_assertions))
    })
}

/// Log newly detected XRuns on the reporter thread (RT-safe).
fn log_new_xruns(last_reported: &mut u64, current: u64) {
    if current <= *last_reported {
        return;
    }
    if xrun_debug_enabled() {
        let delta = current - *last_reported;
        eprintln!("[audio] XRun détecté — total={current} (+{delta})");
    }
    *last_reported = current;
}

/// Per-channel peak levels shared lock-free between the audio callback (writer)
/// and the position reporter (reader → Tauri event).
///
/// Levels are stored as f32 bit-patterns in `AtomicU32`, following the same
/// pattern used in `SharedTransport` for `bpm_bits` / `sample_rate_bits`.
struct ChannelLevels {
    /// Output peak per channel, written each audio block.
    output: Box<[AtomicU32; MAX_METER_CHANNELS]>,
    /// Input peak per channel — zeroed until an input stream is opened.
    input: Box<[AtomicU32; MAX_METER_CHANNELS]>,
}

impl ChannelLevels {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            output: Box::new(std::array::from_fn(|_| AtomicU32::new(0))),
            input: Box::new(std::array::from_fn(|_| AtomicU32::new(0))),
        })
    }

    /// Write a peak amplitude to output channel `ch` (no-op if `ch` ≥ 16).
    #[inline]
    fn set_output(&self, ch: usize, peak: f32) {
        if ch < MAX_METER_CHANNELS {
            self.output[ch].store(peak.to_bits(), Ordering::Relaxed);
        }
    }

    /// Read all output levels as a fixed-size array.
    fn read_outputs(&self) -> [f32; MAX_METER_CHANNELS] {
        std::array::from_fn(|i| f32::from_bits(self.output[i].load(Ordering::Relaxed)))
    }

    /// Read all input levels as a fixed-size array.
    fn read_inputs(&self) -> [f32; MAX_METER_CHANNELS] {
        std::array::from_fn(|i| f32::from_bits(self.input[i].load(Ordering::Relaxed)))
    }
}

/// The real-time audio engine.  Held behind `Mutex<AudioEngine>` in `AppState`.
pub struct AudioEngine {
    /// Pending scheduler waiting to be picked up atomically by the audio callback.
    snapshot: Arc<RuntimeSnapshot>,

    /// Shared atomic transport state (also held by the callback closure and the
    /// position reporter thread).
    pub transport: Arc<SharedTransport>,

    /// Keeps the CPAL stream alive for the lifetime of the engine.
    /// Dropping this stops audio.
    _stream: Option<SendStream>,

    /// Set to `true` to signal the position reporter thread to exit.
    /// Replaced with a fresh `Arc` on every `start_stream_with_settings` call.
    reporter_stop: Arc<AtomicBool>,

    /// Per-channel peak levels — written by the audio callback, read by the
    /// position reporter thread which emits them as `LEVELS_UPDATED` events.
    levels: Arc<ChannelLevels>,

    /// Per-track peak cells from the last graph compile.
    /// Written every block by `MeterNode` instances; read by the reporter thread.
    /// Protected by a `Mutex` so the engine can swap the map on recompile while
    /// the reporter thread holds a clone of this `Arc`.
    pub meter_cells: Arc<Mutex<HashMap<String, MeterCell>>>,

    /// Active output channel count, readable and writable without restarting
    /// the stream.  `0` means "use all hardware channels".
    /// The audio callback reads this atomically every block, so changes
    /// (mono↔stereo, +/−) take effect within one buffer block (~5 ms).
    pub active_output_channels: Arc<AtomicUsize>,

    /// Live parameter registry — atomic cells shared with `AppState` and read
    /// by audio nodes per-block.  Threaded into every compile so the resulting
    /// scheduler holds the matching `Arc` handles.
    live_params: Arc<LiveParams>,

    /// Cumulative underrun count for the current stream session.
    pub xrun_stats: Arc<XrunStats>,

    /// Plugin host — owns the registry, instantiates `Box<dyn AudioNode>` for
    /// every `NodeKind::Plugin` step at compile time, and tracks per-device
    /// lifecycle state.  Lives on the command thread only; consulted by the
    /// graph compiler during `update_from_project`.  The audio thread never
    /// touches it (device instances are moved into the `GraphScheduler`).
    pub plugin_host: PluginHost,
}

impl AudioEngine {
    pub fn new(transport: Arc<SharedTransport>, live_params: Arc<LiveParams>) -> Self {
        Self {
            snapshot: Arc::new(RuntimeSnapshot::new()),
            transport,
            _stream: None,
            // `true` so any stale reporter from a previous engine instance exits.
            reporter_stop: Arc::new(AtomicBool::new(true)),
            levels: ChannelLevels::new(),
            meter_cells: Arc::new(Mutex::new(HashMap::new())),
            active_output_channels: Arc::new(AtomicUsize::new(0)),
            live_params,
            xrun_stats: XrunStats::new(),
            plugin_host: {
                let mut host = PluginHost::new();
                host.scan_plugins();
                host
            },
        }
    }

    /// Update the active output channel count without restarting the stream.
    ///
    /// `count = 0` means "use all hardware channels" (default).
    /// The change takes effect within one audio callback block (~5 ms).
    pub fn set_active_output_channels(&self, count: usize) {
        self.active_output_channels.store(count, Ordering::Relaxed);
    }

    /// Compile a new `GraphScheduler` from `project` and enqueue it for atomic
    /// pickup by the audio callback.
    ///
    /// **Never call from the audio thread.**
    pub fn update_from_project(&mut self, project: &AppProject) {
        let config = CompileConfig {
            sample_rate: self.transport.sample_rate(),
            max_block_size: MAX_BLOCK_FRAMES,
            num_channels: DEFAULT_COMPILE_NUM_CHANNELS,
            live: true,
        };
        let (scheduler, cells, duration_samples) = AudioGraphCompiler::compile_scheduler(
            project,
            config,
            &self.live_params,
            &mut self.plugin_host,
        );
        self.transport.set_duration(duration_samples);
        // Also covers project load and undo/redo, which never go through UPDATE_TEMPO.
        self.transport.set_tempo(
            project.bpm,
            TimeSignature::new(project.numerator, project.denominator),
        );
        if let Ok(mut map) = self.meter_cells.lock() {
            *map = cells;
        }
        self.snapshot.enqueue(scheduler);
    }

    /// Stop any running stream and signal the position reporter to exit.
    ///
    /// Dropping `_stream` stops the CPAL audio callback immediately (it drains
    /// any pending buffer before the `Stream` destructor returns).
    pub fn stop_stream(&mut self) {
        self.reporter_stop.store(true, Ordering::Relaxed);
        self._stream = None;
    }

    /// Open an output stream using the provided `settings`, compile the initial
    /// graph, and spawn the position-reporter background thread.
    ///
    /// Any previously running stream is stopped first — this is safe to call
    /// multiple times (e.g. when the user changes devices in the Settings panel).
    ///
    /// On failure the method returns an error and the engine remains silent.
    pub fn start_stream_with_settings(
        &mut self,
        project: &AppProject,
        app: tauri::AppHandle,
        settings: &AudioSettings,
    ) -> Result<(), String> {
            self.stop_stream();

        let host = cpal::default_host();
        let device = find_output_device(&host, &settings.output_device)
            .ok_or_else(|| "Aucun périphérique de sortie audio trouvé".to_string())?;

        let (base_cfg, fmt) = resolve_stream_config(&device, settings.sample_rate)?;
        let actual_sr = base_cfg.sample_rate.0;
        let hw_channels = base_cfg.channels as usize;

        // `0` means "use all hardware channels" (default / unset).
        let active_channels = if settings.output_channels == 0 {
            hw_channels
        } else {
            (settings.output_channels as usize).min(hw_channels).max(1)
        };

        eprintln!(
            "[audio] {} — {} Hz, {} hw-ch, {} active-ch, {:?}",
            device
                .name()
                .unwrap_or_else(|_| "Périphérique inconnu".to_string()),
            actual_sr,
            hw_channels,
            active_channels,
            fmt,
        );

        self.transport.set_sample_rate(actual_sr as f32);

        let compile_cfg = CompileConfig {
            sample_rate: actual_sr as f32,
            max_block_size: MAX_BLOCK_FRAMES,
            num_channels: DEFAULT_COMPILE_NUM_CHANNELS,
            live: true,
        };
        let (initial_scheduler, initial_cells, initial_duration) =
            AudioGraphCompiler::compile_scheduler(
                project,
                compile_cfg,
                &self.live_params,
                &mut self.plugin_host,
            );
        self.transport.set_duration(initial_duration);
        if let Ok(mut map) = self.meter_cells.lock() {
            *map = initial_cells;
        }
        self.snapshot.enqueue(initial_scheduler);

        self.active_output_channels
            .store(active_channels, Ordering::Relaxed);
        self.xrun_stats.reset();

        let snap_a = Arc::clone(&self.snapshot);
        let tport_a = Arc::clone(&self.transport);
        let ch_ctrl_a = Arc::clone(&self.active_output_channels);
        let levels_a = Arc::clone(&self.levels);
        let xruns_a = Arc::clone(&self.xrun_stats);
        let clock_a = Arc::clone(&self.live_params.live_input_clock);
        let snap_b = Arc::clone(&self.snapshot);
        let tport_b = Arc::clone(&self.transport);
        let ch_ctrl_b = Arc::clone(&self.active_output_channels);
        let levels_b = Arc::clone(&self.levels);
        let xruns_b = Arc::clone(&self.xrun_stats);
        let clock_b = Arc::clone(&self.live_params.live_input_clock);

        let fixed_cfg = cpal::StreamConfig {
            buffer_size: cpal::BufferSize::Fixed(settings.buffer_size),
            ..base_cfg.clone()
        };

        let stream = build_typed_stream(
            &device,
            &fixed_cfg,
            fmt,
            snap_a,
            tport_a,
            hw_channels,
            ch_ctrl_a,
            levels_a,
            xruns_a,
            clock_a,
        )
        .or_else(|e| {
            eprintln!(
                "[audio] Buffer fixe ({} frames) non supporté ({e}), \
                         passage au mode par défaut",
                settings.buffer_size
            );
            let default_cfg = cpal::StreamConfig {
                buffer_size: cpal::BufferSize::Default,
                ..base_cfg
            };
            build_typed_stream(
                &device,
                &default_cfg,
                fmt,
                snap_b,
                tport_b,
                hw_channels,
                ch_ctrl_b,
                levels_b,
                xruns_b,
                clock_b,
            )
        })?;

        stream
            .play()
            .map_err(|e| format!("Impossible de démarrer le flux audio : {e}"))?;

        self.reporter_stop = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&self.reporter_stop);
        let reporter_transport = Arc::clone(&self.transport);
        let reporter_levels = Arc::clone(&self.levels);
        let reporter_meter_cells = Arc::clone(&self.meter_cells);
        let reporter_xruns = Arc::clone(&self.xrun_stats);

        std::thread::Builder::new()
            .name("cpal-pos-reporter".into())
            .spawn(move || {
                position_reporter(
                    reporter_transport,
                    reporter_levels,
                    reporter_meter_cells,
                    reporter_xruns,
                    app,
                    stop,
                )
            })
            .map_err(|e| format!("Impossible de démarrer le reporter de position : {e}"))?;

        self._stream = Some(SendStream(stream));
        eprintln!("[audio] Moteur démarré.");
        Ok(())
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new(
            SharedTransport::new(48_000.0, 120.0),
            Arc::new(LiveParams::new()),
        )
    }
}

/// Return the output device matching `name`, or the system default.
///
/// `"default"` always resolves to `host.default_output_device()`.
/// For any other name, we iterate `host.output_devices()` and match on the
/// CPAL device name; on no match we fall back to the system default so the
/// engine never silently stops.
fn find_output_device(host: &cpal::Host, name: &str) -> Option<cpal::Device> {
    if name == "default" {
        return host.default_output_device();
    }

    if let Ok(mut devices) = host.output_devices() {
        if let Some(d) = devices.find(|d| d.name().map(|n| n == name).unwrap_or(false)) {
            return Some(d);
        }
    }

    eprintln!(
        "[audio] Périphérique '{}' introuvable, utilisation du périphérique par défaut",
        name
    );
    host.default_output_device()
}

/// Resolve the best `StreamConfig` + `SampleFormat` for `device` at `requested_sr`.
///
/// ## Format preference order
///
/// A modern DAW always processes in f32 internally. We therefore request the
/// device in F32 first so no sample-format conversion is needed on the wire.
/// I16 is the second choice (very common on consumer hardware); exotic formats
/// (U8, U16, I32 …) are a last resort.
///
/// ## Sample-rate fallback ladder
///
/// 1. `requested_sr` + F32 or I16  ← ideal
/// 2. `requested_sr` + any format   ← rare devices with no float support
/// 3. Device-default SR + F32 or I16 ← requested SR not available
/// 4. Whatever `default_output_config()` reports  ← absolute last resort
fn resolve_stream_config(
    device: &cpal::Device,
    requested_sr: u32,
) -> Result<(cpal::StreamConfig, cpal::SampleFormat), String> {
    let ranges: Vec<_> = device
        .supported_output_configs()
        .map(|it| it.collect::<Vec<_>>())
        .unwrap_or_default();

    let find = |sr: cpal::SampleRate, fmt: cpal::SampleFormat| {
        ranges
            .iter()
            .find(|r| {
                r.sample_format() == fmt && r.min_sample_rate() <= sr && r.max_sample_rate() >= sr
            })
            .map(|r| -> (cpal::StreamConfig, cpal::SampleFormat) {
                (r.with_sample_rate(sr).into(), fmt)
            })
    };

    let req = cpal::SampleRate(requested_sr);

    for fmt in [cpal::SampleFormat::F32, cpal::SampleFormat::I16] {
        if let Some(found) = find(req, fmt) {
            return Ok(found);
        }
    }

    if let Some(range) = ranges
        .iter()
        .find(|r| r.min_sample_rate() <= req && r.max_sample_rate() >= req)
    {
        let fmt = range.sample_format();
        eprintln!(
            "[audio] F32/I16 indisponibles à {} Hz — utilisation du format {:?}",
            requested_sr, fmt
        );
        return Ok((range.with_sample_rate(req).into(), fmt));
    }

    let default_cfg = device
        .default_output_config()
        .map_err(|e| format!("Impossible d'obtenir la configuration audio : {e}"))?;

    let fallback_sr = default_cfg.sample_rate();
    eprintln!(
        "[audio] {} Hz non supporté — passage à {} Hz",
        requested_sr, fallback_sr.0
    );

    for fmt in [cpal::SampleFormat::F32, cpal::SampleFormat::I16] {
        if let Some(found) = find(fallback_sr, fmt) {
            return Ok(found);
        }
    }

    let fmt = default_cfg.sample_format();
    Ok((default_cfg.into(), fmt))
}

/// Dispatch to the correct sample-type variant of `build_stream`.
/// Live MIDI plays one callback period after it arrived (ADR-022): sample 0 of
/// a slice whose `frames_left` run to the end of this callback maps back to
/// `callback_start - frames_left / sample_rate`.
fn live_event_window_start(callback_start: u64, frames_left: usize, sample_rate: f32) -> u64 {
    if sample_rate <= 0.0 {
        return 0;
    }
    let lead = (frames_left as f64 * 1e9 / sample_rate as f64) as u64;
    callback_start.saturating_sub(lead)
}

fn build_typed_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    fmt: cpal::SampleFormat,
    snapshot: Arc<RuntimeSnapshot>,
    transport: Arc<SharedTransport>,
    hw_channels: usize,
    ch_ctrl: Arc<AtomicUsize>,
    levels: Arc<ChannelLevels>,
    xrun_stats: Arc<XrunStats>,
    live_clock: Arc<LiveInputClock>,
) -> Result<cpal::Stream, String> {
    match fmt {
        cpal::SampleFormat::F32 => build_stream::<f32>(
            device,
            config,
            snapshot,
            transport,
            hw_channels,
            ch_ctrl,
            levels,
            xrun_stats,
            live_clock,
        ),
        cpal::SampleFormat::I16 => build_stream::<i16>(
            device,
            config,
            snapshot,
            transport,
            hw_channels,
            ch_ctrl,
            levels,
            xrun_stats,
            live_clock,
        ),
        cpal::SampleFormat::U16 => build_stream::<u16>(
            device,
            config,
            snapshot,
            transport,
            hw_channels,
            ch_ctrl,
            levels,
            xrun_stats,
            live_clock,
        ),
        other => {
            eprintln!("[audio] Format {other:?} non géré nativement, tentative F32");
            build_stream::<f32>(
                device,
                config,
                snapshot,
                transport,
                hw_channels,
                ch_ctrl,
                levels,
                xrun_stats,
                live_clock,
            )
        }
    }
}

/// Build a CPAL output stream for sample type `T`.
///
/// `hw_channels` — total channels the CPAL buffer contains (from the device config).
/// `ch_ctrl`     — atomic holding the number of active channels; updated without
///                 restarting the stream (live preview from the Settings panel).
///                 `0` = use all hardware channels.
///
/// The closure captures pre-allocated state so the callback never allocates.
fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    snapshot: Arc<RuntimeSnapshot>,
    transport: Arc<SharedTransport>,
    hw_channels: usize,
    ch_ctrl: Arc<AtomicUsize>,
    levels: Arc<ChannelLevels>,
    xrun_stats: Arc<XrunStats>,
    live_clock: Arc<LiveInputClock>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let mut mix_buf = vec![0.0_f32; MAX_BLOCK_FRAMES * 2];
    // Boxed so pointer swaps displace the old scheduler without drop on the audio thread.
    let mut scheduler = Box::new(GraphScheduler::empty(transport.sample_rate()));
    // ITU-R BS.775 / Pro Tools standard: −3 dB for stereo→mono downmix.
    let mono_gain: f32 = 1.0 / 2.0_f32.sqrt();
    let mut prev_callback: Option<cpal::StreamInstant> = None;
    let mut prev_frames: usize = 0;
    let mut first_callback: Option<cpal::StreamInstant> = None;
    // FTZ/DAZ must be set per-thread; earliest hook is the first callback.
    let mut denormal_guard_set = false;
    let xrun_stats_cb = Arc::clone(&xrun_stats);
    let xrun_stats_err = Arc::clone(&xrun_stats);

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let callback_start = live_clock.now_nanos();
                if !denormal_guard_set {
                    enable_flush_to_zero();
                    raise_audio_thread_priority();
                    denormal_guard_set = true;
                }

                let total_samples = data.len();
                let Some(frames) = total_samples.checked_div(hw_channels) else {
                    return;
                };

                let now = info.timestamp().callback;
                let warming_up = first_callback
                    .and_then(|start| now.duration_since(&start))
                    .map(|since_start| since_start < XRUN_WARMUP)
                    .unwrap_or(true);
                if first_callback.is_none() {
                    first_callback = Some(now);
                }
                if !warming_up {
                    if let Some(prev) = prev_callback {
                        if let Some(elapsed) = now.duration_since(&prev) {
                            let sample_rate = transport.sample_rate();
                            if sample_rate > 0.0 {
                                let expected = Duration::from_secs_f64(
                                    prev_frames as f64 / sample_rate as f64,
                                );
                                if is_callback_xrun(elapsed, expected) {
                                    xrun_stats_cb.record();
                                }
                            }
                        }
                    }
                }
                prev_callback = Some(now);
                prev_frames = frames;

                if frames > MAX_BLOCK_FRAMES {
                    // Block exceeds our pre-allocated buffer — fill with silence.
                    // Should never happen with typical WASAPI buffer sizes.
                    for s in data.iter_mut() {
                        *s = T::from_sample_(0.0_f32);
                    }
                    return;
                }

                snapshot.try_swap(&mut scheduler);

                // A loop point inside the block is rendered as two (or more)
                // slices so notes land on the wrapped position, not a block late.
                let mut rendered = 0usize;
                while rendered < frames {
                    let playing = transport.is_playing.load(Ordering::Relaxed);
                    let current = transport.position_samples.load(Ordering::Relaxed);
                    let (position, chunk) =
                        playback_chunk(current, frames - rendered, playing, transport.loop_range());
                    if playing && position != current {
                        transport.position_samples.store(position, Ordering::Relaxed);
                    }
                    let info = transport.snapshot(chunk);
                    let end = rendered + chunk;
                    live_clock.set_event_window_start(live_event_window_start(
                        callback_start,
                        frames - rendered,
                        info.sample_rate,
                    ));
                    scheduler.process_block(&info, &mut mix_buf[rendered * 2..end * 2]);
                    if playing {
                        transport
                            .position_samples
                            .store(position + chunk as u64, Ordering::Relaxed);
                    }
                    rendered = end;
                    if chunk == 0 {
                        break;
                    }
                }
                let mix = &mix_buf[..frames * 2];

                let raw_active = ch_ctrl.load(Ordering::Relaxed);
                let active = if raw_active == 0 {
                    hw_channels
                } else {
                    raw_active.min(hw_channels).max(1)
                };
                let mono = active == 1;

                let (mut peak_l, mut peak_r) = (0.0_f32, 0.0_f32);
                for f in 0..frames {
                    let s_l = mix[f * 2].abs();
                    let s_r = mix[f * 2 + 1].abs();
                    if s_l > peak_l {
                        peak_l = s_l;
                    }
                    if s_r > peak_r {
                        peak_r = s_r;
                    }
                }
                for ch in 0..active.min(MAX_METER_CHANNELS) {
                    levels.set_output(ch, if ch % 2 == 0 { peak_l } else { peak_r });
                }

                // Mono: ITU-R BS.775 downmix (L+R)×(1/√2) broadcast to all hw
                //       channels so both ears hear the mono mix.
                // Stereo/multi: L→even channels, R→odd channels.
                // Channels beyond `active` receive silence.
                for frame in 0..frames {
                    let mix_l = mix[frame * 2];
                    let mix_r = mix[frame * 2 + 1];
                    let mono_s = if mono {
                        (mix_l + mix_r) * mono_gain
                    } else {
                        0.0
                    };

                    for ch in 0..hw_channels {
                        let idx = frame * hw_channels + ch;
                        if idx < total_samples {
                            let s = if ch < active {
                                if mono {
                                    mono_s
                                } else if ch % 2 == 0 {
                                    mix_l
                                } else {
                                    mix_r
                                }
                            } else {
                                0.0
                            };
                            data[idx] = T::from_sample_(s);
                        }
                    }
                }
            },
            move |err| {
                xrun_stats_err.record();
                eprintln!("[audio] Erreur de flux : {err}");
            },
            None,
        )
        .map_err(|e| e.to_string())?;

    Ok(stream)
}

/// Reusable telemetry buffers for the ~30 Hz reporter (no per-tick `json!` / `Map::new`).
struct PositionReporterState {
    levels: LevelsUpdatedPayload,
    position: PositionUpdatedPayload,
}

impl PositionReporterState {
    fn new() -> Self {
        Self {
            levels: LevelsUpdatedPayload {
                output: ZERO_METER_LEVELS,
                input: ZERO_METER_LEVELS,
                tracks: HashMap::new(),
                xruns: 0,
            },
            position: PositionUpdatedPayload {
                position: 0.0,
                output: ZERO_METER_LEVELS,
                input: ZERO_METER_LEVELS,
                tracks: HashMap::new(),
                xruns: 0,
            },
        }
    }

    fn fill_meters(
        &mut self,
        output: [f32; MAX_METER_CHANNELS],
        input: [f32; MAX_METER_CHANNELS],
        xruns: u64,
        meter_cells: &Arc<Mutex<HashMap<String, MeterCell>>>,
    ) {
        self.levels.output = output;
        self.levels.input = input;
        self.levels.xruns = xruns;
        self.position.output = output;
        self.position.input = input;
        self.position.xruns = xruns;

        self.levels.tracks.clear();
        if let Ok(cells) = meter_cells.try_lock() {
            for (id, (l, r)) in cells.iter() {
                self.levels.tracks.insert(
                    id.clone(),
                    [
                        f32::from_bits(l.load(Ordering::Relaxed)),
                        f32::from_bits(r.load(Ordering::Relaxed)),
                    ],
                );
            }
        }
        self.position.tracks.clone_from(&self.levels.tracks);
    }
}

/// Background thread: emits engine events at ~30 fps.
///
/// Always emits `LEVELS_UPDATED` (for the Settings panel meters + track meters).
/// Only emits `POSITION_UPDATED` while playing.
/// Exits as soon as `stop` is set to `true` (signalled by `stop_stream`).
fn position_reporter(
    transport: Arc<SharedTransport>,
    levels: Arc<ChannelLevels>,
    meter_cells: Arc<Mutex<HashMap<String, MeterCell>>>,
    xrun_stats: Arc<XrunStats>,
    app: tauri::AppHandle,
    stop: Arc<AtomicBool>,
) {
    let interval = std::time::Duration::from_millis(REPORTER_INTERVAL_MS);
    let mut last_reported_xruns = 0_u64;
    let mut state = PositionReporterState::new();

    loop {
        std::thread::sleep(interval);

        if stop.load(Ordering::Relaxed) {
            break;
        }

        let out = levels.read_outputs();
        let inp = levels.read_inputs();
        let xruns = xrun_stats.total();
        log_new_xruns(&mut last_reported_xruns, xruns);
        state.fill_meters(out, inp, xruns, &meter_cells);

        if transport.is_playing.load(Ordering::Relaxed) {
            let sr = transport.sample_rate();
            let pos = transport.position_samples.load(Ordering::Relaxed);
            let dur = transport.duration_samples.load(Ordering::Relaxed);

            // Auto-stop when playhead reaches the end of the timeline.
            // An active loop wraps forever, including through silence past the clips.
            if transport.loop_range().is_none() && dur > 0 && pos >= dur {
                transport.stop();
                let _ = app.emit(
                    "engine-event",
                    &crate::engine_gateway::events::EngineEvent::with_payload(
                        ev::PLAY_STOPPED,
                        "",
                        serde_json::json!({}),
                    ),
                );
                state.position.position = 0.0;
                let _ = app.emit(
                    "engine-event",
                    &TelemetryEngineEvent {
                        id: "pos",
                        project_id: "",
                        event_type: ev::POSITION_UPDATED,
                        timestamp: 0,
                        payload: &state.position,
                    },
                );
                continue;
            }

            state.position.position = pos as f64 / sr as f64;
            let _ = app.emit(
                "engine-event",
                &TelemetryEngineEvent {
                    id: "pos",
                    project_id: "",
                    event_type: ev::POSITION_UPDATED,
                    timestamp: 0,
                    payload: &state.position,
                },
            );
        } else {
            let _ = app.emit(
                "engine-event",
                &TelemetryEngineEvent {
                    id: "levels",
                    project_id: "",
                    event_type: ev::LEVELS_UPDATED,
                    timestamp: 0,
                    payload: &state.levels,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_xrun_requires_materially_late_delivery() {
        let block = Duration::from_millis(10);
        assert!(!is_callback_xrun(block, block));
        assert!(!is_callback_xrun(Duration::from_millis(11), block));
        assert!(is_callback_xrun(Duration::from_millis(20), block));
    }
}
