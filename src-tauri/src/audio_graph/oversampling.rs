//! 2× oversampling for nonlinear / steep-filter DSP paths.
//!
//! Upsamples with linear interpolation, applies a one-pole anti-imaging low-pass,
//! runs the caller's per-sample processor at 2× rate, then anti-alias filters and
//! decimates back to base rate. All scratch is allocated in [`Oversample2x::prepare`].

/// Oversampling factor (fixed at 2× for CPU / quality balance).
pub const OVERSAMPLE_FACTOR: usize = 2;

/// One-pole coefficient target: ~0.43 × base Nyquist (anti-alias margin at 2× rate).
const LP_CUTOFF_RATIO: f32 = 0.43;

#[inline]
fn one_pole_lp(x: f32, z: &mut f32, coeff: f32) -> f32 {
    *z = coeff * *z + (1.0 - coeff) * x;
    *z
}

/// Two cascaded one-poles (~12 dB/oct) for stronger anti-imaging / anti-alias.
#[inline]
fn one_pole_lp2(x: f32, z: &mut [f32; 2], coeff: f32) -> f32 {
    let a = one_pole_lp(x, &mut z[0], coeff);
    one_pole_lp(a, &mut z[1], coeff)
}

/// Pre-allocated 2× oversampler for up to two channels.
pub struct Oversample2x {
    max_os_frames: usize,
    scratch: [Vec<f32>; 2],
    prev_in: [f32; 2],
    lp_up_z: [[f32; 2]; 2],
    lp_down_z: [[f32; 2]; 2],
    lp_coeff: f32,
}

impl Default for Oversample2x {
    fn default() -> Self {
        Self {
            max_os_frames: 0,
            scratch: [Vec::new(), Vec::new()],
            prev_in: [0.0; 2],
            lp_up_z: [[0.0; 2]; 2],
            lp_down_z: [[0.0; 2]; 2],
            lp_coeff: 0.0,
        }
    }
}

impl Oversample2x {
    pub fn prepare(&mut self, max_block_size: usize, sample_rate: f32) {
        self.max_os_frames = max_block_size * OVERSAMPLE_FACTOR;
        for buf in &mut self.scratch {
            buf.resize(self.max_os_frames, 0.0);
        }
        let sr_os = sample_rate * OVERSAMPLE_FACTOR as f32;
        let fc = sample_rate * LP_CUTOFF_RATIO;
        self.lp_coeff = (-2.0 * std::f32::consts::PI * fc / sr_os).exp();
    }

    pub fn reset(&mut self) {
        self.prev_in = [0.0; 2];
        self.lp_up_z = [[0.0; 2]; 2];
        self.lp_down_z = [[0.0; 2]; 2];
        for buf in &mut self.scratch {
            buf.fill(0.0);
        }
    }

    /// Process one channel: `input[0..frames]` → `output[0..frames]` via 2× oversampling.
    #[inline]
    pub fn process_channel<F>(
        &mut self,
        ch: usize,
        input: &[f32],
        output: &mut [f32],
        frames: usize,
        mut sample_fn: F,
    ) where
        F: FnMut(f32) -> f32,
    {
        debug_assert!(ch < 2);
        debug_assert!(frames <= output.len() && frames * OVERSAMPLE_FACTOR <= self.max_os_frames);

        let os = &mut self.scratch[ch];
        let a = self.lp_coeff;
        let mut prev = self.prev_in[ch];
        let mut up_z = self.lp_up_z[ch];
        let mut down_z = self.lp_down_z[ch];

        for i in 0..frames {
            let x = input[i];
            let mid = 0.5 * (prev + x);
            let mut s0 = one_pole_lp2(mid, &mut up_z, a);
            let mut s1 = one_pole_lp2(x, &mut up_z, a);
            s0 = sample_fn(s0);
            s1 = sample_fn(s1);
            os[i * 2] = s0;
            os[i * 2 + 1] = s1;
            prev = x;
        }
        self.prev_in[ch] = prev;
        self.lp_up_z[ch] = up_z;

        for i in 0..frames {
            let y0 = one_pole_lp2(os[i * 2], &mut down_z, a);
            let y1 = one_pole_lp2(os[i * 2 + 1], &mut down_z, a);
            output[i] = 0.5 * (y0 + y1);
        }
        self.lp_down_z[ch] = down_z;
    }

    /// In-place variant (same slice for input and output).
    pub fn process_inplace<F>(&mut self, ch: usize, data: &mut [f32], frames: usize, mut sample_fn: F)
    where
        F: FnMut(f32) -> f32,
    {
        debug_assert!(ch < 2);
        debug_assert!(frames <= data.len() && frames * OVERSAMPLE_FACTOR <= self.max_os_frames);

        let os = &mut self.scratch[ch];
        let a = self.lp_coeff;
        let mut prev = self.prev_in[ch];
        let mut up_z = self.lp_up_z[ch];
        let mut down_z = self.lp_down_z[ch];

        for i in 0..frames {
            let x = data[i];
            let mid = 0.5 * (prev + x);
            let mut s0 = one_pole_lp2(mid, &mut up_z, a);
            let mut s1 = one_pole_lp2(x, &mut up_z, a);
            s0 = sample_fn(s0);
            s1 = sample_fn(s1);
            os[i * 2] = s0;
            os[i * 2 + 1] = s1;
            prev = x;
        }
        self.prev_in[ch] = prev;
        self.lp_up_z[ch] = up_z;

        for i in 0..frames {
            let y0 = one_pole_lp2(os[i * 2], &mut down_z, a);
            let y1 = one_pole_lp2(os[i * 2 + 1], &mut down_z, a);
            data[i] = 0.5 * (y0 + y1);
        }
        self.lp_down_z[ch] = down_z;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversample_passthrough_near_unity() {
        let mut os = Oversample2x::default();
        os.prepare(64, 48_000.0);
        let input = [0.25_f32; 32];
        let mut output = [0.0_f32; 32];
        // Warm up filter state for a DC plateau.
        os.process_channel(0, &input, &mut output, 32, |x| x);
        os.process_channel(0, &input, &mut output, 32, |x| x);
        for (i, o) in output.iter().enumerate().skip(4) {
            assert!(
                (o - input[i]).abs() < 0.03,
                "frame {i}: expected ~{}, got {o}",
                input[i]
            );
        }
    }

    #[test]
    fn oversample_reduces_hard_clip_aliasing_energy() {
        let mut os = Oversample2x::default();
        os.prepare(128, 48_000.0);
        let sr = 48_000.0f32;
        let freq = 12_000.0;
        let frames = 128usize;
        let mut direct = vec![0.0_f32; frames];
        let mut oversampled = vec![0.0_f32; frames];
        for i in 0..frames {
            let t = i as f32 / sr;
            let x = (2.0 * std::f32::consts::PI * freq * t).sin() * 2.0;
            direct[i] = x.clamp(-1.0, 1.0);
        }
        let input: Vec<f32> = (0..frames)
            .map(|i| {
                let t = i as f32 / sr;
                (2.0 * std::f32::consts::PI * freq * t).sin() * 2.0
            })
            .collect();
        os.process_channel(0, &input, &mut oversampled, frames, |x| x.clamp(-1.0, 1.0));

        fn hf_energy(buf: &[f32]) -> f32 {
            let mut e = 0.0_f32;
            for w in buf.windows(2) {
                let d = w[1] - w[0];
                e += d * d;
            }
            e / buf.len() as f32
        }
        assert!(
            hf_energy(&oversampled) <= hf_energy(&direct),
            "2× OS should not increase high-frequency foldback from hard clip"
        );
    }
}
