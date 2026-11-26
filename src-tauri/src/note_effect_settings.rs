//! Note effects saved with a track (ADR-038): transpose, scale and chord.
//!
//! The note logic lives in `audio_graph::note_stage`; this module only holds the
//! configuration the project file, the commands and the live cells share.

use serde::{Deserialize, Serialize};

pub const MIN_TRANSPOSE_SEMITONES: i8 = -24;
pub const MAX_TRANSPOSE_SEMITONES: i8 = 24;
pub const MAX_CHORD_VOICES: usize = 4;
const PITCH_CLASSES: u8 = 12;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleKind {
    #[default]
    Major,
    NaturalMinor,
    HarmonicMinor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    PentatonicMajor,
    PentatonicMinor,
    Blues,
}

impl ScaleKind {
    pub fn index(self) -> u8 {
        match self {
            Self::Major => 0,
            Self::NaturalMinor => 1,
            Self::HarmonicMinor => 2,
            Self::Dorian => 3,
            Self::Phrygian => 4,
            Self::Lydian => 5,
            Self::Mixolydian => 6,
            Self::PentatonicMajor => 7,
            Self::PentatonicMinor => 8,
            Self::Blues => 9,
        }
    }

    pub fn from_index(index: u8) -> Self {
        match index {
            1 => Self::NaturalMinor,
            2 => Self::HarmonicMinor,
            3 => Self::Dorian,
            4 => Self::Phrygian,
            5 => Self::Lydian,
            6 => Self::Mixolydian,
            7 => Self::PentatonicMajor,
            8 => Self::PentatonicMinor,
            9 => Self::Blues,
            _ => Self::Major,
        }
    }

    /// Semitones above the root, ascending.
    pub fn degrees(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::NaturalMinor => &[0, 2, 3, 5, 7, 8, 10],
            Self::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Self::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Self::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Self::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Self::PentatonicMajor => &[0, 2, 4, 7, 9],
            Self::PentatonicMinor => &[0, 3, 5, 7, 10],
            Self::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChordShape {
    Octave,
    Fifth,
    #[default]
    Major,
    Minor,
    Sus2,
    Sus4,
    Diminished,
    Augmented,
    Major7,
    Minor7,
    Dominant7,
}

impl ChordShape {
    pub fn index(self) -> u8 {
        match self {
            Self::Octave => 0,
            Self::Fifth => 1,
            Self::Major => 2,
            Self::Minor => 3,
            Self::Sus2 => 4,
            Self::Sus4 => 5,
            Self::Diminished => 6,
            Self::Augmented => 7,
            Self::Major7 => 8,
            Self::Minor7 => 9,
            Self::Dominant7 => 10,
        }
    }

    pub fn from_index(index: u8) -> Self {
        match index {
            0 => Self::Octave,
            1 => Self::Fifth,
            3 => Self::Minor,
            4 => Self::Sus2,
            5 => Self::Sus4,
            6 => Self::Diminished,
            7 => Self::Augmented,
            8 => Self::Major7,
            9 => Self::Minor7,
            10 => Self::Dominant7,
            _ => Self::Major,
        }
    }

    /// Semitones above the played pitch, the played pitch included (at most
    /// `MAX_CHORD_VOICES`).
    pub fn offsets(self) -> &'static [u8] {
        match self {
            Self::Octave => &[0, 12],
            Self::Fifth => &[0, 7],
            Self::Major => &[0, 4, 7],
            Self::Minor => &[0, 3, 7],
            Self::Sus2 => &[0, 2, 7],
            Self::Sus4 => &[0, 5, 7],
            Self::Diminished => &[0, 3, 6],
            Self::Augmented => &[0, 4, 8],
            Self::Major7 => &[0, 4, 7, 11],
            Self::Minor7 => &[0, 3, 7, 10],
            Self::Dominant7 => &[0, 4, 7, 10],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransposeSettings {
    pub enabled: bool,
    pub semitones: i8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScaleSettings {
    pub enabled: bool,
    /// Pitch class of the scale's first degree, 0 = C … 11 = B.
    pub root: u8,
    pub scale: ScaleKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChordSettings {
    pub enabled: bool,
    pub shape: ChordShape,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoteEffectSettings {
    pub transpose: TransposeSettings,
    pub scale: ScaleSettings,
    pub chord: ChordSettings,
}

impl NoteEffectSettings {
    /// Settings held inside the ranges ADR-038 defines.
    pub fn clamped(mut self) -> Self {
        self.transpose.semitones = self
            .transpose
            .semitones
            .clamp(MIN_TRANSPOSE_SEMITONES, MAX_TRANSPOSE_SEMITONES);
        self.scale.root = self.scale.root.min(PITCH_CLASSES - 1);
        self
    }

    pub fn any_enabled(&self) -> bool {
        self.transpose.enabled || self.scale.enabled || self.chord.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_their_defaults() {
        let settings: NoteEffectSettings =
            serde_json::from_str(r#"{ "chord": { "enabled": true } }"#).unwrap();
        assert!(settings.chord.enabled);
        assert_eq!(settings.chord.shape, ChordShape::Major);
        assert!(!settings.transpose.enabled);
        assert_eq!(settings.transpose.semitones, 0);
        assert_eq!(settings.scale.scale, ScaleKind::Major);
    }

    #[test]
    fn settings_are_held_in_range() {
        let settings = NoteEffectSettings {
            transpose: TransposeSettings {
                enabled: true,
                semitones: -100,
            },
            scale: ScaleSettings {
                root: 40,
                ..ScaleSettings::default()
            },
            ..NoteEffectSettings::default()
        }
        .clamped();
        assert_eq!(settings.transpose.semitones, MIN_TRANSPOSE_SEMITONES);
        assert_eq!(settings.scale.root, 11);
    }

    #[test]
    fn enum_indices_round_trip() {
        for index in 0..=9 {
            assert_eq!(ScaleKind::from_index(index).index(), index);
        }
        for index in 0..=10 {
            assert_eq!(ChordShape::from_index(index).index(), index);
        }
    }

    #[test]
    fn every_chord_shape_starts_on_the_played_pitch_and_fits_the_voice_limit() {
        for index in 0..=10 {
            let offsets = ChordShape::from_index(index).offsets();
            assert_eq!(offsets[0], 0);
            assert!(offsets.len() <= MAX_CHORD_VOICES);
            assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }

    #[test]
    fn every_scale_starts_on_its_root_and_stays_inside_the_octave() {
        for index in 0..=9 {
            let degrees = ScaleKind::from_index(index).degrees();
            assert_eq!(degrees[0], 0);
            assert!(degrees.iter().all(|&degree| degree < 12));
            assert!(degrees.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }
}
