//! Keeping the event tap alive: what the listener does when the system disables its tap.
//!
//! macOS disables an event tap whose callback is slow (`kCGEventTapDisabledByTimeout`) and in
//! some cases on user input (`kCGEventTapDisabledByUserInput`), and tells the callback so with
//! a pseudo-event. A tap can also be found disabled without any notice. In every case events
//! were lost, so the listener re-enables the tap and delivers `Overflow` (ADR-0010).
//!
//! | Found by | When | Recovery time |
//! | --- | --- | --- |
//! | [`Recovery::disabled`], from the tap callback | at once, in the callback that carried the notice | that callback |
//! | [`Recovery::check`], from the listener thread's watchdog | every [`WATCHDOG_INTERVAL`] | at most one interval |
//! | [`Recovery::check`], from `self_test` | on a self-test that did not see its event | that call |
//!
//! A disable the process asks for itself (`CGEventTapEnable(tap, false)`) is announced as well,
//! with `kCGEventTapDisabledByUserInput` (seen on macOS 26.7). The tests that need a tap
//! disabled *without* notice therefore keep that one notice from the listener
//! (`Request::DisableTap` in `tap.rs`).
//!
//! Both bounds are far below the one second the plan allows. A tap that will not stay enabled
//! (the user revoked the permission while the listener ran) is reported as
//! [`TapHealth::Lost`]; the worker's self-test then fails and the worker reinstalls the
//! listener (architecture §3.2).
//!
//! The machine is plain data driven through the [`TapSwitch`] seam, so it is tested without an
//! event tap, on every system.

use std::time::{Duration, Instant};

/// How often the listener thread looks at its tap when no event tells it to.
pub(crate) const WATCHDOG_INTERVAL: Duration = Duration::from_millis(250);

/// After this many looks in a row at a tap that would not enable, the tap counts as lost.
const LOST_AFTER_FAILED_CHECKS: u32 = 4;

/// The switch of an event tap (`CGEventTapEnable`, `CGEventTapIsEnabled`).
pub(crate) trait TapSwitch {
    /// Enables the tap.
    fn enable(&mut self);
    /// Whether the tap is enabled now.
    fn is_enabled(&self) -> bool;
}

/// How the listener learnt that its tap was disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisableReason {
    /// The system's notice `kCGEventTapDisabledByTimeout`.
    Timeout,
    /// The system's notice `kCGEventTapDisabledByUserInput`.
    UserInput,
    /// No notice: the watchdog or a self-test found the tap disabled.
    Unannounced,
}

/// What a look at the tap found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TapCheck {
    /// The tap was enabled.
    Enabled,
    /// The tap was disabled and is enabled again. Events were lost.
    Reenabled,
    /// The tap is disabled and would not enable.
    StillDisabled,
}

/// The state of the tap, for the listener's health report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TapHealth {
    /// Enabled, as far as the listener knows.
    Working,
    /// Disabled since `since`; the listener is trying to enable it.
    Recovering {
        /// When the tap was found disabled.
        since: Instant,
    },
    /// Disabled since `since` and it will not enable.
    Lost {
        /// When the tap was found disabled.
        since: Instant,
    },
}

/// Counters of the recovery machine. No user data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RecoveryStats {
    /// Notices of a disable by timeout.
    pub(crate) by_timeout: u64,
    /// Notices of a disable by user input.
    pub(crate) by_user_input: u64,
    /// Times the tap was found disabled without a notice.
    pub(crate) unannounced: u64,
    /// Times the tap came back.
    pub(crate) recoveries: u64,
    /// How long the last recovery took, from finding the tap disabled to seeing it enabled.
    pub(crate) last_recovery: Option<Duration>,
    /// The longest recovery so far.
    pub(crate) longest_recovery: Option<Duration>,
}

/// The recovery state machine of one event tap.
#[derive(Debug, Default)]
pub(crate) struct Recovery {
    down_since: Option<Instant>,
    failed_checks: u32,
    stats: RecoveryStats,
}

impl Recovery {
    /// A machine for a tap that was just enabled.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The system told the callback that it disabled the tap: enable it again at once.
    ///
    /// Events were lost whatever the result, so the caller delivers `Overflow`. Returns
    /// whether the tap is enabled again.
    pub(crate) fn disabled(
        &mut self,
        reason: DisableReason,
        now: Instant,
        tap: &mut dyn TapSwitch,
    ) -> bool {
        self.count(reason);
        self.down_since.get_or_insert(now);
        self.try_enable(now, tap)
    }

