//! The key listener (architecture §3.1, §4.1, §5; ADR-0010, ADR-0011, ADR-0012; the trait
//! docs of [`KeyListener`](iamb_platform::KeyListener)).
//!
//! # Parts
//!
//! | Module | Runs on | Does |
//! | --- | --- | --- |
//! | `keys` | every system | virtual keys as numbers; what kind of key each is |
//! | `keystate` | every system | which keys are down, tracked from the hook stream |
//! | `deadkey` | every system | dead keys, composed with the layout's own table |
//! | `ime` | every system | when to report `Composing` |
//! | `pipeline` | every system | one raw hook event in, listener events out |
//! | `selftest` | every system | the hand-shake of the self-test |
//! | `os` | Windows | each system call behind a small safe function |
//! | `layouts` | Windows | the dead-key tables, read from the layout files |
//! | `thread` | Windows | the hook procedures and the message loop |
//!
//! The first six make no system call. They are compiled and tested on the Linux build
//! machine; what they need from Windows is a trait (`pipeline::Host`) that `thread`
//! implements and the tests fake.
//!
//! # Threads
//!
//! `start` creates the listener thread. It reads the dead-key tables of the user's layouts,
//! installs a low-level keyboard hook and a low-level mouse hook, registers the hotkeys and
//! then retrieves messages until `stop` posts the message that ends the loop. The system
//! calls the hook procedures on that thread, so the listener's state needs no lock. Other
//! threads reach it in two ways only: a command through a channel plus a wake-up message
//! (hotkeys, reinstalling the hooks), and three atomics (the self-test's word, the sequence
//! counter, the last `Composing` value).
//!
//! A hook call translates the key, takes a sequence number, delivers to the sink and passes
//! the call on. It never waits for another thread, never loads a file and never swallows or
//! delays a key. `start` and `stop` must not be called from the sink: they wait for the
//! thread that is calling it.
//!
//! # Translation
//!
//! The layout is that of the thread of the window in front. Keys are translated with
//! `ToUnicodeEx` in the mode that does not change the system's keyboard state, against the
//! listener's own picture of which keys are down (`keystate`); dead keys are composed by the
//! listener with the layout's own table (`deadkey`, `layouts`). `pipeline` documents how each
//! key is classified, including why `AltGr` types and Ctrl alone does not.
//!
//! # Self-test
//!
//! [`self_test`](iamb_platform::KeyListener::self_test) posts a release of a key no keyboard
//! has, marked [`Marker::SelfTest`](iamb_platform::Marker::SelfTest) with a fresh nonce, and
//! waits until the hook has seen it. If it has not after `timeout`, the hooks are removed and
//! installed again once (the sink then receives `Overflow`, because whatever went by
//! meanwhile is lost) and the test is repeated; `Timeout` means the second attempt failed
//! too, so the call can take twice `timeout`. A probe that could not be posted at all is an
//! `Os` error, not a `Timeout`: it says nothing about the hook.
//!
//! # Limits
//!
//! Stated here so that nobody has to find them out:
//!
//! - **Windows 10 version 1607 or later.** Earlier systems ignore the translation mode that
//!   leaves the keyboard state alone, and the listener would then disturb dead keys.
//! - **Windows the hook cannot see into.** While a window of a higher privilege level or the
//!   secure desktop has the keyboard, the hooks are not called. The picture of the keyboard
//!   is corrected at the first key after the user is back (`keystate`).
//! - **Hooks of other programs.** A hook installed after the listener's is called first and
//!   may swallow a key the listener then never sees; one installed before it is called later
//!   and may swallow a key the listener has already reported. Swallowed modifier and Caps
//!   Lock presses are detected and taken back; a swallowed character key is reported as
//!   typed.
//! - **The Caps Lock toggle** is read when the hooks are installed and then followed from the
//!   stream; see `keystate` for when that can go wrong.
//! - **Alt+numeric-keypad character codes** are not reported as characters: the digits are
//!   command chords, so the engine's buffer is cleared, and the character that appears when
//!   Alt is released goes unreported.
//! - **The layout** is that of the thread that owns the window in front. Where the focused
//!   control belongs to another thread with another layout (possible only with the system
//!   option that gives each window its own input language), the wrong layout is used.
//! - **Dead keys** need the layout's file to be readable (a 64-bit process, a layout listed in
//!   the registry). Without it, and for a layout first met while the hook is running until
//!   its table has been read, a dead-key sequence is reported as `Boundary(Other)`.
//! - **A character another program injects while a dead key waits** is reported as
//!   `Boundary(Other)`, and so is the next key that types: what the application receives then
//!   is not documented (`deadkey`).
//! - **A dead key still waiting when an expansion's output is about to start** cancels the
//!   expansion: the application would spend the first Backspace of the deletion on it
//!   (`pipeline`).
//! - **Input methods:** see `ime`.
//!
//! # Privacy (architecture §8)
//!
//! Typed characters pass through the listener and are not kept, with two exceptions that are
//! in the nature of the job, both in the listener thread's own memory, both overwritten when
//! used, neither reachable by any call: a dead character waiting for the next key, and the
//! first half of a character outside the Basic Multilingual Plane that another program is
//! injecting. The picture of the keyboard says which keys are down at this moment, as the
//! system itself tells any program. Nothing here logs. No `Debug` output, error or panic
//! message of this module contains a key code or a character: errors carry the name of a
//! system call and its code, and the types that hold key data print their shape only.

