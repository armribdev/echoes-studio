//! Shared atomic transport state.
//!
//! `SharedTransport` is the single point of truth for playback state that
//! must be readable from **both** the Tauri command thread (PLAY/STOP/SEEK)
//! and the CPAL audio callback, without any blocking synchronisation.
//!
//! ## Threading contract
//!
//! All fields use `Relaxed` ordering.  We only need *visibility* (the audio
//! thread will eventually see the new value within a handful of blocks) — we
//! do **not** need happens-before ordering between the command thread and the
//! audio thread for correctness.
//!
//! ## f32-in-AtomicU32 pattern
//!
//! `f32` has no atomic variant in `std`.  We store the bit-pattern as `u32`
//! and reinterpret with `f32::from_bits` / `f32::to_bits`.  This is safe
//! because we never read back a bit-pattern that was not written by us.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::audio_graph::node::{TimeSignature, TransportInfo};

/// Atomic transport state shared between the command thread and the CPAL callback.
pub struct SharedTransport {
    /// `true` while the engine is rendering audio.
    pub is_playing: AtomicBool,

    /// Playback cursor — absolute position in samples from the start of the
    /// project.  Incremented by the audio callback each block.
    pub position_samples: AtomicU64,

    /// `true` while MIDI is being recorded: playback then runs past the last
    /// clip instead of auto-stopping at the end of the timeline.
    recording: AtomicBool,

    /// Click mixed into the output while playing (monitoring only, never exported).
    metronome_enabled: AtomicBool,

    /// Samples of count-in still to play before the transport really starts.
    /// The position stays put and only the click sounds while it is non-zero.
    count_in_samples: AtomicU64,

    /// End of the timeline in samples (latest audio or MIDI clip end).  `0` means
    /// no clips → no auto-stop.
    /// Updated every time the graph is recompiled.
    pub duration_samples: AtomicU64,

    /// BPM stored as the bit-pattern of an `f32`.
    bpm_bits: AtomicU32,

    /// Time signature packed as `numerator << 8 | denominator`.
    time_signature_bits: AtomicU32,

    /// Device sample rate stored as the bit-pattern of an `f32`.
    sample_rate_bits: AtomicU32,

    /// When set, playback wraps from `loop_end_samples` back to `loop_start_samples`.
    loop_enabled: AtomicBool,
    loop_start_samples: AtomicU64,
    loop_end_samples: AtomicU64,
}

impl SharedTransport {
    pub fn new(sample_rate: f32, bpm: f32) -> Arc<Self> {
        Arc::new(Self {
            is_playing: AtomicBool::new(false),
            position_samples: AtomicU64::new(0),
            recording: AtomicBool::new(false),
            metronome_enabled: AtomicBool::new(false),
            count_in_samples: AtomicU64::new(0),
            duration_samples: AtomicU64::new(0),
            bpm_bits: AtomicU32::new(bpm.to_bits()),
            time_signature_bits: AtomicU32::new(pack_time_signature(TimeSignature::default())),
            sample_rate_bits: AtomicU32::new(sample_rate.to_bits()),
            loop_enabled: AtomicBool::new(false),
            loop_start_samples: AtomicU64::new(0),
            loop_end_samples: AtomicU64::new(0),
        })
    }

