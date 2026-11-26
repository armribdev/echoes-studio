//! Built-in drum sampler (ADR-028) — one mono sample per pad, triggered by MIDI notes.
//!
//! ## Real-time contract
//!
//! `process_midi` never allocates: voices are fixed arrays of `Copy` structs
//! that index a pad.  Sample data is shared `Arc` PCM cloned at compile time
//! and released with the displaced scheduler on the command thread.

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::{MidiEvent, MidiEventKind, MidiEventList};
use crate::audio_graph::node::{PrepareContext, TransportInfo};
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::sampler_kit::{PadPlayMode, SamplerPad};

const MAX_VOICES: usize = 32;
const MAX_FADING_VOICES: usize = 8;
/// Fade used for choke and voice stealing, and the shortest gate release.
const DECLICK_SECONDS: f32 = 0.005;

struct Pad {
    samples: Option<Arc<Vec<f32>>>,
    native_sample_rate: u32,
    note: u8,
    gain_left: f32,
    gain_right: f32,
    tune_ratio: f64,
    mode: PadPlayMode,
    attack_seconds: f32,
    release_seconds: f32,
    choke_group: u8,
    /// Source frames advanced per output frame; set in `prepare`.
    step: f64,
    attack_increment: f32,
    release_decrement: f32,
}

impl Pad {
    fn from_settings(settings: &SamplerPad) -> Self {
        let gain = 10.0_f32.powf(settings.gain_db / 20.0);
        let pan = settings.pan.clamp(-1.0, 1.0);
        let sample = settings.sample.as_ref();
        Self {
            samples: sample.map(|s| Arc::clone(&s.samples)),
            native_sample_rate: sample.map_or(0, |s| s.sample_rate),
            note: settings.note,
            gain_left: gain * (1.0 - pan).min(1.0),
            gain_right: gain * (1.0 + pan).min(1.0),
            tune_ratio: 2.0_f64.powf(settings.tune_semitones as f64 / 12.0),
            mode: settings.mode,
            attack_seconds: settings.attack_ms / 1000.0,
            release_seconds: (settings.release_ms / 1000.0).max(DECLICK_SECONDS),
            choke_group: settings.choke_group,
            step: 1.0,
            attack_increment: 1.0,
            release_decrement: 1.0,
        }
    }

    fn prepare(&mut self, sample_rate: f32) {
        let rate_ratio = if self.native_sample_rate > 0 {
            self.native_sample_rate as f64 / sample_rate as f64
        } else {
            1.0
        };
        self.step = rate_ratio * self.tune_ratio;
        self.attack_increment = ramp_per_frame(self.attack_seconds, sample_rate);
        self.release_decrement = ramp_per_frame(self.release_seconds, sample_rate);
    }

    /// Next source sample for a voice, `None` once past the sample's end.
    #[inline]
    fn sample_at(&self, frames_played: u64) -> Option<f32> {
        let samples = self.samples.as_deref()?;
        let position = frames_played as f64 * self.step;
        let index = position as usize;
        let current = *samples.get(index)?;
        let next = samples.get(index + 1).copied().unwrap_or(current);
        let fraction = (position - index as f64) as f32;
        Some(current + fraction * (next - current))
    }
}

