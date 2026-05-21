use hound::WavReader;
use std::error::Error;
use std::fs::File;

pub struct AudioClip {
    pub raw_samples: Vec<f32>,
    pub raw_sample_rate: u32,
}

impl AudioClip {
    pub fn from_wav_file(file_path: &str) -> Result<AudioClip, Box<dyn Error>> {
        let file = File::open(file_path)?;
        let mut reader = WavReader::new(file)?;

        let spec = reader.spec();
        let raw_samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
            hound::SampleFormat::Int => {
                let max = (1i32 << (spec.bits_per_sample - 1)) as f32;
                reader.samples::<i32>().map(|s| s.unwrap() as f32 / max).collect()
            }
        };
        let raw_sample_rate = reader.spec().sample_rate;

        Ok(AudioClip {
            raw_samples,
            raw_sample_rate,
        })
    }
}
