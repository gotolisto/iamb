//! A wrapper for typed characters and user-entered values (architecture §8 rules 1–3,
//! ADR-0012).
//!
//! [`Sensitive<T>`] implements neither `Debug`, `Display` nor `Serialize`, so a value inside it
//! cannot be logged or serialised by accident: reading it takes an explicit
//! [`expose`](Sensitive::expose) that a reviewer will see.
//!
//! ```
//! use iamb_core::sensitive::Sensitive;
//!
//! let typed = Sensitive::new('x');
//! assert_eq!(*typed.expose(), 'x');
//! ```
//!
//! ```compile_fail
//! let typed = iamb_core::sensitive::Sensitive::new('x');
//! println!("{typed:?}");
//! ```
//!
//! ```compile_fail
//! let typed = iamb_core::sensitive::Sensitive::new(String::from("secret"));
//! println!("{typed}");
//! ```
//!
//! ```compile_fail
//! fn needs_serialize<T: serde::Serialize>() {}
//! needs_serialize::<iamb_core::sensitive::Sensitive<String>>();
//! ```

/// A value that must never be formatted, logged or serialised: a typed character, a form value
/// or clipboard text.
///
/// It can be cloned and compared, because the engine keeps the last form values and tests
/// compare actions; neither prints anything.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Sensitive<T>(T);

impl<T> Sensitive<T> {
    /// Wraps `value`.
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// The wrapped value. Every call site is a place where sensitive data is read; keep the
    /// result out of logs and diagnostics.
    #[must_use]
    pub const fn expose(&self) -> &T {
        &self.0
    }

    /// The wrapped value, mutably. Every call site is a place where sensitive data is read or
    /// changed; keep the value out of logs and diagnostics.
    ///
    /// ```
    /// use iamb_core::sensitive::Sensitive;
    ///
    /// let mut value = Sensitive::new(String::from("ab"));
    /// value.expose_mut().push('c');
    /// assert_eq!(value.expose(), "abc");
    /// ```
    #[must_use]
    pub const fn expose_mut(&mut self) -> &mut T {
        &mut self.0
    }

    /// Unwraps the value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for Sensitive<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}
