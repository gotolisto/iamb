//! The keystroke buffer (architecture §4.1, §8 rule 1, ADR-0012).
//!
//! # Known gaps
//!
//! - **Zero-on-drop is not tested.** `Drop` calls [`KeyBuffer::clear`], which overwrites every
//!   slot. Whether the memory really holds zeros after the value is gone can only be observed by
//!   reading freed memory, which needs `unsafe`, and `unsafe` is forbidden in the workspace. The
//!   matcher's mutation run (review 1.9, §4) therefore misses exactly one mutant: replacing the
//!   body of `drop` with `()`. What is tested is the part `drop` delegates to: `clear` zeroes
//!   every slot, read back through the crate-internal `raw_storage` accessor
//!   (`clear_zeros_every_slot`, `clear_zeros_storage_after_overflow_and_backspace`). Closing the
//!   gap needs either a test-only crate outside the workspace lint that inspects the memory
//!   after drop, or this recorded exception.

use std::hint::black_box;

use crate::model::ABBREVIATION_MAX_CHARS;

/// Number of characters a [`KeyBuffer`] holds: the character before the abbreviation (for the
/// word-start rule), the longest abbreviation and one delimiter (review 1.9 finding 6).
pub const KEY_BUFFER_CAPACITY: usize = ABBREVIATION_MAX_CHARS + 2;

/// The in-memory keystroke buffer the matcher reads.
///
/// Holds the last [`KEY_BUFFER_CAPACITY`] typed characters (one before the abbreviation, 64 for
/// the abbreviation and one delimiter), one per `Char` event, so a 64-character delimiter
/// abbreviation with word start can still see the character before it. When it is full, the
/// oldest character falls off.
///
/// # Privacy guarantees
///
/// Typed text is the most sensitive data Iamb handles (architecture §8, ADR-0012). This type is
/// built so that leaking it takes a deliberate change a reviewer will see:
///
/// - It implements neither `Debug`, `Display`, `Serialize` nor `Clone` (nor `Copy`), so it
///   cannot be formatted, logged, serialised or duplicated. The tests assert this.
/// - No public method returns the contents. The matcher reads them through crate-private
///   access only; outside `iamb-core` the only observable facts are the length and the result
///   of [`Matcher::lookup`](super::Matcher::lookup).
/// - [`clear`](Self::clear) and `Drop` overwrite every slot with zeros, and
///   [`backspace`](Self::backspace) zeros the slot it frees. The writes are passed through
///   [`std::hint::black_box`] so the optimiser cannot remove them as dead stores.
/// - The storage is a fixed array inside the value, with no heap allocation, so there are no
///   stale reallocated copies.
///
/// What this type cannot do on its own, being part of the pure core: lock its memory against
/// swapping (the owner places it, e.g. inside the engine, in locked memory) and wipe copies left
/// behind when the value is moved. Create it where it will live and do not move it after
/// characters have been pushed.
///
/// ```compile_fail
/// fn needs_debug<T: std::fmt::Debug>() {}
/// needs_debug::<iamb_core::matcher::KeyBuffer>();
/// ```
///
/// ```compile_fail
/// fn needs_display<T: std::fmt::Display>() {}
/// needs_display::<iamb_core::matcher::KeyBuffer>();
/// ```
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<iamb_core::matcher::KeyBuffer>();
/// ```
///
/// ```compile_fail
/// fn needs_serialize<T: serde::Serialize>() {}
/// needs_serialize::<iamb_core::matcher::KeyBuffer>();
/// ```
pub struct KeyBuffer {
    chars: [char; KEY_BUFFER_CAPACITY],
    len: usize,
    /// True when a character has fallen off the front since the last clear, so index 0 is not
    /// the real start of the typed text.
    truncated: bool,
}

