//! Factory drum one-shots, synthesised in code for the library `drums` source.
//!
//! Output is deterministic: noise comes from a fixed-seed generator, never from
//! the clock or the system RNG. Nothing here runs on the audio thread.

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::audio_recording::write_wav;
use crate::sampler_kit::SamplerKit;

pub const FACTORY_SAMPLE_RATE: u32 = 48_000;

const SAMPLES_FOLDER: &str = "factory_samples";
/// Bumped when a sound changes: saved projects keep pointing at the old files.
const SET_VERSION: &str = "v1";

/// −2 dBFS.
const PEAK_LEVEL: f32 = 0.794;
const FADE_IN_SECONDS: f32 = 0.0005;
const FADE_OUT_SECONDS: f32 = 0.005;

/// Closed, pedal and open hats cut each other.
const HI_HAT_NOTES: [u8; 3] = [42, 44, 46];
const HI_HAT_CHOKE_GROUP: u8 = 1;

/// Square-wave partials of the classic analogue hi-hat (Hz).
const METALLIC_PARTIALS_HZ: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

pub struct FactoryDrum {
    /// General MIDI note, also the pad the sound is meant for.
    pub note: u8,
    pub slug: &'static str,
    pub name: &'static str,
    generate: fn() -> Vec<f32>,
}

impl FactoryDrum {
    /// Mono samples at [`FACTORY_SAMPLE_RATE`], starting and ending on zero.
    pub fn render(&self) -> Vec<f32> {
        finish((self.generate)())
    }
}

pub static FACTORY_DRUMS: [FactoryDrum; 16] = [
    FactoryDrum {
        note: 36,
        slug: "kick",
        name: "Grosse caisse",
        generate: kick,
    },
    FactoryDrum {
        note: 37,
        slug: "rim",
        name: "Rim",
        generate: rim,
    },
    FactoryDrum {
        note: 38,
        slug: "snare",
        name: "Caisse claire",
        generate: snare,
    },
    FactoryDrum {
        note: 39,
        slug: "clap",
        name: "Clap",
        generate: clap,
    },
    FactoryDrum {
        note: 40,
        slug: "snare-dry",
        name: "Caisse claire sèche",
        generate: snare_dry,
    },
    FactoryDrum {
        note: 41,
        slug: "tom-low",
        name: "Tom bas",
        generate: || tom(80.0),
    },
    FactoryDrum {
        note: 42,
        slug: "hat-closed",
        name: "Charleston fermé",
        generate: hat_closed,
    },
    FactoryDrum {
        note: 43,
        slug: "tom-low-2",
        name: "Tom grave 2",
        generate: || tom(100.0),
    },
    FactoryDrum {
        note: 44,
        slug: "hat-pedal",
        name: "Charleston pédale",
        generate: hat_pedal,
    },
    FactoryDrum {
        note: 45,
        slug: "tom-mid-low",
        name: "Tom médium bas",
        generate: || tom(125.0),
    },
    FactoryDrum {
        note: 46,
        slug: "hat-open",
        name: "Charleston ouvert",
        generate: hat_open,
    },
    FactoryDrum {
        note: 47,
        slug: "tom-mid",
        name: "Tom médium",
        generate: || tom(155.0),
    },
    FactoryDrum {
        note: 48,
        slug: "tom-mid-high",
        name: "Tom médium haut",
        generate: || tom(185.0),
    },
    FactoryDrum {
        note: 49,
        slug: "crash",
        name: "Crash",
        generate: crash,
    },
    FactoryDrum {
        note: 50,
        slug: "tom-high",
        name: "Tom haut",
        generate: || tom(220.0),
    },
    FactoryDrum {
        note: 51,
        slug: "ride",
        name: "Ride",
        generate: ride,
    },
];

/// A factory sound on disk, as the library lists it.
#[derive(Debug, Clone, Serialize)]
pub struct FactoryDrumFile {
    pub id: String,
    pub name: String,
    pub note: u8,
    pub path: String,
    pub duration_sec: f32,
}

pub fn samples_directory(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(SAMPLES_FOLDER).join(SET_VERSION)
}

/// Writes the sounds missing from `directory`, then lists all of them.
pub fn ensure_files(directory: &Path) -> Result<Vec<FactoryDrumFile>, String> {
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    FACTORY_DRUMS
        .iter()
        .map(|drum| ensure_file(drum, directory))
        .collect()
}

