//! The injector: synthetic key events and pasteboard pastes (architecture §4.2, §4.6, §5;
//! ADR-0010, ADR-0019; the protocol in the trait docs of
//! [`Injector`](iamb_platform::Injector)).
//!
//! # One expansion
//!
//! 1. Without the right to post events (Accessibility; `CGPreflightPostEventAccess`) nothing
//!    is posted and the outcome is `Failed(PermissionDenied(Accessibility))`: the system
//!    would drop the events silently.
//! 2. A private event source (`kCGEventSourceStatePrivate`) is created, so the events neither
//!    read nor change the modifier state of the real keyboard, and its user data
//!    (`kCGEventSourceUserData`) is set to [`Marker::Output`](iamb_platform::Marker). Every
//!    event also gets its marker set explicitly, and its modifier flags, so a key the user
//!    holds down does not leak into Iamb's output.
//! 3. The start marker: a release of F20 (a key that types nothing, which few keyboards have,
//!    like the listener's self-test) marked `InjectionStart`.
//! 4. [`TypedHandoff::wait_sealed`](iamb_platform::TypedHandoff::wait_sealed) gives `k`, or
//!    the reason to cancel. Then the deletion (`n + k` presses of Delete, keycode 51,
//!    [`compensate`](iamb_platform::compensate)), the steps, the `k` characters again, and
//!    the end marker (F20 released, marked `InjectionEnd`). The driver is [`run`]; the plan
//!    of key operations is [`plan`].
//!
//! macOS cannot deliver a batch of events that real keys cannot interleave. A key the user
//! types between the two markers lands inside the output; the listener reports it between
//! `InjectionStarted` and `InjectionFinished`, and the engine records `TypedDuringInjection`
//! (architecture §4.6). The injector keeps that window short: the plan is made before the
//! start marker, and between the markers it only posts.
//!
//! # Where events are posted
//!
//! At the HID event tap (`kCGHIDEventTap`), the point where events from the keyboard enter the
//! system. From there they pass through every tap of the login session, the listener's
//! included, in the same queue and the same order as real keys. That is what the marker
//! protocol needs: the start and end markers must be seen by the listener at their true
//! position among real keys. Posting at the session tap would skip any tap placed before it,
//! which real keys pass through and Iamb's events would not.
//!
//! # Text
//!
//! Text travels as the Unicode string of key events (`CGEventKeyboardSetUnicodeString`) on a
//! neutral key code, at most 20 UTF-16 code units per event, cut only at grapheme cluster
//! boundaries ([`chunk`], [`segment`]): accents, emoji with skin tones, flags and ZWJ sequences
//! arrive whole. Line breaks are the strategy's Return (plain, Shift, Option, Control+Option),
//! tabs are Tab. Text above the strategy's threshold, and all text where the strategy says so,
//! is pasted instead.
//!
//! # Pasting
//!
//! A `Paste` step, and text sent by paste, go through the pasteboard: snapshot, transient
//! write, Command+V marked as output (the key code of V, with the string "v" so that the
//! shortcut is found on any keyboard layout), a bounded wait (`paste_wait_ms`), restore. See
//! `clipboard::paste`.
//!
//! # The strategy table
//!
//! [`strategy`] holds the per-application table (bundle identifier → typing method, paste
//! threshold, line-break key, key delay, paste wait, chunk size) with a reason per built-in
//! entry. The user's overrides arrive through
//! [`Injector::configure`](iamb_platform::Injector::configure) in the format both systems
//! share (`iamb_platform::injection_settings`, ADR-0025).
//!
//! # The focus between steps
//!
//! Before every key operation, paste and the retyped characters the driver compares the
//! frontmost process, the secure-input flag and the count of the focus watcher's
//! notifications with their values at the start marker, and stops with
//! `Cancelled(FocusMoved)` when one changed (`run`). None of them asks the application
//! anything; the flag is a question to the worker's own main thread, which waits at most
//! 20 ms and counts as unchanged when it gets no answer. Without a running focus watcher
//! the count does not move, and only a change of application or of the flag is seen.
//!
//! # Failures
//!
//! Every way out is a clean [`InjectionOutcome`](iamb_platform::InjectionOutcome): `Failed`
//! before the start marker (permission, event source), `Cancelled` when the hand-off says so
//! (no end marker follows), `Failed` without an end marker when the system refuses an event
//! after the start marker, as the trait requires; the service then sends
//! `InjectionCancelled`, which ends the flight in the engine.
//!
//! # Privacy
//!
//! Text to type lives in fixed-size chunks that zero themselves when dropped and show only
//! their length in `Debug`. The characters typed after the trigger are copied out of the
//! hand-off into a stack buffer, chunked, posted and zeroed. Nothing here logs.

pub(crate) mod chunk;
pub(crate) mod plan;
pub(crate) mod run;
pub(crate) mod segment;
pub(crate) mod strategy;

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
pub(crate) use mac::State;
