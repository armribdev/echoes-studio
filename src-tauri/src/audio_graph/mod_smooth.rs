use std::f32::consts::PI;

const SMOOTHING_CUTOFF_HZ: f32 = 10.0;

/// Block-rate one-pole smoother for modulation deltas.
///
/// Eliminates zipper noise when a modulation source changes value between blocks.
/// All state is fixed-size stack data — no heap allocation, RT-safe.
pub struct ModSmooth<const N: usize> {
    smoothed: [f32; N],
    target: [f32; N],
    coeff: f32,
}

impl<const N: usize> ModSmooth<N> {
    pub fn new() -> Self {
        Self {
            smoothed: [0.0; N],
            target: [0.0; N],
            coeff: 0.0,
        }
    }

    pub fn prepare(&mut self, sample_rate: f32, block_size: usize) {
        let block_rate = sample_rate / block_size.max(1) as f32;
        self.coeff = 1.0 - (-2.0 * PI * SMOOTHING_CUTOFF_HZ / block_rate).exp();
    }

    /// Reset the accumulation target for this block (called by `reset_modulation`).
    /// Smoothed values continue tracking toward the new target.
    pub fn reset_target(&mut self) {
        self.target = [0.0; N];
    }

    /// Accumulate a modulation delta into the target (called by `apply_modulation`).
    pub fn add_target(&mut self, idx: usize, delta: f32) {
        if idx < N {
            self.target[idx] += delta;
        }
    }

    /// Advance smoothed values one step toward the current targets and return a copy.
    /// Returning by value avoids borrow conflicts when the caller needs `&mut self` after.
    pub fn advance(&mut self) -> [f32; N] {
        for i in 0..N {
            self.smoothed[i] += self.coeff * (self.target[i] - self.smoothed[i]);
        }
        self.smoothed
    }

    /// Hard reset: zero both smoothed and target (called on graph stop/seek).
    pub fn reset(&mut self) {
        self.smoothed = [0.0; N];
        self.target = [0.0; N];
    }
}

impl<const N: usize> Default for ModSmooth<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoothing_converges_to_target() {
        let mut s = ModSmooth::<4>::new();
        s.prepare(48_000.0, 512);

        for _ in 0..500 {
            s.reset_target();
            s.add_target(0, 1.0);
            s.advance();
        }
        let out = s.advance();
        assert!((out[0] - 1.0).abs() < 1e-4, "must converge to target 1.0");
        assert_eq!(out[1], 0.0);
    }

    #[test]
    fn smoothing_is_non_instantaneous() {
        let mut s = ModSmooth::<2>::new();
        s.prepare(48_000.0, 512);

        s.add_target(0, 1.0);
        let out = s.advance();
        assert!(out[0] > 0.0 && out[0] < 1.0, "first block must be a partial step");
    }

    #[test]
    fn decays_to_zero_when_target_cleared() {
        let mut s = ModSmooth::<2>::new();
        s.prepare(48_000.0, 512);

        for _ in 0..500 {
            s.reset_target();
            s.add_target(0, 1.0);
            s.advance();
        }
        for _ in 0..500 {
            s.reset_target();
            s.advance();
        }
        let out = s.advance();
        assert!(out[0].abs() < 1e-4, "must decay to zero after target cleared");
    }

    #[test]
    fn hard_reset_zeroes_smoothed_state() {
        let mut s = ModSmooth::<3>::new();
        s.prepare(48_000.0, 512);
        s.add_target(0, 5.0);
        s.advance();
        s.reset();
        let out = s.advance();
        assert_eq!(out[0], 0.0, "hard reset must zero smoothed");
    }
}
