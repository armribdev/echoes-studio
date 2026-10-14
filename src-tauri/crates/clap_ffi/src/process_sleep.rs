//! When the host may stop calling a CLAP plugin's `process` (status + `clap.tail`).
//!
//! The decision depends only on the signal and on sample counts, never on wall
//! time, so an offline render sleeps exactly where a live one does.  Plain
//! integers and booleans: usable on the audio thread without allocating.

use crate::{
    ClapProcessStatus, CLAP_PROCESS_CONTINUE, CLAP_PROCESS_CONTINUE_IF_NOT_QUIET,
    CLAP_PROCESS_ERROR, CLAP_PROCESS_SLEEP, CLAP_PROCESS_TAIL,
};

/// -120 dBFS.  Below it a block counts as silent.
pub const SILENCE_THRESHOLD: f32 = 1e-6;

/// `clap.tail` values from this up mean "never ends".
const INFINITE_TAIL: u32 = i32::MAX as u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessStatus {
    Error,
    Continue,
    ContinueIfNotQuiet,
    Tail,
    Sleep,
}

impl ProcessStatus {
    /// An unknown value is treated as `Continue`: the plugin keeps running.
    pub fn from_raw(raw: ClapProcessStatus) -> Self {
        match raw {
            CLAP_PROCESS_ERROR => Self::Error,
            CLAP_PROCESS_CONTINUE_IF_NOT_QUIET => Self::ContinueIfNotQuiet,
            CLAP_PROCESS_TAIL => Self::Tail,
            CLAP_PROCESS_SLEEP => Self::Sleep,
            CLAP_PROCESS_CONTINUE => Self::Continue,
            _ => Self::Continue,
        }
    }
}

pub fn is_silent(samples: &[f32]) -> bool {
    samples
        .iter()
        .all(|sample| sample.abs() <= SILENCE_THRESHOLD)
}

/// What reaches the plugin in one block, apart from the transport position.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockActivity {
    pub frames: u32,
    pub has_events: bool,
    /// Only meaningful when [`ProcessSleep::wants_input_scan`] was true.
    pub input_audible: bool,
    pub playing: bool,
    /// The plugin called `clap_host.request_process` since the last block.
    pub process_requested: bool,
}

#[derive(Debug, Default)]
pub struct ProcessSleep {
    asleep: bool,
    last_status: Option<ProcessStatus>,
    tail: Option<u32>,
    quiet_input_samples: u64,
    playing: bool,
}

impl ProcessSleep {
    pub fn new() -> Self {
        Self::default()
    }

    /// Without a tail, only `Sleep` can put the plugin to sleep.
    pub fn set_tail(&mut self, tail: Option<u32>) {
        self.tail = tail.filter(|samples| *samples < INFINITE_TAIL);
    }

    pub fn is_asleep(&self) -> bool {
        self.asleep
    }

    /// Back to normal processing (reset, reactivation).
    pub fn wake(&mut self) {
        self.asleep = false;
        self.last_status = None;
        self.quiet_input_samples = 0;
    }

    /// Whether the caller should measure the input level this block: always
    /// while asleep (to wake on a signal), and while a tail is being counted.
    pub fn wants_input_scan(&self) -> bool {
        self.asleep || self.counts_quiet_input()
    }

    fn counts_quiet_input(&self) -> bool {
        self.tail.is_some()
            && matches!(
                self.last_status,
                Some(ProcessStatus::ContinueIfNotQuiet | ProcessStatus::Tail)
            )
    }

    /// Call at the start of each block.  `true` means skip `process` and
    /// output silence.
    pub fn begin_block(&mut self, activity: BlockActivity) -> bool {
        let play_state_changed = activity.playing != self.playing;
        self.playing = activity.playing;
        let signal = activity.has_events || (self.wants_input_scan() && activity.input_audible);
        if signal || play_state_changed || activity.process_requested {
            self.asleep = false;
            self.quiet_input_samples = 0;
            return false;
        }
        if self.wants_input_scan() {
            self.quiet_input_samples += u64::from(activity.frames);
        } else {
            self.quiet_input_samples = 0;
        }
        self.asleep
    }

