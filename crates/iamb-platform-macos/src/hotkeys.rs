//! Global hotkeys through the system hotkey API (architecture §4.5, §5; the trait docs of
//! [`Hotkeys`](iamb_platform::Hotkeys)).
//!
//! A hotkey is registered with `RegisterEventHotKey`, never watched for in the event tap: the
//! system swallows the chord and tells Iamb, and the listener stays observe-only (ADR-0010).
//!
//! # From a press to the sink
//!
//! ```text
//! system ──▶ main thread: the application event loop dispatches the Carbon event
//!                 │   hot_key_pressed looks up which listener and which id
//!                 ▼
//!            listener thread: takes the next sequence number, delivers Hotkey { id, seq }
//! ```
//!
//! The press is delivered by the listener thread through the listener's sink, so it has one
//! place in the stream of sequence numbers, between the keys the listener reported before and
//! after it. (The system does not say where the chord fell between two keys that arrived in
//! the same instant; nothing can.)
//!
//! # Which thread does what
//!
//! The Carbon hotkey functions are not thread safe and their events are dispatched by the
//! application event loop, so **every Carbon call is made on the main thread**
//! (`listener::main_thread`), not on the listener thread:
//!
//! - `register` and `unregister`, called from any thread, record the wish in the
//!   [`table`] and, while the listener runs, hand the system call to the main thread and wait
//!   for its answer. When the main thread does not answer they fail with
//!   [`Timeout`](iamb_platform::PlatformError::Timeout) and change nothing.
//! - The listener's `start` and `stop` ask the main thread to register and release all
//!   wanted chords. Registrations made while the listener is stopped are kept and applied at
//!   the next start.
//!
//! **The worker's main thread must run the application event loop** (`NSApplication`'s
//! `run`; Carbon's own `RunApplicationEventLoop` is for 32-bit programs only): that is what
//! dispatches Carbon events to their handlers. A bare `CFRunLoopRun` serves the registration
//! calls above; that it also delivers hotkey presses is not something to count on.
//!
//! # Taken chords
//!
//! Chords are registered with the exclusive option. The system then refuses the registration
//! when a hotkey for the same key and modifiers is registered already, and this crate
//! reports [`HotkeyUnavailable`](iamb_platform::PlatformError::HotkeyUnavailable); while
//! Iamb holds the chord, registrations of other applications without that option are
//! accepted by the system but receive nothing. Two ids of one platform object cannot share a
//! chord either; that is refused before the system is asked.
//!
//! # Limits
//!
//! - A chord taken by a system shortcut (Spotlight, input-source switching) is not reported
//!   as taken by the hotkey API: the registration succeeds and the system keeps the chord.
//! - Letters and digits are registered at their positions on the US keyboard
//!   ([`hotkey_code`](crate::listener::hotkey_code)).
//! - A chord that the system refuses when the listener starts stays wanted, silently; the
//!   service learns of it when it registers the chord again while the listener runs.

pub(crate) mod table;

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
pub(crate) use mac::State;
