//! Who may call the plugins: the live stream or an export (ADR-036).
//!
//! A plugin instance is one object (ADR-007). The live scheduler and the export's
//! scheduler both hold the node that reaches it, so two threads would feed one DSP
//! state, and the export's non-blocking claim of the instance would fail whenever the
//! live block was inside the plugin, leaving a dry block in the file.
//!
//! The export therefore takes the plugins away from the live stream for its whole
//! render. The audio callback claims the gate once per block with a single compare and
//! swap, never waits, and writes silence when the export holds it. The export waits,
//! off the audio thread, for a block in flight to end.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A live block is inside the plugins.
const LIVE_BLOCK: u8 = 1;
/// An export holds the plugins, or waits for the live block in flight to end.
const EXPORT: u8 = 2;

/// How long an export waits for a live block in flight before giving up.
pub const EXPORT_HOLD_BUDGET: Duration = Duration::from_secs(2);

/// Announcing the export first keeps new live blocks out, so a stream that runs
/// block after block cannot starve it.
pub struct LiveStreamGate {
    state: AtomicU8,
}

/// A live block is running; the export cannot start until it drops.
pub struct LiveBlock<'a>(&'a AtomicU8);

impl Drop for LiveBlock<'_> {
    fn drop(&mut self) {
        self.0.fetch_and(!LIVE_BLOCK, Ordering::Release);
    }
}

/// The export owns the plugins; the live stream stays silent until it drops.
pub struct ExportHold(Arc<LiveStreamGate>);

impl Drop for ExportHold {
    fn drop(&mut self) {
        self.0.state.fetch_and(!EXPORT, Ordering::Release);
    }
}

impl LiveStreamGate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(0),
        })
    }

    /// Audio thread. Never waits: `None` means an export holds the plugins and the
    /// block must be silence.
    pub fn enter_live(&self) -> Option<LiveBlock<'_>> {
        self.state
            .compare_exchange(0, LIVE_BLOCK, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| LiveBlock(&self.state))
    }

    /// Command thread. Waits up to `budget` for another export, then for the live
    /// block in flight, to end; `None` when it did not.
    pub fn hold_for_export(self: &Arc<Self>, budget: Duration) -> Option<ExportHold> {
        let deadline = Instant::now() + budget;
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state & EXPORT == 0
                && self
                    .state
                    .compare_exchange(state, state | EXPORT, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                break;
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let hold = ExportHold(Arc::clone(self));
        while self.state.load(Ordering::Acquire) & LIVE_BLOCK != 0 {
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Some(hold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(20);

    #[test]
    fn an_idle_gate_lets_a_live_block_run_and_frees_it_afterwards() {
        let gate = LiveStreamGate::new();
        {
            let _block = gate.enter_live().expect("idle gate");
            assert!(gate.enter_live().is_none(), "one block at a time");
        }
        assert!(gate.enter_live().is_some());
    }

    #[test]
    fn an_export_silences_the_live_stream_until_it_is_done() {
        let gate = LiveStreamGate::new();
        let hold = gate.hold_for_export(SHORT).expect("idle gate");
        assert!(gate.enter_live().is_none());
        drop(hold);
        assert!(gate.enter_live().is_some());
    }

    #[test]
    fn an_export_waits_for_the_block_in_flight() {
        let gate = LiveStreamGate::new();
        let in_flight = Arc::clone(&gate);
        let (entered, wait_until_entered) = std::sync::mpsc::channel();
        let block = std::thread::spawn(move || {
            let _block = in_flight.enter_live().expect("idle gate");
            entered.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(60));
        });
        wait_until_entered.recv().unwrap();
        assert!(gate.enter_live().is_none(), "the block is in flight");
        assert!(
            gate.hold_for_export(Duration::from_secs(2)).is_some(),
            "the block ends, then the export gets the plugins"
        );
        block.join().unwrap();
    }

    #[test]
    fn an_export_gives_up_when_a_block_never_ends() {
        let gate = LiveStreamGate::new();
        let stuck = gate.enter_live().expect("idle gate");
        assert!(gate.hold_for_export(SHORT).is_none());
        drop(stuck);
        assert!(
            gate.enter_live().is_some(),
            "a failed export leaves the live stream running"
        );
    }

    #[test]
    fn a_stream_that_never_pauses_cannot_starve_the_export() {
        let gate = LiveStreamGate::new();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stream = {
            let (gate, stop) = (Arc::clone(&gate), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    drop(gate.enter_live());
                }
            })
        };
        assert!(gate.hold_for_export(Duration::from_secs(2)).is_some());
        stop.store(true, Ordering::Relaxed);
        stream.join().unwrap();
    }

    #[test]
    fn two_exports_never_hold_the_plugins_together() {
        let gate = LiveStreamGate::new();
        let _first = gate.hold_for_export(SHORT).expect("idle gate");
        assert!(gate.hold_for_export(SHORT).is_none());
    }
}
