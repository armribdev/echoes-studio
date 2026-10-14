//! `clap.timer-support`: the timers a plugin registers, and when they are due.
//!
//! Checked against `clap/ext/timer-support.h` (CLAP main): the plugin's
//! `on_timer(plugin, timer_id)` is `[main-thread]`; the host's
//! `register_timer(host, period_ms, *timer_id)` may raise a period that is under
//! a threshold ("30 Hz should be allowed") and, like `unregister_timer`, is
//! `[main-thread]`.  The header says nothing about how ids are chosen, about a
//! maximum period, or about the link with `clap.gui` (`gui.h` only advises a
//! hidden window to stop its painting timers) — those are Echoes' decisions,
//! documented in `plugin-wrapper-system.md`.
//!
//! Registration is a lock-free table in the plugin's [`ClapHostContext`]: a
//! plugin may call `register_timer` from any thread, so neither lock nor
//! allocation.  Nothing here ever calls the plugin from a host callback; a
//! message loop owns a [`TimerSchedule`] and runs [`deliver_due_timers`].

use std::ffi::c_char;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::{host_context, ClapHost, ClapHostContext, ClapPlugin};

pub const CLAP_EXT_TIMER_SUPPORT: &[u8] = b"clap.timer-support\0";

/// `CLAP_INVALID_ID` (`clap/id.h`); never given to a timer.
pub const CLAP_INVALID_ID: u32 = u32::MAX;

/// Periods under this are raised to it.  The Windows timer tick is about
/// 15.6 ms, so a shorter period would not be honoured anyway.
pub const MIN_TIMER_PERIOD_MS: u32 = 16;

/// Timers a single plugin may hold at once; a plugin uses one or two.
pub const MAX_TIMERS: usize = 16;

/// `clap_plugin_timer_support`, `[main-thread]`.
#[repr(C)]
pub struct ClapPluginTimerSupport {
    pub on_timer: unsafe extern "C" fn(*const ClapPlugin, u32),
}

/// `clap_host_timer_support`.
#[repr(C)]
pub struct ClapHostTimerSupport {
    pub register_timer: unsafe extern "C" fn(*const ClapHost, u32, *mut u32) -> bool,
    pub unregister_timer: unsafe extern "C" fn(*const ClapHost, u32) -> bool,
}

pub(crate) static HOST_TIMER_SUPPORT: ClapHostTimerSupport = ClapHostTimerSupport {
    register_timer: host_register_timer,
    unregister_timer: host_unregister_timer,
};

// Any thread, no lock, no allocation.  Without an out parameter the plugin could
// never unregister the timer, so nothing is registered.  The out parameter is set
// to `CLAP_INVALID_ID` on failure, for a plugin that does not check the result.
unsafe extern "C" fn host_register_timer(
    host: *const ClapHost,
    period_ms: u32,
    timer_id: *mut u32,
) -> bool {
    if timer_id.is_null() {
        return false;
    }
    let id = host_context(host).and_then(|context| context.timers().register(period_ms));
    *timer_id = id.unwrap_or(CLAP_INVALID_ID);
    id.is_some()
}

unsafe extern "C" fn host_unregister_timer(host: *const ClapHost, timer_id: u32) -> bool {
    host_context(host).is_some_and(|context| context.timers().unregister(timer_id))
}

/// The timers one plugin has registered.  A slot packs `id << 32 | period_ms`;
/// 0 is a free slot (a registered period is never 0, nor is an id).
pub struct TimerTable {
    next_id: AtomicU32,
    slots: [AtomicU64; MAX_TIMERS],
}

