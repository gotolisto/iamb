//! The mock's text field: what a window's text and caret look like after real and injected
//! keys.

use iamb_core::template::KeyName;

/// A plain text field. It counts in characters (the mock does not model grapheme clusters or
/// selections) and treats `\n` as a line break for Home and End.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Field {
    chars: Vec<char>,
    caret: usize,
}

impl Field {
    /// A field holding `text`, with the caret at the end.
    pub(super) fn with_text(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let caret = chars.len();
        Self { chars, caret }
    }

    pub(super) fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub(super) const fn caret(&self) -> usize {
        self.caret
    }

    pub(super) fn insert(&mut self, ch: char) {
        self.chars.insert(self.caret, ch);
        self.caret += 1;
    }

    pub(super) fn insert_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.insert(ch);
        }
    }

    /// Backspace, `n` times.
    pub(super) fn delete_back(&mut self, n: usize) {
        let start = self.caret.saturating_sub(n);
        self.chars.drain(start..self.caret);
        self.caret = start;
    }

    /// Delete, `n` times.
    fn delete_forward(&mut self, n: usize) {
        let end = (self.caret + n).min(self.chars.len());
        self.chars.drain(self.caret..end);
    }

    /// Left, `n` times.
    pub(super) fn left(&mut self, n: usize) {
        self.caret = self.caret.saturating_sub(n);
    }

    fn right(&mut self, n: usize) {
        self.caret = (self.caret + n).min(self.chars.len());
    }

    fn home(&mut self) {
        self.caret = self.chars[..self.caret]
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(0, |i| i + 1);
    }

    fn end(&mut self) {
        self.caret = self.chars[self.caret..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(self.chars.len(), |i| self.caret + i);
    }

    /// A named key, `times` times. Keys with no effect on a plain field (Escape, Up, Down,
    /// Page Up, Page Down) do nothing.
    pub(super) fn press(&mut self, key: KeyName, times: usize) {
        match key {
            KeyName::Enter => (0..times).for_each(|_| self.insert('\n')),
            KeyName::Tab => (0..times).for_each(|_| self.insert('\t')),
            KeyName::Space => (0..times).for_each(|_| self.insert(' ')),
            KeyName::Backspace => self.delete_back(times),
            KeyName::Delete => self.delete_forward(times),
            KeyName::Left => self.left(times),
            KeyName::Right => self.right(times),
            KeyName::Home => self.home(),
            KeyName::End => self.end(),
            KeyName::Escape | KeyName::Up | KeyName::Down | KeyName::PageUp | KeyName::PageDown => {
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_deleting_around_the_caret() {
        let mut field = Field::default();
        field.insert_str("hello");
        assert_eq!((field.text().as_str(), field.caret()), ("hello", 5));
        field.left(2);
        field.insert('X');
        assert_eq!((field.text().as_str(), field.caret()), ("helXlo", 4));
        field.delete_back(2);
        assert_eq!((field.text().as_str(), field.caret()), ("helo", 2));
        field.delete_back(10);
        assert_eq!((field.text().as_str(), field.caret()), ("lo", 0));
        field.left(3);
        assert_eq!(field.caret(), 0);
    }

    #[test]
    fn named_keys() {
        let mut field = Field::with_text("ab");
        assert_eq!(field.caret(), 2);
        field.press(KeyName::Enter, 1);
        field.press(KeyName::Tab, 1);
        field.press(KeyName::Space, 2);
        assert_eq!(field.text(), "ab\n\t  ");
        field.press(KeyName::Backspace, 3);
        assert_eq!(field.text(), "ab\n");
        field.press(KeyName::Left, 2);
        field.press(KeyName::Delete, 1);
        assert_eq!((field.text().as_str(), field.caret()), ("a\n", 1));
        field.press(KeyName::Right, 9);
        assert_eq!(field.caret(), 2);
        field.press(KeyName::Delete, 4);
        assert_eq!(field.text(), "a\n");
        for key in [
            KeyName::Escape,
            KeyName::Up,
            KeyName::Down,
            KeyName::PageUp,
            KeyName::PageDown,
        ] {
            field.press(key, 3);
        }
        assert_eq!((field.text().as_str(), field.caret()), ("a\n", 2));
    }

    #[test]
    fn home_and_end_stay_on_the_line() {
        let mut field = Field::with_text("one\ntwo\nthree");
        field.left(7); // after "tw"
        assert_eq!(field.caret(), 6);
        field.press(KeyName::Home, 1);
        assert_eq!(field.caret(), 4);
        field.press(KeyName::End, 1);
        assert_eq!(field.caret(), 7);
        field.press(KeyName::End, 1);
        assert_eq!(field.caret(), 7);
        field.left(7);
        field.press(KeyName::Home, 1);
        assert_eq!(field.caret(), 0);
        field.press(KeyName::End, 1);
        assert_eq!(field.caret(), 3);
        let mut last = Field::with_text("one\ntwo");
        last.press(KeyName::Home, 1);
        assert_eq!(last.caret(), 4);
        last.press(KeyName::End, 1);
        assert_eq!(last.caret(), 7);
    }
}
