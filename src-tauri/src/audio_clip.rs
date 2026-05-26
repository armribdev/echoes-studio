use hound::WavReader;
use serde::Serialize;
use std::error::Error;
use std::fs::File;

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
    pub fn from_wav_file(file_path: &str) -> Result<AudioClip, Box<dyn Error>> {
        let file = File::open(file_path)?;
        let mut reader = WavReader::new(file)?;

        let spec = reader.spec();
        let channels = spec.channels as usize;

        let all_samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
            hound::SampleFormat::Int => {
                let max = (1i32 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.unwrap() as f32 / max)
                    .collect()
            }
        };

        // Downmix to mono by averaging channels (avoids duration doubling on stereo files)
        let raw_samples: Vec<f32> = if channels <= 1 {
            all_samples
        } else {
            all_samples
                .chunks(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                .collect()
        };

        let raw_sample_rate = spec.sample_rate;

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
