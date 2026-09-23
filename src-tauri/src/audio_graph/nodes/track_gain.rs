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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::live_params::TrackParams;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::audio_graph::nodes::gain::{SmoothedGain, PARAM_GAIN};

pub struct TrackGainNode {
    id: NodeId,
    params: Arc<TrackParams>,
    any_solo: Arc<AtomicBool>,
    /// De-zippers volume modulation/automation and fader moves, and de-clicks
    /// mute/solo toggles (ADR-019).
    smoother: SmoothedGain,
}

impl TrackGainNode {
    pub fn new(id: NodeId, params: Arc<TrackParams>, any_solo: Arc<AtomicBool>) -> Self {
        let mut node = Self {
            id,
            params,
            any_solo,
            smoother: SmoothedGain::new(0.0),
        };
        node.smoother.snap_to(node.current_gain());
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
        self.smoother.prepare(ctx.sample_rate, self.current_gain());
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        let input = inputs[0];
        let output = &mut outputs[0];
        let frames = transport.block_size.min(output.num_frames).min(input.num_frames);
        let gain = self.current_gain();
        self.smoother.process(gain, input, output, frames);
    }

    fn reset(&mut self) {
        self.reset_modulation();
        self.smoother.snap_to(self.current_gain());
    }

    fn reset_modulation(&mut self) {
        self.smoother.clear_modulation();
    }

    fn param_id(&self, name: &str) -> Option<u32> {
        match name {
            "gain" => Some(PARAM_GAIN),
            _ => None,
        }
    }

    fn apply_modulation(&mut self, param_id: u32, delta: f32) {
        if param_id == PARAM_GAIN {
            self.smoother.add_block_start_delta(delta);
        }
    }

    fn apply_modulation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        if param_id == PARAM_GAIN {
            self.smoother.add_delta_at(sample_offset, value);
        }
    }
}
