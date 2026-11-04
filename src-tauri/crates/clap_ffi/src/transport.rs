//! The host transport as a `clap_event_transport`, from plain values.
//!
//! One formula for the engine (CLAP in the Echoes process) and for the bridge's
//! CLAP adapter, which receives the same values over the wire.

use crate::{
    ClapEventHeader, ClapEventTransport, CLAP_CORE_EVENT_SPACE, CLAP_EVENT_TRANSPORT,
    CLAP_TIME_FACTOR, CLAP_TRANSPORT_HAS_BEATS_TIMELINE, CLAP_TRANSPORT_HAS_SECONDS_TIMELINE,
    CLAP_TRANSPORT_HAS_TEMPO, CLAP_TRANSPORT_HAS_TIME_SIGNATURE, CLAP_TRANSPORT_IS_PLAYING,
};

/// Where the host transport is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportValues {
    pub playing: bool,
    pub position_beats: f64,
    pub position_seconds: f64,
    pub tempo: f64,
    pub bar_start_beats: f64,
    pub bar_number: i32,
    pub time_signature_numerator: u16,
    pub time_signature_denominator: u16,
}

/// Built on the stack every block; no allocation.
pub fn transport_event(values: &TransportValues) -> ClapEventTransport {
    let mut flags = CLAP_TRANSPORT_HAS_TEMPO
        | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
        | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE
        | CLAP_TRANSPORT_HAS_TIME_SIGNATURE;
    if values.playing {
        flags |= CLAP_TRANSPORT_IS_PLAYING;
    }
    ClapEventTransport {
        header: ClapEventHeader {
            size: std::mem::size_of::<ClapEventTransport>() as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE,
            r#type: CLAP_EVENT_TRANSPORT,
            flags: 0,
        },
        flags,
        song_pos_beats: to_clap_time(values.position_beats),
        song_pos_seconds: to_clap_time(values.position_seconds),
        tempo: values.tempo,
        tempo_inc: 0.0,
        loop_start_beats: 0,
        loop_end_beats: 0,
        loop_start_seconds: 0,
        loop_end_seconds: 0,
        bar_start: to_clap_time(values.bar_start_beats),
        bar_number: values.bar_number,
        tsig_num: values.time_signature_numerator,
        tsig_denom: values.time_signature_denominator,
    }
}

fn to_clap_time(value: f64) -> i64 {
    (value * CLAP_TIME_FACTOR).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(playing: bool) -> TransportValues {
        TransportValues {
            playing,
            position_beats: 8.0,
            position_seconds: 4.0,
            tempo: 120.0,
            bar_start_beats: 6.0,
            bar_number: 2,
            time_signature_numerator: 3,
            time_signature_denominator: 4,
        }
    }

    #[test]
    fn the_event_carries_the_values_in_clap_units() {
        let event = transport_event(&values(true));
        assert_eq!(event.song_pos_beats, to_clap_time(8.0));
        assert_eq!(event.song_pos_seconds, to_clap_time(4.0));
        assert_eq!(event.bar_start, to_clap_time(6.0));
        assert_eq!(event.bar_number, 2);
        assert_eq!(event.tempo, 120.0);
        assert_eq!((event.tsig_num, event.tsig_denom), (3, 4));
        assert_ne!(event.flags & CLAP_TRANSPORT_IS_PLAYING, 0);
        assert_eq!(
            transport_event(&values(false)).flags & CLAP_TRANSPORT_IS_PLAYING,
            0
        );
    }

    #[test]
    fn the_header_identifies_a_transport_event() {
        let event = transport_event(&values(false));
        assert_eq!(event.header.r#type, CLAP_EVENT_TRANSPORT);
        assert_eq!(
            event.header.size as usize,
            std::mem::size_of::<ClapEventTransport>()
        );
        assert_eq!(std::mem::size_of::<ClapEventTransport>(), 104);
    }
}
