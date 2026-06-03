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
        AudioBuffer {
            data: vec![0.0_f32; num_channels * num_frames],
            num_channels,
            num_frames,
        }
    }

    /// A single-channel, single-frame silence buffer used as a no-op sentinel.
    pub fn silence() -> Self {
        AudioBuffer::new(1, 1)
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
    #[inline]
    pub fn mix_from(&mut self, src: &AudioBuffer, gain: f32) {
        debug_assert_eq!(self.data.len(), src.data.len(), "mix_from: size mismatch");
        for (d, &s) in self.data.iter_mut().zip(src.data.iter()) {
            *d += s * gain;
        }
    }

    /// Scale all samples by `gain` in place.
    #[inline]
    pub fn apply_gain(&mut self, gain: f32) {
        for s in self.data.iter_mut() {
            *s *= gain;
        }
    }

    /// Duplicate channel 0 into all other channels (mono → stereo/multichannel).
    ///
    /// Operates entirely within `self.data`; no allocation.
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
