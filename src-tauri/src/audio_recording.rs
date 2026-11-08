//! Recording of CPAL input into an audio clip (ADR-027).
//!
//! A drain thread pulls stereo floats from [`AudioInputHub`]'s record ring and
//! appends them while a take is active (after count-in). Finish writes a WAV
//! under the app-data `recordings/` folder and emits the path for `ADD_CLIP`.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::audio_engine::transport::SharedTransport;
use crate::audio_graph::midi::LiveInputClock;
use crate::audio_input::AudioInputHub;
use crate::project::generate_id;

/// Emitted when a recording ends. Payload: `Option<RecordedAudioClip>`.
pub const AUDIO_RECORDING_FINISHED_EVENT: &str = "audio-recording-finished";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedAudioClip {
    pub track_id: String,
    /// Seconds on the timeline (latency-compensated start of the first sample).
    pub start: f64,
    pub end: f64,
    pub file_path: String,
}

const PEAK_CHUNK_FRAMES: usize = 512;
const RECORDING_CHANNELS: u16 = 2;

/// Live waveform of the running take, polled by the arrange view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingPeaks {
    pub sample_rate: f32,
    pub peaks: Vec<[f32; 2]>,
}

#[derive(Default)]
struct PeakAccumulator {
    chunks: Vec<[f32; 2]>,
    min: f32,
    max: f32,
    frames_in_chunk: usize,
}

impl PeakAccumulator {
    fn push_interleaved(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(2) {
            let mono = (frame[0] + frame[1]) * 0.5;
            self.min = self.min.min(mono);
            self.max = self.max.max(mono);
            self.frames_in_chunk += 1;
            if self.frames_in_chunk == PEAK_CHUNK_FRAMES {
                self.chunks.push([self.min, self.max]);
                self.min = 0.0;
                self.max = 0.0;
                self.frames_in_chunk = 0;
            }
        }
    }
}

struct Recording {
    track_id: String,
    /// Wall time of the first kept sample (for LiveInputClock placement).
    first_received_at: Option<u64>,
    samples: Vec<f32>,
    peaks: PeakAccumulator,
}

pub struct AudioRecorder {
    recording: Mutex<Option<Recording>>,
    transport: Arc<SharedTransport>,
    clock: Arc<LiveInputClock>,
    hub: Arc<AudioInputHub>,
    drain_stop: Arc<std::sync::atomic::AtomicBool>,
    _drain: Mutex<Option<JoinHandle<()>>>,
}

impl AudioRecorder {
    pub fn new(
        transport: Arc<SharedTransport>,
        clock: Arc<LiveInputClock>,
        hub: Arc<AudioInputHub>,
    ) -> Arc<Self> {
        let recorder = Arc::new(Self {
            recording: Mutex::new(None),
            transport,
            clock,
            hub: Arc::clone(&hub),
            drain_stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            _drain: Mutex::new(None),
        });
        let drain_recorder = Arc::clone(&recorder);
        let handle = thread::Builder::new()
            .name("audio-record-drain".into())
            .spawn(move || drain_recorder.drain_loop())
            .expect("audio-record-drain thread");
        if let Ok(mut slot) = recorder._drain.lock() {
            *slot = Some(handle);
        }
        recorder
    }

    /// Returns the timeline position (seconds) where the recording starts.
    pub fn start(&self, track_id: String) -> Result<f64, String> {
        if !self.hub.available.load(Ordering::Relaxed) {
            return Err("Aucune entrée audio disponible".into());
        }
        let mut recording = self.lock();
        if recording.is_some() {
            return Err("Un enregistrement audio est déjà en cours".into());
        }
        self.hub.record_ring.clear();
        let start_position_samples = self.transport.position_samples.load(Ordering::Relaxed);
        self.transport.set_recording(true);
        *recording = Some(Recording {
            track_id,
            first_received_at: None,
            samples: Vec::with_capacity(48_000 * 2 * 8),
            peaks: PeakAccumulator::default(),
        });
        Ok(start_position_samples as f64 / self.transport.sample_rate() as f64)
    }

    pub fn is_recording(&self) -> bool {
        self.lock().is_some()
    }

    /// Peak chunks added since `from`; `None` when no take is running.
    pub fn peaks_since(&self, from: usize) -> Option<RecordingPeaks> {
        let recording = self.lock();
        let chunks = &recording.as_ref()?.peaks.chunks;
        Some(RecordingPeaks {
            sample_rate: self.capture_sample_rate(),
            peaks: chunks.get(from..).unwrap_or_default().to_vec(),
        })
    }

    fn capture_sample_rate(&self) -> f32 {
        let input_sample_rate = self.hub.sample_rate();
        if input_sample_rate > 0.0 {
            input_sample_rate
        } else {
            self.transport.sample_rate()
        }
    }

