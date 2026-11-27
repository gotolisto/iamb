//! The answer to `secure_state`, step by step, against a small [`Desk`] trait and the
//! inspection thread. No system call: `os` implements the trait and the tests fake it.
//!
//! 1. The window asked about must be the one in front, before anything is read. Otherwise
//!    the answer is `Unknown(FocusMoved)`, or `Unknown(NoFocus)` when nothing is in front.
//! 2. A window of a higher privilege level than Iamb's is `Unknown(Elevated)`: Windows
//!    refuses both the inspection and the injection there.
//! 3. The control with the keyboard focus is looked at by its window class and style
//!    (`classify`). A system edit control answers the question by itself.
//! 4. For any other control the inspection thread asks UI Automation, within what is left of
//!    the caller's time. No answer in time is `Unknown(Timeout)`. While the watcher runs, the
//!    question names the window and the watcher's count of reported focus changes (`epoch`),
//!    so that an answer that came too late for the question before serves this one when
//!    nothing has changed since (`inspect`, "A late answer").
//! 5. The window must still be in front. If it is not, the answer just obtained may be about
//!    another window and is thrown away: `Unknown(FocusMoved)`.
//!
//! Steps 1 to 3 and 5 read values the system keeps itself and never wait for the
//! application; only step 4 can, and it has the limit.

use std::time::{Duration, Instant};

use iamb_platform::{FieldSecurity, FocusToken, UnknownReason};

use super::classify::{ControlKind, Judgement, classify};
use super::inspect::{Answer, Inspector, ProbeSource, Subject};

/// The window in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Front {
    /// The window handle, as a number.
    pub(crate) window: u64,
    /// The process that owns the window.
    pub(crate) pid: u32,
}

impl Front {
    /// The token that names this window.
    pub(crate) const fn token(self) -> FocusToken {
        FocusToken::from_raw(self.pid, self.window)
    }
}

/// How a process's privilege level compares with Iamb's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Privilege {
    /// The same or lower: Iamb may inspect it and type into it.
    NotHigher,
    /// Higher: Windows refuses both.
    Higher,
    /// The system did not say.
    Unknown,
}

/// What `secure_state` reads from the system without waiting for an application.
pub(crate) trait Desk {
    /// The window in front and its process.
    fn front(&self) -> Option<Front>;
    /// The privilege level of process `pid`, compared with this process's.
    fn privilege(&self, pid: u32) -> Privilege;
    /// The kind of the control that has the keyboard focus in `front`.
    fn control(&self, front: Front) -> ControlKind;
}

/// `Ok` when `target` is the window in front.
fn in_front<D: Desk>(desk: &D, target: &FocusToken) -> Result<Front, UnknownReason> {
    let front = desk.front().ok_or(UnknownReason::NoFocus)?;
    if front.token() == *target {
        Ok(front)
    } else {
        Err(UnknownReason::FocusMoved)
    }
}