impl KeyBuffer {
    /// Creates an empty buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            chars: ['\0'; KEY_BUFFER_CAPACITY],
            len: 0,
            truncated: false,
        }
    }

    /// Appends one typed character. When the buffer is full, the oldest character falls off
    /// (its slot is overwritten).
    pub fn push(&mut self, ch: char) {
        if self.len == KEY_BUFFER_CAPACITY {
            self.chars.copy_within(1.., 0);
            self.chars[KEY_BUFFER_CAPACITY - 1] = ch;
            self.truncated = true;
        } else {
            self.chars[self.len] = ch;
            self.len += 1;
        }
    }

    /// Removes the last character, if any, and zeros its slot.
    ///
    /// Characters that fell off the front are not restored.
    pub fn backspace(&mut self) {
        if self.len > 0 {
            self.len -= 1;
            self.chars[self.len] = '\0';
            black_box(&mut self.chars);
        }
    }

    /// Empties the buffer and overwrites every slot with zeros.
    pub fn clear(&mut self) {
        self.chars.fill('\0');
        self.len = 0;
        self.truncated = false;
        black_box(&mut self.chars);
    }

    /// Number of characters held.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True when no characters are held.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The characters held, oldest first. Crate-private: no API outside the core returns them.
    pub(crate) fn chars(&self) -> &[char] {
        &self.chars[..self.len]
    }

    /// Whether characters fell off the front since the last clear, or the start of the buffer
    /// was marked as unknown.
    pub(crate) const fn truncated(&self) -> bool {
        self.truncated
    }

    /// Marks the start of the buffer as not a known boundary: something the buffer does not
    /// hold may precede it (a Backspace deleted a character it never held, or a failed expansion
    /// left text on screen). Like overflow, this makes a word-start abbreviation at the very
    /// start of the buffer not fire; [`clear`](Self::clear) resets it.
    pub(crate) fn mark_unknown_prefix(&mut self) {
        self.truncated = true;
    }

    #[cfg(test)]
    pub(crate) const fn raw_storage(&self) -> &[char; KEY_BUFFER_CAPACITY] {
        &self.chars
    }
}

impl Default for KeyBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for KeyBuffer {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{KEY_BUFFER_CAPACITY, KeyBuffer};

    #[test]
    fn push_backspace_clear() {
        let mut b = KeyBuffer::new();
        assert!(b.is_empty());
        for c in "abc".chars() {
            b.push(c);
        }
        assert_eq!(b.chars(), ['a', 'b', 'c']);
        b.backspace();
        assert_eq!(b.chars(), ['a', 'b']);
        assert_eq!(b.raw_storage()[2], '\0');
        b.clear();
        assert!(b.is_empty());
        b.backspace();
        assert!(b.is_empty());
    }

    #[test]
    fn overflow_drops_oldest_and_marks_truncated() {
        let mut b = KeyBuffer::new();
        for i in 0..KEY_BUFFER_CAPACITY {
            b.push(char::from(b'a' + u8::try_from(i % 26).unwrap_or(0)));
        }
        assert!(!b.truncated());
        b.push('#');
        assert!(b.truncated());
        assert_eq!(b.len(), KEY_BUFFER_CAPACITY);
        assert_eq!(b.chars()[0], 'b');
        assert_eq!(b.chars()[KEY_BUFFER_CAPACITY - 1], '#');
        b.backspace();
        assert!(
            b.truncated(),
            "backspace does not bring dropped characters back"
        );
    }

    #[test]
    fn clear_zeros_storage_after_overflow_and_backspace() {
        let mut b = KeyBuffer::new();
        for c in "the quick brown fox jumps over the lazy dog "
            .chars()
            .cycle()
            .take(150)
        {
            b.push(c);
        }
        b.backspace();
        b.mark_unknown_prefix();
        assert!(
            b.raw_storage()
                .iter()
                .take(KEY_BUFFER_CAPACITY - 1)
                .all(|&c| c != '\0')
        );
        b.clear();
        assert!(b.raw_storage().iter().all(|&c| c == '\0'));
        assert!(b.is_empty());
        assert!(!b.truncated(), "clear resets an unknown prefix too");
        b.push('a');
        assert_eq!(b.chars(), ['a']);
    }

    #[test]
    fn clear_zeros_every_slot() {
        let mut b = KeyBuffer::new();
        for _ in 0..200 {
            b.push('x');
        }
        b.clear();
        assert!(b.raw_storage().iter().all(|&c| c == '\0'));
        assert!(!b.truncated());
    }
}