    /// Looks at the tap and enables it if it is disabled. On
    /// [`Reenabled`](TapCheck::Reenabled) the caller delivers `Overflow`.
    pub(crate) fn check(&mut self, now: Instant, tap: &mut dyn TapSwitch) -> TapCheck {
        if tap.is_enabled() {
            if self.down_since.is_some() {
                // An earlier attempt to enable took effect in the meantime. The keys typed
                // while the tap was down are gone, so this is a loss to announce as well.
                self.recovered(now);
                return TapCheck::Reenabled;
            }
            return TapCheck::Enabled;
        }
        if self.down_since.is_none() {
            self.count(DisableReason::Unannounced);
            self.down_since = Some(now);
        }
        if self.try_enable(now, tap) {
            TapCheck::Reenabled
        } else {
            TapCheck::StillDisabled
        }
    }

    /// The state of the tap as far as the machine knows.
    pub(crate) const fn health(&self) -> TapHealth {
        match self.down_since {
            None => TapHealth::Working,
            Some(since) if self.failed_checks >= LOST_AFTER_FAILED_CHECKS => {
                TapHealth::Lost { since }
            }
            Some(since) => TapHealth::Recovering { since },
        }
    }

    /// The counters.
    pub(crate) const fn stats(&self) -> RecoveryStats {
        self.stats
    }

    fn try_enable(&mut self, now: Instant, tap: &mut dyn TapSwitch) -> bool {
        tap.enable();
        if tap.is_enabled() {
            self.recovered(now);
            true
        } else {
            self.failed_checks = self.failed_checks.saturating_add(1);
            false
        }
    }

    fn recovered(&mut self, now: Instant) {
        if let Some(since) = self.down_since.take() {
            let took = now.saturating_duration_since(since);
            self.stats.recoveries += 1;
            self.stats.last_recovery = Some(took);
            self.stats.longest_recovery =
                Some(self.stats.longest_recovery.map_or(took, |d| d.max(took)));
        }
        self.failed_checks = 0;
    }

