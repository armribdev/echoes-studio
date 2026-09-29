use serde::Serialize;
use std::error::Error;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, errors::Error as SymphoniaError,
    formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
};

pub struct AudioClip {
    pub raw_samples: Vec<f32>,
    pub raw_sample_rate: u32,
}

#[derive(Serialize)]
pub struct AudioClipData {
    /// Shared via `Arc` so the project, audio cache, and audio thread can all
    /// reference the same heap allocation without cloning.  Critical: this
    /// allocation can be 100+ MB for long stereo files — recompiling the audio
    /// graph must never clone it.
    #[serde(serialize_with = "serialize_arc_vec_f32")]
    pub raw_samples: Arc<Vec<f32>>,
    pub sample_rate: u32,
    pub peaks_64: Vec<[f32; 2]>,
    pub peaks_512: Vec<[f32; 2]>,
    pub peaks_4096: Vec<[f32; 2]>,
}

/// UI progress callback payload during decode / peak build.
#[derive(Clone)]
pub struct DecodeProgressUpdate {
    pub progress: f32,
    pub sample_rate: u32,
    pub decoded_duration_sec: f64,
    /// Full lane replace (finalize / cache hit). Prefer append fields while decoding.
    pub peaks_64: Option<Vec<[f32; 2]>>,
    pub peaks_512: Option<Vec<[f32; 2]>>,
    pub peaks_4096: Option<Vec<[f32; 2]>>,
    /// Truncate lane to `from` then append — O(batch) IPC instead of O(file).
    pub peaks_512_from: Option<usize>,
    pub peaks_512_append: Option<Vec<[f32; 2]>>,
    pub peaks_4096_from: Option<usize>,
    pub peaks_4096_append: Option<Vec<[f32; 2]>>,
}

const PEAK_CHUNK_64: usize = 64;
const PEAK_CHUNK_512: usize = 512;
const PEAK_CHUNK_4096: usize = 4096;

/// Decode occupies most of the import; peaks are the last ~8%.
const DECODE_PROGRESS_SPAN: f32 = 0.92;
/// Progressive peak snapshots (delta lanes only — keep cadence steady).
const PEAKS_EMIT_STEP: f32 = 0.03;
/// First PCM publish once this much audio is decoded (ADR-024).
const PCM_FIRST_SEC: f64 = 0.35;
/// Progress gates for later immutable prefix publishes (within decode span).
const PCM_PROGRESS_GATES: &[f32] = &[0.15, 0.35, 0.55, 0.75, 0.90];

pub(crate) fn serialize_arc_vec_f32<S>(v: &Arc<Vec<f32>>, ser: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serde::Serialize::serialize(&**v, ser)
}

fn compute_peaks(samples: &[f32], chunk_size: usize) -> Vec<[f32; 2]> {
    samples
        .chunks(chunk_size)
        .map(|chunk| {
            let mut min = 0.0f32;
            let mut max = 0.0f32;
            for &s in chunk {
                if s < min {
                    min = s;
                }
                if s > max {
                    max = s;
                }
            }
            [min, max]
        })
        .collect()
}

/// Grow a peak lane from newly decoded samples. Returns `(from, append)` for delta IPC.
fn sync_peaks_lane(
    peaks: &mut Vec<[f32; 2]>,
    samples: &[f32],
    chunk_size: usize,
) -> (usize, Vec<[f32; 2]>) {
    let full_chunks = samples.len() / chunk_size;
    // Rewind past any previous partial chunk so it can be recomputed.
    let from = full_chunks.min(peaks.len());
    peaks.truncate(from);
    let start = from * chunk_size;
    if start < samples.len() {
        peaks.extend(compute_peaks(&samples[start..], chunk_size));
    }
    (from, peaks[from..].to_vec())
}

impl AudioClip {
    pub fn from_file(file_path: &str) -> Result<AudioClip, Box<dyn Error>> {
        Self::from_file_with_progress(file_path, |_| {}, |_, _, _, _| {}, || false)
    }

