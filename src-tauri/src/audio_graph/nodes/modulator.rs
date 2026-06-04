//! Modulator nodes — produce normalised (−1…+1) modulation signals.
//!
//! These nodes have zero audio inputs and one audio output.  The output buffer
//! carries the raw modulation curve for one block; the scheduler reads it and
//! delivers scaled deltas to target nodes via `apply_modulation`.

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use std::sync::atomic::Ordering;

use crate::audio_graph::live_params::{EnvelopeCells, MacroCells, ModulatorCells};
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::project::LfoWaveform;

/// LFO with selectable waveform.  Output is normalised to −1…+1.
///
/// Phase accumulates across blocks; `reset()` returns it to 0.
///
/// `rate_cells` lets the command thread retune the LFO without triggering a
/// graph recompile.  When `None`, falls back to the constant `rate_hz` baked
/// at construction time (used by placeholder modulators).
pub struct LfoNode {
    id: NodeId,
    /// Fallback rate used when `rate_cells` is `None` (placeholder / test nodes).
    rate_hz: f32,
    rate_cells: Option<Arc<ModulatorCells>>,
    waveform: LfoWaveform,
    phase: f32,
    /// Tracks the previous block's `is_playing` to detect transport start.
    was_playing: bool,
}

impl LfoNode {
    pub fn new(id: NodeId, rate_hz: f32, waveform: LfoWaveform) -> Self {
        Self {
            id,
            rate_hz,
            rate_cells: None,
            waveform,
            phase: 0.0,
            was_playing: false,
        }
    }

    pub fn with_rate_cells(mut self, cells: Arc<ModulatorCells>) -> Self {
        self.rate_cells = Some(cells);
        self
    }