fn ensure_file(drum: &FactoryDrum, directory: &Path) -> Result<FactoryDrumFile, String> {
    let samples = drum.render();
    let path = directory.join(format!("{}.wav", drum.slug));
    if !path.is_file() {
        write_atomically(&path, &samples)?;
    }
    Ok(FactoryDrumFile {
        id: format!("factory://drums/{}", drum.slug),
        name: drum.name.to_string(),
        note: drum.note,
        path: path.to_string_lossy().into_owned(),
        duration_sec: samples.len() as f32 / FACTORY_SAMPLE_RATE as f32,
    })
}

/// The kit a new sampler track starts with: one factory sound per pad.
pub fn starting_kit(directory: &Path) -> Result<SamplerKit, String> {
    let files = ensure_files(directory)?;
    let mut kit = SamplerKit::default();
    for pad in &mut kit.pads {
        pad.sample_path = files
            .iter()
            .find(|file| file.note == pad.note)
            .map(|file| file.path.clone());
        if HI_HAT_NOTES.contains(&pad.note) {
            pad.choke_group = HI_HAT_CHOKE_GROUP;
        }
    }
    Ok(kit)
}

/// A crash mid-write must not leave a truncated file that looks valid.
fn write_atomically(path: &Path, samples: &[f32]) -> Result<(), String> {
    let partial = path.with_extension("wav.part");
    write_wav(&partial, samples, 1, FACTORY_SAMPLE_RATE)?;
    std::fs::rename(&partial, path).map_err(|e| e.to_string())
}

fn kick() -> Vec<f32> {
    swept_sine(0.4, 140.0, 48.0, 28.0, 9.0)
}

fn rim() -> Vec<f32> {
    mix(&[
        (swept_sine(0.06, 1700.0, 1700.0, 0.0, 80.0), 0.6),
        (swept_sine(0.06, 480.0, 480.0, 0.0, 80.0), 0.6),
        (noise_layer(0.06, 11, 2000.0, 12_000.0, 90.0), 0.5),
    ])
}

fn snare() -> Vec<f32> {
    mix(&[
        (swept_sine(0.25, 230.0, 175.0, 40.0, 22.0), 0.6),
        (noise_layer(0.25, 21, 1500.0, 14_000.0, 16.0), 1.0),
    ])
}

fn snare_dry() -> Vec<f32> {
    mix(&[
        (swept_sine(0.15, 250.0, 200.0, 50.0, 40.0), 0.4),
        (noise_layer(0.15, 22, 2000.0, 14_000.0, 30.0), 1.0),
    ])
}

fn clap() -> Vec<f32> {
    const BURST_STARTS_SECONDS: [f32; 3] = [0.0, 0.011, 0.022];
    const BURST_DECAY: f32 = 160.0;
    const TAIL_DECAY: f32 = 28.0;

    let mut noise = Noise::new(31);
    let mut high_pass = HighPass::new(800.0);
    let mut low_pass = LowPass::new(4500.0);
    (0..frame_count(0.3))
        .map(|frame| {
            let t = time_of(frame);
            let shaped = low_pass.process(high_pass.process(noise.next()));
            let last_burst = BURST_STARTS_SECONDS.len() - 1;
            let (index, start) = BURST_STARTS_SECONDS
                .iter()
                .enumerate()
                .rev()
                .find(|(_, start)| t >= **start)
                .unwrap_or((0, &0.0));
            let decay = if index == last_burst {
                TAIL_DECAY
            } else {
                BURST_DECAY
            };
            shaped * envelope(t - start, decay)
        })
        .collect()
}

fn tom(base_hz: f32) -> Vec<f32> {
    swept_sine(0.5, base_hz * 1.35, base_hz, 22.0, 7.0)
}

fn hat_closed() -> Vec<f32> {
    mix(&[
        (metallic(0.08, 1.0, 7000.0, 55.0), 1.0),
        (noise_layer(0.08, 41, 8000.0, 18_000.0, 55.0), 0.3),
    ])
}

fn hat_pedal() -> Vec<f32> {
    mix(&[
        (metallic(0.12, 0.9, 6500.0, 35.0), 1.0),
        (noise_layer(0.12, 44, 7500.0, 18_000.0, 35.0), 0.3),
    ])
}