    /// Ends the recording and writes a WAV. `Some(None)` when no audio was kept.
    pub fn finish(&self, app: &AppHandle) -> Option<Option<RecordedAudioClip>> {
        let recording = self.lock().take()?;
        self.transport.set_recording(false);
        self.transport.cancel_count_in();

        if recording.samples.is_empty() {
            return Some(None);
        }

        let sample_rate = self.capture_sample_rate();
        let frames = recording.samples.len() / 2;
        let duration_sec = frames as f64 / sample_rate as f64;

        let start_samples = recording
            .first_received_at
            .and_then(|t| self.clock.transport_position_at(t, sample_rate))
            .unwrap_or_else(|| self.transport.position_samples.load(Ordering::Relaxed));
        let start = start_samples as f64 / sample_rate as f64;
        let end = start + duration_sec;

        let path = match write_recording_wav(app, &recording.samples, sample_rate as u32) {
            Ok(path) => path,
            Err(e) => {
                eprintln!("[audio] Échec écriture enregistrement : {e}");
                return Some(None);
            }
        };

        Some(Some(RecordedAudioClip {
            track_id: recording.track_id,
            start,
            end,
            file_path: path.to_string_lossy().into_owned(),
        }))
    }

    fn drain_loop(&self) {
        let mut scratch = vec![0.0_f32; 4096];
        while !self.drain_stop.load(Ordering::Relaxed) {
            let n = self.hub.record_ring.pop_slice(&mut scratch);
            if n == 0 {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            if self.transport.count_in_remaining() > 0 {
                continue;
            }
            let mut recording = self.lock();
            let Some(recording) = recording.as_mut() else {
                continue;
            };
            if recording.first_received_at.is_none() {
                recording.first_received_at = Some(self.clock.now_nanos());
            }
            recording.samples.extend_from_slice(&scratch[..n]);
            recording.peaks.push_interleaved(&scratch[..n]);
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Recording>> {
        self.recording
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        self.drain_stop.store(true, Ordering::Relaxed);
    }
}

/// Ends the recording and tells the frontend.
pub fn finish_and_emit(app: &AppHandle, recorder: &AudioRecorder) {
    if let Some(clip) = recorder.finish(app) {
        let _ = app.emit(AUDIO_RECORDING_FINISHED_EVENT, clip);
    }
}

fn write_recording_wav(
    app: &AppHandle,
    samples: &[f32],
    sample_rate: u32,
) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("recordings");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("rec-{}.wav", generate_id()));
    write_wav(&path, samples, RECORDING_CHANNELS, sample_rate)?;
    Ok(path)
}

pub(crate) fn write_wav(
    path: &Path,
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for &sample in samples {
        writer.write_sample(sample).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_input::AudioInputHub;

    #[test]
    fn recording_keeps_the_transport_from_auto_stopping() {
        let transport = SharedTransport::new(48_000.0, 120.0);
        let hub = AudioInputHub::new();
        hub.available.store(true, Ordering::Relaxed);
        hub.sample_rate_bits
            .store(48_000.0_f32.to_bits(), Ordering::Relaxed);
        let recorder =
            AudioRecorder::new(Arc::clone(&transport), Arc::new(LiveInputClock::new()), hub);
        recorder.start("track".into()).unwrap();
        assert!(transport.is_recording());
        // finish needs AppHandle for WAV — just clear via lock for this unit test
        recorder.lock().take();
        transport.set_recording(false);
        assert!(!transport.is_recording());
    }

    #[test]
    fn peaks_cover_each_chunk_of_frames() {
        let mut accumulator = PeakAccumulator::default();
        let mut samples = vec![0.5_f32; PEAK_CHUNK_FRAMES * 2];
        samples[0] = -1.0;
        samples[1] = -1.0;
        samples.extend_from_slice(&[0.25; 20]);
        accumulator.push_interleaved(&samples);
        assert_eq!(accumulator.chunks, vec![[-1.0, 0.5]]);
        assert_eq!(accumulator.frames_in_chunk, 10);
    }

    #[test]
    fn start_fails_without_input() {
        let transport = SharedTransport::new(48_000.0, 120.0);
        let hub = AudioInputHub::new();
        let recorder =
            AudioRecorder::new(Arc::clone(&transport), Arc::new(LiveInputClock::new()), hub);
        assert!(recorder.start("track".into()).is_err());
    }

    #[test]
    fn samples_during_count_in_are_discarded() {
        let transport = SharedTransport::new(48_000.0, 120.0);
        let hub = AudioInputHub::new();
        hub.available.store(true, Ordering::Relaxed);
        let recorder = AudioRecorder::new(
            Arc::clone(&transport),
            Arc::new(LiveInputClock::new()),
            Arc::clone(&hub),
        );
        recorder.start("track".into()).unwrap();
        transport.start_count_in(1_000);
        let _ = hub.record_ring.push_slice(&[0.5; 8]);
        // Give the drain thread a moment.
        thread::sleep(Duration::from_millis(20));
        assert!(recorder.lock().as_ref().unwrap().samples.is_empty());
        transport.cancel_count_in();
        let _ = hub.record_ring.push_slice(&[0.25; 8]);
        thread::sleep(Duration::from_millis(20));
        assert!(!recorder.lock().as_ref().unwrap().samples.is_empty());
        recorder.lock().take();
        transport.set_recording(false);
    }
}