// The parts that run on every system are only called from the Windows-only parts; on other
// systems they exist for their tests.
#![cfg_attr(not(windows), allow(dead_code))]

mod deadkey;
mod ime;
pub(crate) mod keys;
mod keystate;
mod pipeline;
mod selftest;

#[cfg(windows)]
mod layouts;
#[cfg(windows)]
pub(crate) mod os;
#[cfg(windows)]
mod thread;

#[cfg(all(test, windows))]
mod os_tests;

pub(crate) use pipeline::HotkeyBinding;

#[cfg(windows)]
pub(crate) use windows::State;

#[cfg(windows)]
use windows::Shared;

#[cfg(windows)]
mod windows {
    use std::fmt;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use iamb_platform::{EventSink, KeyListener, Marker, PlatformError, Result, SeqSource};

    use super::keys::VK_UNASSIGNED;
    use super::os;
    use super::selftest::{SelfTestGate, next_nonce};
    use super::thread::{self, Command, Startup};
    use crate::Platform;
    use crate::hotkeys::Registration;

    /// How long a caller waits for the listener thread to carry out a command. The thread
    /// answers between two hook calls, which is at once unless it is stuck.
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

    /// How often `stop` tries to post the message that ends the loop to a full queue.
    const STOP_ATTEMPTS: u32 = 50;

    /// Locks `mutex`. A thread that panicked while holding it left data that is still
    /// consistent: every critical section here only replaces whole values.
    pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the listener thread shares with the threads that call the listener.
    #[derive(Debug, Default)]
    pub(super) struct Shared {
        /// The sequence numbers: one source for the life of the listener, across restarts.
        pub(super) seq: SeqSource,
        /// The self-test's hand-shake.
        pub(super) self_test: SelfTestGate,
        /// The last `Composing` value delivered, so that a restarted hook knows what the
        /// engine was told.
        pub(super) composing: AtomicBool,
    }

    /// A running listener thread.
    struct Running {
        thread: JoinHandle<Result<()>>,
        link: Link,
    }

    /// The way to ask a running listener thread to do something.
    #[derive(Clone)]
    pub(crate) struct Link {
        thread_id: u32,
        commands: Sender<Command>,
    }

    impl Link {
        fn call(&self, command: impl FnOnce(SyncSender<Result<()>>) -> Command) -> Result<()> {
            let (answer, answered) = mpsc::sync_channel(1);
            self.commands
                .send(command(answer))
                .map_err(|_| PlatformError::NotRunning)?;
            os::post_thread_message(self.thread_id, os::WM_WAKE, 0)?;
            match answered.recv_timeout(COMMAND_TIMEOUT) {
                Ok(result) => result,
                Err(RecvTimeoutError::Timeout) => Err(PlatformError::Timeout),
                Err(RecvTimeoutError::Disconnected) => Err(PlatformError::NotRunning),
            }
        }

        /// Registers a hotkey on the listener thread.
        pub(crate) fn register(&self, registration: Registration) -> Result<()> {
            self.call(|answer| Command::Register(registration, answer))
        }

        /// Unregisters the hotkey with the system id `os_id` on the listener thread.
        pub(crate) fn unregister(&self, os_id: i32) -> Result<()> {
            self.call(|answer| Command::Unregister(os_id, answer))
        }

        fn reinstall(&self) -> Result<()> {
            self.call(Command::Reinstall)
        }
    }

    /// The listener's state: the hook thread and what it shares with the callers.
    #[derive(Default)]
    pub(crate) struct State {
        shared: Arc<Shared>,
        running: Mutex<Option<Running>>,
        /// Held for the length of a self-test: one at a time.
        testing: Mutex<()>,
    }

