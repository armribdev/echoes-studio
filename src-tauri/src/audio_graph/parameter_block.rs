//! Per-effect hot-parameter block — ADR-009.
//!
//! ## Why this exists
//!
//! Before this, every knob turn on an internal effect (EQ, Compressor, …) went
//! through `UPDATE_EFFECT → recompile_if_needed`, rebuilding the whole graph:
//! re-`prepare` on every node, reallocation of reverb combs / delay rings,
//! mid-stream DSP reset, and a 50–200 ms command-thread stall on large
//! projects (see `target_architecture.md` §4.1).  That is the wrong shape for
//! a continuous, modulatable parameter.
//!
//! ADR-009 introduces a `ParameterBlock`: a small bank of atomic cells, one per
//! hot parameter of one effect.  The node reads the cells per block; the
//! command thread writes them on a knob turn.  A full recompile is then
//! reserved for **structural** changes (add/remove effect, change effect type,
//! topology, tempo).
//!
//! ## Real-time contract
//!
//! - The audio thread only ever calls [`ParameterBlock::get`], a single
//!   `AtomicU32::load(Relaxed)` per parameter.  No allocation, no lock.
//! - The command thread only ever calls [`ParameterBlock::set`], a single
//!   `AtomicU32::store(Relaxed)`.
//! - The block is allocated off the audio thread (at compile / prepare) and is
//!   immutable in structure thereafter (BP-8): the `Vec<AtomicU32>` is never
//!   resized once shared.  The node captures an `Arc<ParameterBlock>` clone at
//!   compile time, so runtime access is lock-free.
//!
//! ## Value model
//!
//! Cells hold parameter values in **natural units** (Hz, dB, Q, …) bit-cast
//! through `f32::to_bits`.  Unlike plugin parameters (which travel normalised
//! through the SPSC queue, see `plugin_system::param_queue`), internal effects
//! own their ranges, so storing plain values keeps the node arithmetic trivial
//! and avoids a denormalise step on the hot path.
//!
//! Ordering is `Relaxed`: a parameter cell carries no happens-before
//! relationship with any other memory.  A torn read across two cells (e.g. the
//! command thread updates `freq` and `gain` between two `get`s) is harmless —
//! it resolves on the very next block, exactly like a one-block-late
//! automation value.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// A fixed bank of atomic `f32` cells for one effect's hot parameters.
///
/// The index→meaning mapping is owned by the effect node (e.g.
/// `nodes::eq::EqSlot`).  This type is deliberately layout-agnostic so the same
/// container serves every internal effect as the variant-by-variant migration
/// proceeds (EQ first, then Compressor / Reverb / Delay).
pub struct ParameterBlock {
    cells: Vec<AtomicU32>,
}

impl ParameterBlock {
    /// Allocate a block seeded with `values` (one cell per element).
    ///
    /// Called off the audio thread (compiler `build_node`).  The length is
    /// fixed for the life of the block.
    pub fn new(values: &[f32]) -> Arc<Self> {
        let cells = values.iter().map(|v| AtomicU32::new(v.to_bits())).collect();
        Arc::new(Self { cells })
    }

    /// Number of parameter cells.
    #[cfg(test)]
    #[inline]
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Read cell `idx` (audio thread).  Out-of-range indices return `0.0`
    /// rather than panicking — a layout mismatch must never abort `process`.
    #[inline]
    pub fn get(&self, idx: usize) -> f32 {
        match self.cells.get(idx) {
            Some(cell) => f32::from_bits(cell.load(Ordering::Relaxed)),
            None => 0.0,
        }
    }

    /// Write cell `idx` (command thread).  Out-of-range indices are ignored.
    #[inline]
    pub fn set(&self, idx: usize, value: f32) {
        if let Some(cell) = self.cells.get(idx) {
            cell.store(value.to_bits(), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_values() {
        let b = ParameterBlock::new(&[250.0, -3.5, 1.0]);
        assert_eq!(b.len(), 3);
        assert_eq!(b.get(0), 250.0);
        assert_eq!(b.get(1), -3.5);
        assert_eq!(b.get(2), 1.0);
    }

    #[test]
    fn set_updates_cell() {
        let b = ParameterBlock::new(&[0.0, 0.0]);
        b.set(1, 12.0);
        assert_eq!(b.get(1), 12.0);
        assert_eq!(b.get(0), 0.0);
    }

    #[test]
    fn out_of_range_is_safe() {
        let b = ParameterBlock::new(&[1.0]);
        b.set(99, 5.0); // ignored
        assert_eq!(b.get(99), 0.0); // no panic, returns 0
    }
}
