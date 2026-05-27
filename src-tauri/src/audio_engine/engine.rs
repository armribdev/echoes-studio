//! `AudioEngine` — owns the CPAL stream and the current `GraphScheduler`.
//!
//! ## Architecture
//!
//! ```text
//! Command thread                      Audio thread (CPAL callback)
//! ──────────────                      ────────────────────────────
//! AudioGraphCompiler::compile()
//! GraphScheduler::new(compiled)
//! RuntimeSnapshot::enqueue(sched)  ──► try_take() → swap in
//!
//! SharedTransport::play()          ──► transport.snapshot() → TransportInfo
//!                                       scheduler.process_block()
//!                                       write to CPAL output buffer
//!                                       position_samples.fetch_add(block)
//! ```
//!
//! ## Real-time safety contract
//!
//! The audio callback:
//!   ✓ Never allocates (mix_buf pre-allocated before stream start)
//!   ✓ Never acquires a blocking lock (`try_take` uses `try_lock`)
//!   ✓ Never performs I/O
//!   ✓ Never calls `async`/`await`
//!   ✓ Only reads from `GraphScheduler` (owned for the callback's duration)

use std::sync::{atomic::Ordering, Arc};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, SizedSample,
};
use serde_json::json;
use tauri::Emitter;

use crate::audio_graph::{CompileConfig, GraphScheduler, RuntimeSnapshot};
use crate::audio_engine::graph::AudioGraphCompiler;
use crate::audio_engine::transport::SharedTransport;
use crate::project::AppProject;

// ── SendStream wrapper ────────────────────────────────────────────────────────

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
struct SendStream(cpal::Stream);
// SAFETY: see doc comment above.
unsafe impl Send for SendStream {}

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of frames per audio block.
///
/// Pre-allocates the mix buffer.  Must be ≥ the device's actual block size.
/// 4096 frames @ 48 kHz = ~85 ms — well above any typical WASAPI buffer.
const MAX_BLOCK_FRAMES: usize = 4096;

/// Position-reporter poll interval in milliseconds.
const REPORTER_INTERVAL_MS: u64 = 33;

// ── AudioEngine ───────────────────────────────────────────────────────────────

/// The real-time audio engine.  Held behind `Mutex<AudioEngine>` in `AppState`.
pub struct AudioEngine {
    /// Pending scheduler waiting to be picked up atomically by the audio callback.
    snapshot: Arc<RuntimeSnapshot>,

    /// Shared atomic transport state (also held by the callback closure and the
    /// position reporter thread).
    pub transport: Arc<SharedTransport>,

    /// Keeps the CPAL stream alive for the lifetime of the engine.
    /// Dropping this would stop the audio.
    _stream: Option<SendStream>,
}

impl AudioEngine {
    pub fn new(transport: Arc<SharedTransport>) -> Self {
        AudioEngine {
            snapshot: Arc::new(RuntimeSnapshot::new()),
            transport,
            _stream: None,
        }
    }

    // ── Graph compilation ─────────────────────────────────────────────────────

    /// Compile a new `GraphScheduler` from `project` and enqueue it for atomic
    /// pickup by the audio callback.
    ///
    /// **Never call from the audio thread.**
    pub fn update_from_project(&mut self, project: &AppProject) {
        let config = CompileConfig {
            sample_rate: self.transport.sample_rate(),
            // Use a generous max to handle any CPAL block size
            max_block_size: MAX_BLOCK_FRAMES,
            num_channels: 2,
        };
        let scheduler = AudioGraphCompiler::compile_scheduler(project, config);
        self.snapshot.enqueue(scheduler);
    }

    // ── Stream lifecycle ──────────────────────────────────────────────────────

    /// Open the default audio output device, start the CPAL stream, and spawn
    /// the position-reporter background thread.
    ///
    /// Call once during application setup, **after** the Tauri `AppHandle` is
    /// available.  On failure the engine falls back to silence (no crash).
    pub fn start_stream(
        &mut self,
        project: &AppProject,
        app: tauri::AppHandle,
    ) -> Result<(), String> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or_else(|| "Aucun périphérique de sortie audio trouvé".to_string())?;

        let supported = device
            .default_output_config()
            .map_err(|e| format!("Impossible d'obtenir la configuration audio : {e}"))?;

        let sample_rate = supported.sample_rate().0 as f32;
        let channels = supported.channels() as usize;
        let fmt = supported.sample_format();

        eprintln!(
            "[audio] {} — {} Hz, {} ch, {:?}",
            device.name().unwrap_or_else(|_| "Périphérique inconnu".to_string()),
            sample_rate as u32,
            channels,
            fmt,
        );

        // Propagate the real sample rate so future graph compilations are correct
        self.transport.set_sample_rate(sample_rate);

        // Compile the initial graph at the correct sample rate
        let compile_cfg = CompileConfig {
            sample_rate,
            max_block_size: MAX_BLOCK_FRAMES,
            num_channels: 2,
        };
        let initial_sched = AudioGraphCompiler::compile_scheduler(project, compile_cfg);
        self.snapshot.enqueue(initial_sched);

