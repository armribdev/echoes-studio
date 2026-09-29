//! Metronome click for live monitoring (ADR-026).
//!
//! Generated inside the CPAL callback and added to the output after the graph
//! has rendered, so it never enters the project mix, the meters or an export.
//! Everything is pre-sized: `render` only does arithmetic on the caller's slice.

use std::f32::consts::TAU;

const ACCENT_FREQUENCY_HZ: f32 = 1_600.0;
const BEAT_FREQUENCY_HZ: f32 = 1_000.0;
const CLICK_SECONDS: f32 = 0.04;
const ACCENT_GAIN: f32 = 0.35;
const BEAT_GAIN: f32 = 0.25;

/// Where a slice sits on the beat grid.
#[derive(Debug, Clone, Copy)]
pub struct ClickTiming {
    /// Transport sample of the slice's first frame. Negative during a count-in
    /// that starts before sample 0.
    pub first_sample: i64,
    pub bpm: f32,
    pub sample_rate: f32,
    pub beats_per_bar: u8,
}

#[derive(Default)]
pub struct Metronome {
    remaining_samples: u32,
    length_samples: u32,
    phase: f32,
    phase_step: f32,
    gain: f32,
}

impl Metronome {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fills `out` with the click for one slice; silence when `timing` is `None`.
    /// A click starts on the first frame at or after each beat, wherever the
    /// slice boundaries fall.
    pub fn render(&mut self, out: &mut [f32], timing: Option<ClickTiming>) {
        let Some(timing) = timing.filter(|t| t.bpm > 0.0 && t.sample_rate > 0.0) else {
            self.remaining_samples = 0;
            out.fill(0.0);
            return;
        };
        let beats_per_sample = timing.bpm as f64 / 60.0 / timing.sample_rate as f64;
        let beat_index = |sample: i64| (sample as f64 * beats_per_sample).floor() as i64;
        let beats_per_bar = timing.beats_per_bar.max(1) as i64;

        let mut previous_beat = beat_index(timing.first_sample - 1);
        for (offset, frame) in out.iter_mut().enumerate() {
            let beat = beat_index(timing.first_sample + offset as i64);
            if beat != previous_beat {
                let accent = beat.rem_euclid(beats_per_bar) == 0;
                self.start_click(accent, timing.sample_rate);
                previous_beat = beat;
            }
            *frame = self.next_sample();
        }
    }

    fn start_click(&mut self, accent: bool, sample_rate: f32) {
        let frequency = if accent {
            ACCENT_FREQUENCY_HZ
        } else {
            BEAT_FREQUENCY_HZ
        };
        self.length_samples = (CLICK_SECONDS * sample_rate) as u32;
        self.remaining_samples = self.length_samples;
        self.phase = 0.0;
        self.phase_step = frequency / sample_rate;
        self.gain = if accent { ACCENT_GAIN } else { BEAT_GAIN };
    }

    fn next_sample(&mut self) -> f32 {
        if self.remaining_samples == 0 {
            return 0.0;
        }
        let envelope = self.remaining_samples as f32 / self.length_samples as f32;
        let sample = self.gain * envelope * envelope * (self.phase * TAU).sin();
        self.phase = (self.phase + self.phase_step).fract();
        self.remaining_samples -= 1;
        sample
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: f32 = 48_000.0;
    const SAMPLES_PER_BEAT: usize = 24_000;

    fn timing(first_sample: i64) -> Option<ClickTiming> {
        Some(ClickTiming {
            first_sample,
            bpm: 120.0,
            sample_rate: SAMPLE_RATE,
            beats_per_bar: 4,
        })
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |peak, s| peak.max(s.abs()))
    }

    fn render(first_sample: i64, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames];
        Metronome::new().render(&mut out, timing(first_sample));
        out
    }

    #[test]
    fn clicks_on_each_beat_and_is_silent_between() {
        let out = render(0, SAMPLES_PER_BEAT * 2);
        assert!(peak(&out[..2_000]) > 0.0);
        assert!(peak(&out[SAMPLES_PER_BEAT..SAMPLES_PER_BEAT + 2_000]) > 0.0);
        assert_eq!(peak(&out[10_000..20_000]), 0.0);
    }

    #[test]
    fn first_beat_of_the_bar_is_accented() {
        let out = render(0, SAMPLES_PER_BEAT * 2);
        assert!(peak(&out[..2_000]) > peak(&out[SAMPLES_PER_BEAT..SAMPLES_PER_BEAT + 2_000]),);
    }

    #[test]
    fn starting_mid_beat_does_not_click_at_once() {
        let out = render(12_000, 4_000);
        assert_eq!(peak(&out), 0.0);
    }

    #[test]
    fn count_in_before_sample_zero_clicks_and_accents_the_bar_start() {
        let out = render(-(SAMPLES_PER_BEAT as i64) * 4, SAMPLES_PER_BEAT * 4);
        let accent = peak(&out[..2_000]);
        assert!(accent > 0.0);
        assert!(peak(&out[SAMPLES_PER_BEAT..SAMPLES_PER_BEAT + 2_000]) < accent);
        assert!(peak(&out[SAMPLES_PER_BEAT * 3..SAMPLES_PER_BEAT * 3 + 2_000]) > 0.0);
    }

    #[test]
    fn click_is_the_same_whatever_the_block_size() {
        let whole = render(0, 30_000);

        let mut metronome = Metronome::new();
        let mut split = vec![0.0; 30_000];
        let mut start = 0;
        for size in [1_000, 23_500, 5_500] {
            metronome.render(&mut split[start..start + size], timing(start as i64));
            start += size;
        }
        assert_eq!(whole, split);
    }

    #[test]
    fn no_timing_gives_silence_and_cuts_a_ringing_click() {
        let mut metronome = Metronome::new();
        let mut out = vec![0.0; 100];
        metronome.render(&mut out, timing(0));
        assert!(peak(&out) > 0.0);
        metronome.render(&mut out, None);
        assert_eq!(peak(&out), 0.0);
        metronome.render(&mut out, timing(12_000));
        assert_eq!(peak(&out), 0.0);
    }
}
