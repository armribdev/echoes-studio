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
//! | master   | MasterNode    | N      | 1*      | Final mix + soft-clip + metering       |
//! | send     | SendNode      | 1      | 1       | Tap signal at gain → return bus        |
//! | meter    | MeterNode     | 1      | 1       | Pass-through + peak/RMS measurement    |
//! | modulator| LfoNode       | 0      | 1       | Sine-wave LFO modulation source        |
//!
//! *MasterNode declares 1 output internally; the compiler emits it as a sink
//!  (0 outputs) so the scheduler uses the special `write_master_output` path.

pub mod bus;
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
