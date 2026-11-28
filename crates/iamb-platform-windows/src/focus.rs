//! The focused application, window and field (architecture §3.1, §4.1, §4.5, §5; ADR-0012,
//! ADR-0013; the trait docs of [`FocusInspector`](iamb_platform::FocusInspector)).
//!
//! # Parts
//!
//! | Module | Runs on | Does |
//! | --- | --- | --- |
//! | `identity` | every system | the application name and the `own` flag, from the executable's path |
//! | `classify` | every system | what a window class, a style and a UI Automation answer mean |
//! | `inspect` | every system | the inspection thread: one question at a time, with a limit |
//! | `secure` | every system | the steps of `secure_state`, against a small `Desk` trait |
//! | `rules` | every system | which window events are reported, when a window is reported again, the wait of `restore_focus` |
//! | `os` | Windows | each system call behind a small safe function |
//! | `uia` | Windows | UI Automation: the three interfaces and the methods the question needs, one call per question |
//! | `diagnose` | every system (the run: Windows) | `iamb-service --diagnose-focus`: each focus change, the answer and the verdict, as lines |
//! | `watch` | Windows | the window-event hooks and their message loop |
//!
//! The first five, and the lines of `diagnose`, make no system call and are tested on the Linux
//! build machine.
//!
//! # What each call does
//!
//! - **`focused_app`**: the process of the window in front. The application name is its
//!   executable's file name in lower case (`notepad.exe`), never a window title and never a
//!   path. `own` is true when the executable is in the directory of the running one. For an
//!   application that runs inside the system's application frame, the process is the
//!   application's, not the frame's.
//! - **`secure_state`**: see below.
//! - **`focus_token`**: the window in front: its process id and its handle.
//! - **`restore_focus`**: asks the system to bring the token's window to the front and
//!   returns once it is, within
//!   [`RESTORE_FOCUS_TIMEOUT`](iamb_platform::RESTORE_FOCUS_TIMEOUT). A window that is gone,
//!   or whose handle now belongs to another process, is `FocusLost`; so is a refusal by the
//!   system. The window gives the keyboard focus back to the control that had it.
//! - **`allow_foreground`**: lets the UI host's process bring its own window to the front
//!   (`AllowSetForegroundWindow`).
//! - **`watch`** / **`unwatch`**: a thread with two window-event hooks reports every change
//!   of the window in front and every focus change inside it (`watch`). Neither may be
//!   called from the sink.
//!
//! Windows lets a program hand the foreground on only while it holds it or has sent the
//! last input. `restore_focus` is called by the UI host, which holds it. When the system
//! refuses all the same (and for `allow_foreground`, which the service calls from the
//! background), the call posts one release of a key no keyboard has, marked as Iamb's own
//! output so that the listener drops it, and tries once more. The grant of
//! `allow_foreground` lasts until the user's next key press, by the system's rule.
//!
//! # Is the focused field a password field?
//!
//! The steps are in `secure`, the rules in `classify`. In short:
//!
//! 1. The answer is only ever about the window asked for: it must be in front before and
//!    after, else `Unknown(FocusMoved)`.
//! 2. A window of a higher privilege level than Iamb's is `Unknown(Elevated)`.
//! 3. A system edit control answers by its style: with the password style it is `Secure`,
//!    without it `Normal`. This reads what the system keeps about the control.
//! 4. Everything else is asked through UI Automation: the focused element's `IsPassword`.
//!    This is what sees the password fields of browsers and of applications built on a
//!    browser engine, of the newer Windows toolkits, and of every toolkit that reports the
//!    "protected" state through the older accessibility interface, which the system
//!    translates.
//!
//! **The limit.** UI Automation is answered by the application, which may be busy or hung.
//! The call is made on the inspection thread and the caller waits at most `timeout`
//! (100 ms in the service): then the answer is `Unknown(Timeout)`, the thread is left to
//! its call and the next question gets a fresh one (`inspect`). Steps 1 to 3 never wait for
//! the application.
//!
//! **Undetermined is never normal.** Every failure is an `Unknown`: no window in front,
//! a process the system will not describe, a call that fails, no focused element, no answer
//! in time. In a browser engine's window, "not a password field" is believed only from an
//! element of the built page: until the engine has built its accessibility tree (it starts
//! when first asked), what it shows as focused is the window or the empty page, and that is
//! `Unknown(NotInspectable)`.
//!
//! **Looking again.** The service asks once per reported focus change. When the answer is
//! unknown for a reason that may pass (a busy application, an engine still building its
//! tree, the automation library still loading), the watcher reports the same window again
//! after 150 ms, 400 ms, 1 s, 2 s and 4 s (`rules::LOOK_AGAIN_MS`). After that the state stays
//! unknown, and the engine suspended, until the focus changes.
//!
//! **A late answer.** An application that answers UI Automation, but slower than the 100 ms
//! the service waits, would time out at every report if each asked afresh: every call is as
//! slow as the one before, and each would start another inspection thread. So the answer of a
//! question that timed out is kept, and **its arrival makes the watcher report the window again
//! at once**. That report's question takes it if its element was obtained at most 250 ms
//! before and nothing has changed since (`inspect`, "A late answer"). What it is: the field **as it was when the
//! application answered**, not what a call made at the later question would say. Why it is
//! safe to use:
//!
//! - **Fresh.** It is used at most 250 ms after it arrived. The probe gets the element and its
//!   properties in one call (`uia`), and the application reads them somewhere inside that call,
//!   so the answer is as old as its call took plus that wait; with the call limited too (below)
//!   that is at most 1.25 s. An older one is dropped and the application asked afresh.
//!   A field that turns into a password field without a focus change (a page that changes the
//!   type of its field) in that time is the same race every answer has between the application
//!   answering and the verdict arriving, widened by the call.
//! - **Only for a report again.** Only a question for a report the watcher made again of the
//!   same window (after a pause, or for the arrival) may take it: the watcher marks such a
//!   report, with the count of focus changes, just before it makes it. Every change, the
//!   watcher's and the service's own reading of the focus (`focus_now`, for keys that went to a
//!   window the watcher did not report), adds to the count and clears the mark, before anyone
//!   hears of it. So the question about a change, a move into a password field included, is
//!   never served an answer obtained before it; it asks afresh. (A change the watcher has not
//!   seen yet is the race every answer has: its own report follows and is judged afresh.)
//! - **About the same control.** The question names the window, the control with the keyboard
//!   focus in it and that control's kind; a late answer serves only the same three.
//! - **About this window.** The element must be provided by the window's process, by the
//!   process of the focused control's window (an embedded engine's), or by one that owns a
//!   window on that control's line (its ancestors up to the window in front, its descendants);
//!   otherwise the answer is `Unknown(NotInspectable)`. Process ancestry is not read: it cannot
//!   be told reliably.
//! - **Quick enough.** The call that produced it took at most 1 s (`inspect::LATE_CALL_LIMIT`,
//!   not lower than the 300 ms first look, or every late answer after a first look would be
//!   refused): an answer is the field as the application read it somewhere inside its call, so
//!   a late answer can be as old as its call took plus the quarter of a second it may wait. A
//!   slower call's answer is dropped and the application asked afresh.
//! - **On the watcher's budget.** The report a late answer's arrival makes counts as one of the
//!   window's looks again (`rules::Settle`), like a pause's: after one focus change the window
//!   is asked at most six times, whatever the application answers, and then stays unknown
//!   until the next change. A slow application is not called back to back.
//! - The window must still be in front before and after (step 1), the answer is classified as
//!   any other (a password element is `Secure`, a browser engine's shell is still not
//!   believed), and without a running watcher no late answer is kept or used.
//!
//! # Windows of applications built on a browser engine (web views)
//!
//! An application that shows its interface in an embedded browser engine (the system's web
//! view, as Iamb's own window does) has its top-level window in its own process, and the
//! engine's windows, the one with the keyboard focus among them, in a process of the engine
//! that the application started. The answer does not depend on that split: the token, the
//! privilege check and the "in front" rule are about the top-level window and its process;
//! the engine's windows are child windows of it, whose input state the system shares with the
//! top-level window's thread, so the focused control read in step 3 is the engine's window
//! (`Chrome_RenderWidgetHostHWND`, a browser engine: `classify`); and UI Automation finds the
//! focused element in whichever process holds it. What such a window does need is time: the
//! engine switches its accessibility on when the first question arrives and builds the tree of
//! a large page in the background, and its first answers may be slow (`Timeout`) or the page
//! as a whole (`NotInspectable`). That is what "looking again" and "a late answer" are for.
//! Whatever the reason, the `suspended_unknown_focus` diagnostic now names it, so a window that
//! stays unknown says why.
//!
//! # Limits
//!
//! Stated here so that nobody has to find them out:
//!
//! - **Terminals.** Windows has no system-wide "secure input" state, and a console's
//!   password prompt (`ssh`, `sudo` in a Linux shell, `runas`, a PowerShell secure prompt)
//!   is not a password field: the terminal's window shows the same text area whether the
//!   program in it echoes or not. Whether a console echoes can only be read from inside
//!   that console, the terminals that host several consoles do not say which one has the
//!   keyboard, and a prompt of a remote machine turns echo off there, not here. So a
//!   password typed at a terminal prompt is **not detected**: it passes through the
//!   engine's 66-character buffer in locked memory like any typing (never stored, never
//!   logged, cleared at the next boundary), and an abbreviation inside it would expand. The
//!   protection is the exclusion list (add the terminal) or the pause.
//! - **Fields nobody describes.** An application that draws its own fields and reports
//!   nothing about them (games, some remote-desktop and virtual-machine windows showing
//!   another system's password field, Java applications without the accessibility bridge,
//!   older custom toolkits) shows a password field as a plain window. It is `Normal`:
//!   refusing every such window would stop Iamb in most terminals and several editors.
//!   Exclude such an application.
//! - **A field that changes without a focus event.** The state is judged when the focus
//!   changes. A "show password" switch that turns a password field into a text field, or a
//!   page script that does the reverse, is seen at the next focus change only. Likewise an
//!   application that draws its own fields and moves between them without raising a focus
//!   event is not judged again; the browser engines raise it while their accessibility is
//!   on, which the first question switches on.
//! - **Editable regions a browser engine reports as the page itself** (an editable `body`
//!   in a frame, as some older web editors use) are `Unknown`, because the page as a whole
//!   is also what the engine shows before its tree exists. Nothing expands there. An editable
//!   region the engine reports as a *custom* element is believed only when its framework is
//!   the engine's own, it has the keyboard focus, it names no window, and the same answer comes
//!   at a second look (`classify`, "A control without a kind"); one reported as the document or
//!   the window itself is not. A password box a page draws itself with such a role and no
//!   `IsPassword` is then an ordinary field: a listed limit (architecture §8).
//! - **Focus events of lists that follow typing.** An application that moves the
//!   accessibility focus to an entry of a suggestion list while the user types raises focus
//!   events, each of which is reported and clears what the engine holds. Dropping them
//!   would need a way to tell them from the move into another field without asking the
//!   application, and there is none.
//! - **The application frame.** The token of an application inside the system's frame names
//!   the frame's window and process; the application name is the application's.
//! - **The foreground grant** depends on the system's rules above. Where they refuse it,
//!   the UI host's window flashes in the task bar instead of coming to the front.
//!
//! # Privacy (architecture §8)
//!
//! This module reads a window's handle, class and style, its process id, the path of the
//! process's executable (kept as the file name only), its privilege level, and from UI
//! Automation the focused element's control type, `IsPassword`, `HasKeyboardFocus`, whether it
//! is a window's own element, its providing process and its framework's name (compared with
//! the system's names and not kept). It never reads a window
//! title, an element's name or value, or any text, and the UI Automation methods that
//! could are not even declared (`uia`). Nothing here logs. Errors carry the name of a
//! system call and its code.