    const fn count(&mut self, reason: DisableReason) {
        match reason {
            DisableReason::Timeout => self.stats.by_timeout += 1,
            DisableReason::UserInput => self.stats.by_user_input += 1,
            DisableReason::Unannounced => self.stats.unannounced += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::FakeTap;
    use super::*;

    const KPI: Duration = Duration::from_secs(1);

    #[test]
    fn a_notice_from_the_system_is_answered_in_the_same_callback() {
        for reason in [DisableReason::Timeout, DisableReason::UserInput] {
            let mut tap = FakeTap::enabled();
            let mut recovery = Recovery::new();
            let now = Instant::now();
            tap.system_disables();

            assert!(recovery.disabled(reason, now, &mut tap));
            assert!(tap.is_enabled());
            assert_eq!(tap.enable_calls(), 1);
            assert_eq!(recovery.health(), TapHealth::Working);
            let stats = recovery.stats();
            assert_eq!(stats.recoveries, 1);
            assert_eq!(stats.last_recovery, Some(Duration::ZERO));
            assert_eq!(
                (stats.by_timeout, stats.by_user_input, stats.unannounced),
                match reason {
                    DisableReason::Timeout => (1, 0, 0),
                    _ => (0, 1, 0),
                }
            );
        }
    }

    #[test]
    fn a_tap_disabled_without_notice_is_found_within_one_watchdog_interval() {
        let mut tap = FakeTap::enabled();
        let mut recovery = Recovery::new();
        let start = Instant::now();
        assert_eq!(recovery.check(start, &mut tap), TapCheck::Enabled);
        assert_eq!(tap.enable_calls(), 0);

        // The tap goes down right after a look; the next look is one interval later.
        tap.system_disables();
        let next = start + WATCHDOG_INTERVAL;
        assert_eq!(recovery.check(next, &mut tap), TapCheck::Reenabled);
        assert!(tap.is_enabled());
        assert_eq!(recovery.stats().unannounced, 1);
        assert_eq!(recovery.stats().last_recovery, Some(Duration::ZERO));
        assert_eq!(recovery.health(), TapHealth::Working);
        // From going down to being enabled again: at most the interval, far below the KPI.
        assert!(next.duration_since(start) < KPI);
        assert!(WATCHDOG_INTERVAL * 2 < KPI);

        assert_eq!(
            recovery.check(next + WATCHDOG_INTERVAL, &mut tap),
            TapCheck::Enabled
        );
        assert_eq!(tap.enable_calls(), 1);
    }

    #[test]
    fn a_tap_that_will_not_enable_is_retried_and_then_reported_lost() {
        let mut tap = FakeTap::enabled();
        let mut recovery = Recovery::new();
        let start = Instant::now();
        tap.system_disables();
        tap.refuse_to_enable(true);

        assert!(!recovery.disabled(DisableReason::UserInput, start, &mut tap));
        assert_eq!(recovery.health(), TapHealth::Recovering { since: start });
        let mut now = start;
        for _ in 0..2 {
            now += WATCHDOG_INTERVAL;
            assert_eq!(recovery.check(now, &mut tap), TapCheck::StillDisabled);
            assert_eq!(recovery.health(), TapHealth::Recovering { since: start });
        }
        now += WATCHDOG_INTERVAL;
        assert_eq!(recovery.check(now, &mut tap), TapCheck::StillDisabled);
        assert_eq!(recovery.health(), TapHealth::Lost { since: start });
        // One disable, however many attempts.
        assert_eq!(recovery.stats().by_user_input, 1);
        assert_eq!(recovery.stats().unannounced, 0);
        assert_eq!(recovery.stats().recoveries, 0);
        assert_eq!(tap.enable_calls(), 4);

        // The permission comes back: the next look recovers and measures the whole outage.
        tap.refuse_to_enable(false);
        now += WATCHDOG_INTERVAL;
        assert_eq!(recovery.check(now, &mut tap), TapCheck::Reenabled);
        assert_eq!(recovery.health(), TapHealth::Working);
        assert_eq!(recovery.stats().last_recovery, Some(WATCHDOG_INTERVAL * 4));
        assert_eq!(
            recovery.stats().longest_recovery,
            Some(WATCHDOG_INTERVAL * 4)
        );
    }

    #[test]
    fn an_enable_that_takes_effect_late_is_still_announced_once() {
        let mut tap = FakeTap::enabled();
        let mut recovery = Recovery::new();
        let start = Instant::now();
        tap.system_disables();
        tap.refuse_to_enable(true);
        assert!(!recovery.disabled(DisableReason::Timeout, start, &mut tap));

        // The system enabled the tap between two looks.
        tap.refuse_to_enable(false);
        tap.enable();
        let later = start + Duration::from_millis(40);
        assert_eq!(recovery.check(later, &mut tap), TapCheck::Reenabled);
        assert_eq!(recovery.check(later, &mut tap), TapCheck::Enabled);
        assert_eq!(recovery.stats().recoveries, 1);
        assert_eq!(
            recovery.stats().last_recovery,
            Some(Duration::from_millis(40))
        );
    }

    #[test]
    fn the_longest_recovery_is_kept() {
        let mut tap = FakeTap::enabled();
        let mut recovery = Recovery::new();
        let start = Instant::now();

        tap.system_disables();
        tap.refuse_to_enable(true);
        assert_eq!(recovery.check(start, &mut tap), TapCheck::StillDisabled);
        tap.refuse_to_enable(false);
        assert_eq!(
            recovery.check(start + Duration::from_millis(300), &mut tap),
            TapCheck::Reenabled
        );

        tap.system_disables();
        assert!(recovery.disabled(
            DisableReason::Timeout,
            start + Duration::from_secs(5),
            &mut tap
        ));
        let stats = recovery.stats();
        assert_eq!(stats.recoveries, 2);
        assert_eq!(stats.last_recovery, Some(Duration::ZERO));
        assert_eq!(stats.longest_recovery, Some(Duration::from_millis(300)));
        assert_eq!((stats.by_timeout, stats.unannounced), (1, 1));
    }

    #[test]
    fn a_repeated_notice_does_not_restart_the_clock() {
        let mut tap = FakeTap::enabled();
        let mut recovery = Recovery::new();
        let start = Instant::now();
        tap.system_disables();
        tap.refuse_to_enable(true);
        assert!(!recovery.disabled(DisableReason::Timeout, start, &mut tap));
        assert!(!recovery.disabled(
            DisableReason::Timeout,
            start + Duration::from_millis(10),
            &mut tap
        ));
        assert_eq!(recovery.health(), TapHealth::Recovering { since: start });
        assert_eq!(recovery.stats().by_timeout, 2);
    }
}