fn hat_open() -> Vec<f32> {
    mix(&[
        (metallic(0.6, 1.0, 6500.0, 7.5), 1.0),
        (noise_layer(0.6, 46, 7500.0, 18_000.0, 7.5), 0.25),
    ])
}

fn crash() -> Vec<f32> {
    mix(&[
        (metallic(1.8, 0.8, 4500.0, 2.6), 1.0),
        (noise_layer(1.8, 49, 3500.0, 18_000.0, 2.6), 0.6),
    ])
}

fn ride() -> Vec<f32> {
    mix(&[
        (metallic(1.6, 1.4, 6000.0, 3.2), 1.0),
        (swept_sine(1.6, 3300.0, 3300.0, 0.0, 5.0), 0.15),
        (swept_sine(1.6, 5100.0, 5100.0, 0.0, 5.0), 0.1),
    ])
}

fn frame_count(seconds: f32) -> usize {
    (seconds * FACTORY_SAMPLE_RATE as f32).round() as usize
}

fn time_of(frame: usize) -> f32 {
    frame as f32 / FACTORY_SAMPLE_RATE as f32
}

fn envelope(seconds: f32, decay: f32) -> f32 {
    (-seconds * decay).exp()
}

/// Sine whose pitch falls from `start_hz` to `end_hz`, with an exponential decay.
fn swept_sine(duration: f32, start_hz: f32, end_hz: f32, sweep_rate: f32, decay: f32) -> Vec<f32> {
    let mut phase = 0.0_f32;
    (0..frame_count(duration))
        .map(|frame| {
            let t = time_of(frame);
            let frequency = end_hz + (start_hz - end_hz) * envelope(t, sweep_rate);
            phase += TAU * frequency / FACTORY_SAMPLE_RATE as f32;
            phase.sin() * envelope(t, decay)
        })
        .collect()
}

fn noise_layer(
    duration: f32,
    seed: u32,
    high_pass_hz: f32,
    low_pass_hz: f32,
    decay: f32,
) -> Vec<f32> {
    let mut noise = Noise::new(seed);
    let mut high_pass = HighPass::new(high_pass_hz);
    let mut low_pass = LowPass::new(low_pass_hz);
    (0..frame_count(duration))
        .map(|frame| {
            let shaped = low_pass.process(high_pass.process(noise.next()));
            shaped * envelope(time_of(frame), decay)
        })
        .collect()
}

/// Inharmonic square partials, high-passed twice for a metallic shimmer.
fn metallic(duration: f32, frequency_scale: f32, high_pass_hz: f32, decay: f32) -> Vec<f32> {
    let mut first_pass = HighPass::new(high_pass_hz);
    let mut second_pass = HighPass::new(high_pass_hz);
    (0..frame_count(duration))
        .map(|frame| {
            let t = time_of(frame);
            let squares: f32 = METALLIC_PARTIALS_HZ
                .iter()
                .map(|partial| (TAU * partial * frequency_scale * t).sin().signum())
                .sum();
            let shaped = second_pass.process(first_pass.process(squares / 6.0));
            shaped * envelope(t, decay)
        })
        .collect()
}

fn mix(layers: &[(Vec<f32>, f32)]) -> Vec<f32> {
    let length = layers
        .iter()
        .map(|(samples, _)| samples.len())
        .max()
        .unwrap_or(0);
    let mut mixed = vec![0.0; length];
    for (samples, gain) in layers {
        for (target, sample) in mixed.iter_mut().zip(samples) {
            *target += sample * gain;
        }
    }
    mixed
}

fn finish(mut samples: Vec<f32>) -> Vec<f32> {
    remove_offset(&mut samples);
    apply_fades(&mut samples);
    normalize_peak(&mut samples);
    samples
}

fn remove_offset(samples: &mut [f32]) {
    if samples.is_empty() {
        return;
    }
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    samples.iter_mut().for_each(|sample| *sample -= mean);
}

fn apply_fades(samples: &mut [f32]) {
    let length = samples.len();
    let fade_in = frame_count(FADE_IN_SECONDS).max(1);
    let fade_out = frame_count(FADE_OUT_SECONDS).max(1);
    for (frame, sample) in samples.iter_mut().enumerate().take(fade_in) {
        *sample *= frame as f32 / fade_in as f32;
    }
    for (frame, sample) in samples
        .iter_mut()
        .enumerate()
        .skip(length.saturating_sub(fade_out))
    {
        *sample *= (length - 1 - frame) as f32 / fade_out as f32;
    }
}