// The parts that run on every system are only called from the Windows-only parts; on other
// systems they exist for their tests.
#![cfg_attr(not(windows), allow(dead_code))]

mod classify;
mod diagnose;
mod identity;
mod inspect;
mod rules;
mod secure;

#[cfg(windows)]
mod os;
#[cfg(windows)]
mod uia;
#[cfg(windows)]
mod watch;

#[cfg(all(test, windows))]
mod os_tests;

#[cfg(windows)]
pub use diagnose::diagnose_focus;
#[cfg(windows)]
pub(crate) use os::keyboard_focus;
#[cfg(windows)]
pub use os::window_class;
#[cfg(windows)]
pub(crate) use windows::State;

#[cfg(windows)]
mod windows {
    use std::fmt;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
    use std::time::Duration;

    use iamb_platform::{
        AppIdentity, FieldSecurity, FocusChange, FocusInspector, FocusSink, FocusToken, Marker,
        PlatformError, RESTORE_FOCUS_TIMEOUT, Result,
    };

    use super::inspect::Inspector;
    use super::rules::{Counts, Seen, await_front};
    use super::{os, secure, uia, watch};
    use crate::Platform;
    use crate::listener::keys::VK_UNASSIGNED;
    use crate::listener::os::send_marked_key_up;

    /// How often `restore_focus` looks whether the window is in front.
    const RESTORE_POLL: Duration = Duration::from_millis(2);