    #[inline]
    fn sample(phase: f32, waveform: &LfoWaveform) -> f32 {
        match waveform {
            LfoWaveform::Sine => (phase * std::f32::consts::TAU).sin(),
            // Triangle: 0 → +1 → 0 → -1 → 0 over one cycle
            LfoWaveform::Triangle => {
                if phase < 0.25 {
                    4.0 * phase
                } else if phase < 0.75 {
                    2.0 - 4.0 * phase
                } else {
                    4.0 * phase - 4.0
                }
            }
            // Rising sawtooth: −1 → +1
            LfoWaveform::Saw => 2.0 * phase - 1.0,
            // Square: +1 for first half, −1 for second half
            LfoWaveform::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
}

/// ADSR envelope modulator.  Output is in 0…1.
///
/// The gate signal comes from the `InstrumentNode` on the same track via
/// `EnvelopeCells::gate` (written on the audio thread at block boundaries).
/// All ADSR params are hot-editable through `EnvelopeCells` atomics.
pub struct EnvelopeNode {
    id: NodeId,
    cells: Arc<EnvelopeCells>,
    phase: AdsrPhase,
    /// Current envelope amplitude (0..1).  Audio-thread-exclusive — no atomic needed.
    level: f32,
    gate_was_on: bool,
    /// Release rate (level/sample) captured at the gate falling edge.
    release_rate: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum AdsrPhase {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

impl EnvelopeNode {
    pub fn new(id: NodeId, cells: Arc<EnvelopeCells>) -> Self {
        Self {
            id,
            cells,
            phase: AdsrPhase::Idle,
            level: 0.0,
            gate_was_on: false,
            release_rate: 0.0,
        }
    }
}

impl AudioNode for EnvelopeNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "Envelope"
    }
    fn num_input_ports(&self) -> usize {
        0
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, _ctx: PrepareContext) {}

    fn process(
        &mut self,
        _inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        if outputs.is_empty() {
            return;
        }
        let output = &mut outputs[0];
        let block_size = transport.block_size.min(output.num_frames);
        let sr = transport.sample_rate;

        // Read gate + ADSR params (block-rate, Relaxed — no ordering guarantee needed).
        let gate_on = self.cells.gate.load(Ordering::Relaxed);
        let attack_ms = self.cells.attack_ms_value();
        let decay_ms = self.cells.decay_ms_value();
        let sustain = self.cells.sustain_value().clamp(0.0, 1.0);
        let release_ms = self.cells.release_ms_value();

        // Derived rates (per-sample increments/decrements).
        let attack_rate = if attack_ms > 0.0 {
            1.0 / (attack_ms * 0.001 * sr)
        } else {
            f32::INFINITY
        };
        let decay_rate = if decay_ms > 0.0 {
            (1.0 - sustain) / (decay_ms * 0.001 * sr)
        } else {
            f32::INFINITY
        };

        // Detect block-boundary gate transitions.
        if gate_on && !self.gate_was_on {
            self.phase = AdsrPhase::Attack;
            self.level = 0.0;
        } else if !gate_on && self.gate_was_on {
            let release_samples = (release_ms * 0.001 * sr).max(1.0);
            self.release_rate = self.level / release_samples;
            self.phase = AdsrPhase::Release;
        }
        self.gate_was_on = gate_on;

        let ch0 = output.channel_mut(0);
        for i in 0..block_size {
            match self.phase {
                AdsrPhase::Idle => {
                    self.level = 0.0;
                }
                AdsrPhase::Attack => {
                    self.level += attack_rate;
                    if self.level >= 1.0 {
                        self.level = 1.0;
                        self.phase = AdsrPhase::Decay;
                    }
                }
                AdsrPhase::Decay => {
                    self.level -= decay_rate;
                    if self.level <= sustain {
                        self.level = sustain;
                        self.phase = AdsrPhase::Sustain;
                    }
                }
                AdsrPhase::Sustain => {
                    self.level = sustain;
                }
                AdsrPhase::Release => {
                    self.level -= self.release_rate;
                    if self.level <= 0.0 {
                        self.level = 0.0;
                        self.phase = AdsrPhase::Idle;
                    }
                }
            }
            ch0[i] = self.level;
        }
        output.mono_to_all_channels();
    }

    fn reset(&mut self) {
        self.phase = AdsrPhase::Idle;
        self.level = 0.0;
        self.gate_was_on = false;
        self.release_rate = 0.0;
    }
}

/// Macro knob — fans a single user-controlled value (0…1) out to multiple targets.
///
/// The value is held constant for the entire block; the command thread updates it
/// via `MacroCells` without triggering a recompile.
pub struct MacroNode {
    id: NodeId,
    value_cells: Arc<MacroCells>,
}

impl MacroNode {
    pub fn new(id: NodeId, value_cells: Arc<MacroCells>) -> Self {
        Self { id, value_cells }
    }
}

impl AudioNode for MacroNode {
    fn node_id(&self) -> NodeId {
        self.id
    }
    fn label(&self) -> &str {
        "Macro"
    }
    fn num_input_ports(&self) -> usize {
        0
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, _ctx: PrepareContext) {}

    fn process(
        &mut self,
        _inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        if outputs.is_empty() {
            return;
        }
        let output = &mut outputs[0];
        let block_size = transport.block_size.min(output.num_frames);
        let value = self.value_cells.value();
        output.channel_mut(0)[..block_size].fill(value);
        output.mono_to_all_channels();
    }

    fn reset(&mut self) {}
}

impl AudioNode for LfoNode {
    fn node_id(&self) -> NodeId {
        self.id
    }

    fn label(&self) -> &str {
        "LFO"
    }
    fn num_input_ports(&self) -> usize {
        0
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, _ctx: PrepareContext) {}

    fn process(
        &mut self,
        _inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        if outputs.is_empty() {
            return;
        }
        let output = &mut outputs[0];
        let block_size = transport.block_size.min(output.num_frames);

        // Reset phase on transport start so all LFOs begin in sync.
        if transport.is_playing && !self.was_playing {
            self.phase = 0.0;
        }
        self.was_playing = transport.is_playing;

        let (rate_hz, tempo_sync, beat_division) = match &self.rate_cells {
            Some(cells) => (cells.rate_hz(), cells.tempo_sync(), cells.beat_division()),
            None => (self.rate_hz, false, 1.0),
        };

        // Tempo sync: derive rate from BPM and beat division.
        // beat_division=1 → one cycle per beat, beat_division=4 → one cycle per bar.
        let effective_rate_hz = if tempo_sync && beat_division > 0.0 {
            transport.bpm / (60.0 * beat_division)
        } else {
            rate_hz
        };

        let phase_inc = effective_rate_hz / transport.sample_rate;

        {
            let ch0 = output.channel_mut(0);
            let mut phase = self.phase;
            for i in 0..block_size {
                ch0[i] = Self::sample(phase, &self.waveform);
                phase = (phase + phase_inc).fract();
            }
            self.phase = phase;
        }
        output.mono_to_all_channels();
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.was_playing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::live_params::MacroCells;
    use std::sync::atomic::AtomicU32;
    use std::sync::Arc;

    fn make_macro_cells(value: f32) -> Arc<MacroCells> {
        Arc::new(MacroCells {
            value: AtomicU32::new(value.to_bits()),
        })
    }

    fn make_lfo_cells(rate_hz: f32, tempo_sync: bool, beat_division: f32) -> Arc<ModulatorCells> {
        use std::sync::atomic::AtomicBool;
        Arc::new(ModulatorCells {
            rate_hz: AtomicU32::new(rate_hz.to_bits()),
            tempo_sync: AtomicBool::new(tempo_sync),
            beat_division: AtomicU32::new(beat_division.to_bits()),
        })
    }

    fn run_lfo_block(node: &mut LfoNode, transport: &crate::audio_graph::node::TransportInfo) -> Vec<f32> {
        let mut output = crate::audio_graph::buffer::AudioBuffer::new(1, transport.block_size);
        node.process(&[], std::slice::from_mut(&mut output), transport);
        output.channel(0).to_vec()
    }

    fn playing_transport(sr: f32, bpm: f32, block: usize, pos: u64) -> crate::audio_graph::node::TransportInfo {
        crate::audio_graph::node::TransportInfo {
            is_playing: true,
            position_samples: pos,
            position_seconds: pos as f64 / sr as f64,
            bpm,
            sample_rate: sr,
            block_size: block,
        }
    }

    #[test]
    fn lfo_phase_resets_on_transport_start() {
        let sr = 48_000.0;
        let cells = make_lfo_cells(1.0, false, 1.0);
        let mut node = LfoNode::new(NodeId(20), 1.0, LfoWaveform::Sine).with_rate_cells(cells);

        // First playing block: phase should reset to 0, so sample[0] = sin(0) = 0.
        let t = playing_transport(sr, 120.0, 64, 0);
        let out = run_lfo_block(&mut node, &t);
        assert!(
            out[0].abs() < 1e-6,
            "first sample after transport start must be sin(0) = 0 (phase reset)"
        );

        // Advance a few blocks without reset; phase has moved.
        let t2 = playing_transport(sr, 120.0, 64, 64);
        let out2 = run_lfo_block(&mut node, &t2);
        let first_sample_after_advance = out2[0];

        // Stop transport, then start again — phase must reset.
        let stopped = crate::audio_graph::node::TransportInfo::stopped(sr);
        run_lfo_block(&mut node, &stopped);

        let t3 = playing_transport(sr, 120.0, 64, 0);
        let out3 = run_lfo_block(&mut node, &t3);
        assert!(
            out3[0].abs() < 1e-6,
            "phase must reset to 0 when transport restarts"
        );
        // Phase before and after reset must differ.
        assert!(
            (first_sample_after_advance - out3[0]).abs() > 1e-4,
            "samples before and after phase reset must differ"
        );
    }

    #[test]
    fn lfo_tempo_sync_rate_matches_bpm() {
        let sr = 48_000.0;
        let bpm = 120.0_f32;
        // beat_division = 1.0 → one cycle per beat → rate = bpm/60 = 2 Hz
        // period = sr / 2 = 24_000 samples
        let cells = make_lfo_cells(0.0, true, 1.0);
        let mut node = LfoNode::new(NodeId(21), 0.0, LfoWaveform::Sine).with_rate_cells(cells);

        // Run one full period (24_000 samples) starting from transport start.
        let t = playing_transport(sr, bpm, 24_000, 0);
        let out = run_lfo_block(&mut node, &t);
        // At sample 0 (after phase reset): sin(0) = 0.
        assert!(out[0].abs() < 1e-5, "initial phase must be 0");
        // After exactly one period the sine returns to 0 with rising slope.
        assert!(
            out[24_000 - 1].abs() < 0.002,
            "after one full period the sine must be near 0 again (got {})",
            out[24_000 - 1]
        );
    }

    #[test]
    fn lfo_tempo_sync_hot_switchable() {
        let sr = 48_000.0;
        let cells = make_lfo_cells(0.5, false, 1.0);
        let mut node = LfoNode::new(NodeId(22), 0.5, LfoWaveform::Sine).with_rate_cells(Arc::clone(&cells));

        let t = playing_transport(sr, 120.0, 64, 0);
        let out_free = run_lfo_block(&mut node, &t);

        // Enable tempo sync at runtime (no recompile).
        cells.tempo_sync.store(true, Ordering::Relaxed);
        cells.beat_division.store(1.0_f32.to_bits(), Ordering::Relaxed);
        let t2 = playing_transport(sr, 120.0, 64, 64);
        let out_sync = run_lfo_block(&mut node, &t2);

        // With tempo sync the phase_inc changes; outputs should differ over the same block offset.
        // (free-running: phase_inc=0.5/48000; synced: phase_inc=2.0/48000)
        assert_ne!(
            (out_free[63] * 1000.0) as i32,
            (out_sync[63] * 1000.0) as i32,
            "free-running and tempo-synced outputs must differ"
        );
    }

    #[test]
    fn macro_node_fills_output_with_value() {
        let cells = make_macro_cells(0.75);
        let mut node = MacroNode::new(NodeId(1), Arc::clone(&cells));
        let mut output = crate::audio_graph::buffer::AudioBuffer::new(2, 64);
        let t = crate::audio_graph::node::TransportInfo::stopped(48_000.0);
        node.process(&[], std::slice::from_mut(&mut output), &t);
        assert!(
            output.channel(0).iter().all(|&s| (s - 0.75).abs() < 1e-6),
            "macro output must equal the cell value"
        );
    }

    #[test]
    fn macro_node_tracks_live_cell_update() {
        use std::sync::atomic::Ordering;
        let cells = make_macro_cells(0.0);
        let mut node = MacroNode::new(NodeId(1), Arc::clone(&cells));
        let mut output = crate::audio_graph::buffer::AudioBuffer::new(1, 32);
        let t = crate::audio_graph::node::TransportInfo::stopped(48_000.0);

        cells.value.store(0.5_f32.to_bits(), Ordering::Relaxed);
        node.process(&[], std::slice::from_mut(&mut output), &t);
        assert!(
            (output.channel(0)[0] - 0.5).abs() < 1e-6,
            "macro must reflect cell update without recompile"
        );
    }

    fn make_envelope_cells(
        attack_ms: f32,
        decay_ms: f32,
        sustain: f32,
        release_ms: f32,
    ) -> Arc<EnvelopeCells> {
        use crate::project::EnvelopeParams;
        EnvelopeCells::new(&EnvelopeParams {
            attack_ms,
            decay_ms,
            sustain_level: sustain,
            release_ms,
        })
    }

    fn run_block(node: &mut EnvelopeNode, sr: f32, block: usize) -> Vec<f32> {
        let mut output = crate::audio_graph::buffer::AudioBuffer::new(1, block);
        let t = crate::audio_graph::node::TransportInfo {
            is_playing: true,
            position_samples: 0,
            position_seconds: 0.0,
            bpm: 120.0,
            sample_rate: sr,
            block_size: block,
        };
        node.process(&[], std::slice::from_mut(&mut output), &t);
        output.channel(0).to_vec()
    }

    #[test]
    fn envelope_idle_outputs_zero() {
        let cells = make_envelope_cells(10.0, 100.0, 0.7, 200.0);
        let mut node = EnvelopeNode::new(NodeId(10), Arc::clone(&cells));
        let out = run_block(&mut node, 48_000.0, 64);
        assert!(out.iter().all(|&s| s == 0.0), "no gate → output must be zero");
    }

    #[test]
    fn envelope_attack_ramps_to_one() {
        // attack_ms = 1.0 ms at 48 kHz ≈ 48 samples; block = 128 samples.
        let cells = make_envelope_cells(1.0, 1000.0, 0.5, 1000.0);
        cells.gate.store(true, Ordering::Relaxed);
        let mut node = EnvelopeNode::new(NodeId(11), Arc::clone(&cells));
        let out = run_block(&mut node, 48_000.0, 128);
        // Level must reach 1.0 within the first 60 samples.
        let peak_idx = out.iter().take(60).position(|&s| (s - 1.0).abs() < 1e-5)
            .expect("envelope must reach 1.0 within ~60 samples for 1ms attack");
        // Decay must start after the peak.
        if peak_idx + 2 < out.len() {
            assert!(
                out[peak_idx + 2] < 1.0,
                "decay must have started a couple samples after the attack peak"
            );
        }
    }

    #[test]
    fn envelope_release_decays_to_zero() {
        // attack = 0ms (instant), decay = 0ms (instant), sustain = 1.0
        // So gate-on → immediately at 1.0
        let cells = make_envelope_cells(0.0, 0.0, 1.0, 10.0); // 10ms release at 48kHz = 480 samples
        cells.gate.store(true, Ordering::Relaxed);
        let mut node = EnvelopeNode::new(NodeId(12), Arc::clone(&cells));
        run_block(&mut node, 48_000.0, 64); // sustain block

        // Gate off → release
        cells.gate.store(false, Ordering::Relaxed);
        let out = run_block(&mut node, 48_000.0, 512);
        // After 480 samples the release should be complete.
        assert!(
            out[479] < 0.01,
            "envelope must reach zero within release_ms"
        );
        assert_eq!(out[511], 0.0, "output must be exactly zero after Idle");
    }

    #[test]
    fn envelope_reset_returns_to_idle() {
        let cells = make_envelope_cells(100.0, 100.0, 0.5, 100.0);
        cells.gate.store(true, Ordering::Relaxed);
        let mut node = EnvelopeNode::new(NodeId(13), Arc::clone(&cells));
        run_block(&mut node, 48_000.0, 64);
        node.reset();
        cells.gate.store(false, Ordering::Relaxed);
        let out = run_block(&mut node, 48_000.0, 64);
        assert!(out.iter().all(|&s| s == 0.0), "after reset envelope must stay silent");
    }
}
