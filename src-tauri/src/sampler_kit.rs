//! Built-in sampler kit: the pad settings saved with a track (ADR-028).
//!
//! The DSP lives in `audio_graph::nodes::sampler`; this module only holds the
//! configuration and the decoded PCM the compiler hands to it.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::audio_clip::AudioClip;

/// `AppTrack.instrument` value selecting the built-in sampler.
pub const SAMPLER_INSTRUMENT_ID: &str = "builtin:sampler";
pub const SAMPLER_PAD_COUNT: usize = 16;
/// General MIDI kick: pad 1 answers the factory drum MIDI as-is.
const FIRST_PAD_NOTE: u8 = 36;
pub const MAX_CHOKE_GROUP: u8 = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadPlayMode {
    /// Plays the whole sample; note-off is ignored.
    #[default]
    OneShot,
    /// Releases on note-off.
    Gate,
}

/// Decoded mono PCM of a pad, shared with the audio cache.
#[derive(Clone)]
pub struct PadSample {
    pub samples: Arc<Vec<f32>>,
    pub sample_rate: u32,
}

impl fmt::Debug for PadSample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PadSample")
            .field("frames", &self.samples.len())
            .field("sample_rate", &self.sample_rate)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplerPad {
    pub note: u8,
    pub sample_path: Option<String>,
    pub gain_db: f32,
    /// −1 (left) … +1 (right); centre plays at unity on both channels.
    pub pan: f32,
    /// Fractional part = cents.
    pub tune_semitones: f32,
    pub mode: PadPlayMode,
    pub attack_ms: f32,
    /// Gate mode only.
    pub release_ms: f32,
    /// 0 = none; a pad cuts every sounding pad of its group.
    pub choke_group: u8,
    #[serde(skip)]
    pub sample: Option<PadSample>,
}

impl Default for SamplerPad {
    fn default() -> Self {
        Self {
            note: FIRST_PAD_NOTE,
            sample_path: None,
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            mode: PadPlayMode::OneShot,
            attack_ms: 0.0,
            release_ms: 10.0,
            choke_group: 0,
            sample: None,
        }
    }
}

impl SamplerPad {
    /// Settings held inside the ranges ADR-028 defines.
    pub fn clamped(mut self) -> Self {
        self.note = self.note.min(127);
        self.gain_db = self.gain_db.clamp(-60.0, 12.0);
        self.pan = self.pan.clamp(-1.0, 1.0);
        self.tune_semitones = self.tune_semitones.clamp(-24.0, 24.0);
        self.attack_ms = self.attack_ms.clamp(0.0, 100.0);
        self.release_ms = self.release_ms.clamp(5.0, 2000.0);
        self.choke_group = self.choke_group.min(MAX_CHOKE_GROUP);
        self
    }

    /// A sample was chosen but could not be read.
    pub fn is_missing(&self) -> bool {
        self.sample_path.is_some() && self.sample.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplerKit {
    pub pads: Vec<SamplerPad>,
}

impl Default for SamplerKit {
    fn default() -> Self {
        let pads = (0..SAMPLER_PAD_COUNT as u8)
            .map(|index| SamplerPad {
                note: FIRST_PAD_NOTE + index,
                ..SamplerPad::default()
            })
            .collect();
        Self { pads }
    }
}

impl SamplerKit {
    pub fn load_samples(&mut self, mut load: impl FnMut(&str) -> Option<PadSample>) {
        for pad in &mut self.pads {
            pad.sample = pad.sample_path.as_deref().and_then(&mut load);
        }
    }

    pub fn to_dto(&self) -> SamplerKitDto {
        SamplerKitDto {
            pads: self
                .pads
                .iter()
                .map(|pad| SamplerPadDto {
                    missing: pad.is_missing(),
                    settings: pad.clone(),
                })
                .collect(),
        }
    }
}

pub fn decode_pad_sample(path: &str) -> Option<PadSample> {
    match AudioClip::from_file(path) {
        Ok(audio) => Some(PadSample {
            samples: Arc::new(audio.raw_samples),
            sample_rate: audio.raw_sample_rate,
        }),
        Err(e) => {
            eprintln!("[audio] Échantillon introuvable '{path}': {e}");
            None
        }
    }
}

#[derive(Serialize, Clone)]
pub struct SamplerKitDto {
    pub pads: Vec<SamplerPadDto>,
}

#[derive(Serialize, Clone)]
pub struct SamplerPadDto {
    #[serde(flatten)]
    pub settings: SamplerPad,
    pub missing: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_kit_covers_sixteen_notes_from_the_kick() {
        let kit = SamplerKit::default();
        assert_eq!(kit.pads.len(), SAMPLER_PAD_COUNT);
        assert_eq!(kit.pads[0].note, 36);
        assert_eq!(kit.pads[15].note, 51);
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let pad: SamplerPad = serde_json::from_str(r#"{ "note": 38 }"#).unwrap();
        assert_eq!(pad.note, 38);
        assert_eq!(pad.mode, PadPlayMode::OneShot);
        assert_eq!(pad.release_ms, 10.0);
    }

    #[test]
    fn unreadable_sample_is_reported_missing() {
        let mut kit = SamplerKit::default();
        kit.pads[0].sample_path = Some("does-not-exist.wav".into());
        kit.load_samples(|_| None);
        assert!(kit.pads[0].is_missing());
        assert!(!kit.pads[1].is_missing());
    }

    #[test]
    fn settings_are_held_in_range() {
        let pad = SamplerPad {
            gain_db: 40.0,
            pan: -3.0,
            tune_semitones: 99.0,
            release_ms: 0.0,
            choke_group: 9,
            ..SamplerPad::default()
        }
        .clamped();
        assert_eq!(pad.gain_db, 12.0);
        assert_eq!(pad.pan, -1.0);
        assert_eq!(pad.tune_semitones, 24.0);
        assert_eq!(pad.release_ms, 5.0);
        assert_eq!(pad.choke_group, MAX_CHOKE_GROUP);
    }

    #[test]
    fn dto_keeps_settings_flat() {
        let json = serde_json::to_value(SamplerKit::default().to_dto()).unwrap();
        assert_eq!(json["pads"][0]["note"], 36);
        assert_eq!(json["pads"][0]["missing"], false);
        assert!(json["pads"][0].get("sample").is_none());
    }
}
