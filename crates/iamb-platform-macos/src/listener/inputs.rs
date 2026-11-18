//! What the system's text input is set to, as the listener needs it: the keyboard layout to
//! translate with, and whether an input method owns the keys.
//!
//! # Why this is a seam
//!
//! An application must ask macOS about input sources on its main thread (the Text Input
//! Sources header says so, and recent systems are reported to stop a process that does not).
//! The listener's callback runs on its own thread, so it never asks: the main thread reads the
//! current layout, copies its data and publishes an [`InputSnapshot`]; the callback only
//! compares a generation number and, when it changed, takes the new snapshot. The
//! implementation is in `layout.rs`; tests use fixed snapshots.
//!
//! # Input methods and `Composing`
//!
//! An event tap sees key presses, not the marked text an input method builds from them, and
//! the listener may not ask the focused application (that is the focus thread's job). What
//! it can know is whether the selected input source is a plain keyboard layout or an input
//! method (Japanese, Chinese, Korean and so on). While an input method is selected:
//!
//! - the listener reports `Composing { on: true }`, and `Composing { on: false }` when a
//!   keyboard layout is selected again;
//! - no key is reported as text, because the text is whatever the input method makes of the
//!   keys.
//!
//! Limits, recorded for the focus thread of task 2.5, which can ask the application:
//!
//! - This is coarser than "has marked text": an input method's Latin mode composes nothing,
//!   yet counts as composing here, so nothing expands in it. It fails closed.
//! - A dead key's pending accent is marked text for the application too. It is not reported
//!   as composing: the listener's own dead-key state follows it exactly.
//! - The press-and-hold accent menu is invisible to an event tap.

use std::sync::Arc;

use super::translate::KeyLayout;

/// The text-input state at one moment.
pub(crate) struct InputSnapshot {
    /// The keyboard layout keys are translated with; `None` when the system offers none with
    /// Unicode layout data.
    pub(crate) layout: Option<Arc<dyn KeyLayout>>,
    /// True while the selected input source is an input method, not a keyboard layout.
    pub(crate) input_method: bool,
}

/// Where the listener gets the text-input state from.
pub(crate) trait InputSources: Send + Sync {
    /// A number that changes whenever [`current`](Self::current) would return something new.
    /// Read on every key event, so it must be cheap.
    fn generation(&self) -> u64;

    /// The latest snapshot; `None` before the first one is known.
    fn current(&self) -> Option<Arc<InputSnapshot>>;

    /// Asks for a fresh snapshot. Returns at once; the snapshot arrives later.
    fn request_refresh(&self);
}