    /// The focus state: the inspection thread and the watcher.
    pub(crate) struct State {
        inspector: Inspector<uia::Source>,
        watcher: Mutex<Option<watch::Running>>,
        /// The count of focus changes and the mark of a report again (`watch`).
        counts: Arc<Counts>,
    }

    impl Default for State {
        fn default() -> Self {
            Self {
                inspector: Inspector::new(uia::Source),
                watcher: Mutex::new(None),
                counts: Arc::new(Counts::default()),
            }
        }
    }

    impl fmt::Debug for State {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let watching = self
                .watcher()
                .as_ref()
                .is_some_and(watch::Running::is_running);
            f.debug_struct("State")
                .field("watching", &watching)
                .field("inspector", &self.inspector)
                .finish_non_exhaustive()
        }
    }

    impl State {
        fn watcher(&self) -> MutexGuard<'_, Option<watch::Running>> {
            // A panic while the slot was locked left a whole value behind.
            self.watcher.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl Drop for State {
        fn drop(&mut self) {
            if let Some(watcher) = self.watcher().take() {
                // Nobody is left to tell about a failure.
                let _ = watcher.stop();
            }
        }
    }

    impl Platform {
        /// Reports what the inspection does from now on (`diagnose`).
        pub(crate) fn trace_focus(&self, tracer: super::inspect::Tracer) {
            self.focus.inspector.set_tracer(tracer);
        }

        /// Runs `work` with the running watcher, if there is one and its lock is free. Not
        /// waited for: `watch` and `unwatch` hold the lock while the watcher thread starts and
        /// stops, and that thread may be the caller's sink.
        fn with_watcher<T>(&self, work: impl FnOnce(&watch::Running) -> T) -> Option<T> {
            let watcher = match self.focus.watcher.try_lock() {
                Ok(watcher) => watcher,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => return None,
            };
            watcher
                .as_ref()
                .filter(|running| running.is_running())
                .map(work)
        }
    }

    /// Makes this process the one that sent the last input, which is what lets it hand the
    /// foreground on: one release of a key no keyboard has, marked as Iamb's output so that
    /// the listener drops it.
    fn claim_input() -> Result<()> {
        send_marked_key_up(VK_UNASSIGNED, Marker::Output)
    }

    impl FocusInspector for Platform {
        fn focused_app(&self) -> Result<AppIdentity> {
            os::foreground()
                .map(os::identity_of)
                .ok_or(PlatformError::NoFocus)
        }

        fn secure_state(&self, target: &FocusToken, timeout: Duration) -> FieldSecurity {
            let window = target.window();
            // A late answer may serve only a report the watcher made again of this window,
            // with nothing counted since (`inspect`, "A late answer"). Without a running
            // watcher none is kept or used.
            let watching = self.with_watcher(|running| {
                let epoch = self.focus.counts.epoch();
                secure::Watching {
                    epoch,
                    again: self.focus.counts.take_again(window, epoch),
                    wake: running.late_answer_wake(window),
                }
            });
            let judgement = secure::judge(
                &os::SystemDesk,
                &self.focus.inspector,
                target,
                watching,
                timeout,
            );
            if judgement.look_again {
                // If the window is not reported again its state stays unknown.
                let _ = self.with_watcher(|running| running.look_again(window));
            }
            judgement.field
        }

        fn focus_token(&self) -> Result<FocusToken> {
            os::foreground()
                .map(secure::Front::token)
                .ok_or(PlatformError::NoFocus)
        }

        /// One reading of the window in front gives both the token and the application, as
        /// the watcher builds its reports (the close-out review's C3).
        fn focus_now(&self) -> Result<FocusChange> {
            // The service reads the focus itself because the watcher may have missed a
            // change: counted as one, so that no late answer from before serves it (the
            // late-answer review's finding L3).
            self.focus.counts.count_change();
            os::foreground()
                .map(|front| FocusChange {
                    app: os::identity_of(front),
                    token: front.token(),
                })
                .ok_or(PlatformError::NoFocus)
        }

        fn restore_focus(&self, token: &FocusToken) -> Result<()> {
            let window = token.window();
            let look = || {
                if !os::is_window_of(window, token.pid()) {
                    Seen::Gone
                } else if os::foreground().is_some_and(|front| front.token() == *token) {
                    Seen::InFront
                } else {
                    Seen::NotYet
                }
            };
            match look() {
                Seen::InFront => return Ok(()),
                Seen::Gone => return Err(PlatformError::FocusLost),
                Seen::NotYet => {}
            }
            if !os::set_foreground(window) {
                // See the module docs: the right to hand the foreground on.
                claim_input().map_err(|_| PlatformError::FocusLost)?;
                if !os::set_foreground(window) {
                    return Err(PlatformError::FocusLost);
                }
            }
            await_front(look, RESTORE_FOCUS_TIMEOUT, RESTORE_POLL)
        }

        fn window_exists(&self, token: &FocusToken) -> bool {
            os::is_window_of(token.window(), token.pid())
        }

        fn allow_foreground(&self, pid: u32) -> Result<()> {
            if os::allow_set_foreground(pid).is_ok() {
                return Ok(());
            }
            // See the module docs: the right to hand the foreground on.
            claim_input()?;
            os::allow_set_foreground(pid)
        }

        fn watch(&self, sink: Box<dyn FocusSink>) -> Result<()> {
            let mut watcher = self.focus.watcher();
            if let Some(existing) = watcher.take() {
                if existing.is_running() {
                    *watcher = Some(existing);
                    return Err(PlatformError::AlreadyRunning);
                }
                // The thread ended by itself (its message loop failed); its hooks went with
                // it.
                let _ = existing.stop();
            }
            // The first question follows the first report at once: have the automation
            // library loading by then.
            self.focus.inspector.warm();
            *watcher = Some(watch::start(sink, Arc::clone(&self.focus.counts))?);
            Ok(())
        }

        fn is_watching(&self) -> bool {
            // `watch` and `unwatch` hold the lock only while a thread starts or stops; a
            // watcher being started or stopped is not counted as running.
            match self.focus.watcher.try_lock() {
                Ok(watcher) => watcher.as_ref().is_some_and(watch::Running::is_running),
                Err(TryLockError::Poisoned(poisoned)) => poisoned
                    .into_inner()
                    .as_ref()
                    .is_some_and(watch::Running::is_running),
                Err(TryLockError::WouldBlock) => false,
            }
        }

        fn unwatch(&self) -> Result<()> {
            let mut watcher = self.focus.watcher();
            let Some(running) = watcher.take() else {
                return Ok(());
            };
            running.stop().map_err(|(running, error)| {
                *watcher = Some(running);
                error
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use iamb_platform::UnknownReason;

        use super::*;

        /// A token no window has: window handles are multiples of two, and process 0 owns
        /// no window.
        const NOBODY: FocusToken = FocusToken::from_raw(0, 0x7FFF_FFF1);

        #[test]
        fn a_window_that_is_not_in_front_is_never_normal() {
            let platform = Platform::new();
            let state = platform.secure_state(&NOBODY, Duration::from_millis(100));
            assert!(
                matches!(
                    state,
                    FieldSecurity::Unknown(UnknownReason::FocusMoved | UnknownReason::NoFocus)
                ),
                "{state:?}"
            );
        }

        #[test]
        fn a_token_whose_window_is_gone_is_lost() {
            let platform = Platform::new();
            assert_eq!(
                platform.restore_focus(&NOBODY),
                Err(PlatformError::FocusLost)
            );
        }

        #[test]
        fn not_watching_is_not_an_error() {
            let platform = Platform::new();
            assert!(!platform.is_watching());
            assert_eq!(platform.unwatch(), Ok(()));
            assert_eq!(platform.unwatch(), Ok(()));
            assert!(!platform.is_watching());
            assert_eq!(
                format!("{:?}", platform.focus),
                "State { watching: false, inspector: Inspector { workers: 0, .. }, .. }"
            );
        }
    }
}