    /// Decode with optional UI progress (`0.0..=DECODE_PROGRESS_SPAN` during decode).
    /// `should_cancel` is polled between packets; returning true aborts with an error.
    /// `on_pcm_prefix` receives immutable decoded prefixes for early playback (ADR-024).
    pub fn from_file_with_progress(
        file_path: &str,
        mut on_progress: impl FnMut(DecodeProgressUpdate),
        mut on_pcm_prefix: impl FnMut(&[f32], u32, f64, f32),
        mut should_cancel: impl FnMut() -> bool,
    ) -> Result<AudioClip, Box<dyn Error>> {
        let src = File::open(file_path)?;
        let mss = MediaSourceStream::new(Box::new(src), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = Path::new(file_path).extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe().format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )?;

        let mut format = probed.format;
        let track = format
            .default_track()
            .ok_or("No audio track found in file")?;
        let track_id = track.id;
        let raw_sample_rate = track
            .codec_params
            .sample_rate
            .ok_or("Unknown sample rate")?;
        let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);
        let expected_frames = track.codec_params.n_frames;

        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())?;

        let mut raw_samples: Vec<f32> = Vec::new();
        if let Some(n) = expected_frames {
            raw_samples.reserve(n as usize);
        }

        let mut decoded_frames: u64 = 0;
        let mut packet_count: u64 = 0;
        let mut last_emitted = -1.0f32;
        let mut last_peaks_at = -1.0f32;
        let mut next_pcm_gate: usize = 0;
        let mut pcm_first_published = false;
        // Progressive preview uses mid/coarse lanes only (delta IPC). Dense 64 is built at finalize.
        let mut peaks_512: Vec<[f32; 2]> = Vec::new();
        let mut peaks_4096: Vec<[f32; 2]> = Vec::new();

        let duration_sec = |frames: u64| -> f64 {
            if raw_sample_rate == 0 {
                0.0
            } else {
                frames as f64 / raw_sample_rate as f64
            }
        };

        let mut emit = |p: f32, frames: u64, samples: &[f32], include_peaks: bool| {
            let clamped = p.clamp(0.0, DECODE_PROGRESS_SPAN);
            let should_peaks = include_peaks
                && (last_peaks_at < 0.0
                    || clamped - last_peaks_at >= PEAKS_EMIT_STEP
                    || clamped >= DECODE_PROGRESS_SPAN);

            let dur = duration_sec(frames);
            let mut publish_pcm = false;
            if include_peaks && !samples.is_empty() {
                if !pcm_first_published
                    && (dur >= PCM_FIRST_SEC || clamped >= PCM_PROGRESS_GATES[0])
                {
                    pcm_first_published = true;
                    publish_pcm = true;
                }
                while next_pcm_gate < PCM_PROGRESS_GATES.len()
                    && clamped >= PCM_PROGRESS_GATES[next_pcm_gate]
                {
                    publish_pcm = true;
                    next_pcm_gate += 1;
                }
            }

            if (clamped - last_emitted).abs() < 0.01
                && clamped < DECODE_PROGRESS_SPAN
                && !should_peaks
                && !publish_pcm
            {
                return;
            }
            last_emitted = clamped;

            let mut peaks_512_from = None;
            let mut peaks_512_append = None;
            let mut peaks_4096_from = None;
            let mut peaks_4096_append = None;
            if should_peaks && !samples.is_empty() {
                last_peaks_at = clamped;
                let (from512, app512) = sync_peaks_lane(&mut peaks_512, samples, PEAK_CHUNK_512);
                let (from4096, app4096) =
                    sync_peaks_lane(&mut peaks_4096, samples, PEAK_CHUNK_4096);
                peaks_512_from = Some(from512);
                peaks_512_append = Some(app512);
                peaks_4096_from = Some(from4096);
                peaks_4096_append = Some(app4096);
            }

            on_progress(DecodeProgressUpdate {
                progress: clamped,
                sample_rate: raw_sample_rate,
                decoded_duration_sec: dur,
                peaks_64: None,
                peaks_512: None,
                peaks_4096: None,
                peaks_512_from,
                peaks_512_append,
                peaks_4096_from,
                peaks_4096_append,
            });
            if publish_pcm {
                on_pcm_prefix(samples, raw_sample_rate, dur, clamped);
            }
        };

        emit(0.0, 0, &[], false);

        loop {
            if should_cancel() {
                return Err("Import cancelled".into());
            }

            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::IoError(_)) => break,
                Err(SymphoniaError::ResetRequired) => {
                    decoder.reset();
                    continue;
                }
                Err(e) => return Err(Box::new(e)),
            };

            if packet.track_id() != track_id {
                continue;
            }

            let decoded = match decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(e) => return Err(Box::new(e)),
            };

            let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
            buf.copy_interleaved_ref(decoded);

            let frames_in_packet = if channels <= 1 {
                let samples = buf.samples();
                raw_samples.extend_from_slice(samples);
                samples.len() as u64
            } else {
                let mut n = 0u64;
                for frame in buf.samples().chunks(channels) {
                    raw_samples.push(frame.iter().sum::<f32>() / channels as f32);
                    n += 1;
                }
                n
            };

            decoded_frames += frames_in_packet;
            packet_count += 1;

            let progress = if let Some(total) = expected_frames.filter(|t| *t > 0) {
                DECODE_PROGRESS_SPAN * (decoded_frames as f32 / total as f32).min(1.0)
            } else {
                DECODE_PROGRESS_SPAN * (1.0 - 1.0 / (1.0 + packet_count as f32 / 400.0))
            };
            emit(progress, decoded_frames, &raw_samples, true);
        }

        emit(DECODE_PROGRESS_SPAN, decoded_frames, &raw_samples, true);

        Ok(AudioClip {
            raw_samples,
            raw_sample_rate,
        })
    }

    pub fn into_clip_data(self) -> AudioClipData {
        self.into_clip_data_with_progress(|_| {})
    }

    pub fn into_clip_data_with_progress(
        self,
        mut on_progress: impl FnMut(DecodeProgressUpdate),
    ) -> AudioClipData {
        let sample_rate = self.raw_sample_rate;
        let duration_sec = if sample_rate == 0 {
            0.0
        } else {
            self.raw_samples.len() as f64 / sample_rate as f64
        };
        let mut tick = |progress: f32,
                        peaks_64: Option<Vec<[f32; 2]>>,
                        peaks_512: Option<Vec<[f32; 2]>>,
                        peaks_4096: Option<Vec<[f32; 2]>>| {
            on_progress(DecodeProgressUpdate {
                progress,
                sample_rate,
                decoded_duration_sec: duration_sec,
                peaks_64,
                peaks_512,
                peaks_4096,
                peaks_512_from: None,
                peaks_512_append: None,
                peaks_4096_from: None,
                peaks_4096_append: None,
            });
        };

        tick(0.94, None, None, None);
        let peaks_64 = compute_peaks(&self.raw_samples, PEAK_CHUNK_64);
        tick(0.96, Some(peaks_64.clone()), None, None);
        let peaks_512 = compute_peaks(&self.raw_samples, PEAK_CHUNK_512);
        tick(0.98, None, Some(peaks_512.clone()), None);
        let peaks_4096 = compute_peaks(&self.raw_samples, PEAK_CHUNK_4096);
        tick(
            1.0,
            Some(peaks_64.clone()),
            Some(peaks_512.clone()),
            Some(peaks_4096.clone()),
        );
        AudioClipData {
            raw_samples: Arc::new(self.raw_samples),
            sample_rate,
            peaks_64,
            peaks_512,
            peaks_4096,
        }
    }
}
