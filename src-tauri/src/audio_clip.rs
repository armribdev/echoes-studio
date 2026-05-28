use serde::Serialize;
use std::error::Error;
use std::fs::File;
use std::path::Path;

use symphonia::core::{
    audio::SampleBuffer,
    codecs::DecoderOptions,
    errors::Error as SymphoniaError,
    formats::FormatOptions,
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
};

pub struct AudioClip {
    pub raw_samples: Vec<f32>,
    pub raw_sample_rate: u32,
}

#[derive(Serialize)]
pub struct AudioClipData {
    pub raw_samples: Vec<f32>,
    pub sample_rate: u32,
    pub peaks_64: Vec<[f32; 2]>,
    pub peaks_512: Vec<[f32; 2]>,
    pub peaks_4096: Vec<[f32; 2]>,
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

impl AudioClip {
    pub fn from_file(file_path: &str) -> Result<AudioClip, Box<dyn Error>> {
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
        let channels = track
            .codec_params
            .channels
            .map(|c| c.count())
            .unwrap_or(1);

        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())?;

        let mut raw_samples: Vec<f32> = Vec::new();

        loop {
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

            let mut buf =
                SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
            buf.copy_interleaved_ref(decoded);

            // Downmix to mono by averaging channels
            if channels <= 1 {
                raw_samples.extend_from_slice(buf.samples());
            } else {
                for frame in buf.samples().chunks(channels) {
                    raw_samples.push(frame.iter().sum::<f32>() / channels as f32);
                }
            }
        }

        Ok(AudioClip {
            raw_samples,
            raw_sample_rate,
        })
    }

    pub fn into_clip_data(self) -> AudioClipData {
        let peaks_64 = compute_peaks(&self.raw_samples, 64);
        let peaks_512 = compute_peaks(&self.raw_samples, 512);
        let peaks_4096 = compute_peaks(&self.raw_samples, 4096);
        AudioClipData {
            raw_samples: self.raw_samples,
            sample_rate: self.raw_sample_rate,
            peaks_64,
            peaks_512,
            peaks_4096,
        }
    }
}
