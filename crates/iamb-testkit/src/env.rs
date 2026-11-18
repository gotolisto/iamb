//! A fake [`Environment`]: a fixed clock, an optional clipboard, and call counters.
//!
//! This file is also compiled into `iamb-core`'s integration tests with `#[path]`, so it refers
//! to nothing in the testkit outside itself.

use std::cell::{Cell, RefCell};

use iamb_core::datetime::LocalDateTime;
use iamb_core::engine::Environment;

/// A deterministic environment for engine tests. Interior mutability lets a test change the
/// clock or clipboard between events while the engine holds `&dyn Environment` only per call.
#[derive(Debug)]
pub struct FakeEnv {
    now: Cell<LocalDateTime>,
    clipboard: RefCell<Option<String>>,
    now_calls: Cell<usize>,
    clipboard_calls: Cell<usize>,
}

impl FakeEnv {
    /// The default time of the template examples: Wednesday 2026-03-18 14:05:09.
    ///
    /// # Panics
    ///
    /// Never: the date is valid.
    #[must_use]
    pub fn default_now() -> LocalDateTime {
        LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("valid date")
    }

    /// An environment at `now` with an empty clipboard.
    #[must_use]
    pub fn new(now: LocalDateTime) -> Self {
        Self {
            now: Cell::new(now),
            clipboard: RefCell::new(None),
            now_calls: Cell::new(0),
            clipboard_calls: Cell::new(0),
        }
    }

    /// The same environment with `text` on the clipboard.
    #[must_use]
    pub fn with_clipboard(self, text: impl Into<String>) -> Self {
        self.set_clipboard(Some(text.into()));
        self
    }

    /// Changes the clock.
    pub fn set_now(&self, now: LocalDateTime) {
        self.now.set(now);
    }

    /// Changes the clipboard (`None`: empty or no text).
    pub fn set_clipboard(&self, text: Option<String>) {
        *self.clipboard.borrow_mut() = text;
    }

    /// How many times [`Environment::now`] was called.
    #[must_use]
    pub fn now_calls(&self) -> usize {
        self.now_calls.get()
    }

    /// How many times [`Environment::clipboard_text`] was called.
    #[must_use]
    pub fn clipboard_calls(&self) -> usize {
        self.clipboard_calls.get()
    }
}

impl Default for FakeEnv {
    fn default() -> Self {
        Self::new(Self::default_now())
    }
}

impl Environment for FakeEnv {
    fn now(&self) -> LocalDateTime {
        self.now_calls.set(self.now_calls.get() + 1);
        self.now.get()
    }

    fn clipboard_text(&self) -> Option<String> {
        self.clipboard_calls.set(self.clipboard_calls.get() + 1);
        self.clipboard.borrow().clone()
    }
}
