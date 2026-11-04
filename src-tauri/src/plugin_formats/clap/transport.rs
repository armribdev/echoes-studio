//! Engine transport → CLAP `clap_event_transport`.

use super::ffi::{transport_event, ClapEventTransport, TransportValues};
use crate::audio_graph::node::TransportInfo;

/// Built on the stack every block; no allocation.
pub fn clap_transport_event(transport: &TransportInfo) -> ClapEventTransport {
    let position = transport.musical_position();
    transport_event(&TransportValues {
        playing: transport.is_playing,
        position_beats: position.beats,
        position_seconds: transport.position_seconds,
        tempo: transport.bpm as f64,
        bar_start_beats: position.bar_start_beats,
        bar_number: position.bar_number,
        time_signature_numerator: transport.time_signature.numerator as u16,
        time_signature_denominator: transport.time_signature.denominator as u16,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::node::TimeSignature;
    use crate::plugin_formats::clap::ffi::{
        CLAP_EVENT_TRANSPORT, CLAP_TIME_FACTOR, CLAP_TRANSPORT_IS_PLAYING,
    };

    fn to_clap_time(value: f64) -> i64 {
        (value * CLAP_TIME_FACTOR).round() as i64
    }

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
        let event = clap_transport_event(&transport_at(4.0, 120.0, TimeSignature::new(3, 4), true));
        assert_eq!(event.song_pos_beats, to_clap_time(8.0));
        assert_eq!(event.bar_number, 2);
        assert_eq!(event.bar_start, to_clap_time(6.0));
        assert_eq!(event.song_pos_seconds, to_clap_time(4.0));
        assert_eq!(event.tempo, 120.0);
        assert_eq!((event.tsig_num, event.tsig_denom), (3, 4));
        assert_ne!(event.flags & CLAP_TRANSPORT_IS_PLAYING, 0);
    }

    #[test]
    fn eighth_note_denominator_shortens_bars() {
        // 6/8 = 3 quarter notes per bar.
        let event =
            clap_transport_event(&transport_at(1.5, 120.0, TimeSignature::new(6, 8), false));
        assert_eq!(event.bar_number, 1);
        assert_eq!(event.bar_start, to_clap_time(3.0));
        assert_eq!(event.flags & CLAP_TRANSPORT_IS_PLAYING, 0);
    }

    #[test]
    fn header_identifies_a_transport_event() {
        let event = clap_transport_event(&TransportInfo::stopped(48_000.0));
        assert_eq!(event.header.r#type, CLAP_EVENT_TRANSPORT);
        assert_eq!(
            event.header.size as usize,
            std::mem::size_of::<ClapEventTransport>()
        );
        assert_eq!(
            std::mem::size_of::<ClapEventTransport>(),
            104,
            "must match the C layout of clap_event_transport"
        );
    }
}
