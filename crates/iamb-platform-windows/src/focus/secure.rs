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
//!    question names the window, its focused control and that control's kind, and the count of
//!    focus changes (`epoch`). An answer that came too late for an earlier question about the
//!    same subject serves a report the watcher made again ([`Watching::again`]) if it arrived
//!    at most a quarter of a second ago and its call took at most a quarter of a second
//!    (`inspect`, "A late answer"), and only when the element it describes is provided by the
//!    window's process, by the process of its focused control's window, or by a process that
//!    owns a window on that control's own line: one of its ancestors up to the window in front,
//!    or one of its descendants (an embedded browser engine runs several processes, and the one
//!    that answers for the page need not own the control the system names as focused). A
//!    process with a window elsewhere inside the window in front (a preview pane, a plug-in
//!    beside the page) is not accepted; any element it provides is `Unknown(NotInspectable)`.
//!    Process ancestry is not looked at: it cannot be read reliably (a parent may have ended
//!    and its id been given to another process), so the processes are the ones the system
//!    names as owners of those windows.
//! 5. The window must still be in front. If it is not, the answer just obtained may be about
//!    another window and is thrown away: `Unknown(FocusMoved)`.
//!
//! Steps 1 to 3 and 5 read values the system keeps itself and never wait for the
//! application; only step 4 can, and it has the limit.

use std::time::{Duration, Instant};

use iamb_platform::{FieldSecurity, FocusToken, UnknownReason};

use super::classify::{ControlKind, Judgement, Observation, classify, custom_candidate};
use super::inspect::{Answer, Inspector, Lateness, ProbeSource, Subject, Wake};

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

/// The control with the keyboard focus in the window in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Control {
    /// Its window handle: the window in front itself when the system names no control.
    pub(crate) window: u64,
    /// The process that owns that window.
    pub(crate) pid: u32,
    /// Its kind.
    pub(crate) kind: ControlKind,
}

/// What `secure_state` reads from the system without waiting for an application.
pub(crate) trait Desk {
    /// The window in front and its process.
    fn front(&self) -> Option<Front>;
    /// The privilege level of process `pid`, compared with this process's.
    fn privilege(&self, pid: u32) -> Privilege;
    /// The control that has the keyboard focus in `front`.
    fn control(&self, front: Front) -> Control;
    /// True when process `pid` owns a window on the line of `control`, the focused control's
    /// window in `front`: one of its ancestors up to `front`, or one of its descendants. A web
    /// view's engine process does, for the window it draws the page in.
    fn on_control_line(&self, front: Front, control: u64, pid: u32) -> bool;
}