    /// Call after a `process` call.  `output_silent` is only evaluated when
    /// the status could allow sleeping.
    pub fn end_block(
        &mut self,
        raw_status: ClapProcessStatus,
        output_silent: impl FnOnce() -> bool,
    ) {
        let status = ProcessStatus::from_raw(raw_status);
        self.last_status = Some(status);
        self.asleep = match status {
            ProcessStatus::Error | ProcessStatus::Continue => false,
            ProcessStatus::Sleep => output_silent(),
            ProcessStatus::ContinueIfNotQuiet | ProcessStatus::Tail => {
                self.tail
                    .is_some_and(|samples| self.quiet_input_samples >= u64::from(samples))
                    && output_silent()
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAMES: u32 = 64;

    fn quiet() -> BlockActivity {
        BlockActivity {
            frames: FRAMES,
            ..BlockActivity::default()
        }
    }

    fn with(change: impl FnOnce(&mut BlockActivity)) -> BlockActivity {
        let mut activity = quiet();
        change(&mut activity);
        activity
    }

    fn run_block(
        sleep: &mut ProcessSleep,
        activity: BlockActivity,
        status: i32,
        output_silent: bool,
    ) -> bool {
        let skipped = sleep.begin_block(activity);
        if !skipped {
            sleep.end_block(status, || output_silent);
        }
        skipped
    }

    #[test]
    fn raw_statuses_map_and_unknown_values_keep_running() {
        assert_eq!(ProcessStatus::from_raw(0), ProcessStatus::Error);
        assert_eq!(ProcessStatus::from_raw(1), ProcessStatus::Continue);
        assert_eq!(
            ProcessStatus::from_raw(2),
            ProcessStatus::ContinueIfNotQuiet
        );
        assert_eq!(ProcessStatus::from_raw(3), ProcessStatus::Tail);
        assert_eq!(ProcessStatus::from_raw(4), ProcessStatus::Sleep);
        assert_eq!(ProcessStatus::from_raw(99), ProcessStatus::Continue);
        assert_eq!(ProcessStatus::from_raw(-1), ProcessStatus::Continue);
    }

    #[test]
    fn silence_is_judged_at_the_threshold() {
        assert!(is_silent(&[0.0, SILENCE_THRESHOLD, -SILENCE_THRESHOLD]));
        assert!(!is_silent(&[0.0, 2e-6]));
        assert!(is_silent(&[]));
    }

    #[test]
    fn continue_never_sleeps() {
        let mut sleep = ProcessSleep::new();
        sleep.set_tail(Some(0));
        for _ in 0..100 {
            assert!(!run_block(&mut sleep, quiet(), CLAP_PROCESS_CONTINUE, true));
        }
        assert!(!sleep.is_asleep());
    }

    #[test]
    fn continue_does_not_ask_for_an_input_scan() {
        let mut sleep = ProcessSleep::new();
        run_block(&mut sleep, quiet(), CLAP_PROCESS_CONTINUE, true);
        assert!(!sleep.wants_input_scan());
    }

    #[test]
    fn sleep_status_sleeps_once_the_output_is_silent() {
        let mut sleep = ProcessSleep::new();
        assert!(!run_block(&mut sleep, quiet(), CLAP_PROCESS_SLEEP, true));
        assert!(sleep.is_asleep());
        assert!(run_block(&mut sleep, quiet(), CLAP_PROCESS_CONTINUE, true));
    }

    #[test]
    fn a_plugin_that_sleeps_while_still_sounding_stays_awake() {
        let mut sleep = ProcessSleep::new();
        run_block(&mut sleep, quiet(), CLAP_PROCESS_SLEEP, false);
        assert!(!sleep.is_asleep());
        assert!(!run_block(&mut sleep, quiet(), CLAP_PROCESS_SLEEP, false));
    }

    #[test]
    fn every_incoming_signal_wakes_a_sleeping_plugin() {
        let wakers: [fn(&mut BlockActivity); 4] = [
            |a| a.has_events = true,
            |a| a.input_audible = true,
            |a| a.process_requested = true,
            |a| a.playing = true,
        ];
        for waker in wakers {
            let mut sleep = ProcessSleep::new();
            run_block(&mut sleep, quiet(), CLAP_PROCESS_SLEEP, true);
            assert!(sleep.is_asleep());
            assert!(!sleep.begin_block(with(waker)));
            assert!(!sleep.is_asleep());
        }
    }

    #[test]
    fn a_tail_keeps_the_plugin_running_until_the_input_has_been_quiet_long_enough() {
        let mut sleep = ProcessSleep::new();
        sleep.set_tail(Some(FRAMES * 3));
        run_block(
            &mut sleep,
            with(|a| a.input_audible = true),
            CLAP_PROCESS_TAIL,
            true,
        );
        for quiet_blocks in 1..3 {
            run_block(&mut sleep, quiet(), CLAP_PROCESS_TAIL, true);
            assert!(
                !sleep.is_asleep(),
                "asleep after only {quiet_blocks} quiet blocks"
            );
        }
        run_block(&mut sleep, quiet(), CLAP_PROCESS_TAIL, true);
        assert!(sleep.is_asleep(), "the whole tail has elapsed");
    }

    #[test]
    fn a_sound_in_the_tail_is_never_cut() {
        let mut sleep = ProcessSleep::new();
        sleep.set_tail(Some(FRAMES));
        run_block(
            &mut sleep,
            quiet(),
            CLAP_PROCESS_CONTINUE_IF_NOT_QUIET,
            true,
        );
        for _ in 0..5 {
            run_block(
                &mut sleep,
                quiet(),
                CLAP_PROCESS_CONTINUE_IF_NOT_QUIET,
                false,
            );
            assert!(!sleep.is_asleep());
        }
    }

    #[test]
    fn input_during_the_tail_restarts_the_count() {
        let mut sleep = ProcessSleep::new();
        sleep.set_tail(Some(FRAMES * 2));
        run_block(&mut sleep, quiet(), CLAP_PROCESS_TAIL, false);
        run_block(&mut sleep, quiet(), CLAP_PROCESS_TAIL, true);
        run_block(
            &mut sleep,
            with(|a| a.input_audible = true),
            CLAP_PROCESS_TAIL,
            true,
        );
        assert!(!sleep.is_asleep());
        run_block(&mut sleep, quiet(), CLAP_PROCESS_TAIL, true);
        assert!(!sleep.is_asleep());
        run_block(&mut sleep, quiet(), CLAP_PROCESS_TAIL, true);
        assert!(sleep.is_asleep());
    }

    #[test]
    fn events_restart_the_count_too() {
        let mut sleep = ProcessSleep::new();
        sleep.set_tail(Some(FRAMES * 2));
        for _ in 0..5 {
            run_block(
                &mut sleep,
                with(|a| a.has_events = true),
                CLAP_PROCESS_TAIL,
                true,
            );
        }
        assert!(!sleep.is_asleep());
    }

    #[test]
    fn without_a_tail_only_the_sleep_status_sleeps() {
        for status in [CLAP_PROCESS_CONTINUE_IF_NOT_QUIET, CLAP_PROCESS_TAIL] {
            let mut sleep = ProcessSleep::new();
            sleep.set_tail(None);
            for _ in 0..1000 {
                assert!(!run_block(&mut sleep, quiet(), status, true));
            }
            assert!(!sleep.is_asleep());
        }
    }

    #[test]
    fn an_infinite_tail_counts_as_no_tail() {
        let mut sleep = ProcessSleep::new();
        sleep.set_tail(Some(u32::MAX));
        for _ in 0..1000 {
            run_block(&mut sleep, quiet(), CLAP_PROCESS_TAIL, true);
        }
        assert!(!sleep.is_asleep());
    }

    #[test]
    fn an_error_status_keeps_the_plugin_awake() {
        let mut sleep = ProcessSleep::new();
        run_block(&mut sleep, quiet(), CLAP_PROCESS_SLEEP, true);
        assert!(sleep.is_asleep());
        run_block(
            &mut sleep,
            with(|a| a.has_events = true),
            CLAP_PROCESS_ERROR,
            true,
        );
        assert!(!sleep.is_asleep());
    }

    #[test]
    fn wake_cancels_sleep_and_the_count() {
        let mut sleep = ProcessSleep::new();
        run_block(&mut sleep, quiet(), CLAP_PROCESS_SLEEP, true);
        sleep.wake();
        assert!(!sleep.is_asleep());
        assert!(!sleep.begin_block(quiet()));
    }

    #[test]
    fn a_sleeping_plugin_asks_for_the_input_scan_to_wake_on_a_signal() {
        let mut sleep = ProcessSleep::new();
        run_block(&mut sleep, quiet(), CLAP_PROCESS_SLEEP, true);
        assert!(sleep.wants_input_scan());
    }

    #[test]
    fn the_decision_only_depends_on_the_signal() {
        let script = |sleep: &mut ProcessSleep| -> Vec<bool> {
            sleep.set_tail(Some(FRAMES * 2));
            (0..12)
                .map(|block| {
                    let activity = with(|a| a.input_audible = block < 3);
                    run_block(sleep, activity, CLAP_PROCESS_TAIL, block >= 3)
                })
                .collect()
        };
        assert_eq!(
            script(&mut ProcessSleep::new()),
            script(&mut ProcessSleep::new())
        );
    }
}