    #[inline]
    pub fn sample_rate(&self) -> f32 {
        f32::from_bits(self.sample_rate_bits.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn set_sample_rate(&self, sr: f32) {
        self.sample_rate_bits.store(sr.to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn bpm(&self) -> f32 {
        f32::from_bits(self.bpm_bits.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn time_signature(&self) -> TimeSignature {
        let bits = self.time_signature_bits.load(Ordering::Relaxed);
        TimeSignature::new((bits >> 8) as u8, bits as u8)
    }

    /// The two stores are independent; a block may briefly see a new tempo with
    /// the old signature, which only affects the bar positions sent to plugins.
    #[inline]
    pub fn set_tempo(&self, bpm: f32, time_signature: TimeSignature) {
        self.bpm_bits.store(bpm.to_bits(), Ordering::Relaxed);
        self.time_signature_bits
            .store(pack_time_signature(time_signature), Ordering::Relaxed);
    }

    #[inline]
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn set_recording(&self, recording: bool) {
        self.recording.store(recording, Ordering::Relaxed);
    }

    #[inline]
    pub fn metronome_enabled(&self) -> bool {
        self.metronome_enabled.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn set_metronome_enabled(&self, enabled: bool) {
        self.metronome_enabled.store(enabled, Ordering::Relaxed);
    }

    #[inline]
    pub fn count_in_remaining(&self) -> u64 {
        self.count_in_samples.load(Ordering::Relaxed)
    }

    /// Command thread, before `play`.
    pub fn start_count_in(&self, samples: u64) {
        self.count_in_samples.store(samples, Ordering::Relaxed);
    }

    pub fn cancel_count_in(&self) {
        self.count_in_samples.store(0, Ordering::Relaxed);
    }

    /// Audio callback, once per rendered count-in slice.
    #[inline]
    pub fn consume_count_in(&self, samples: u64) {
        let _ =
            self.count_in_samples
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                    Some(remaining.saturating_sub(samples))
                });
    }

    #[inline]
    pub fn set_duration(&self, samples: u64) {
        self.duration_samples.store(samples, Ordering::Relaxed);
    }

    /// Start playback from the current position.
    pub fn play(&self) {
        self.is_playing.store(true, Ordering::Relaxed);
    }

    /// Stop playback and reset the cursor to the beginning.
    pub fn stop(&self) {
        self.cancel_count_in();
        self.is_playing.store(false, Ordering::Relaxed);
        self.position_samples.store(0, Ordering::Relaxed);
    }

    /// Pause playback without resetting the cursor.
    pub fn pause(&self) {
        self.cancel_count_in();
        self.is_playing.store(false, Ordering::Relaxed);
    }

    /// Seek to an absolute position in seconds.
    pub fn seek(&self, position_seconds: f64) {
        let pos = (position_seconds * self.sample_rate() as f64) as u64;
        self.position_samples.store(pos, Ordering::Relaxed);
    }

    /// Enable or move the playback loop. Bounds are in seconds; a region shorter
    /// than one block is extended so the audio callback cannot spin.
    pub fn set_loop(&self, enabled: bool, start_seconds: f64, end_seconds: f64) {
        let sr = self.sample_rate() as f64;
        let start = (start_seconds.max(0.0) * sr).max(0.0) as u64;
        let mut end = (end_seconds.max(0.0) * sr).max(0.0) as u64;
        if end < start.saturating_add(MIN_LOOP_SAMPLES) {
            end = start + MIN_LOOP_SAMPLES;
        }
        self.loop_start_samples.store(start, Ordering::Relaxed);
        self.loop_end_samples.store(end, Ordering::Relaxed);
        self.loop_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Active loop bounds in samples, or `None` when looping is off or invalid.
    #[inline]
    pub fn loop_range(&self) -> Option<(u64, u64)> {
        if !self.loop_enabled.load(Ordering::Relaxed) {
            return None;
        }
        let start = self.loop_start_samples.load(Ordering::Relaxed);
        let end = self.loop_end_samples.load(Ordering::Relaxed);
        if end > start {
            Some((start, end))
        } else {
            None
        }
    }

    /// Snapshot the current transport state into a `TransportInfo` value.
    ///
    /// Called at the start of every audio block (inside the CPAL callback).
    /// No locking — pure atomic reads.
    #[inline]
    pub fn snapshot(&self, block_size: usize) -> TransportInfo {
        let sr = self.sample_rate();
        let pos = self.position_samples.load(Ordering::Relaxed);
        TransportInfo {
            is_playing: self.is_playing.load(Ordering::Relaxed),
            position_samples: pos,
            position_seconds: pos as f64 / sr as f64,
            bpm: self.bpm(),
            time_signature: self.time_signature(),
            sample_rate: sr,
            block_size,
        }
    }
}

fn pack_time_signature(time_signature: TimeSignature) -> u32 {
    (time_signature.numerator as u32) << 8 | time_signature.denominator as u32
}

/// Shortest loop the command thread will install. Keeps a callback from
/// splitting a hardware block into hundreds of one-sample renders.
const MIN_LOOP_SAMPLES: u64 = 256;

/// How the next render slice runs: a count-in slice keeps the transport still
/// (only the click sounds) and stops exactly where the count-in ends, so the
/// music starts on the sample.
#[derive(Debug, PartialEq)]
pub struct SlicePlan {
    pub playing: bool,
    pub counting_in: bool,
    pub max_frames: usize,
}

pub fn plan_slice(running: bool, count_in_remaining: u64, frames_left: usize) -> SlicePlan {
    let counting_in = running && count_in_remaining > 0;
    let max_frames = if counting_in {
        frames_left.min(count_in_remaining.min(usize::MAX as u64) as usize)
    } else {
        frames_left
    };
    SlicePlan {
        playing: running && !counting_in,
        counting_in,
        max_frames,
    }
}

/// Playback position and frame count for the next render slice.
///
/// When `range` is set and playback has reached `end`, the position wraps to
/// `start` first. The slice then stops at the loop end so the caller can
/// render the remainder from the start in a following call.
pub fn playback_chunk(
    position: u64,
    frames: usize,
    playing: bool,
    range: Option<(u64, u64)>,
) -> (u64, usize) {
    if frames == 0 || !playing {
        return (position, frames);
    }
    let Some((start, end)) = range.filter(|(start, end)| end > start) else {
        return (position, frames);
    };
    let len = end - start;
    let position = if position >= end {
        start + (position - end) % len
    } else {
        position
    };
    let until_end = end - position;
    let chunk = if until_end >= frames as u64 {
        frames
    } else {
        until_end as usize
    };
    (position, chunk.max(1).min(frames))
}

#[cfg(test)]
mod tests {
    use super::{plan_slice, playback_chunk, SharedTransport, SlicePlan};

    #[test]
    fn count_in_slice_stops_where_the_count_in_ends() {
        let plan = plan_slice(true, 100, 256);
        assert_eq!(
            plan,
            SlicePlan {
                playing: false,
                counting_in: true,
                max_frames: 100
            }
        );
    }

    #[test]
    fn count_in_longer_than_the_block_takes_the_whole_block() {
        assert_eq!(plan_slice(true, 1000, 256).max_frames, 256);
    }

    #[test]
    fn without_count_in_the_slice_plays() {
        let plan = plan_slice(true, 0, 256);
        assert!(plan.playing && !plan.counting_in);
        assert_eq!(plan.max_frames, 256);
    }

    #[test]
    fn stopped_transport_ignores_a_pending_count_in() {
        let plan = plan_slice(false, 100, 256);
        assert!(!plan.playing && !plan.counting_in);
        assert_eq!(plan.max_frames, 256);
    }

    #[test]
    fn consuming_more_than_remaining_stops_at_zero() {
        let transport = SharedTransport::new(48_000.0, 120.0);
        transport.start_count_in(100);
        transport.consume_count_in(60);
        assert_eq!(transport.count_in_remaining(), 40);
        transport.consume_count_in(60);
        assert_eq!(transport.count_in_remaining(), 0);
    }

    #[test]
    fn stop_and_pause_cancel_the_count_in() {
        let transport = SharedTransport::new(48_000.0, 120.0);
        transport.start_count_in(100);
        transport.pause();
        assert_eq!(transport.count_in_remaining(), 0);
        transport.start_count_in(100);
        transport.stop();
        assert_eq!(transport.count_in_remaining(), 0);
    }

    #[test]
    fn no_loop_consumes_the_whole_block() {
        assert_eq!(playback_chunk(100, 64, true, None), (100, 64));
    }

    #[test]
    fn stopped_transport_does_not_wrap() {
        assert_eq!(playback_chunk(500, 64, false, Some((0, 200))), (500, 64));
    }

    #[test]
    fn chunk_stops_at_the_loop_end() {
        assert_eq!(playback_chunk(180, 64, true, Some((100, 200))), (180, 20));
    }

    #[test]
    fn position_past_the_end_wraps_before_the_chunk() {
        assert_eq!(playback_chunk(250, 64, true, Some((100, 200))), (150, 50));
    }

    #[test]
    fn playhead_before_the_region_runs_until_the_end() {
        assert_eq!(playback_chunk(0, 64, true, Some((100, 200))), (0, 64));
    }
}
