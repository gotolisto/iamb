//! `iamb-service --diagnose-focus <seconds>`: what the secure-field check sees, focus change by
//! focus change, for a person to run on their own machine and send back.
//!
//! It runs in a process of its own, beside the running service, with a platform of its own: its
//! own watcher (window-event hooks out of context, which any number of processes may have) and
//! its own inspection threads, asking exactly as the service does (`secure_state` with the
//! service's two limits, the watcher's reports again and late answers included). It installs no
//! keyboard hook, sends no key and types nothing.
//!
//! # What a line holds (architecture §8)
//!
//! Only facts the check itself reads: the executable's file name, the window classes of the
//! window in front and of the focused control, the control's kind and the owning process ids,
//! the element's control type, `IsPassword`, `HasKeyboardFocus`, whether it is a window's own
//! element, its framework as one of the system's names, the providing process id, durations,
//! and the outcome. Never a window title, an element's name or value, or any text: the check
//! reads none, so this cannot print one. Window class names are what a program registered its
//! windows under, not anything the user typed.
//!
//! The formatting is plain code, tested on every system; the run itself is Windows only.

use std::fmt::Write as _;
use std::time::Duration;

use iamb_platform::FieldSecurity;

use super::classify::{ControlKind, Detail, Framework, Observation, control_type};
use super::inspect::TraceEvent;

/// The most characters of a name a program chose (an executable, a window class) that a line
/// shows.
const NAME_SHOWN: usize = 64;

/// A name a program chose, as a line shows it: at most [`NAME_SHOWN`] characters, each outside
/// `A-Z a-z 0-9 . + - _` replaced by `?`, and `...` when it was longer. The rule of the
/// service's log for client text (the 2.8 review's finding 10): a line break in a window class
/// would otherwise write lines of the program's choosing into the file (the cached-probe
/// review's L6).
pub(crate) fn shown(name: &str) -> String {
    let mut out: String = name
        .chars()
        .take(NAME_SHOWN)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_') {
                c
            } else {
                '?'
            }
        })
        .collect();
    if name.chars().nth(NAME_SHOWN).is_some() {
        out.push_str("...");
    }
    out
}

/// Milliseconds, as a whole number.
fn ms(duration: Duration) -> u128 {
    duration.as_millis()
}

/// `[  12.345s]`.
fn stamp(at: Duration) -> String {
    format!("[{:>4}.{:03}s]", at.as_secs(), at.subsec_millis())
}

/// The name of a UI Automation control type, or its number.
fn type_name(id: i32) -> String {
    let name = match id {
        50000 => "button",
        50003 => "combo box",
        50004 => "edit",
        50020 => "text",
        control_type::CUSTOM => "custom",
        50026 => "group",
        control_type::DOCUMENT => "document",
        control_type::WINDOW => "window",
        control_type::PANE => "pane",
        _ => return format!("type {id}"),
    };
    format!("{name} ({id})")
}

const fn kind_name(kind: ControlKind) -> &'static str {
    match kind {
        ControlKind::PasswordEdit => "system password edit",
        ControlKind::PlainEdit => "system edit",
        ControlKind::WebEngine => "browser engine",
        ControlKind::Other => "other",
    }
}

const fn framework_name(framework: Framework) -> &'static str {
    match framework {
        Framework::Chrome => "Chrome",
        Framework::Xaml => "XAML",
        Framework::Win32 => "Win32",
        Framework::Wpf => "WPF",
        Framework::WinForm => "WinForm",
        Framework::Qt => "Qt",
        Framework::Other => "other",
    }
}

const fn yes(flag: bool) -> &'static str {
    if flag { "yes" } else { "no" }
}

/// What an observation says, in words.
fn observation(observation: Observation) -> String {
    match observation {
        Observation::NoElement => "no focused element".to_owned(),
        Observation::Failed => "the call failed".to_owned(),
        Observation::Element {
            password,
            control_type,
            process,
            detail:
                Detail {
                    has_focus,
                    window_root,
                    framework,
                },
        } => format!(
            "{} password={} focus={} window's own={} framework={} process={process}",
            type_name(control_type),
            yes(password),
            yes(has_focus),
            yes(window_root),
            framework_name(framework),
        ),
    }
}

/// The outcome, as the diagnostics list says it.
fn outcome(field: FieldSecurity) -> String {
    match field {
        FieldSecurity::Normal => "Normal".to_owned(),
        FieldSecurity::Secure => "Secure".to_owned(),
        FieldSecurity::Unknown(reason) => format!("Unknown ({})", reason.code()),
    }
}

