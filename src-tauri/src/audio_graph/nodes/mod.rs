//! Concrete DSP node implementations.
//!
//! Each module provides one node type.  All nodes implement `AudioNode` and
//! are instantiated by `GraphCompiler::build_node`.
//!
//! ## Node catalogue
//!
//! | Module   | Type          | Inputs | Outputs | Role                                   |
//! |----------|---------------|--------|---------|----------------------------------------|
//! | track    | TrackNode     | 0      | 1       | Read timeline clips, output audio      |
//! | gain     | GainNode      | 1      | 1       | Linear gain / track volume             |
//! | pan      | PanNode       | 1      | 1       | Equal-power stereo panner              |
//! | bus      | BusNode       | N      | 1       | Sum N inputs (group / master bus)      |
//! | bypass   | BypassNode    | =inner | =inner  | Disabled plugin: kept alive, audio passes through |
//! | master   | MasterNode    | N      | 1*      | Final mix + soft-clip + metering       |
//! | send     | SendNode      | 1      | 1       | Tap signal at gain → return bus        |
//! | meter    | MeterNode     | 1      | 1       | Pass-through + peak/RMS measurement    |
//! | modulator| LfoNode       | 0      | 1       | Sine-wave LFO modulation source        |
//!
//! *MasterNode declares 1 output internally; the compiler emits it as a sink
//!  (0 outputs) so the scheduler uses the special `write_master_output` path.

pub mod bus;
pub mod bypass;
pub mod clap_instrument;
pub mod compressor;
pub mod delay;
pub mod eq;
pub mod gain;
pub mod instrument;
pub mod master;
pub mod meter;
pub mod modulator;
pub mod pan;
pub mod poly_synth;
pub mod reverb;
pub mod send;
pub mod track;
pub mod track_gain;

/// Modulated value held inside `range`, widened to include `base` so an
/// unmodulated value (and any legacy out-of-range setting) passes through bit-identical.
pub fn modulated(base: f32, delta: f32, range: (f32, f32)) -> f32 {
    let (min, max) = range;
    (base + delta).clamp(min.min(base), max.max(base))
}

#[cfg(test)]
mod tests {
    use super::modulated;

    #[test]
    fn modulated_value_stays_in_range() {
        assert_eq!(modulated(0.4, 0.9, (0.0, 0.95)), 0.95);
        assert_eq!(modulated(1000.0, -5000.0, (20.0, 20_000.0)), 20.0);
        assert_eq!(modulated(0.4, 0.1, (0.0, 0.95)), 0.5);
    }

    #[test]
    fn out_of_range_base_passes_through_unmodulated() {
        assert_eq!(modulated(1.2, 0.0, (0.0, 0.95)), 1.2);
        assert_eq!(modulated(1.2, 0.5, (0.0, 0.95)), 1.2);
        assert_eq!(modulated(1.2, -0.5, (0.0, 0.95)), 1.2 - 0.5);
    }
}
