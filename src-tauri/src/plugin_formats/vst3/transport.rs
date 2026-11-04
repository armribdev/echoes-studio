//! Engine transport → VST3 `ProcessContext`.

use crate::audio_graph::node::TransportInfo;
use vst3_ffi::ProcessContext;
use vst3_host::transport::{process_context, TransportState};

/// Built on the stack every block; no allocation.
pub fn vst3_process_context(transport: &TransportInfo) -> ProcessContext {
    let position = transport.musical_position();
    process_context(&TransportState {
        playing: transport.is_playing,
        sample_rate: transport.sample_rate as f64,
        position_samples: transport.position_samples as i64,
        position_beats: position.beats,
        bar_start_beats: position.bar_start_beats,
        tempo: transport.bpm as f64,
        time_signature_numerator: transport.time_signature.numerator as i32,
        time_signature_denominator: transport.time_signature.denominator as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::node::TimeSignature;
    use vst3_ffi::{
        CONTEXT_BAR_POSITION_VALID, CONTEXT_PLAYING, CONTEXT_PROJECT_TIME_MUSIC_VALID,
        CONTEXT_TEMPO_VALID, CONTEXT_TIME_SIG_VALID,
    };

    fn transport_at(
        seconds: f64,
        bpm: f32,
        time_signature: TimeSignature,
        is_playing: bool,
    ) -> TransportInfo {
        TransportInfo {
            is_playing,
            position_samples: (seconds * 48_000.0) as u64,
            position_seconds: seconds,
            bpm,
            time_signature,
            sample_rate: 48_000.0,
            block_size: 512,
        }
    }

    #[test]
    fn reports_position_in_three_four() {
        // 120 BPM → 2 beats per second; 3/4 bars are 3 beats long.
        let context =
            vst3_process_context(&transport_at(4.0, 120.0, TimeSignature::new(3, 4), true));
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
        let context =
            vst3_process_context(&transport_at(1.5, 90.0, TimeSignature::new(6, 8), false));
        assert_eq!(context.state & CONTEXT_PLAYING, 0);
        assert_eq!(context.tempo, 90.0);
        // 6/8 = 3 quarter notes per bar; 1.5 s at 90 BPM = 2.25 beats.
        assert_eq!(context.project_time_music, 2.25);
        assert_eq!(context.bar_position_music, 0.0);
        for flag in [
            CONTEXT_PROJECT_TIME_MUSIC_VALID,
            CONTEXT_BAR_POSITION_VALID,
            CONTEXT_TEMPO_VALID,
            CONTEXT_TIME_SIG_VALID,
        ] {
            assert_ne!(context.state & flag, 0);
        }
    }
}