/// A focus change the watcher reported, before it is judged.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Seen<'a> {
    /// Since the start of the run.
    pub(crate) at: Duration,
    /// The executable's file name.
    pub(crate) app: &'a str,
    /// The window in front: its class and process.
    pub(crate) window_class: &'a str,
    pub(crate) window_pid: u32,
    /// The focused control: its class, kind and process.
    pub(crate) control_class: &'a str,
    pub(crate) control_kind: ControlKind,
    pub(crate) control_pid: u32,
    /// The limit of this look.
    pub(crate) limit: Duration,
}

/// The line of a reported focus change.
pub(crate) fn seen_line(seen: &Seen<'_>) -> String {
    format!(
        "{} focus   {} window={} (pid {}) control={} [{}] (pid {}) limit={} ms",
        stamp(seen.at),
        shown(seen.app),
        shown(seen.window_class),
        seen.window_pid,
        shown(seen.control_class),
        kind_name(seen.control_kind),
        seen.control_pid,
        ms(seen.limit),
    )
}

/// The line of a verdict: what `secure_state` answered and how long it took.
pub(crate) fn verdict_line(at: Duration, field: FieldSecurity, took: Duration) -> String {
    format!(
        "{} verdict {} after {} ms",
        stamp(at),
        outcome(field),
        ms(took)
    )
}

/// The line of what the inspection did.
pub(crate) fn trace_line(at: Duration, event: TraceEvent) -> String {
    let mut line = stamp(at);
    match event {
        TraceEvent::Probed {
            observation: seen,
            took,
            after_question,
            late_by,
        } => {
            let _ = write!(
                line,
                " uia     call {} ms, answer {} ms after the question, ",
                ms(took),
                ms(after_question)
            );
            match late_by {
                None => line.push_str("in time: "),
                Some(late) => {
                    let _ = write!(line, "LATE by {} ms: ", ms(late));
                }
            }
            line.push_str(&observation(seen));
        }
        TraceEvent::LateTaken(age) => {
            let _ = write!(line, " late    answer taken, {} ms old", ms(age));
        }
        TraceEvent::LateTooSlow(took) => {
            let _ = write!(
                line,
                " late    answer refused, its call took {} ms (too slow)",
                ms(took)
            );
        }
        TraceEvent::LateTooOld(age) => {
            let _ = write!(
                line,
                " late    answer refused, {} ms old (too old)",
                ms(age)
            );
        }
    }
    line
}

#[cfg(windows)]
pub use run::diagnose_focus;

#[cfg(windows)]
mod run {
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant};

    use iamb_platform::{
        FIRST_LOOK_TIMEOUT, FocusChange, FocusInspector, FocusToken, Result, SECURE_STATE_TIMEOUT,
    };

    use super::super::inspect::TraceEvent;
    use super::super::os::{self, SystemDesk};
    use super::super::secure::{Desk, Front};
    use super::{Seen, seen_line, trace_line, verdict_line};
    use crate::Platform;

    /// Watches and judges the focus for `duration`, as the service does, and hands each line
    /// to `out` (module docs).
    ///
    /// # Errors
    ///
    /// When the watcher cannot start.
    pub fn diagnose_focus(duration: Duration, out: &mut dyn FnMut(&str)) -> Result<()> {
        let platform = Platform::new();
        let started = Instant::now();
        let (to_log, traces) = mpsc::channel::<(Instant, TraceEvent)>();
        platform.trace_focus(Arc::new(move |event| {
            let _ = to_log.send((Instant::now(), event));
        }));
        let (reported, changes) = mpsc::channel::<FocusChange>();
        platform.watch(Box::new(move |change: FocusChange| {
            let _ = reported.send(change);
        }))?;
        out(&format!(
            "iamb focus diagnosis for {} s: each focus change (the application in front, its window classes), what UI Automation answered, and the verdict. No text, title or typed key is read.",
            duration.as_secs()
        ));
        let drain = |out: &mut dyn FnMut(&str), traces: &Receiver<(Instant, TraceEvent)>| {
            while let Ok((at, event)) = traces.try_recv() {
                out(&trace_line(at - started, event));
            }
        };
        let deadline = started + duration;
        let mut judged: Option<FocusToken> = None;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match changes.recv_timeout(left.min(Duration::from_millis(50))) {
                Ok(change) => {
                    drain(out, &traces);
                    let token = change.token;
                    let limit = if judged == Some(token) {
                        SECURE_STATE_TIMEOUT
                    } else {
                        FIRST_LOOK_TIMEOUT
                    };
                    judged = Some(token);
                    let front = Front {
                        window: token.window(),
                        pid: token.pid(),
                    };
                    let control = SystemDesk.control(front);
                    let window_class = os::window_class(front.window).unwrap_or_default();
                    let control_class = os::window_class(control.window).unwrap_or_default();
                    out(&seen_line(&Seen {
                        at: started.elapsed(),
                        app: &change.app.app.0,
                        window_class: &window_class,
                        window_pid: front.pid,
                        control_class: &control_class,
                        control_kind: control.kind,
                        control_pid: control.pid,
                        limit,
                    }));
                    let asked = Instant::now();
                    let field = platform.secure_state(&token, limit);
                    let took = asked.elapsed();
                    // The probe's own line comes from its thread a moment after the answer.
                    std::thread::sleep(Duration::from_millis(5));
                    drain(out, &traces);
                    out(&verdict_line(started.elapsed(), field, took));
                }
                Err(RecvTimeoutError::Timeout) => drain(out, &traces),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        // Late answers that arrive after the end are not waited for.
        drain(out, &traces);
        platform.unwatch()
    }
}

