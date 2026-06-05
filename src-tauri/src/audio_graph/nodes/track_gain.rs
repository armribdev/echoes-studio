//! Track gain node — applies volume + mute/solo gating for one track.
//!
//! ## Why it differs from `GainNode`
//!
//! `GainNode` holds a constant baked at compile time.  Turning a volume knob
//! therefore required a full graph recompile (reallocating reverbs, resetting
//! envelopes, cloning every clip buffer — see audit C3 / I1).
//!
//! `TrackGainNode` reads its parameters from `Arc<TrackParams>` cells held in
//! the `LiveParams` registry.  Volume, mute, solo, and the global `any_solo`
//! flag are loaded once per block and used to derive the effective linear
//! gain.  The command thread can write these atomics directly without
//! triggering a recompile.

use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::live_params::TrackParams;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};

/// Per-sample gain-smoothing cutoff (ADR-019 Tranche 2).  De-zippers volume
/// modulation/automation and fader moves, and de-clicks mute/solo toggles.
/// ~12 Hz ≈ 13 ms; derived from the sample rate only (block-size independent).
const GAIN_SMOOTH_CUTOFF_HZ: f32 = 12.0;

pub struct TrackGainNode {
    id: NodeId,
    params: Arc<TrackParams>,
    any_solo: Arc<AtomicBool>,
    /// Accumulated modulation delta for the current block (reset each block).
    mod_delta: f32,
    /// Per-sample smoothed gain; ramps toward `current_gain() + mod_delta`.
    smoothed_gain: f32,
    /// One-pole coefficient set in `prepare` from the sample rate.
    gain_coeff: f32,
}

impl TrackGainNode {
    pub fn new(id: NodeId, params: Arc<TrackParams>, any_solo: Arc<AtomicBool>) -> Self {
        let mut node = Self {
            id,
            params,
            any_solo,
            mod_delta: 0.0,
            smoothed_gain: 0.0,
            gain_coeff: 0.0,
        };
        node.smoothed_gain = node.current_gain();
        node
    }

    /// Compute the linear gain for this block from the atomic cells.
    ///
    /// Logic: `volume × mute_gate × solo_gate`.
    /// - mute_gate = 0 if muted, else 1.
    /// - solo_gate = 1 if no track is solo'd, or this track is solo'd; else 0.
    #[inline]
    fn current_gain(&self) -> f32 {
        let volume = self.params.volume();
        if self.params.muted() {
            return 0.0;
        }
        if self.any_solo.load(Ordering::Relaxed) && !self.params.solo() {
            return 0.0;
        }
        volume
    }
}

impl AudioNode for TrackGainNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "TrackGainNode"
    }
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.gain_coeff = 1.0 - (-TAU * GAIN_SMOOTH_CUTOFF_HZ / ctx.sample_rate).exp();
        self.smoothed_gain = self.current_gain();
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        let input = &inputs[0];
        let output = &mut outputs[0];

        let channels = output.num_channels.min(input.num_channels);
        let frames = transport.block_size.min(output.num_frames).min(input.num_frames);
        let target = (self.current_gain() + self.mod_delta).max(0.0);
        let coeff = self.gain_coeff;

        // Frame-outer so the smoothed gain advances once per frame, shared across
        // channels.  When already at target (static volume, no mod) the step is
        // exactly 0, so a static track stays bit-identical.
        for i in 0..frames {
            self.smoothed_gain += coeff * (target - self.smoothed_gain);
            let g = self.smoothed_gain;
            for ch in 0..channels {
                output.channel_mut(ch)[i] = input.channel(ch)[i] * g;
            }
        }
    }

    fn reset(&mut self) {
        self.reset_modulation();
        self.smoothed_gain = self.current_gain();
    }

    fn reset_modulation(&mut self) {
        self.mod_delta = 0.0;
    }

    fn param_id(&self, name: &str) -> Option<u32> {
        match name {
            "gain" => Some(PARAM_GAIN),
            _ => None,
        }
    }

    fn apply_modulation(&mut self, param_id: u32, delta: f32) {
        if param_id == PARAM_GAIN {
            self.mod_delta += delta;
        }
    }
}

const PARAM_GAIN: u32 = 0;
