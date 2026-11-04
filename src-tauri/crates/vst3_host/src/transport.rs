//! Host transport → VST3 `ProcessContext`.

use vst3_ffi::{
    ProcessContext, CONTEXT_BAR_POSITION_VALID, CONTEXT_PLAYING, CONTEXT_PROJECT_TIME_MUSIC_VALID,
    CONTEXT_TEMPO_VALID, CONTEXT_TIME_SIG_VALID,
};

/// Where the host transport is, in the units VST3 wants. The engine (and, later, the
/// plugin bridge) fills it; this crate knows nothing of the engine's own transport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportState {
    pub playing: bool,
    pub sample_rate: f64,
    pub position_samples: i64,
    /// Musical position in quarter notes.
    pub position_beats: f64,
    /// Position of the start of the current bar, in quarter notes.
    pub bar_start_beats: f64,
    pub tempo: f64,
    pub time_signature_numerator: i32,
    pub time_signature_denominator: i32,
}

/// Built on the stack every block; no allocation.  Cycle, system time, SMPTE
/// and clock fields stay unset (their `*_VALID` flags are off).
pub fn process_context(transport: &TransportState) -> ProcessContext {
    let mut state = CONTEXT_PROJECT_TIME_MUSIC_VALID
        | CONTEXT_BAR_POSITION_VALID
        | CONTEXT_TEMPO_VALID
        | CONTEXT_TIME_SIG_VALID;
    if transport.playing {
        state |= CONTEXT_PLAYING;
    }
    ProcessContext {
        state,
        sample_rate: transport.sample_rate,
        project_time_samples: transport.position_samples,
        project_time_music: transport.position_beats,
        bar_position_music: transport.bar_start_beats,
        tempo: transport.tempo,
        time_sig_numerator: transport.time_signature_numerator,
        time_sig_denominator: transport.time_signature_denominator,
        ..ProcessContext::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    fn state(playing: bool) -> TransportState {
        TransportState {
            playing,
            sample_rate: 48_000.0,
            position_samples: 192_000,
            position_beats: 8.0,
            bar_start_beats: 6.0,
            tempo: 120.0,
            time_signature_numerator: 3,
            time_signature_denominator: 4,
        }
    }

    #[test]
    fn the_context_carries_the_transport() {
        let context = process_context(&state(true));
        assert_eq!(context.project_time_music, 8.0);
        assert_eq!(context.bar_position_music, 6.0);
        assert_eq!(context.project_time_samples, 192_000);
        assert_eq!(context.tempo, 120.0);
        assert_eq!(context.sample_rate, 48_000.0);
        assert_eq!(
            (context.time_sig_numerator, context.time_sig_denominator),
            (3, 4)
        );
        assert_ne!(context.state & CONTEXT_PLAYING, 0);
    }

    #[test]
    fn a_stopped_transport_still_reports_tempo_and_position() {
        let context = process_context(&state(false));
        assert_eq!(context.state & CONTEXT_PLAYING, 0);
        assert_eq!(context.tempo, 120.0);
        for flag in [
            CONTEXT_PROJECT_TIME_MUSIC_VALID,
            CONTEXT_BAR_POSITION_VALID,
            CONTEXT_TEMPO_VALID,
            CONTEXT_TIME_SIG_VALID,
        ] {
            assert_ne!(context.state & flag, 0);
        }
    }

    #[test]
    fn matches_the_c_layout() {
        assert_eq!(offset_of!(ProcessContext, sample_rate), 8);
        assert_eq!(offset_of!(ProcessContext, project_time_samples), 16);
        assert_eq!(offset_of!(ProcessContext, project_time_music), 40);
        assert_eq!(offset_of!(ProcessContext, tempo), 72);
        assert_eq!(offset_of!(ProcessContext, time_sig_numerator), 80);
        assert_eq!(offset_of!(ProcessContext, chord), 88);
        assert_eq!(offset_of!(ProcessContext, frame_rate), 96);
        assert_eq!(offset_of!(ProcessContext, samples_to_next_clock), 104);
    }
}
