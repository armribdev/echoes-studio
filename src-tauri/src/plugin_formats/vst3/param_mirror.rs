//! The values the processor got from the host, kept for the plugin window.
//!
//! A VST3 controller never hears what the host sends its processor, and the host
//! must tell it (`setParamNormalized`, on the window thread) for the knobs of an
//! open editor to follow automation and modulation.  The audio thread publishes
//! the last value of each parameter here; the window thread applies them.  The
//! table is preallocated and made of atomics only: the audio thread never waits,
//! allocates or calls the controller (ADR-008).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const SLOT_BITS: u32 = 11;
const SLOTS: usize = 1 << SLOT_BITS;
const MAX_PROBES: usize = 32;
/// `kNoParamId`: reserved by the SDK, so it can mark a free slot.
const FREE: u32 = u32::MAX;

struct Slot {
    id: AtomicU32,
    dirty: AtomicU32,
    value: AtomicU64,
}

pub struct ParamMirror {
    slots: Box<[Slot]>,
    /// Open plugin windows: nothing is published while nobody applies the values.
    watchers: AtomicU32,
}

impl ParamMirror {
    pub fn new() -> Self {
        Self {
            slots: (0..SLOTS)
                .map(|_| Slot {
                    id: AtomicU32::new(FREE),
                    dirty: AtomicU32::new(0),
                    value: AtomicU64::new(0),
                })
                .collect(),
            watchers: AtomicU32::new(0),
        }
    }

    pub fn watch(&self) {
        self.watchers.fetch_add(1, Ordering::AcqRel);
    }

    pub fn unwatch(&self) {
        let _ = self
            .watchers
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
    }

    /// Audio thread: `value` is now the latest of `native_id`.  Bounded probing, no
    /// allocation; a parameter that finds no slot is not mirrored.
    pub fn record(&self, native_id: u32, value: f64) {
        if native_id == FREE || self.watchers.load(Ordering::Acquire) == 0 {
            return;
        }
        let first = (native_id.wrapping_mul(0x9E37_79B1) >> (32 - SLOT_BITS)) as usize;
        for probe in 0..MAX_PROBES {
            let slot = &self.slots[(first + probe) & (SLOTS - 1)];
            let owner =
                match slot
                    .id
                    .compare_exchange(FREE, native_id, Ordering::AcqRel, Ordering::Acquire)
                {
                    Ok(_) => native_id,
                    Err(owner) => owner,
                };
            if owner == native_id {
                slot.value.store(value.to_bits(), Ordering::Relaxed);
                slot.dirty.store(1, Ordering::Release);
                return;
            }
        }
    }

    /// Window thread: calls `apply` with the latest value of each parameter set
    /// since the last call.
    pub fn take_changes(&self, mut apply: impl FnMut(u32, f64)) {
        for slot in self.slots.iter() {
            if slot.dirty.load(Ordering::Relaxed) != 0 && slot.dirty.swap(0, Ordering::Acquire) != 0
            {
                let value = f64::from_bits(slot.value.load(Ordering::Relaxed));
                apply(slot.id.load(Ordering::Relaxed), value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(mirror: &ParamMirror) -> Vec<(u32, f64)> {
        let mut changes = Vec::new();
        mirror.take_changes(|id, value| changes.push((id, value)));
        changes.sort_by_key(|&(id, _)| id);
        changes
    }

    fn watched() -> ParamMirror {
        let mirror = ParamMirror::new();
        mirror.watch();
        mirror
    }

    #[test]
    fn nothing_is_kept_while_no_window_is_open() {
        let mirror = ParamMirror::new();
        mirror.record(7, 0.5);
        mirror.watch();
        assert!(taken(&mirror).is_empty());
        mirror.unwatch();
        mirror.record(7, 0.5);
        mirror.watch();
        assert!(taken(&mirror).is_empty());
    }

    #[test]
    fn an_unmatched_unwatch_does_not_wrap_around() {
        let mirror = ParamMirror::new();
        mirror.unwatch();
        mirror.record(7, 0.5);
        assert!(taken(&mirror).is_empty());
    }

    #[test]
    fn the_latest_value_of_each_parameter_is_taken_once() {
        let mirror = watched();
        mirror.record(7, 0.1);
        mirror.record(9, 0.9);
        mirror.record(7, 0.3);
        assert_eq!(taken(&mirror), [(7, 0.3), (9, 0.9)]);
        assert!(taken(&mirror).is_empty());
        mirror.record(7, 0.4);
        assert_eq!(taken(&mirror), [(7, 0.4)]);
    }

    #[test]
    fn the_reserved_id_is_refused() {
        let mirror = watched();
        mirror.record(u32::MAX, 0.5);
        assert!(taken(&mirror).is_empty());
    }

    #[test]
    fn ids_that_collide_each_keep_their_own_slot() {
        let mirror = watched();
        let ids: Vec<u32> = (0..16).map(|n| 5 + n * SLOTS as u32).collect();
        for (n, &id) in ids.iter().enumerate() {
            mirror.record(id, n as f64 / 16.0);
        }
        let mut expected: Vec<(u32, f64)> = ids
            .iter()
            .enumerate()
            .map(|(n, &id)| (id, n as f64 / 16.0))
            .collect();
        expected.sort_by_key(|&(id, _)| id);
        assert_eq!(taken(&mirror), expected);
    }

    #[test]
    fn a_full_table_drops_values_without_failing() {
        let mirror = watched();
        let total = SLOTS as u32 * 2;
        for id in 0..total {
            mirror.record(id, 0.5);
        }
        let kept = taken(&mirror);
        assert!(!kept.is_empty() && kept.len() <= SLOTS);
        assert!(kept.iter().all(|&(_, value)| value == 0.5));
        mirror.record(total + 1, 0.25);
    }

    #[test]
    fn a_window_thread_reads_while_the_audio_thread_writes() {
        let mirror = watched();
        let running = std::sync::atomic::AtomicBool::new(true);
        let mut last = std::thread::scope(|scope| {
            let window = scope.spawn(|| {
                let mut last = [0.0f64; 50];
                while running.load(Ordering::Acquire) {
                    mirror.take_changes(|id, value| {
                        assert!(value >= last[id as usize], "an older value came after");
                        last[id as usize] = value;
                    });
                }
                last
            });
            for round in 1..=2000u32 {
                for id in 0..50u32 {
                    mirror.record(id, f64::from(round));
                }
            }
            running.store(false, Ordering::Release);
            window.join().unwrap()
        });
        mirror.take_changes(|id, value| last[id as usize] = value);
        assert!(last.iter().all(|&value| value == 2000.0));
    }
}
