//! Audio graph — the core DSP runtime system.
//!
//! ## Architecture
//!
//! ```text
//! AppProject  ──► AudioGraph::from_project()
//!                     │
//!                     ▼
//!             GraphCompiler::compile()
//!                     │
//!               CompiledGraph { plan, processors }
//!                     │
//!             GraphScheduler::new(compiled)
//!                     │  (atomic swap via RuntimeSnapshot)
//!                     ▼
//!             GraphScheduler::process_block()   ←── audio callback
//! ```
//!
//! ## Module map
//!
//! - [`buffer`]         — `AudioBuffer`: planar f32, no allocation after construction
//! - [`node`]           — `AudioNode` trait: the DSP processing unit
//! - [`modulation`]     — Modulation routing types (ModulationMap, ModulationRoute)
//! - [`execution_plan`] — `ExecutionPlan`: immutable compiled graph description
//! - [`graph`]          — `AudioGraph`: editor-side topology (nodes + edges)
//! - [`compiler`]       — `GraphCompiler`: topology → execution plan
//! - [`scheduler`]      — `GraphScheduler`: drives plan in the audio callback
//! - [`snapshot`]       — `RuntimeSnapshot`: lock-free atomic scheduler swap
//! - [`nodes`]          — concrete `AudioNode` implementations

pub mod arpeggiator;
pub mod automation;
pub mod buffer;
pub mod compiler;
pub mod denormal;
pub mod execution_plan;
pub mod graph;
pub mod live_params;
pub mod midi;
pub mod midi_controls;
pub mod modulation;
pub mod node;
pub mod nodes;
pub mod note_expressions;
pub mod note_stage;
pub mod oversampling;
pub mod parameter_block;
pub mod plugin_params;
pub mod scheduler;
pub mod snapshot;
pub mod snapshot_dto;
pub mod worker_pool;

pub use compiler::{CompileConfig, GraphCompiler, MeterCell};
pub use graph::AudioGraph;
pub use live_params::LiveParams;
pub use scheduler::GraphScheduler;
pub use snapshot::RuntimeSnapshot;
pub use snapshot_dto::GraphSnapshotDto;