/// Full-scale level change per frame for a ramp of `seconds`; 0 s jumps at once.
fn ramp_per_frame(seconds: f32, sample_rate: f32) -> f32 {
    if seconds <= 0.0 {
        1.0
    } else {
        1.0 / (seconds * sample_rate)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Idle,
    Attack,
    Hold,
    Release,
}

#[derive(Clone, Copy)]
struct Voice {
    stage: Stage,
    pad: usize,
    pitch: u8,
    velocity_gain: f32,
    level: f32,
    release_decrement: f32,
    frames_played: u64,
    /// Trigger order — the oldest voice is stolen first.
    birth: u64,
}

impl Voice {
    const fn idle() -> Self {
        Self {
            stage: Stage::Idle,
            pad: 0,
            pitch: 0,
            velocity_gain: 0.0,
            level: 0.0,
            release_decrement: 1.0,
            frames_played: 0,
            birth: 0,
        }
    }

    fn is_sounding(&self) -> bool {
        self.stage != Stage::Idle
    }

    fn release(&mut self, decrement: f32) {
        if matches!(self.stage, Stage::Attack | Stage::Hold) {
            self.stage = Stage::Release;
            self.release_decrement = decrement;
        }
    }

    #[inline]
    fn next_level(&mut self, attack_increment: f32) -> f32 {
        match self.stage {
            Stage::Attack => {
                self.level += attack_increment;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Hold;
                }
            }
            Stage::Release => {
                self.level -= self.release_decrement;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Hold | Stage::Idle => {}
        }
        self.level
    }

    /// One output frame, before panning.
    #[inline]
    fn render(&mut self, pad: &Pad) -> f32 {
        let Some(sample) = pad.sample_at(self.frames_played) else {
            self.stage = Stage::Idle;
            return 0.0;
        };
        self.frames_played += 1;
        sample * self.next_level(pad.attack_increment) * self.velocity_gain
    }
}

pub struct Sampler {
    pads: Vec<Pad>,
    voices: [Voice; MAX_VOICES],
    /// Stolen voices finishing their fade while their slot plays a new note.
    fading: [Voice; MAX_FADING_VOICES],
    birth_counter: u64,
    declick_decrement: f32,
    num_channels: usize,
}

impl Sampler {
    pub fn new(pads: &[SamplerPad]) -> Self {
        Self {
            pads: pads.iter().map(Pad::from_settings).collect(),
            voices: [const { Voice::idle() }; MAX_VOICES],
            fading: [const { Voice::idle() }; MAX_FADING_VOICES],
            birth_counter: 0,
            declick_decrement: 1.0,
            num_channels: 2,
        }
    }

    fn note_on(&mut self, pitch: u8, velocity: u8) {
        // Choke first, so layered pads of one group do not cut each other.
        for pad_index in 0..self.pads.len() {
            let group = self.pads[pad_index].choke_group;
            if self.pads[pad_index].note == pitch && group != 0 {
                self.choke(group);
            }
        }
        for pad_index in 0..self.pads.len() {
            let pad = &self.pads[pad_index];
            if pad.note == pitch && pad.samples.is_some() {
                self.start_voice(pad_index, pitch, velocity);
            }
        }
    }

    fn choke(&mut self, group: u8) {
        let decrement = self.declick_decrement;
        for voice in &mut self.voices {
            if voice.is_sounding() && self.pads[voice.pad].choke_group == group {
                voice.release(decrement);
            }
        }
    }

    fn start_voice(&mut self, pad_index: usize, pitch: u8, velocity: u8) {
        self.birth_counter += 1;
        let pad = &self.pads[pad_index];
        let (stage, level) = if pad.attack_increment >= 1.0 {
            (Stage::Hold, 1.0)
        } else {
            (Stage::Attack, 0.0)
        };
        let voice = Voice {
            stage,
            pad: pad_index,
            pitch,
            velocity_gain: velocity as f32 / 127.0,
            level,
            release_decrement: pad.release_decrement,
            frames_played: 0,
            birth: self.birth_counter,
        };
        let slot = self.free_slot();
        self.voices[slot] = voice;
    }

    /// An idle voice slot, or the oldest one after moving its voice to a fade-out slot.
    fn free_slot(&mut self) -> usize {
        if let Some(index) = self.voices.iter().position(|v| !v.is_sounding()) {
            return index;
        }
        let oldest = (0..MAX_VOICES)
            .min_by_key(|&i| self.voices[i].birth)
            .unwrap_or(0);
        let fade_slot = (0..MAX_FADING_VOICES)
            .find(|&i| !self.fading[i].is_sounding())
            .or_else(|| (0..MAX_FADING_VOICES).min_by_key(|&i| self.fading[i].birth))
            .unwrap_or(0);
        let mut stolen = self.voices[oldest];
        stolen.stage = Stage::Release;
        stolen.release_decrement = self.declick_decrement;
        self.fading[fade_slot] = stolen;
        oldest
    }

    fn note_off(&mut self, pitch: u8) {
        for voice in &mut self.voices {
            if voice.is_sounding()
                && voice.pitch == pitch
                && self.pads[voice.pad].mode == PadPlayMode::Gate
            {
                voice.release(voice.release_decrement);
            }
        }
    }

    /// Applies the events due at `frame`; returns the index of the next one.
    fn apply_events_until(&mut self, events: &[MidiEvent], mut next: usize, frame: usize) -> usize {
        while next < events.len() && events[next].offset as usize <= frame {
            match events[next].kind {
                MidiEventKind::NoteOn { pitch, velocity } => self.note_on(pitch, velocity),
                MidiEventKind::NoteOff { pitch } => self.note_off(pitch),
                MidiEventKind::Control { .. } | MidiEventKind::NoteExpression { .. } => {}
            }
            next += 1;
        }
        next
    }

    /// Sum of every sounding voice for one frame, as (left, right).
    #[inline]
    fn render_frame(&mut self) -> (f32, f32) {
        let mut left = 0.0;
        let mut right = 0.0;
        for voice in self.voices.iter_mut().chain(self.fading.iter_mut()) {
            if !voice.is_sounding() {
                continue;
            }
            let pad = &self.pads[voice.pad];
            let sample = voice.render(pad);
            left += sample * pad.gain_left;
            right += sample * pad.gain_right;
        }
        (left, right)
    }
}

impl InstrumentDevice for Sampler {
    fn prepare(&mut self, ctx: PrepareContext) {
        self.num_channels = ctx.num_channels;
        self.declick_decrement = ramp_per_frame(DECLICK_SECONDS, ctx.sample_rate);
        for pad in &mut self.pads {
            pad.prepare(ctx.sample_rate);
        }
    }

    fn process_midi(
        &mut self,
        events: &MidiEventList,
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        let output = &mut outputs[0];
        output.clear();
        let events = events.as_slice();
        let mut next_event = 0;

        if self.num_channels >= 2 && output.num_channels >= 2 {
            let (left_channel, right_channel) = output.two_channels_mut(0, 1);
            for frame in 0..transport.block_size {
                next_event = self.apply_events_until(events, next_event, frame);
                let (left, right) = self.render_frame();
                left_channel[frame] = left;
                right_channel[frame] = right;
            }
        } else {
            let mono = &mut output.channel_mut(0)[..transport.block_size];
            for (frame, out) in mono.iter_mut().enumerate() {
                next_event = self.apply_events_until(events, next_event, frame);
                let (left, right) = self.render_frame();
                *out = 0.5 * (left + right);
            }
        }
    }

    fn reset(&mut self) {
        self.voices = [const { Voice::idle() }; MAX_VOICES];
        self.fading = [const { Voice::idle() }; MAX_FADING_VOICES];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler_kit::PadSample;

    const RATE: f32 = 48_000.0;

    fn pad(note: u8, samples: Vec<f32>, settings: SamplerPad) -> SamplerPad {
        SamplerPad {
            note,
            sample: Some(PadSample {
                samples: Arc::new(samples),
                sample_rate: RATE as u32,
            }),
            ..settings
        }
    }

    fn sampler(pads: &[SamplerPad]) -> Sampler {
        let mut sampler = Sampler::new(pads);
        sampler.prepare(PrepareContext {
            sample_rate: RATE,
            max_block_size: 4096,
            num_channels: 2,
            live: false,
        });
        sampler
    }

    fn transport(block_size: usize) -> TransportInfo {
        TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            time_signature: crate::audio_graph::node::TimeSignature::default(),
            sample_rate: RATE,
            block_size,
        }
    }

    fn render(sampler: &mut Sampler, events: &[MidiEvent], block_size: usize) -> AudioBuffer {
        let mut list = MidiEventList::new();
        for event in events {
            list.push(*event);
        }
        let mut outputs = vec![AudioBuffer::new(2, block_size)];
        sampler.process_midi(&list, &mut outputs, &transport(block_size));
        outputs.remove(0)
    }

    fn note_on(offset: u32, pitch: u8) -> MidiEvent {
        MidiEvent {
            offset,
            kind: MidiEventKind::NoteOn {
                pitch,
                velocity: 127,
            },
        }
    }

    fn note_off(offset: u32, pitch: u8) -> MidiEvent {
        MidiEvent {
            offset,
            kind: MidiEventKind::NoteOff { pitch },
        }
    }

    #[test]
    fn one_shot_plays_the_sample_at_its_note_and_ignores_note_off() {
        let mut sampler = sampler(&[pad(36, vec![0.5; 100], SamplerPad::default())]);
        let out = render(&mut sampler, &[note_on(10, 36), note_off(20, 36)], 128);
        let left = out.channel(0);
        assert_eq!(left[9], 0.0);
        assert_eq!(left[10], 0.5);
        assert_eq!(left[60], 0.5, "one-shot keeps playing after note-off");
        assert_eq!(left[110], 0.0, "silent past the sample's end");
    }

    #[test]
    fn an_empty_kit_stays_silent() {
        let mut sampler = sampler(&[]);
        let out = render(&mut sampler, &[note_on(0, 0), note_off(1, 0)], 64);
        assert!(out.channel(0).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn other_notes_do_not_trigger_the_pad() {
        let mut sampler = sampler(&[pad(36, vec![0.5; 100], SamplerPad::default())]);
        let out = render(&mut sampler, &[note_on(0, 37)], 64);
        assert!(out.channel(0).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn gate_releases_on_note_off_without_a_hard_cut() {
        let settings = SamplerPad {
            mode: PadPlayMode::Gate,
            release_ms: 5.0,
            ..SamplerPad::default()
        };
        let mut sampler = sampler(&[pad(38, vec![1.0; 2000], settings)]);
        let out = render(&mut sampler, &[note_on(0, 38), note_off(100, 38)], 600);
        let left = out.channel(0);
        assert!(left[101] > 0.9 && left[101] < 1.0, "fade starts, no cut");
        assert_eq!(left[400], 0.0, "5 ms = 240 frames later the voice is done");
    }

    #[test]
    fn choke_group_fades_the_other_pad() {
        let group = SamplerPad {
            choke_group: 1,
            ..SamplerPad::default()
        };
        let mut sampler = sampler(&[
            pad(46, vec![1.0; 2000], group.clone()),
            pad(42, vec![0.25; 2000], group),
        ]);
        let out = render(&mut sampler, &[note_on(0, 46), note_on(100, 42)], 600);
        let left = out.channel(0);
        assert_eq!(left[99], 1.0);
        assert!(left[150] > 0.25 && left[150] < 1.25, "open hat fading out");
        assert_eq!(left[500], 0.25, "only the closed hat is left");
    }

    #[test]
    fn pan_and_gain_shape_the_two_channels() {
        let settings = SamplerPad {
            pan: 1.0,
            gain_db: -6.0,
            ..SamplerPad::default()
        };
        let mut sampler = sampler(&[pad(36, vec![1.0; 100], settings)]);
        let out = render(&mut sampler, &[note_on(0, 36)], 16);
        assert_eq!(out.channel(0)[0], 0.0);
        assert!((out.channel(1)[0] - 0.501).abs() < 0.001);
    }

    #[test]
    fn an_octave_up_plays_twice_as_fast() {
        let ramp: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let settings = SamplerPad {
            tune_semitones: 12.0,
            ..SamplerPad::default()
        };
        let mut sampler = sampler(&[pad(36, ramp, settings)]);
        let out = render(&mut sampler, &[note_on(0, 36)], 64);
        assert_eq!(out.channel(0)[10], 20.0);
        assert_eq!(out.channel(0)[60], 0.0, "100 source frames last 50 frames");
    }

    #[test]
    fn a_full_pool_steals_the_oldest_voice_with_a_fade() {
        let mut sampler = sampler(&[pad(36, vec![1.0; 10_000], SamplerPad::default())]);
        let events: Vec<MidiEvent> = (0..=MAX_VOICES as u32).map(|i| note_on(i, 36)).collect();
        let out = render(&mut sampler, &events, 2000);
        let left = out.channel(0);
        assert_eq!(left[MAX_VOICES - 1], MAX_VOICES as f32);
        assert!(
            left[MAX_VOICES + 1] > MAX_VOICES as f32,
            "the stolen voice fades instead of stopping"
        );
        assert_eq!(left[1000], MAX_VOICES as f32, "fade done, pool full again");
    }
}
