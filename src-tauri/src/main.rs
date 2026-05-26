use audio_clip::{AudioClip, AudioClipData};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat, SampleRate,
};
use std::error::Error;
mod audio_clip;

fn main() -> Result<(), Box<dyn Error>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .expect("failed to find output device");
    println!("Output device: {}", device.name()?);

    let mut configs = device
        .supported_output_configs()?
        .filter(|config| config.sample_format() == SampleFormat::F32 && config.channels() == 2);

    let config = configs.next().expect("no suitable configuration found");
    let sample_rate = if config.min_sample_rate() <= SampleRate(44100) && SampleRate(44100) <= config.max_sample_rate() {
        SampleRate(44100)
    } else {
        config.max_sample_rate()
    };
    let config = config.with_sample_rate(sample_rate);

    println!("{:?}", config);

    // Chargez votre AudioClip à partir d'un fichier WAV, ajustez le chemin en conséquence
    // let audio_clip =
    //     create_new_audio_clip("/home/armand/dev/echoes-studio/sample_44100.wav").unwrap();

    //run(&device, &config.into(), audio_clip)?;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![create_new_audio_clip, get_audio_clip_duration])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    Ok(())
}

#[tauri::command]
async fn get_audio_clip_duration(file_path: &str) -> Result<f64, String> {
    let file = std::fs::File::open(file_path).map_err(|e| e.to_string())?;
    let reader = hound::WavReader::new(file).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    Ok(reader.duration() as f64 / spec.sample_rate as f64)
}

#[tauri::command]
async fn create_new_audio_clip(file_path: &str) -> Result<AudioClipData, String> {
    AudioClip::from_wav_file(file_path)
        .map(|clip| clip.into_clip_data())
        .map_err(|e| e.to_string())
}

pub fn run(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    audio_clip: AudioClip,
) -> Result<(), Box<dyn Error>> {
    println!("{:?}", config);
    let channels = config.channels as usize;

    let mut sample_clock = 0usize;

    let err_fn = |err| eprintln!("an error occurred on stream: {}", err);

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            write_data(data, channels, &audio_clip, &mut sample_clock)
        },
        err_fn,
        None,
    )?;
    stream.play()?;

    std::thread::sleep(std::time::Duration::from_millis(20000));

    Ok(())
}

fn write_data(
    output: &mut [f32],
    channels: usize,
    audio_clip: &AudioClip,
    sample_clock: &mut usize,
) {
    for frame in output.chunks_mut(channels) {
        for sample in frame.iter_mut() {
            *sample = audio_clip.raw_samples[*sample_clock % audio_clip.raw_samples.len()];
            *sample_clock += 1;
        }
    }
}