fn normalize_peak(samples: &mut [f32]) {
    let peak = samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if peak > 0.0 {
        let scale = PEAK_LEVEL / peak;
        samples.iter_mut().for_each(|sample| *sample *= scale);
    }
}

struct Noise(u32);

impl Noise {
    fn new(seed: u32) -> Self {
        Self(seed)
    }

    /// Uniform in [−1, 1).
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

struct HighPass {
    coefficient: f32,
    previous_input: f32,
    previous_output: f32,
}

impl HighPass {
    fn new(cutoff_hz: f32) -> Self {
        let time_constant = 1.0 / (TAU * cutoff_hz);
        let step = 1.0 / FACTORY_SAMPLE_RATE as f32;
        Self {
            coefficient: time_constant / (time_constant + step),
            previous_input: 0.0,
            previous_output: 0.0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.coefficient * (self.previous_output + input - self.previous_input);
        self.previous_input = input;
        self.previous_output = output;
        output
    }
}

struct LowPass {
    smoothing: f32,
    state: f32,
}

impl LowPass {
    fn new(cutoff_hz: f32) -> Self {
        let time_constant = 1.0 / (TAU * cutoff_hz);
        let step = 1.0 / FACTORY_SAMPLE_RATE as f32;
        Self {
            smoothing: step / (time_constant + step),
            state: 0.0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        self.state += self.smoothing * (input - self.state);
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler_kit::SamplerKit;

    const MIN_PEAK: f32 = 0.708; // −3 dBFS
    const MAX_PEAK: f32 = 0.891; // −1 dBFS

    fn duration_seconds(samples: &[f32]) -> f32 {
        samples.len() as f32 / FACTORY_SAMPLE_RATE as f32
    }

    fn peak(samples: &[f32]) -> f32 {
        samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
    }

    fn mean(samples: &[f32]) -> f32 {
        samples.iter().sum::<f32>() / samples.len() as f32
    }

    fn zero_crossings_per_second(samples: &[f32]) -> f32 {
        let crossings = samples
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();
        crossings as f32 / duration_seconds(samples)
    }

    fn expected_duration_range(slug: &str) -> (f32, f32) {
        match slug {
            "kick" => (0.3, 0.5),
            "hat-closed" => (0.05, 0.12),
            "hat-pedal" => (0.08, 0.2),
            "hat-open" => (0.4, 0.8),
            "crash" | "ride" => (1.4, 2.2),
            _ => (0.04, 0.7),
        }
    }

    fn render(slug: &str) -> Vec<f32> {
        FACTORY_DRUMS
            .iter()
            .find(|drum| drum.slug == slug)
            .expect("known slug")
            .render()
    }

    #[test]
    fn notes_match_the_default_kit_pads() {
        let kit_notes: Vec<u8> = SamplerKit::default()
            .pads
            .iter()
            .map(|pad| pad.note)
            .collect();
        let drum_notes: Vec<u8> = FACTORY_DRUMS.iter().map(|drum| drum.note).collect();
        assert_eq!(drum_notes, kit_notes);
    }

    #[test]
    fn slugs_and_names_are_unique() {
        for (index, drum) in FACTORY_DRUMS.iter().enumerate() {
            for other in &FACTORY_DRUMS[index + 1..] {
                assert_ne!(drum.slug, other.slug);
                assert_ne!(drum.name, other.name);
            }
        }
    }

    #[test]
    fn every_sound_is_finite_and_audible() {
        for drum in &FACTORY_DRUMS {
            let samples = drum.render();
            assert!(samples.iter().all(|s| s.is_finite()), "{}", drum.slug);
            assert!(rms(&samples) > 0.01, "{} is nearly silent", drum.slug);
        }
    }

    #[test]
    fn every_sound_has_the_expected_length() {
        for drum in &FACTORY_DRUMS {
            let (min, max) = expected_duration_range(drum.slug);
            let seconds = duration_seconds(&drum.render());
            assert!(
                (min..=max).contains(&seconds),
                "{} lasts {seconds}s",
                drum.slug
            );
        }
    }

    #[test]
    fn peak_stays_between_minus_three_and_minus_one_dbfs() {
        for drum in &FACTORY_DRUMS {
            let level = peak(&drum.render());
            assert!(
                (MIN_PEAK..=MAX_PEAK).contains(&level),
                "{} peaks at {level}",
                drum.slug
            );
        }
    }

    #[test]
    fn every_sound_starts_and_ends_on_zero() {
        for drum in &FACTORY_DRUMS {
            let samples = drum.render();
            assert!(samples[0].abs() < 1e-4, "{} starts with a click", drum.slug);
            assert!(
                samples[samples.len() - 1].abs() < 1e-4,
                "{} ends with a click",
                drum.slug
            );
        }
    }

    #[test]
    fn no_sound_carries_a_dc_offset() {
        for drum in &FACTORY_DRUMS {
            let samples = drum.render();
            assert!(mean(&samples).abs() < 0.01, "{} has an offset", drum.slug);
        }
    }

    #[test]
    fn rendering_twice_gives_identical_samples() {
        for drum in &FACTORY_DRUMS {
            assert_eq!(drum.render(), drum.render(), "{}", drum.slug);
        }
    }

    #[test]
    fn sounds_differ_from_each_other() {
        let rendered: Vec<Vec<f32>> = FACTORY_DRUMS.iter().map(FactoryDrum::render).collect();
        for (index, samples) in rendered.iter().enumerate() {
            assert!(
                !rendered[index + 1..].contains(samples),
                "{}",
                FACTORY_DRUMS[index].slug
            );
        }
    }

    #[test]
    fn closed_hat_is_brighter_than_the_kick_and_shorter_than_the_open_hat() {
        let closed = render("hat-closed");
        assert!(
            zero_crossings_per_second(&closed) > 5.0 * zero_crossings_per_second(&render("kick"))
        );
        assert!(closed.len() < render("hat-open").len());
    }

    #[test]
    fn toms_rise_in_pitch_with_their_note() {
        let tom_slugs = [
            "tom-low",
            "tom-low-2",
            "tom-mid-low",
            "tom-mid",
            "tom-mid-high",
            "tom-high",
        ];
        let rates: Vec<f32> = tom_slugs
            .iter()
            .map(|slug| zero_crossings_per_second(&render(slug)))
            .collect();
        assert!(rates.windows(2).all(|pair| pair[0] < pair[1]), "{rates:?}");
    }

    fn scratch_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "echoes_factory_drums_{}_{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn written_files_read_back_as_mono_48k_with_the_rendered_samples() {
        let directory = scratch_directory("read_back");
        let files = ensure_files(&directory).unwrap();
        assert_eq!(files.len(), FACTORY_DRUMS.len());
        for (file, drum) in files.iter().zip(&FACTORY_DRUMS) {
            let clip = crate::audio_clip::AudioClip::from_file(&file.path).unwrap();
            assert_eq!(clip.raw_sample_rate, FACTORY_SAMPLE_RATE, "{}", drum.slug);
            assert_eq!(clip.raw_samples, drum.render(), "{}", drum.slug);
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn starting_kit_fills_every_pad_and_chokes_the_hats_together() {
        let directory = scratch_directory("starting_kit");
        let kit = starting_kit(&directory).unwrap();
        assert!(kit.pads.iter().all(|pad| pad.sample_path.is_some()));
        for pad in &kit.pads {
            let is_hat = HI_HAT_NOTES.contains(&pad.note);
            assert_eq!(pad.choke_group, if is_hat { HI_HAT_CHOKE_GROUP } else { 0 });
        }
        assert!(kit.pads[0]
            .sample_path
            .as_deref()
            .unwrap()
            .ends_with("kick.wav"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_deleted_file_is_written_again() {
        let directory = scratch_directory("deleted");
        let files = ensure_files(&directory).unwrap();
        std::fs::remove_file(&files[0].path).unwrap();
        ensure_files(&directory).unwrap();
        assert!(Path::new(&files[0].path).is_file());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn listing_is_stable_and_leaves_no_partial_file() {
        let directory = scratch_directory("listing");
        let first = ensure_files(&directory).unwrap();
        let second = ensure_files(&directory).unwrap();
        let ids: Vec<_> = first.iter().map(|file| &file.id).collect();
        assert_eq!(ids, second.iter().map(|file| &file.id).collect::<Vec<_>>());
        assert_eq!(first[0].id, "factory://drums/kick");
        let leftovers = std::fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "part"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
