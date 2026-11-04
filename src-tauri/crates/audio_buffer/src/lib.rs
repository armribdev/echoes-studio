//! Audio buffer — planar (non-interleaved) layout.
//!
//! Each `AudioBuffer` stores `num_channels` independent channels, each
//! containing `num_frames` f32 samples.  The backing store is a single
//! `Vec<f32>` allocation split into equal-length per-channel slices:
//!
//!   data = [ ch0[0..frames] | ch1[0..frames] | … ]
//!
//! ## Real-time contract
//!
//! After construction all operations are allocation-free:
//!   - `clear`, `copy_from`, `mix_from`, `mono_to_stereo`  — O(frames × channels)
//!   - `channel`, `channel_mut` — O(1)
//!
//! Buffers are pre-allocated at graph compile time and reused across blocks.

/// Planar stereo-capable audio buffer.
///
/// Designed for exclusive ownership by a single DSP node.  The scheduler
/// hands mutable references to output buffers and immutable references to
/// input buffers; no buffer is aliased across a single node's process call.
pub struct AudioBuffer {
    /// Backing store:  channel 0 frames, channel 1 frames, …
    data: Vec<f32>,
    pub num_channels: usize,
    pub num_frames: usize,
}

impl AudioBuffer {
    /// Allocate a zeroed buffer.
    ///
    /// Called at graph compile time, **never** inside the audio callback.
    pub fn new(num_channels: usize, num_frames: usize) -> Self {
        assert!(
            num_channels > 0 && num_frames > 0,
            "AudioBuffer: dimensions must be > 0"
        );
        Self {
            data: vec![0.0_f32; num_channels * num_frames],
            num_channels,
            num_frames,
        }
    }

    /// A single-channel, single-frame silence buffer used as a no-op sentinel.
    pub fn silence() -> Self {
        Self::new(1, 1)
    }

    /// Immutable slice for channel `ch`.
    #[inline]
    pub fn channel(&self, ch: usize) -> &[f32] {
        let start = ch * self.num_frames;
        &self.data[start..start + self.num_frames]
    }

    /// Mutable slice for channel `ch`.
    #[inline]
    pub fn channel_mut(&mut self, ch: usize) -> &mut [f32] {
        let start = ch * self.num_frames;
        let end = start + self.num_frames;
        &mut self.data[start..end]
    }

    /// Return mutable slices for channels `a` and `b` simultaneously.
    ///
    /// Panics if `a == b` or either index is out of range.
    #[inline]
    pub fn two_channels_mut(&mut self, a: usize, b: usize) -> (&mut [f32], &mut [f32]) {
        assert_ne!(a, b, "two_channels_mut: channels must differ");
        assert!(
            a < self.num_channels && b < self.num_channels,
            "channel index out of range"
        );
        let frames = self.num_frames;
        if a < b {
            let (left, right) = self.data.split_at_mut(b * frames);
            (
                &mut left[a * frames..(a + 1) * frames],
                &mut right[..frames],
            )
        } else {
            let (left, right) = self.data.split_at_mut(a * frames);
            (
                &mut right[..frames],
                &mut left[b * frames..(b + 1) * frames],
            )
        }
    }

    /// Zero all samples.  No allocation.
    #[inline]
    pub fn clear(&mut self) {
        self.data.fill(0.0);
    }

    /// Copy samples from `src`.  Shapes must match.
    #[inline]
    pub fn copy_from(&mut self, src: &AudioBuffer) {
        debug_assert_eq!(
            self.num_channels, src.num_channels,
            "channel count mismatch in copy_from"
        );
        debug_assert_eq!(
            self.num_frames, src.num_frames,
            "frame count mismatch in copy_from"
        );
        self.data.copy_from_slice(&src.data);
    }

    /// Accumulate `gain * src` into `self` (wet/dry mix).  No allocation.
    #[cfg(any(test, feature = "test-support"))]
    #[inline]
    pub fn mix_from(&mut self, src: &AudioBuffer, gain: f32) {
        debug_assert_eq!(self.data.len(), src.data.len(), "mix_from: size mismatch");
        for (d, &s) in self.data.iter_mut().zip(src.data.iter()) {
            *d += s * gain;
        }
    }

    /// Scale all samples by `gain` in place.
    #[cfg(any(test, feature = "test-support"))]
    #[inline]
    pub fn apply_gain(&mut self, gain: f32) {
        for s in self.data.iter_mut() {
            *s *= gain;
        }
    }

    /// Duplicate channel 0 into all other channels (mono → stereo/multichannel).
    ///
    /// Operates entirely within `self.data`; no allocation.
    #[inline]
    pub fn mono_to_all_channels(&mut self) {
        if self.num_channels <= 1 {
            return;
        }
        let frames = self.num_frames;
        for ch in 1..self.num_channels {
            let (src_half, dst_half) = if ch * frames < frames {
                unreachable!()
            } else {
                self.data.split_at_mut(ch * frames)
            };
            let src = &src_half[0..frames];
            let dst = &mut dst_half[0..frames];
            dst.copy_from_slice(src);
        }
    }