        let stream_config: cpal::StreamConfig = supported.into();

        // Build a type-dispatched output stream
        let snapshot = Arc::clone(&self.snapshot);
        let transport = Arc::clone(&self.transport);

        let stream = match fmt {
            cpal::SampleFormat::F32 => {
                build_stream::<f32>(&device, &stream_config, snapshot, transport, channels)
            }
            cpal::SampleFormat::I16 => {
                build_stream::<i16>(&device, &stream_config, snapshot, transport, channels)
            }
            cpal::SampleFormat::U16 => {
                build_stream::<u16>(&device, &stream_config, snapshot, transport, channels)
            }
            other => {
                // Best-effort: try f32 — most WASAPI shared-mode devices accept it
                eprintln!("[audio] Format {other:?} non géré nativement, tentative F32");
                build_stream::<f32>(&device, &stream_config, snapshot, transport, channels)
            }
        }?;

        stream
            .play()
            .map_err(|e| format!("Impossible de démarrer le flux audio : {e}"))?;

        // Spawn lightweight position reporter (~30 fps)
        let reporter_transport = Arc::clone(&self.transport);
        std::thread::Builder::new()
            .name("cpal-pos-reporter".into())
            .spawn(move || position_reporter(reporter_transport, app))
            .map_err(|e| format!("Impossible de démarrer le reporter de position : {e}"))?;

        self._stream = Some(SendStream(stream));
        eprintln!("[audio] Moteur démarré.");
        Ok(())
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new(SharedTransport::new(48_000.0, 120.0))
    }
}

// ── Stream builder ────────────────────────────────────────────────────────────

/// Build a CPAL output stream for sample type `T`.
///
/// The closure captures pre-allocated state so the callback never allocates.
fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    snapshot: Arc<RuntimeSnapshot>,
    transport: Arc<SharedTransport>,
    device_channels: usize,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    // Pre-allocate the stereo mix buffer once — reused for every block.
    let mut mix_buf = vec![0.0_f32; MAX_BLOCK_FRAMES * 2];
    let mut scheduler = GraphScheduler::empty(transport.sample_rate());

    let stream = device
        .build_output_stream(
            config,
            // ── Audio callback (real-time thread) ─────────────────────────────
            move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
                let total_samples = data.len();
                let frames = if device_channels > 0 {
                    total_samples / device_channels
                } else {
                    return;
                };

                if frames > MAX_BLOCK_FRAMES {
                    // Block exceeds our pre-allocated buffer — fill with silence.
                    // Should never happen with typical WASAPI buffer sizes.
                    for s in data.iter_mut() {
                        *s = T::from_sample_(0.0_f32);
                    }
                    return;
                }

                // Atomically swap in a new scheduler if one is pending.
                if let Some(new_sched) = snapshot.try_take() {
                    scheduler = new_sched;
                }

                let info = transport.snapshot(frames);

                // Render the graph into the stereo mix buffer [L0, R0, L1, R1, …]
                let mix = &mut mix_buf[..frames * 2];
                scheduler.process_block(&info, mix);

                // Advance the transport cursor after rendering.
                if info.is_playing {
                    transport
                        .position_samples
                        .fetch_add(frames as u64, Ordering::Relaxed);
                }

                // Write the stereo mix to the device buffer, spreading L/R to
                // any number of device channels (mono, stereo, surround…).
                for frame in 0..frames {
                    let mix_l = mix[frame * 2];
                    let mix_r = mix[frame * 2 + 1];
                    for ch in 0..device_channels {
                        let idx = frame * device_channels + ch;
                        if idx < total_samples {
                            let s = if ch % 2 == 0 { mix_l } else { mix_r };
                            data[idx] = T::from_sample_(s);
                        }
                    }
                }
            },
            // ── Error callback ─────────────────────────────────────────────────
            move |err| eprintln!("[audio] Erreur de flux : {err}"),
            None,
        )
        .map_err(|e| e.to_string())?;

    Ok(stream)
}

// ── Position reporter ─────────────────────────────────────────────────────────

/// Background thread: emits `POSITION_UPDATED` engine events at ~30 fps.
///
/// Only emits while playing to avoid flooding the frontend with events.
fn position_reporter(transport: Arc<SharedTransport>, app: tauri::AppHandle) {
    let interval = std::time::Duration::from_millis(REPORTER_INTERVAL_MS);
    loop {
        std::thread::sleep(interval);
        if transport.is_playing.load(Ordering::Relaxed) {
            let sr = transport.sample_rate();
            let pos = transport.position_samples.load(Ordering::Relaxed);
            let pos_secs = pos as f64 / sr as f64;

            // Construct an EngineEvent-compatible payload so the frontend
            // transport bridge handles it through the existing code path.
            let _ = app.emit(
                "engine-event",
                json!({
                    "id": "pos",
                    "projectId": "",
                    "type": "POSITION_UPDATED",
                    "timestamp": 0,
                    "payload": { "position": pos_secs }
                }),
            );
        }
    }
}
