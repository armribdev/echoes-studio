//! Arpeggiator settings saved with a track (ADR-030).
//!
//! The note logic lives in `audio_graph::arpeggiator`; this module only holds
//! the configuration the project file, the commands and the live cells share.

use serde::{Deserialize, Serialize};

pub const MIN_GATE_PERCENT: u8 = 5;
pub const MAX_GATE_PERCENT: u8 = 100;
pub const MIN_OCTAVES: u8 = 1;
pub const MAX_OCTAVES: u8 = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArpMode {
    #[default]
    Up,
    Down,
    UpDown,
    Played,
    Random,
}

impl ArpMode {
    pub fn index(self) -> u8 {
        match self {
            Self::Up => 0,
            Self::Down => 1,
            Self::UpDown => 2,
            Self::Played => 3,
            Self::Random => 4,
        }
    }

    pub fn from_index(index: u8) -> Self {
        match index {
            1 => Self::Down,
            2 => Self::UpDown,
            3 => Self::Played,
            4 => Self::Random,
            _ => Self::Up,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArpRate {
    Quarter,
    Eighth,
    #[default]
    Sixteenth,
    ThirtySecond,
}

impl ArpRate {
    pub fn index(self) -> u8 {
        match self {
            Self::Quarter => 0,
            Self::Eighth => 1,
            Self::Sixteenth => 2,
            Self::ThirtySecond => 3,
        }
    }

    pub fn from_index(index: u8) -> Self {
        match index {
            0 => Self::Quarter,
            1 => Self::Eighth,
            3 => Self::ThirtySecond,
            _ => Self::Sixteenth,
        }
    }

    fn beats(self) -> f64 {
        match self {
            Self::Quarter => 1.0,
            Self::Eighth => 0.5,
            Self::Sixteenth => 0.25,
            Self::ThirtySecond => 0.125,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArpeggiatorSettings {
    pub enabled: bool,
    pub mode: ArpMode,
    pub rate: ArpRate,
    pub triplet: bool,
    /// Note length as a share of the step.
    pub gate_percent: u8,
    pub octaves: u8,
}

impl Default for ArpeggiatorSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: ArpMode::Up,
            rate: ArpRate::Sixteenth,
            triplet: false,
            gate_percent: 80,
            octaves: MIN_OCTAVES,
        }
    }
}

impl ArpeggiatorSettings {
    /// Settings held inside the ranges ADR-030 defines.
    pub fn clamped(mut self) -> Self {
        self.gate_percent = self.gate_percent.clamp(MIN_GATE_PERCENT, MAX_GATE_PERCENT);
        self.octaves = self.octaves.clamp(MIN_OCTAVES, MAX_OCTAVES);
        self
    }

    pub fn step_beats(&self) -> f64 {
        let beats = self.rate.beats();
        if self.triplet {
            beats * 2.0 / 3.0
        } else {
            beats
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_their_defaults() {
        let settings: ArpeggiatorSettings = serde_json::from_str(r#"{ "enabled": true }"#).unwrap();
        assert!(settings.enabled);
        assert_eq!(settings.mode, ArpMode::Up);
        assert_eq!(settings.rate, ArpRate::Sixteenth);
        assert_eq!(settings.gate_percent, 80);
        assert_eq!(settings.octaves, 1);
    }

    #[test]
    fn settings_are_held_in_range() {
        let settings = ArpeggiatorSettings {
            gate_percent: 0,
            octaves: 9,
            ..ArpeggiatorSettings::default()
        }
        .clamped();
        assert_eq!(settings.gate_percent, MIN_GATE_PERCENT);
        assert_eq!(settings.octaves, MAX_OCTAVES);
    }

    #[test]
    fn step_lengths_in_beats() {
        let step = |rate, triplet| {
            ArpeggiatorSettings {
                rate,
                triplet,
                ..ArpeggiatorSettings::default()
            }
            .step_beats()
        };
        assert_eq!(step(ArpRate::Quarter, false), 1.0);
        assert_eq!(step(ArpRate::Sixteenth, false), 0.25);
        assert_eq!(step(ArpRate::ThirtySecond, false), 0.125);
        assert!((step(ArpRate::Eighth, true) - 1.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn enum_indices_round_trip() {
        for mode in [
            ArpMode::Up,
            ArpMode::Down,
            ArpMode::UpDown,
            ArpMode::Played,
            ArpMode::Random,
        ] {
            assert_eq!(ArpMode::from_index(mode.index()), mode);
        }
        for rate in [
            ArpRate::Quarter,
            ArpRate::Eighth,
            ArpRate::Sixteenth,
            ArpRate::ThirtySecond,
        ] {
            assert_eq!(ArpRate::from_index(rate.index()), rate);
        }
    }
}
