//! Built-in polyphonic synthesizer — sine-wave oscillators with ADSR envelopes.
//!
//! Implements `InstrumentDevice` for use inside `InstrumentNode`.
//! Up to 16 simultaneous voices; oldest voice is stolen when all are busy.
//!
//! ## Real-time contract
//!
//! `process_midi` never allocates.  All state (voices, counters) is
//! stack/inline-allocated and reused across blocks.

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::{MidiEventKind, MidiEventList};
use crate::audio_graph::node::TransportInfo;
use crate::audio_graph::nodes::instrument::InstrumentDevice;

const MAX_VOICES: usize = 16;
const TWO_PI: f32 = std::f32::consts::TAU;
/// Master gain to avoid clipping with many voices.
const VOICE_SCALE: f32 = 0.15;

// ── ADSR ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Adsr {
    attack_secs: f32,
    decay_secs: f32,
    sustain: f32,
    release_secs: f32,
}

impl Default for Adsr {
    fn default() -> Self {
        Adsr { attack_secs: 0.01, decay_secs: 0.1, sustain: 0.7, release_secs: 0.3 }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Stage { Idle, Attack, Decay, Sustain, Release }

// ── Voice ─────────────────────────────────────────────────────────────────────

struct Voice {
    active: bool,
    pitch: u8,
    velocity: f32,
    phase: f32,
    phase_inc: f32,
    stage: Stage,
    level: f32,
    /// Monotonically increasing counter — used for voice stealing (oldest first).
    birth: u64,
}

impl Voice {
    const fn idle() -> Self {
        Voice {
            active: false,
            pitch: 0,
            velocity: 0.0,
            phase: 0.0,
            phase_inc: 0.0,
            stage: Stage::Idle,
            level: 0.0,
            birth: 0,
        }
    }
}

// ── PolySynth ─────────────────────────────────────────────────────────────────

pub struct PolySynth {
    voices: [Voice; MAX_VOICES],
    adsr: Adsr,
    sample_rate: f32,
    num_channels: usize,
    birth_counter: u64,
}

impl PolySynth {
    pub fn new() -> Self {
        PolySynth {
            voices: [const { Voice::idle() }; MAX_VOICES],
            adsr: Adsr::default(),
            sample_rate: 48_000.0,
            num_channels: 2,
            birth_counter: 0,
        }
    }

    fn pitch_to_freq(pitch: u8) -> f32 {
        440.0 * 2.0_f32.powf((pitch as f32 - 69.0) / 12.0)
    }

    fn note_on(&mut self, pitch: u8, velocity: u8) {
        self.birth_counter += 1;
        let birth = self.birth_counter;
        let freq = Self::pitch_to_freq(pitch);
        let phase_inc = freq * TWO_PI / self.sample_rate;
        let vel = velocity as f32 / 127.0;

        // Re-trigger same pitch if already playing
        for v in &mut self.voices {
            if v.active && v.pitch == pitch {
                v.phase_inc = phase_inc;
                v.velocity = vel;
                v.stage = Stage::Attack;
                v.birth = birth;
                return;
            }
        }

        // Allocate a free voice
        for v in &mut self.voices {
            if !v.active {
                v.active = true;
                v.pitch = pitch;
                v.velocity = vel;
                v.phase = 0.0;
                v.phase_inc = phase_inc;
                v.stage = Stage::Attack;
                v.level = 0.0;
                v.birth = birth;
                return;
            }
        }

        // Steal the oldest active voice
        if let Some(v) = self.voices.iter_mut().min_by_key(|v| v.birth) {
            v.active = true;
            v.pitch = pitch;
            v.velocity = vel;
            v.phase = 0.0;
            v.phase_inc = phase_inc;
            v.stage = Stage::Attack;
            v.level = 0.0;
            v.birth = birth;
        }
    }

    fn note_off(&mut self, pitch: u8) {
        for v in &mut self.voices {
            if v.active && v.pitch == pitch && v.stage != Stage::Release {
                v.stage = Stage::Release;
            }
        }
    }

    /// Tick one sample for a voice. Returns the output sample.
    /// Sets `v.active = false` when the voice finishes releasing.
    #[inline]
    fn tick(v: &mut Voice, adsr: Adsr, sr: f32) -> f32 {
        if !v.active { return 0.0; }

        let attack_rate  = 1.0 / (adsr.attack_secs.max(0.0001)  * sr);
        let decay_rate   = (1.0 - adsr.sustain) / (adsr.decay_secs.max(0.0001)   * sr);
        let release_rate = 1.0 / (adsr.release_secs.max(0.0001) * sr);

        match v.stage {
            Stage::Attack => {
                v.level += attack_rate;
                if v.level >= 1.0 { v.level = 1.0; v.stage = Stage::Decay; }
            }
            Stage::Decay => {
                v.level -= decay_rate;
                if v.level <= adsr.sustain { v.level = adsr.sustain; v.stage = Stage::Sustain; }
            }
            Stage::Sustain => {}
            Stage::Release => {
                v.level -= release_rate;
                if v.level <= 0.0 {
                    v.level = 0.0;
                    v.active = false;
                    v.stage = Stage::Idle;
                    return 0.0;
                }
            }
            Stage::Idle => return 0.0,
        }

        let sample = v.phase.sin() * v.level * v.velocity;
        v.phase += v.phase_inc;
        if v.phase >= TWO_PI { v.phase -= TWO_PI; }
        sample
    }
}

impl InstrumentDevice for PolySynth {
    fn prepare(&mut self, sample_rate: f32, _max_block_size: usize, num_channels: usize) {
        self.sample_rate = sample_rate;
        self.num_channels = num_channels;
        for v in &mut self.voices {
            if v.active {
                let freq = Self::pitch_to_freq(v.pitch);
                v.phase_inc = freq * TWO_PI / sample_rate;
            }
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

        let block_size = transport.block_size;
        let events = events.as_slice();
        let mut ev_idx = 0;
        let adsr = self.adsr;
        let sr = self.sample_rate;

        let ch0 = output.channel_mut(0);

        for frame in 0..block_size {
            // Dispatch all events scheduled at or before this sample
            while ev_idx < events.len() && events[ev_idx].offset as usize <= frame {
                match events[ev_idx].kind {
                    MidiEventKind::NoteOn { pitch, velocity } => self.note_on(pitch, velocity),
                    MidiEventKind::NoteOff { pitch }          => self.note_off(pitch),
                }
                ev_idx += 1;
            }

            let mut sample = 0.0f32;
            for v in &mut self.voices {
                sample += Self::tick(v, adsr, sr);
            }
            ch0[frame] = sample * VOICE_SCALE;
        }

        output.mono_to_all_channels();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            *v = Voice::idle();
        }
    }
}