/// What the running watcher tells a question (`None` to [`judge`] without one).
pub(crate) struct Watching {
    /// The count of focus changes, as it is now.
    pub(crate) epoch: u64,
    /// The question is for a report the watcher made again of the same window (after a pause,
    /// or because a late answer arrived), not for a change: only then may it take a late
    /// answer.
    pub(crate) again: bool,
    /// Has the watcher report the window again, when this question's answer comes late.
    pub(crate) wake: Option<Wake>,
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
/// `timeout` for the application. `watching` is `None` when no watcher runs: then no late
/// answer is kept or used.
pub(crate) fn judge<D: Desk, S: ProbeSource>(
    desk: &D,
    inspector: &Inspector<S>,
    target: &FocusToken,
    watching: Option<Watching>,
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
    let control = desk.control(front);
    let judgement = match control.kind {
        ControlKind::PasswordEdit => Judgement::settled(FieldSecurity::Secure),
        ControlKind::PlainEdit => Judgement::settled(FieldSecurity::Normal),
        kind @ (ControlKind::WebEngine | ControlKind::Other) => {
            let left = timeout.saturating_sub(started.elapsed());
            if left.is_zero() {
                // The caller gave no time to ask in; asking again would be no different.
                unknown(UnknownReason::Timeout)
            } else {
                let subject = watching.as_ref().map(|watching| Subject {
                    window: target.window(),
                    control: control.window,
                    kind,
                    epoch: watching.epoch,
                });
                let again = watching.as_ref().is_some_and(|watching| watching.again);
                let lateness = watching.map_or(Lateness::NONE, |watching| Lateness {
                    subject,
                    may_take: watching.again,
                    wake: watching.wake,
                });
                // A custom element is believed at a second look of the same subject only
                // (`classify`, "A control without a kind").
                let judged = |observation| {
                    let candidate = custom_candidate(kind, observation);
                    let confirmed = inspector.custom_again(subject, candidate) && again;
                    classify(kind, observation, confirmed)
                };
                match inspector.observe(lateness, left) {
                    Answer::Observed(observation) => judged(observation),
                    Answer::Late(observation) => {
                        if provided_by(observation, |process| {
                            process == front.pid
                                || process == control.pid
                                || desk.on_control_line(front, control.window, process)
                        }) {
                            judged(observation)
                        } else {
                            Judgement::passing(UnknownReason::NotInspectable)
                        }
                    }
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

/// False when `observation` is an element provided by a process `known` does not accept, or
/// by process 0 (module docs, step 4).
fn provided_by(observation: Observation, known: impl Fn(u32) -> bool) -> bool {
    match observation {
        Observation::Element { process, .. } => process != 0 && known(process),
        Observation::NoElement | Observation::Failed => true,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    use super::super::inspect::fake::{FakeSource, Step, hang};
    use super::*;

    const LONG: Duration = Duration::from_secs(10);
    const SHORT: Duration = Duration::from_millis(30);
    // The call limit (`LATE_CALL_LIMIT`) is set to `LONG` here: the fake's waits are counted in
    // `SHORT`s, and a slow runner stretches them past 250 ms; `inspect` tests the limit.

    const EDITOR: Front = Front {
        window: 0x1_0000_0A10,
        pid: 40,
    };
    const OTHER: Front = Front {
        window: 0x1_0000_0B20,
        pid: 41,
    };

    /// The engine's window inside `EDITOR`, owned by another process (a web view's).
    const ENGINE: Control = Control {
        window: 0x1_0000_0A20,
        pid: 50,
        kind: ControlKind::WebEngine,
    };

    /// An element provided by `EDITOR`'s process.
    const fn element(password: bool, control_type: i32) -> Observation {
        from(password, control_type, EDITOR.pid)
    }

    const fn from(password: bool, control_type: i32, process: u32) -> Observation {
        Observation::Element {
            password,
            control_type,
            process,
            detail: super::super::classify::Detail::NONE,
        }
    }

    /// A desk that follows a script: each call to `front` takes the next entry, and the last
    /// one stays.
    struct FakeDesk {
        fronts: RefCell<VecDeque<Option<Front>>>,
        privilege: Privilege,
        control: Cell<Control>,
        controls_read: Cell<u32>,
        /// Processes that own windows on the focused control's line.
        inner: Vec<u32>,
    }

    impl FakeDesk {
        fn showing(kind: ControlKind) -> Self {
            Self::with(Control { kind, ..ENGINE })
        }

        fn with(control: Control) -> Self {
            Self {
                fronts: RefCell::new(VecDeque::from([Some(EDITOR)])),
                privilege: Privilege::NotHigher,
                control: Cell::new(control),
                controls_read: Cell::new(0),
                inner: Vec::new(),
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

        fn control(&self, _front: Front) -> Control {
            self.controls_read.set(self.controls_read.get() + 1);
            self.control.get()
        }

        fn on_control_line(&self, _front: Front, _control: u64, pid: u32) -> bool {
            self.inner.contains(&pid)
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
            let inspector = Inspector::new(source.clone()).with_late_call_limit(LONG);
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
            let inspector = Inspector::new(source.clone()).with_late_call_limit(LONG);
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
        let inspector = Inspector::new(source.clone()).with_late_call_limit(LONG);

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
            let inspector = Inspector::new(source.clone()).with_late_call_limit(LONG);
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
            let inspector = Inspector::new(source.clone()).with_late_call_limit(LONG);
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
        let inspector = Inspector::new(source).with_late_call_limit(LONG);
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
        let inspector = Inspector::new(source.clone()).with_late_call_limit(LONG);
        let desk = FakeDesk::showing(ControlKind::Other);
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), None, Duration::ZERO),
            unknown(UnknownReason::Timeout)
        );
        assert_eq!(source.0.asked(), 0);
    }

    /// A question for a change (`again: false`) or a report again, with a wake that says on
    /// `woken` when a late answer arrived.
    #[allow(
        clippy::unnecessary_wraps,
        reason = "the argument of `judge`, beside `None` for no watcher"
    )]
    fn watching(epoch: u64, again: bool, woken: &std::sync::mpsc::Sender<()>) -> Option<Watching> {
        let woken = woken.clone();
        Some(Watching {
            epoch,
            again,
            wake: Some(Box::new(move || {
                let _ = woken.send(());
            })),
        })
    }

    /// The first look of a change times out with `observation` hanging; returns the inspector
    /// once the late answer has arrived and woken the watcher.
    fn late(
        source: &FakeSource,
        desk: &FakeDesk,
        observation: Observation,
    ) -> (Inspector<FakeSource>, std::sync::mpsc::Sender<()>) {
        let (step, release) = hang(observation);
        source.0.push(step);
        let inspector = Inspector::new(source.clone()).with_late_call_limit(LONG);
        inspector.wait_open();
        let (woken, wakes) = std::sync::mpsc::channel();
        assert_eq!(
            judge(
                desk,
                &inspector,
                &EDITOR.token(),
                watching(3, false, &woken),
                SHORT
            ),
            Judgement::passing(UnknownReason::Timeout)
        );
        release.send(()).expect("the thread still waits");
        wakes
            .recv_timeout(LONG)
            .expect("the arrival wakes the watcher");
        (inspector, woken)
    }

    #[test]
    fn a_slow_application_is_judged_by_its_late_answer_at_once() {
        for (observation, field) in [
            (element(false, 50004), FieldSecurity::Normal),
            (element(true, 50004), FieldSecurity::Secure),
            // An embedded engine's element, provided by the process of the engine's window.
            (from(false, 50004, ENGINE.pid), FieldSecurity::Normal),
        ] {
            let source = FakeSource::default();
            let desk = FakeDesk::with(ENGINE);
            let (inspector, woken) = late(&source, &desk, observation);
            // The watcher reports the window again at once: the late answer is its field's.
            assert_eq!(
                judge(
                    &desk,
                    &inspector,
                    &EDITOR.token(),
                    watching(3, true, &woken),
                    LONG
                ),
                Judgement::settled(field),
                "{observation:?}"
            );
            assert_eq!(source.0.asked(), 1);
        }
    }

    #[test]
    fn a_late_answer_that_waited_is_not_taken_and_a_new_password_field_is_secure() {
        // Finding M1 of the late-answer review: the late "plain field" arrived, the page then
        // turned it into a password field without a focus change, and the next look came
        // after the freshness bound.
        let source = FakeSource::default();
        let desk = FakeDesk::with(ENGINE);
        let (step, release) = hang(element(false, 50004));
        source.0.push(step);
        source.0.push(Step::Answer(element(true, 50004)));
        let inspector = Inspector::new(source.clone())
            .with_late_call_limit(LONG)
            .with_late_fresh(SHORT);
        inspector.wait_open();
        let (woken, wakes) = std::sync::mpsc::channel();
        assert_eq!(
            judge(
                &desk,
                &inspector,
                &EDITOR.token(),
                watching(3, false, &woken),
                SHORT
            ),
            Judgement::passing(UnknownReason::Timeout)
        );
        release.send(()).expect("the thread still waits");
        wakes.recv_timeout(LONG).expect("woken");
        std::thread::sleep(SHORT * 3);
        assert_eq!(
            judge(
                &desk,
                &inspector,
                &EDITOR.token(),
                watching(3, true, &woken),
                LONG
            ),
            Judgement::settled(FieldSecurity::Secure)
        );
        assert_eq!(source.0.asked(), 2);
    }

    #[test]
    fn a_late_answer_is_not_taken_by_a_change_after_a_count_or_without_a_watcher() {
        let (woken, _wakes) = std::sync::mpsc::channel();
        for later in [watching(3, false, &woken), watching(4, true, &woken), None] {
            let label = format!("{:?}", later.as_ref().map(|w| (w.epoch, w.again)));
            let source = FakeSource::default();
            let desk = FakeDesk::with(ENGINE);
            let (inspector, _) = late(&source, &desk, element(false, 50004));
            source.0.push(Step::Answer(element(true, 50004)));
            // The user moved into a password field: the late "plain field" is about the field
            // before and is not taken.
            assert_eq!(
                judge(&desk, &inspector, &EDITOR.token(), later, LONG),
                Judgement::settled(FieldSecurity::Secure),
                "{label}"
            );
            assert_eq!(source.0.asked(), 2, "{label}");
        }
    }

    #[test]
    fn a_late_answer_is_not_taken_for_another_focused_control() {
        // Finding L2 of the late-answer review: the engine's window had the focus and answered
        // late with the unbuilt page; the focus then moved to another control of the same
        // window, not counted yet. Under that control's kind the page would be believed.
        for moved in [
            Control {
                window: 0x1_0000_0A30,
                pid: EDITOR.pid,
                kind: ControlKind::Other,
            },
            Control {
                kind: ControlKind::Other,
                ..ENGINE
            },
            Control {
                window: 0x1_0000_0A30,
                ..ENGINE
            },
        ] {
            let source = FakeSource::default();
            let desk = FakeDesk::with(ENGINE);
            let (inspector, woken) = late(&source, &desk, from(false, 50030, ENGINE.pid));
            source.0.push(Step::Answer(element(true, 50004)));
            desk.control.set(moved);
            assert_eq!(
                judge(
                    &desk,
                    &inspector,
                    &EDITOR.token(),
                    watching(3, true, &woken),
                    LONG
                ),
                Judgement::settled(FieldSecurity::Secure),
                "{moved:?}"
            );
            assert_eq!(source.0.asked(), 2, "{moved:?}");
        }
    }

    #[test]
    fn a_focused_custom_element_of_the_engine_is_believed_at_the_second_look_only() {
        use super::super::classify::{Detail, Framework, control_type};
        // The cached-probe review's M1: the first custom answer for a subject is looked at
        // again; the same answer at the watcher's look again settles.
        let custom = Observation::Element {
            password: false,
            control_type: control_type::CUSTOM,
            process: ENGINE.pid,
            detail: Detail {
                has_focus: true,
                window_root: false,
                framework: Framework::Chrome,
            },
        };
        let (woken, _wakes) = std::sync::mpsc::channel();
        let desk = FakeDesk::with(ENGINE);
        let source = FakeSource::answering(&[custom; 6]);
        let inspector = Inspector::new(source.clone()).with_late_call_limit(LONG);
        let look = |epoch, again| {
            judge(
                &desk,
                &inspector,
                &EDITOR.token(),
                watching(epoch, again, &woken),
                LONG,
            )
        };
        assert_eq!(
            look(3, false),
            Judgement::passing(UnknownReason::NotInspectable),
            "the first look"
        );
        assert_eq!(
            look(3, true),
            Judgement::settled(FieldSecurity::Normal),
            "the look again"
        );
        // After a counted change it starts again.
        assert_eq!(
            look(4, false),
            Judgement::passing(UnknownReason::NotInspectable)
        );
        // A question that is not a look again (a re-judge) does not settle it.
        assert_eq!(
            look(4, false),
            Judgement::passing(UnknownReason::NotInspectable)
        );
        // Without a watcher, never.
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), None, LONG),
            Judgement::passing(UnknownReason::NotInspectable)
        );
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), None, LONG),
            Judgement::passing(UnknownReason::NotInspectable)
        );
    }

    #[test]
    fn a_late_answer_from_another_process_with_a_window_inside_is_believed() {
        // The page's process of a web view need not be the one that owns the focused control:
        // one that owns a window on the focused control's line is accepted (and the fake desk's
        // `inner` stands for those; the real line is tested on Windows, `os_tests`).
        let source = FakeSource::default();
        let mut desk = FakeDesk::with(ENGINE);
        desk.inner = vec![77];
        let (inspector, woken) = late(&source, &desk, from(false, 50004, 77));
        assert_eq!(
            judge(
                &desk,
                &inspector,
                &EDITOR.token(),
                watching(3, true, &woken),
                LONG
            ),
            Judgement::settled(FieldSecurity::Normal)
        );
    }

    #[test]
    fn a_late_answer_about_an_element_of_another_process_is_not_believed() {
        // Finding L4 of the late-answer review.
        let source = FakeSource::default();
        let desk = FakeDesk::with(ENGINE);
        let (inspector, woken) = late(&source, &desk, from(false, 50004, 77));
        assert_eq!(
            judge(
                &desk,
                &inspector,
                &EDITOR.token(),
                watching(3, true, &woken),
                LONG
            ),
            Judgement::passing(UnknownReason::NotInspectable)
        );
        assert_eq!(source.0.asked(), 1);
    }

    #[test]
    fn a_late_answer_for_a_window_no_longer_in_front_is_never_taken() {
        let source = FakeSource::default();
        let desk = FakeDesk::with(ENGINE);
        let (inspector, woken) = late(&source, &desk, element(false, 50004));
        let desk = FakeDesk::with(ENGINE).fronts(&[Some(OTHER)]);
        assert_eq!(
            judge(
                &desk,
                &inspector,
                &EDITOR.token(),
                watching(3, true, &woken),
                LONG
            ),
            unknown(UnknownReason::FocusMoved)
        );
    }

    #[test]
    fn a_probe_that_cannot_be_opened_is_not_normal() {
        let source = FakeSource::answering(&[element(false, 50004)]);
        *source.0.refuse_open.lock().expect("not poisoned") = true;
        let inspector = Inspector::new(source).with_late_call_limit(LONG);
        let desk = FakeDesk::showing(ControlKind::Other);
        assert_eq!(
            judge(&desk, &inspector, &EDITOR.token(), None, LONG),
            Judgement::passing(UnknownReason::NotInspectable)
        );
    }
}