impl TimerTable {
    pub(crate) fn new() -> Self {
        TimerTable {
            // Ids start at 1: a plugin may use 0 for "no timer".
            next_id: AtomicU32::new(1),
            slots: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    /// Registers a periodic timer and returns its id.  `None` for a period of 0
    /// or when the table is full.  Ids are unique for the plugin's lifetime and
    /// never reused.
    pub fn register(&self, period_ms: u32) -> Option<u32> {
        if period_ms == 0 {
            return None;
        }
        let id = self
            .next_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |next| {
                (next < CLAP_INVALID_ID).then_some(next + 1)
            })
            .ok()?;
        let entry = (u64::from(id) << 32) | u64::from(period_ms.max(MIN_TIMER_PERIOD_MS));
        self.slots
            .iter()
            .any(|slot| {
                slot.compare_exchange(0, entry, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            })
            .then_some(id)
    }

    /// Whether `id` was registered; it receives no `on_timer` after this.
    pub fn unregister(&self, id: u32) -> bool {
        if id == 0 || id == CLAP_INVALID_ID {
            return false;
        }
        self.slots.iter().any(|slot| {
            let entry = slot.load(Ordering::Acquire);
            entry != 0
                && (entry >> 32) as u32 == id
                && slot
                    .compare_exchange(entry, 0, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
        })
    }

    pub fn period_of(&self, id: u32) -> Option<Duration> {
        self.entries()
            .find_map(|(entry_id, period)| (entry_id == id).then_some(period))
    }

    pub fn is_empty(&self) -> bool {
        self.entries().next().is_none()
    }

    /// `(id, period)` of every registered timer.
    pub fn entries(&self) -> impl Iterator<Item = (u32, Duration)> + '_ {
        self.slots.iter().filter_map(|slot| {
            let entry = slot.load(Ordering::Acquire);
            (entry != 0).then(|| {
                (
                    (entry >> 32) as u32,
                    Duration::from_millis(entry & u64::from(u32::MAX)),
                )
            })
        })
    }
}

struct Scheduled {
    id: u32,
    due: Instant,
}

/// When each timer is due.  Owned by the one message loop that pumps a plugin's
/// timers, so it needs no synchronisation; a new schedule starts every timer
/// one period from now.
#[derive(Default)]
pub struct TimerSchedule {
    entries: Vec<Scheduled>,
}

impl TimerSchedule {
    /// Delivers, through `deliver`, the timers due at `now`; returns how many
    /// were delivered.  Each timer is delivered at most once per call, so a
    /// stalled loop never makes up for lost ticks in a burst.  `deliver` returns
    /// whether the call was made: a timer it declined stays due and is offered
    /// again on the next call.  A timer a plugin unregisters from inside another
    /// `on_timer` of the same call is not delivered.
    pub fn run_due(
        &mut self,
        table: &TimerTable,
        now: Instant,
        mut deliver: impl FnMut(u32) -> bool,
    ) -> usize {
        self.entries
            .retain(|scheduled| table.period_of(scheduled.id).is_some());
        for (id, period) in table.entries() {
            if !self.entries.iter().any(|scheduled| scheduled.id == id) {
                self.entries.push(Scheduled {
                    id,
                    due: now + period,
                });
            }
        }
        let mut delivered = 0;
        for index in 0..self.entries.len() {
            let Scheduled { id, due } = self.entries[index];
            if now < due || table.period_of(id).is_none() || !deliver(id) {
                continue;
            }
            delivered += 1;
            // The plugin may have unregistered this very timer from `on_timer`.
            if let Some(period) = table.period_of(id) {
                self.entries[index].due = next_due(due, period, now);
            }
        }
        delivered
    }
}

/// Keeps the rhythm (`due + period`) unless the loop fell a whole period behind,
/// in which case it starts over from `now`.
fn next_due(due: Instant, period: Duration, now: Instant) -> Instant {
    let next = due + period;
    if next > now {
        next
    } else {
        now + period
    }
}

/// Calls the plugin's `on_timer` for every timer of `context` due at `now`.
/// `enter` runs before each call and returns `None` to decline it (the timer
/// stays due); what it returns is held until the call is over.  Returns how many
/// calls were made.
///
/// # Safety
/// `plugin` must be live, `context` its host context, and the caller must be on
/// the thread where the plugin's main-thread calls run.
pub unsafe fn deliver_due_timers<Guard>(
    plugin: *const ClapPlugin,
    context: &ClapHostContext,
    schedule: &mut TimerSchedule,
    now: Instant,
    mut enter: impl FnMut() -> Option<Guard>,
) -> usize {
    if context.timers().is_empty() {
        *schedule = TimerSchedule::default();
        return 0;
    }
    let extension =
        ((*plugin).get_extension)(plugin, CLAP_EXT_TIMER_SUPPORT.as_ptr() as *const c_char)
            as *const ClapPluginTimerSupport;
    let Some(extension) = extension.as_ref() else {
        return 0;
    };
    schedule.run_due(context.timers(), now, |id| {
        let Some(_guard) = enter() else {
            return false;
        };
        (extension.on_timer)(plugin, id);
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{host_get_extension, make_host};
    use std::mem::{offset_of, size_of};

    fn host_table(host: &ClapHost) -> &'static ClapHostTimerSupport {
        let extension =
            unsafe { host_get_extension(host, CLAP_EXT_TIMER_SUPPORT.as_ptr() as *const c_char) };
        unsafe { (extension as *const ClapHostTimerSupport).as_ref() }.expect("extension answered")
    }

    fn register(host: &ClapHost, period_ms: u32) -> Option<u32> {
        let mut id = 0;
        let accepted = unsafe { (host_table(host).register_timer)(host, period_ms, &mut id) };
        accepted.then_some(id)
    }

    fn at(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    fn run(
        schedule: &mut TimerSchedule,
        table: &TimerTable,
        now: Instant,
        decline: bool,
    ) -> Vec<u32> {
        let mut delivered = Vec::new();
        schedule.run_due(table, now, |id| {
            if decline {
                return false;
            }
            delivered.push(id);
            true
        });
        delivered
    }

    #[test]
    fn the_extension_table_follows_the_spec() {
        assert_eq!(CLAP_EXT_TIMER_SUPPORT, b"clap.timer-support\0");
        assert_eq!(offset_of!(ClapHostTimerSupport, register_timer), 0);
        assert_eq!(
            offset_of!(ClapHostTimerSupport, unregister_timer),
            size_of::<usize>()
        );
        assert_eq!(size_of::<ClapHostTimerSupport>(), 2 * size_of::<usize>());
        assert_eq!(size_of::<ClapPluginTimerSupport>(), size_of::<usize>());
        assert_eq!(CLAP_INVALID_ID, u32::MAX);
    }

    #[test]
    fn a_plugin_registers_and_unregisters_through_the_host_table() {
        let host = make_host();
        let table = host_table(&host);

        let first = register(&host, 33).expect("30 Hz is allowed");
        let second = register(&host, 50).expect("a second timer");
        assert_ne!(first, second, "ids are distinct");
        assert_ne!(first, 0, "0 is left to mean no timer");
        assert_ne!(first, CLAP_INVALID_ID);

        assert!(unsafe { (table.unregister_timer)(&*host, first) });
        assert!(
            !unsafe { (table.unregister_timer)(&*host, first) },
            "a second unregister reports failure"
        );
        assert!(!unsafe { (table.unregister_timer)(&*host, 999) });
        assert!(!unsafe { (table.unregister_timer)(&*host, CLAP_INVALID_ID) });

        let third = register(&host, 33).unwrap();
        assert!(third > second, "an id is never reused");
        assert_eq!(
            host.context().unwrap().timers().period_of(first),
            None,
            "the unregistered timer is gone"
        );
    }

    #[test]
    fn a_period_of_zero_and_unusable_arguments_are_refused() {
        let host = make_host();
        let table = host_table(&host);
        let mut id = 7;
        assert!(!unsafe { (table.register_timer)(&*host, 0, &mut id) });
        assert_eq!(id, CLAP_INVALID_ID, "the out parameter says so");
        assert!(host.context().unwrap().timers().is_empty());

        assert!(
            !unsafe { (table.register_timer)(&*host, 33, std::ptr::null_mut()) },
            "without an out parameter the id would be lost"
        );
        assert!(host.context().unwrap().timers().is_empty());
        assert!(!unsafe { (table.register_timer)(std::ptr::null(), 33, &mut id) });
        assert!(!unsafe { (table.unregister_timer)(std::ptr::null(), 1) });
    }

    #[test]
    fn a_short_period_is_raised_and_a_long_one_is_kept() {
        let host = make_host();
        let timers = host.context().unwrap().timers();
        let short = register(&host, 1).unwrap();
        let long = register(&host, 3_600_000).unwrap();
        assert_eq!(
            timers.period_of(short),
            Some(Duration::from_millis(u64::from(MIN_TIMER_PERIOD_MS)))
        );
        assert_eq!(timers.period_of(long), Some(Duration::from_secs(3600)));
        assert!(Duration::from_millis(33) > Duration::from_millis(u64::from(MIN_TIMER_PERIOD_MS)));
    }

    #[test]
    fn a_plugin_holds_a_bounded_number_of_timers() {
        let host = make_host();
        for _ in 0..MAX_TIMERS {
            assert!(register(&host, 33).is_some());
        }
        assert_eq!(register(&host, 33), None, "the table is full");
        let any = host.context().unwrap().timers().entries().next().unwrap().0;
        assert!(unsafe { (host_table(&host).unregister_timer)(&*host, any) });
        assert!(register(&host, 33).is_some(), "a freed slot is reused");
    }

    #[test]
    fn every_plugin_has_its_own_timers() {
        let first = make_host();
        let second = make_host();
        let id = register(&first, 33).unwrap();
        assert!(second.context().unwrap().timers().is_empty());
        assert!(!unsafe { (host_table(&second).unregister_timer)(&*second, id) });
    }

    #[test]
    fn registrations_from_many_threads_get_distinct_ids() {
        let host = make_host();
        let timers = host.context().unwrap().timers();
        let ids: Vec<Option<u32>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..MAX_TIMERS)
                .map(|_| scope.spawn(|| timers.register(33)))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut ids: Vec<u32> = ids.into_iter().map(|id| id.expect("a slot each")).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MAX_TIMERS);
        assert_eq!(timers.entries().count(), MAX_TIMERS);
    }

    #[test]
    fn a_timer_is_first_due_one_period_after_it_is_first_seen() {
        let host = make_host();
        let timers = host.context().unwrap().timers();
        let id = timers.register(33).unwrap();
        let start = Instant::now();
        let mut schedule = TimerSchedule::default();

        assert!(run(&mut schedule, timers, start, false).is_empty());
        assert!(run(&mut schedule, timers, at(start, 32), false).is_empty());
        assert_eq!(run(&mut schedule, timers, at(start, 33), false), [id]);
        assert!(run(&mut schedule, timers, at(start, 34), false).is_empty());
    }

    #[test]
    fn the_rhythm_is_kept_when_the_loop_is_a_little_late() {
        let host = make_host();
        let timers = host.context().unwrap().timers();
        let id = timers.register(30).unwrap();
        let start = Instant::now();
        let mut schedule = TimerSchedule::default();
        run(&mut schedule, timers, start, false);

        // Passes every 16 ms: due at 30, 60, 90, 120 — delivered at 32, 64, 96, 128.
        let delivered: Vec<u64> = (1..=8)
            .map(|pass| pass * 16)
            .filter(|&millis| !run(&mut schedule, timers, at(start, millis), false).is_empty())
            .collect();
        assert_eq!(delivered, [32, 64, 96, 128]);
        assert!(timers.period_of(id).is_some());
    }

    #[test]
    fn a_stalled_loop_delivers_once_not_a_burst() {
        let host = make_host();
        let timers = host.context().unwrap().timers();
        let id = timers.register(20).unwrap();
        let start = Instant::now();
        let mut schedule = TimerSchedule::default();
        run(&mut schedule, timers, start, false);

        assert_eq!(run(&mut schedule, timers, at(start, 5_000), false), [id]);
        assert!(
            run(&mut schedule, timers, at(start, 5_001), false).is_empty(),
            "no catching up"
        );
        assert!(run(&mut schedule, timers, at(start, 5_019), false).is_empty());
        assert_eq!(run(&mut schedule, timers, at(start, 5_020), false), [id]);
    }

    #[test]
    fn a_declined_timer_stays_due() {
        let host = make_host();
        let timers = host.context().unwrap().timers();
        let id = timers.register(20).unwrap();
        let start = Instant::now();
        let mut schedule = TimerSchedule::default();
        run(&mut schedule, timers, start, false);

        assert!(run(&mut schedule, timers, at(start, 20), true).is_empty());
        assert_eq!(
            run(&mut schedule, timers, at(start, 21), false),
            [id],
            "offered again at the next pass, not one period later"
        );
    }

    #[test]
    fn an_unregistered_timer_is_never_delivered_again() {
        let host = make_host();
        let timers = host.context().unwrap().timers();
        let kept = timers.register(20).unwrap();
        let dropped = timers.register(20).unwrap();
        let start = Instant::now();
        let mut schedule = TimerSchedule::default();
        run(&mut schedule, timers, start, false);
        assert!(timers.unregister(dropped));

        assert_eq!(run(&mut schedule, timers, at(start, 20), false), [kept]);
        assert_eq!(run(&mut schedule, timers, at(start, 40), false), [kept]);
    }

    #[test]
    fn a_timer_unregistered_by_an_earlier_call_of_the_same_pass_is_skipped() {
        let host = make_host();
        let timers = host.context().unwrap().timers();
        let first = timers.register(20).unwrap();
        let second = timers.register(20).unwrap();
        let start = Instant::now();
        let mut schedule = TimerSchedule::default();
        run(&mut schedule, timers, start, false);

        let mut delivered = Vec::new();
        schedule.run_due(timers, at(start, 20), |id| {
            delivered.push(id);
            timers.unregister(first);
            timers.unregister(second);
            true
        });
        assert_eq!(delivered.len(), 1, "the other one is gone before its turn");
        assert!(timers.is_empty());
    }

    #[test]
    fn a_timer_registered_during_a_pass_starts_one_period_later() {
        let host = make_host();
        let timers = host.context().unwrap().timers();
        let first = timers.register(20).unwrap();
        let start = Instant::now();
        let mut schedule = TimerSchedule::default();
        run(&mut schedule, timers, start, false);

        let mut late = None;
        schedule.run_due(timers, at(start, 20), |_| {
            late = timers.register(20);
            true
        });
        let late = late.unwrap();
        // First seen at the next pass (21 ms), so due one period after that.
        assert_eq!(run(&mut schedule, timers, at(start, 21), false), []);
        assert_eq!(run(&mut schedule, timers, at(start, 40), false), [first]);
        assert_eq!(run(&mut schedule, timers, at(start, 41), false), [late]);
    }

    #[test]
    fn a_new_schedule_does_not_make_up_for_the_time_nobody_pumped() {
        let host = make_host();
        let timers = host.context().unwrap().timers();
        let id = timers.register(20).unwrap();
        let start = Instant::now();

        let mut reopened = TimerSchedule::default();
        assert!(run(&mut reopened, timers, at(start, 60_000), false).is_empty());
        assert_eq!(run(&mut reopened, timers, at(start, 60_020), false), [id]);
    }

    #[test]
    fn nothing_is_asked_of_a_plugin_without_timers() {
        let host = make_host();
        let mut schedule = TimerSchedule::default();
        let plugin = std::ptr::null();
        // The plugin pointer is never read when there is no timer.
        let calls = unsafe {
            deliver_due_timers(
                plugin,
                host.context().unwrap(),
                &mut schedule,
                Instant::now(),
                || Some(()),
            )
        };
        assert_eq!(calls, 0);
    }
}