#[cfg(test)]
mod tests {
    use iamb_platform::UnknownReason;

    use super::*;

    #[test]
    fn a_focus_change_names_the_app_the_classes_and_the_processes() {
        let line = seen_line(&Seen {
            at: Duration::from_millis(1_234),
            app: "whatsapp.root.exe",
            window_class: "WinUIDesktopWin32WindowClass",
            window_pid: 100,
            control_class: "Chrome_RenderWidgetHostHWND",
            control_kind: ControlKind::WebEngine,
            control_pid: 200,
            limit: Duration::from_millis(300),
        });
        assert_eq!(
            line,
            "[   1.234s] focus   whatsapp.root.exe window=WinUIDesktopWin32WindowClass (pid 100) \
             control=Chrome_RenderWidgetHostHWND [browser engine] (pid 200) limit=300 ms"
        );
    }

    #[test]
    fn an_answer_says_what_the_element_is_and_whether_it_came_late() {
        let element = Observation::Element {
            password: false,
            control_type: control_type::DOCUMENT,
            process: 200,
            detail: Detail {
                has_focus: true,
                window_root: false,
                framework: Framework::Chrome,
            },
        };
        assert_eq!(
            trace_line(
                Duration::from_millis(1_500),
                TraceEvent::Probed {
                    observation: element,
                    took: Duration::from_millis(180),
                    after_question: Duration::from_millis(182),
                    late_by: Some(Duration::from_millis(82)),
                }
            ),
            "[   1.500s] uia     call 180 ms, answer 182 ms after the question, LATE by 82 ms: \
             document (50030) password=no focus=yes window's own=no framework=Chrome process=200"
        );
        assert!(
            trace_line(
                Duration::ZERO,
                TraceEvent::Probed {
                    observation: Observation::Failed,
                    took: Duration::from_millis(3),
                    after_question: Duration::from_millis(3),
                    late_by: None,
                }
            )
            .ends_with("in time: the call failed")
        );
        assert!(
            trace_line(
                Duration::ZERO,
                TraceEvent::LateTooOld(Duration::from_millis(400))
            )
            .ends_with("answer refused, 400 ms old (too old)")
        );
        assert!(
            trace_line(
                Duration::ZERO,
                TraceEvent::LateTaken(Duration::from_millis(40))
            )
            .ends_with("answer taken, 40 ms old")
        );
    }

    #[test]
    fn a_name_a_program_chose_cannot_write_lines_of_its_own() {
        let forged = "Evil\n[   1.000s] verdict Secure after 2 ms";
        let line = seen_line(&Seen {
            at: Duration::ZERO,
            app: "evil app.exe",
            window_class: forged,
            window_pid: 1,
            control_class: &"x".repeat(300),
            control_kind: ControlKind::Other,
            control_pid: 1,
            limit: Duration::from_millis(100),
        });
        assert_eq!(line.lines().count(), 1, "{line}");
        assert!(line.contains("evil?app.exe"), "{line}");
        assert!(
            line.contains("window=Evil?????1.000s??verdict?Secure?after?2?ms "),
            "{line}"
        );
        assert!(
            line.contains(&format!("control={}... ", "x".repeat(64))),
            "{line}"
        );
        assert_eq!(
            shown("Chrome_RenderWidgetHostHWND"),
            "Chrome_RenderWidgetHostHWND"
        );
    }

    #[test]
    fn a_verdict_says_the_reason_as_the_diagnostics_list_does() {
        assert_eq!(
            verdict_line(
                Duration::from_secs(2),
                FieldSecurity::Unknown(UnknownReason::NotInspectable),
                Duration::from_millis(12)
            ),
            "[   2.000s] verdict Unknown (not_inspectable) after 12 ms"
        );
        assert!(
            verdict_line(Duration::ZERO, FieldSecurity::Secure, Duration::ZERO)
                .contains("verdict Secure")
        );
        assert_eq!(type_name(12345), "type 12345");
    }
}