    impl fmt::Debug for State {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("State")
                .field("running", &self.link().is_some())
                .field("last_seq", &self.shared.seq.last())
                .finish_non_exhaustive()
        }
    }

    impl State {
        /// The way to the listener thread, while it runs.
        pub(crate) fn link(&self) -> Option<Link> {
            lock(&self.running)
                .as_ref()
                .filter(|running| !running.thread.is_finished())
                .map(|running| running.link.clone())
        }
    }

    impl Running {
        /// Ends the thread and waits for it. On an error the thread may still run; `self`
        /// comes back so that the caller can try again.
        fn stop(self) -> std::result::Result<Result<()>, (Self, PlatformError)> {
            let mut attempts = 0;
            // A thread that has ended has an id the system may have given to another thread.
            while !self.thread.is_finished() {
                match os::post_thread_message(self.link.thread_id, os::WM_STOP, 0) {
                    Ok(()) => break,
                    // The queue is full (it holds ten thousand messages): the thread is
                    // working it off.
                    Err(_) if attempts < STOP_ATTEMPTS => {
                        attempts += 1;
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => return Err((self, error)),
                }
            }
            Ok(self
                .thread
                .join()
                .unwrap_or(Err(PlatformError::os(0, "the listener thread panicked"))))
        }
    }

    impl Drop for State {
        fn drop(&mut self) {
            if let Some(running) = lock(&self.running).take() {
                // Nobody is left to tell about a failure.
                let _ = running.stop();
            }
        }
    }

    impl KeyListener for Platform {
        fn start(&self, sink: Box<dyn EventSink>) -> Result<()> {
            // Lock order everywhere: the hotkey registry, then the running thread.
            let mut registry = lock(&self.hotkeys.registry);
            let mut running = lock(&self.listener.running);
            if let Some(existing) = running.take() {
                if !existing.thread.is_finished() {
                    *running = Some(existing);
                    return Err(PlatformError::AlreadyRunning);
                }
                // The thread ended by itself (its message loop failed). Its hooks and hotkeys
                // went with it.
                let _ = existing.thread.join();
                registry.stopped();
            }

            let (commands, received) = mpsc::channel();
            let (ready, announced) = mpsc::sync_channel(1);
            let startup = Startup {
                sink,
                shared: Arc::clone(&self.listener.shared),
                hotkeys: registry.registrations(),
                commands: received,
                ready,
            };
            let thread = std::thread::Builder::new()
                .name("iamb-listener".to_owned())
                .spawn(move || thread::run(startup))
                .map_err(|error| {
                    PlatformError::os(error.raw_os_error().map_or(0, i64::from), "CreateThread")
                })?;
            match announced.recv() {
                Ok(Ok(ready)) => {
                    registry.started(&ready.refused_hotkeys);
                    *running = Some(Running {
                        thread,
                        link: Link {
                            thread_id: ready.thread_id,
                            commands,
                        },
                    });
                    Ok(())
                }
                Ok(Err(error)) => {
                    let _ = thread.join();
                    Err(error)
                }
                Err(_) => {
                    let _ = thread.join();
                    Err(PlatformError::os(
                        0,
                        "the listener thread ended while it started",
                    ))
                }
            }
        }

        fn stop(&self) -> Result<()> {
            let mut registry = lock(&self.hotkeys.registry);
            let mut running = lock(&self.listener.running);
            let Some(thread) = running.take() else {
                return Ok(());
            };
            match thread.stop() {
                Ok(outcome) => {
                    registry.stopped();
                    outcome
                }
                Err((thread, error)) => {
                    *running = Some(thread);
                    Err(error)
                }
            }
        }

        fn self_test(&self, timeout: Duration) -> Result<()> {
            let _one_at_a_time = lock(&self.listener.testing);
            let gate = &self.listener.shared.self_test;
            for attempt in 0..2 {
                // Not held while waiting: `stop` must not wait for a self-test.
                let link = self.listener.link().ok_or(PlatformError::NotRunning)?;
                let nonce = next_nonce();
                gate.arm(nonce);
                let posted = os::send_marked_key_up(VK_UNASSIGNED, Marker::SelfTest(nonce));
                let seen = posted.is_ok() && gate.wait(nonce, timeout);
                gate.disarm();
                // A probe that was never posted says nothing about the hook.
                posted?;
                if seen {
                    return Ok(());
                }
                if attempt == 0 {
                    link.reinstall()?;
                }
            }
            // Stopped while the test waited?
            self.listener.link().ok_or(PlatformError::NotRunning)?;
            Err(PlatformError::Timeout)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_stopped_listener_stops_and_does_not_test() {
            let platform = Platform::new();
            assert_eq!(platform.stop(), Ok(()));
            assert_eq!(
                platform.self_test(Duration::from_millis(10)),
                Err(PlatformError::NotRunning)
            );
            assert_eq!(platform.stop(), Ok(()));
            assert_eq!(
                format!("{:?}", platform.listener),
                "State { running: false, last_seq: 0, .. }"
            );
        }
    }
}