    /// True if all samples are zero (useful for bypass / silence detection).
    #[inline]
    pub fn is_silent(&self) -> bool {
        self.data.iter().all(|&s| s == 0.0)
    }
}

impl std::fmt::Debug for AudioBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AudioBuffer {{ channels: {}, frames: {}, silent: {} }}",
            self.num_channels,
            self.num_frames,
            self.is_silent()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buffer_is_zeroed() {
        let buf = AudioBuffer::new(2, 4);
        assert!(buf.is_silent());
        assert_eq!(buf.channel(0), &[0.0, 0.0, 0.0, 0.0]);
        assert_eq!(buf.channel(1), &[0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn channels_are_independent_planar_slices() {
        let mut buf = AudioBuffer::new(2, 3);
        buf.channel_mut(0).copy_from_slice(&[1.0, 2.0, 3.0]);
        buf.channel_mut(1).copy_from_slice(&[4.0, 5.0, 6.0]);
        assert_eq!(buf.channel(0), &[1.0, 2.0, 3.0]);
        assert_eq!(buf.channel(1), &[4.0, 5.0, 6.0]);
    }

    #[test]
    fn two_channels_mut_writes_correctly_regardless_of_argument_order() {
        // (0, 1) and (1, 0) exercise the two different split_at_mut branches;
        // both must land on the same physical channel.
        let mut buf = AudioBuffer::new(2, 2);
        {
            let (a, b) = buf.two_channels_mut(0, 1);
            a.copy_from_slice(&[1.0, 2.0]);
            b.copy_from_slice(&[3.0, 4.0]);
        }
        assert_eq!(buf.channel(0), &[1.0, 2.0]);
        assert_eq!(buf.channel(1), &[3.0, 4.0]);

        let mut buf = AudioBuffer::new(2, 2);
        {
            let (b, a) = buf.two_channels_mut(1, 0);
            b.copy_from_slice(&[3.0, 4.0]);
            a.copy_from_slice(&[1.0, 2.0]);
        }
        assert_eq!(buf.channel(0), &[1.0, 2.0]);
        assert_eq!(buf.channel(1), &[3.0, 4.0]);
    }

    #[test]
    #[should_panic(expected = "must differ")]
    fn two_channels_mut_rejects_same_index() {
        let mut buf = AudioBuffer::new(2, 2);
        buf.two_channels_mut(0, 0);
    }

    #[test]
    fn clear_zeroes_all_samples() {
        let mut buf = AudioBuffer::new(2, 2);
        buf.channel_mut(0).copy_from_slice(&[1.0, 2.0]);
        buf.channel_mut(1).copy_from_slice(&[3.0, 4.0]);
        buf.clear();
        assert!(buf.is_silent());
    }

    #[test]
    fn copy_from_duplicates_contents() {
        let mut src = AudioBuffer::new(1, 3);
        src.channel_mut(0).copy_from_slice(&[1.0, -1.0, 0.5]);
        let mut dst = AudioBuffer::new(1, 3);
        dst.copy_from(&src);
        assert_eq!(dst.channel(0), src.channel(0));
    }

    #[test]
    fn mix_from_accumulates_scaled_signal() {
        let mut dst = AudioBuffer::new(1, 2);
        dst.channel_mut(0).copy_from_slice(&[1.0, 1.0]);
        let mut src = AudioBuffer::new(1, 2);
        src.channel_mut(0).copy_from_slice(&[2.0, -2.0]);
        dst.mix_from(&src, 0.5);
        assert_eq!(dst.channel(0), &[2.0, 0.0]);
    }

    #[test]
    fn apply_gain_scales_every_sample() {
        let mut buf = AudioBuffer::new(1, 3);
        buf.channel_mut(0).copy_from_slice(&[1.0, 2.0, -3.0]);
        buf.apply_gain(2.0);
        assert_eq!(buf.channel(0), &[2.0, 4.0, -6.0]);
    }

    #[test]
    fn mono_to_all_channels_broadcasts_channel_zero() {
        let mut buf = AudioBuffer::new(3, 2);
        buf.channel_mut(0).copy_from_slice(&[1.0, -1.0]);
        buf.mono_to_all_channels();
        assert_eq!(buf.channel(1), &[1.0, -1.0]);
        assert_eq!(buf.channel(2), &[1.0, -1.0]);
    }

    #[test]
    fn mono_to_all_channels_is_noop_for_single_channel() {
        let mut buf = AudioBuffer::new(1, 2);
        buf.channel_mut(0).copy_from_slice(&[1.0, -1.0]);
        buf.mono_to_all_channels();
        assert_eq!(buf.channel(0), &[1.0, -1.0]);
    }

    #[test]
    fn is_silent_detects_any_nonzero_sample() {
        let mut buf = AudioBuffer::new(1, 2);
        assert!(buf.is_silent());
        buf.channel_mut(0)[1] = f32::EPSILON;
        assert!(!buf.is_silent());
    }
}