/// Whether the focused field of window `target` is a password field, waiting at most
/// `timeout` for the application. `epoch` is the watcher's count of the focus changes it has
/// reported, `None` when no watcher runs (then no late answer is used).
pub(crate) fn judge<D: Desk, S: ProbeSource>(
    desk: &D,
    inspector: &Inspector<S>,
    target: &FocusToken,
    epoch: Option<u64>,
    timeout: Duration,
) -> Judgement {
    let started = Instant::now();
    let unknown = |reason| Judgement::settled(FieldSecurity::Unknown(reason));
    let front = match in_front(desk, target) {
        Ok(front) => front,
        Err(reason) => return unknown(reason),
    };
    match desk.privilege(front.pid) {
        Privilege::NotHigher => {}
        Privilege::Higher => return unknown(UnknownReason::Elevated),
        Privilege::Unknown => return unknown(UnknownReason::NotInspectable),
    }
    let judgement = match desk.control(front) {
        ControlKind::PasswordEdit => Judgement::settled(FieldSecurity::Secure),
        ControlKind::PlainEdit => Judgement::settled(FieldSecurity::Normal),
        kind @ (ControlKind::WebEngine | ControlKind::Other) => {
            let left = timeout.saturating_sub(started.elapsed());
            if left.is_zero() {
                // The caller gave no time to ask in; asking again would be no different.
                unknown(UnknownReason::Timeout)
            } else {
                let subject = epoch.map(|epoch| Subject {
                    window: target.window(),
                    epoch,
                });
                match inspector.observe(subject, left) {
                    Answer::Observed(observation) => classify(kind, observation),
                    Answer::TimedOut => Judgement::passing(UnknownReason::Timeout),
                    Answer::Unavailable => Judgement::passing(UnknownReason::NotInspectable),
                }
            }
        }
    };
    match in_front(desk, target) {
        Ok(_) => judgement,
        Err(reason) => unknown(reason),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    use super::super::classify::Observation;
    use super::super::inspect::fake::{FakeSource, Step, hang};
    use super::*;

    const LONG: Duration = Duration::from_secs(10);
    const SHORT: Duration = Duration::from_millis(30);

    const EDITOR: Front = Front {
        window: 0x1_0000_0A10,
        pid: 40,
    };
    const OTHER: Front = Front {
        window: 0x1_0000_0B20,
        pid: 41,
    };

    const fn element(password: bool, control_type: i32) -> Observation {
        Observation::Element {
            password,
            control_type,
        }
    }

    /// A desk that follows a script: each call to `front` takes the next entry, and the last
    /// one stays.
    struct FakeDesk {
        fronts: RefCell<VecDeque<Option<Front>>>,
        privilege: Privilege,
        control: ControlKind,
        controls_read: Cell<u32>,
    }

    impl FakeDesk {
        fn showing(control: ControlKind) -> Self {
            Self {
                fronts: RefCell::new(VecDeque::from([Some(EDITOR)])),
                privilege: Privilege::NotHigher,
                control,
                controls_read: Cell::new(0),
            }
        }

        fn fronts(self, fronts: &[Option<Front>]) -> Self {
            *self.fronts.borrow_mut() = fronts.iter().copied().collect();
            self
        }
    }

    impl Desk for FakeDesk {
        fn front(&self) -> Option<Front> {
            let mut fronts = self.fronts.borrow_mut();
            if fronts.len() > 1 {
                fronts.pop_front().flatten()
            } else {
                fronts.front().copied().flatten()
            }
        }

        fn privilege(&self, _pid: u32) -> Privilege {
            self.privilege
        }

        fn control(&self, _front: Front) -> ControlKind {
            self.controls_read.set(self.controls_read.get() + 1);
            self.control
        }
    }

    fn unknown(reason: UnknownReason) -> Judgement {
        Judgement::settled(FieldSecurity::Unknown(reason))
    }

    #[test]
    fn a_system_edit_control_answers_without_asking_the_application() {
        for (control, field) in [
            (ControlKind::PasswordEdit, FieldSecurity::Secure),
            (ControlKind::PlainEdit, FieldSecurity::Normal),
        ] {
            let source = FakeSource::default();
            let inspector = Inspector::new(source.clone());
            let desk = FakeDesk::showing(control);
            // Even without any time to wait in.
            assert_eq!(
                judge(&desk, &inspector, &EDITOR.token(), None, Duration::ZERO),
                Judgement::settled(field)
            );
            assert_eq!(source.0.asked(), 0);
            assert_eq!(inspector.workers(), 0, "no inspection thread was needed");
        }
    }

    #[test]
    fn other_controls_are_what_ui_automation_says() {
        for (control, observation, expected) in [
            (
                ControlKind::Other,
                element(false, 50004),
                Judgement::settled(FieldSecurity::Normal),
            ),
            (
                ControlKind::Other,
                element(true, 50004),
                Judgement::settled(FieldSecurity::Secure),
            ),
            (
                ControlKind::WebEngine,
                element(true, 50004),
                Judgement::settled(FieldSecurity::Secure),
            ),
            (
                ControlKind::WebEngine,
                element(false, 50004),
                Judgement::settled(FieldSecurity::Normal),
            ),
            // The browser shows its window, not a field: not believed, asked again.
            (
                ControlKind::WebEngine,
                element(false, 50033),
                Judgement::passing(UnknownReason::NotInspectable),
            ),
            (
                ControlKind::Other,
                Observation::Failed,
                Judgement::passing(UnknownReason::NotInspectable),
            ),
        ] {
            let source = FakeSource::answering(&[observation]);
            let inspector = Inspector::new(source.clone());
            let desk = FakeDesk::showing(control);
            assert_eq!(
                judge(&desk, &inspector, &EDITOR.token(), None, LONG),
                expected,
                "{control:?} {observation:?}"
            );
            assert_eq!(source.0.asked(), 1);
        }
    }

    #[test]
    fn the_answer_is_only_ever_about_the_window_asked_for() {
        let source = FakeSource::answering(&[element(false, 50004)]);
        let inspector = Inspector::new(source.clone());

        // Another window is in front: nothing is read, nothing is asked.
        let desk = FakeDesk::showing(ControlKind::Other).fronts(&[Some(OTHER)]);
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), None, LONG),
            unknown(UnknownReason::FocusMoved)
        );
        assert_eq!(desk.controls_read.get(), 0);

        // The same window handle in another process is another window (handles are reused).
        let reused = Front {
            window: EDITOR.window,
            pid: 99,
        };
        let desk = FakeDesk::showing(ControlKind::Other).fronts(&[Some(reused)]);
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), None, LONG),
            unknown(UnknownReason::FocusMoved)
        );

        // Nothing is in front.
        let desk = FakeDesk::showing(ControlKind::Other).fronts(&[None]);
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), None, LONG),
            unknown(UnknownReason::NoFocus)
        );
        assert_eq!(source.0.asked(), 0);
    }

    #[test]
    fn an_answer_obtained_while_the_focus_moved_is_thrown_away() {
        for (after, reason) in [
            (Some(OTHER), UnknownReason::FocusMoved),
            (None, UnknownReason::NoFocus),
        ] {
            // "Normal" from the probe, and "secure" from the control itself: neither counts.
            let source = FakeSource::answering(&[element(false, 50004)]);
            let inspector = Inspector::new(source.clone());
            let desk = FakeDesk::showing(ControlKind::Other).fronts(&[Some(EDITOR), after]);
            assert_eq!(
                judge(&desk, &inspector, &EDITOR.token(), None, LONG),
                unknown(reason)
            );
            assert_eq!(source.0.asked(), 1);

            let desk = FakeDesk::showing(ControlKind::PasswordEdit).fronts(&[Some(EDITOR), after]);
            assert_eq!(
                judge(&desk, &inspector, &EDITOR.token(), None, LONG),
                unknown(reason)
            );
        }
    }

    #[test]
    fn a_window_of_a_higher_privilege_level_is_not_inspected() {
        for (privilege, reason) in [
            (Privilege::Higher, UnknownReason::Elevated),
            (Privilege::Unknown, UnknownReason::NotInspectable),
        ] {
            let source = FakeSource::answering(&[element(false, 50004)]);
            let inspector = Inspector::new(source.clone());
            let mut desk = FakeDesk::showing(ControlKind::PlainEdit);
            desk.privilege = privilege;
            assert_eq!(
                judge(&desk, &inspector, &EDITOR.token(), None, LONG),
                unknown(reason)
            );
            assert_eq!(desk.controls_read.get(), 0);
            assert_eq!(source.0.asked(), 0);
        }
    }

    #[test]
    fn an_application_that_does_not_answer_in_time_is_a_timeout() {
        let source = FakeSource::default();
        let (step, release) = hang(element(false, 50004));
        source.0.push(step);
        let inspector = Inspector::new(source);
        inspector.wait_open();
        let desk = FakeDesk::showing(ControlKind::Other);

        let started = Instant::now();
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), None, SHORT),
            Judgement::passing(UnknownReason::Timeout)
        );
        let waited = started.elapsed();
        assert!(waited >= SHORT, "{waited:?}");
        assert!(waited < Duration::from_secs(5), "{waited:?}");
        drop(release);
    }

    #[test]
    fn without_time_the_application_is_not_asked() {
        let source = FakeSource::answering(&[element(false, 50004)]);
        let inspector = Inspector::new(source.clone());
        let desk = FakeDesk::showing(ControlKind::Other);
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), None, Duration::ZERO),
            unknown(UnknownReason::Timeout)
        );
        assert_eq!(source.0.asked(), 0);
    }

    #[test]
    fn a_slow_application_is_judged_by_its_late_answer_while_nothing_changed() {
        for (observation, field) in [
            (element(false, 50004), FieldSecurity::Normal),
            (element(true, 50004), FieldSecurity::Secure),
        ] {
            let source = FakeSource::default();
            let (step, release) = hang(observation);
            source.0.push(step);
            let inspector = Inspector::new(source.clone());
            inspector.wait_open();
            let desk = FakeDesk::showing(ControlKind::WebEngine);
            assert_eq!(
                judge(&desk, &inspector, &EDITOR.token(), Some(3), SHORT),
                Judgement::passing(UnknownReason::Timeout)
            );
            release.send(()).expect("the thread still waits");
            // The watcher reports the window again (no focus change since): the answer that
            // came late is the one of the window's field.
            assert_eq!(
                judge(&desk, &inspector, &EDITOR.token(), Some(3), LONG),
                Judgement::settled(field)
            );
            assert_eq!(source.0.asked(), 1);
        }
    }

    #[test]
    fn a_late_answer_is_not_taken_after_a_focus_change_or_without_a_watcher() {
        for later in [Some(4), None] {
            let source = FakeSource::default();
            let (step, release) = hang(element(false, 50004));
            source.0.push(step);
            source.0.push(Step::Answer(element(true, 50004)));
            let inspector = Inspector::new(source.clone());
            inspector.wait_open();
            let desk = FakeDesk::showing(ControlKind::WebEngine);
            assert_eq!(
                judge(&desk, &inspector, &EDITOR.token(), Some(3), SHORT),
                Judgement::passing(UnknownReason::Timeout)
            );
            release.send(()).expect("the thread still waits");
            // The user moved into a password field (a focus change was reported): the late
            // "plain field" is about the field before and is not taken.
            assert_eq!(
                judge(&desk, &inspector, &EDITOR.token(), later, LONG),
                Judgement::settled(FieldSecurity::Secure),
                "{later:?}"
            );
            assert_eq!(source.0.asked(), 2);
        }
    }

    #[test]
    fn a_late_answer_for_a_window_no_longer_in_front_is_never_taken() {
        let source = FakeSource::default();
        let (step, release) = hang(element(false, 50004));
        source.0.push(step);
        let inspector = Inspector::new(source.clone());
        inspector.wait_open();
        let desk = FakeDesk::showing(ControlKind::Other);
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), Some(3), SHORT),
            Judgement::passing(UnknownReason::Timeout)
        );
        release.send(()).expect("the thread still waits");
        let desk = FakeDesk::showing(ControlKind::Other).fronts(&[Some(OTHER)]);
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), Some(3), LONG),
            unknown(UnknownReason::FocusMoved)
        );
    }

    #[test]
    fn a_probe_that_cannot_be_opened_is_not_normal() {
        let source = FakeSource::answering(&[element(false, 50004)]);
        *source.0.refuse_open.lock().expect("not poisoned") = true;
        let inspector = Inspector::new(source);
        let desk = FakeDesk::showing(ControlKind::Other);
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), None, LONG),
            Judgement::passing(UnknownReason::NotInspectable)
        );
    }
}
