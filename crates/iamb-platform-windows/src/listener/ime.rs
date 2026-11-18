//! When to report that an input method is composing (architecture §4.1, §8 rule 7).
//!
//! Version 1 does not expand while an input method composes, so the listener must say when
//! one does. A low-level hook cannot see composition: it runs before the input method and
//! sees the plain keys. What the listener can find out without touching the application's
//! state is:
//!
//! - **the layout in front.** Layouts of languages that are typed through an input method
//!   (Japanese, Chinese, Korean, and any layout whose handle names an input-method file) are
//!   treated as composing unless the next point says otherwise;
//! - **whether that input method is open**, by asking the default input-method window of the
//!   window in front. The question is sent without waiting (the hook never blocks); the
//!   answer arrives on the listener thread a moment later. "Closed" means keys go straight
//!   to the application, as on any other layout;
//! - **a `VK_PROCESSKEY` event**, which only an input method produces.
//!
//! The policy fails closed: on such a layout the listener reports `Composing(true)` until the
//! input method has answered "closed", and it stops trusting that answer whenever something
//! happens that can switch the input method on: a mode key, a command chord, a modifier other
//! than Shift, a mouse click, another window or layout in front.
//!
//! # Limits (also in the listener's module documentation)
//!
//! - An open input method counts as composing even while no composition is in progress, and
//!   an input method that stays open in its Latin mode (some Chinese ones do) therefore
//!   suspends expansion for as long as its layout is selected.
//! - Input methods of other languages that use an ordinary layout handle (Vietnamese Telex,
//!   the transliteration keyboards of Indic languages) are not recognised.
//! - The answer is as old as the last key: an input method switched on by something the
//!   listener does not take for a switch is noticed one key late.
//! - After a click or a change of window the first key may be reported under
//!   `Composing(true)` even though the input method is closed, because the answer has not
//!   arrived yet.

/// The application in front, on a layout that is typed through an input method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Context {
    /// The thread of the window in front.
    pub(crate) thread: u32,
    /// Its keyboard layout handle.
    pub(crate) layout: usize,
}

/// True when the layout handle `layout` belongs to a language typed through an input method:
/// its primary language is Chinese, Japanese or Korean, or its device part names an
/// input-method file.
pub(crate) const fn is_input_method_layout(layout: usize) -> bool {
    const CHINESE: usize = 0x04;
    const JAPANESE: usize = 0x11;
    const KOREAN: usize = 0x12;
    let primary_language = layout & 0x3FF;
    let device = (layout >> 16) & 0xF000;
    matches!(primary_language, CHINESE | JAPANESE | KOREAN) || device == 0xE000
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// Not known: counts as open.
    Unknown,
    /// The input method answered that it is closed, and nothing has happened since that could
    /// have opened it.
    Closed,
    /// The input method answered that it is open, or processed a key.
    Open,
}

/// What the listener does after telling the policy about an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Step {
    /// Report `Composing` with this value, before anything else.
    pub(crate) report: Option<bool>,
    /// Ask the input method in front whether it is open, under this id.
    pub(crate) ask: Option<u32>,
}

/// The input-method policy.
#[derive(Debug)]
pub(crate) struct Ime {
    /// The last `Composing` value reported.
    reported: bool,
    /// The application in front, while its layout is an input-method layout.
    context: Option<Context>,
    status: Status,
    /// A `VK_PROCESSKEY` was the last key, on a layout that is not an input-method layout.
    processing: bool,
    /// The question that has not been answered.
    outstanding: Option<Question>,
    next_id: u32,
}

/// A question on its way to the input method.
#[derive(Debug, Clone, Copy)]
struct Question {
    id: u32,
    /// Something happened after it was sent that could have opened the input method: a
    /// "closed" answer to it proves nothing.
    overtaken: bool,
    /// Ask again when it is answered.
    again: bool,
}

impl Ime {
    /// A policy that last reported `reported` (the value survives a restart of the hook).
    pub(crate) const fn new(reported: bool) -> Self {
        Self {
            reported,
            context: None,
            status: Status::Unknown,
            processing: false,
            outstanding: None,
            next_id: 0,
        }
    }

    /// The last `Composing` value reported.
    pub(crate) const fn reported(&self) -> bool {
        self.reported
    }

    fn report(&mut self, composing: bool) -> Option<bool> {
        (composing != self.reported).then(|| {
            self.reported = composing;
            composing
        })
    }

    fn ask(&mut self) -> Option<u32> {
        if let Some(question) = &mut self.outstanding {
            question.again = true;
            return None;
        }
        self.next_id = self.next_id.wrapping_add(1);
        self.outstanding = Some(Question {
            id: self.next_id,
            overtaken: false,
            again: false,
        });
        Some(self.next_id)
    }

    /// Something happened that could have opened the input method.
    fn doubt(&mut self) {
        self.status = Status::Unknown;
        if let Some(question) = &mut self.outstanding {
            question.overtaken = true;
        }
    }

    /// A key that is not a state key or an input-method key went down. `context` is the
    /// application in front if its layout is an input-method layout; `typing` is false for a
    /// command chord.
    pub(crate) fn key(&mut self, context: Option<Context>, typing: bool) -> Step {
        self.processing = false;
        let Some(context) = context else {
            self.context = None;
            self.status = Status::Unknown;
            self.outstanding = None;
            return Step {
                report: self.report(false),
                ask: None,
            };
        };
        if self.context != Some(context) {
            self.context = Some(context);
            self.status = Status::Unknown;
            // An answer still on its way belongs to the previous window.
            self.outstanding = None;
        }
        if !typing {
            self.doubt();
        }
        let ask = self.ask();
        Step {
            report: self.report(self.status != Status::Closed),
            ask,
        }
    }

    /// Something happened that can switch an input method on or off: a mode key, a modifier
    /// other than Shift, a mouse click.
    pub(crate) fn unsettle(&mut self) -> Step {
        if self.context.is_none() {
            return Step::default();
        }
        self.doubt();
        // Nothing is reported yet: the answer may well be "closed" before the next key.
        Step {
            report: None,
            ask: self.ask(),
        }
    }

    /// A `VK_PROCESSKEY` went down: an input method took the key.
    pub(crate) fn process_key(&mut self) -> Step {
        if self.context.is_some() {
            self.status = Status::Open;
        } else {
            self.processing = true;
        }
        Step {
            report: self.report(true),
            ask: None,
        }
    }

    /// The question `id` could not be sent.
    pub(crate) fn unasked(&mut self, id: u32) {
        if self.outstanding.is_some_and(|question| question.id == id) {
            self.outstanding = None;
        }
    }

    /// The input method answered question `id`.
    pub(crate) fn answer(&mut self, id: u32, open: bool) -> Step {
        let Some(question) = self.outstanding.filter(|question| question.id == id) else {
            return Step::default();
        };
        if self.context.is_none() {
            return Step::default();
        }
        self.outstanding = None;
        let again = question.again || question.overtaken;
        if open {
            self.status = Status::Open;
        } else if !question.overtaken {
            self.status = Status::Closed;
        }
        let report = match self.status {
            Status::Open => self.report(true),
            Status::Closed => self.report(false),
            // Decided at the next key.
            Status::Unknown => None,
        };
        Step {
            report,
            ask: if again { self.ask() } else { None },
        }
    }

    /// Events were lost or the hook was installed again: nothing is known.
    pub(crate) fn reset(&mut self) {
        self.context = None;
        self.status = Status::Unknown;
        self.processing = false;
        self.outstanding = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JAPANESE: usize = 0x0411_0411;
    const EDITOR: Context = Context {
        thread: 7,
        layout: JAPANESE,
    };
    const BROWSER: Context = Context {
        thread: 9,
        layout: JAPANESE,
    };

    fn step(report: Option<bool>, ask: Option<u32>) -> Step {
        Step { report, ask }
    }

    #[test]
    fn layouts_typed_through_an_input_method() {
        for layout in [
            0x0411_0411, // Japanese
            0x0804_0804, // Chinese (Simplified)
            0x0404_0404, // Chinese (Traditional)
            0x0412_0412, // Korean
            0xE001_0411, // an input-method file
            // The same as a 64-bit system holds it: sign-extended.
            (-0x1FDF_F7FC_isize).cast_unsigned(),
        ] {
            assert!(is_input_method_layout(layout), "{layout:#x}");
        }
        for layout in [
            0x0409_0409, // US
            0x040A_040A, // Spanish
            0x0407_0407, // German
            0x040C_040C, // French
            0xF001_0409, // US International
            (-0x0FFE_FBF7_isize).cast_unsigned(),
            0x0419_0419, // Russian
            0,
        ] {
            assert!(!is_input_method_layout(layout), "{layout:#x}");
        }
    }

    #[test]
    fn an_ordinary_layout_never_composes_and_is_never_asked() {
        let mut ime = Ime::new(false);
        for typing in [true, false, true] {
            assert_eq!(ime.key(None, typing), Step::default());
        }
        assert_eq!(ime.unsettle(), Step::default());
        assert_eq!(ime.answer(1, true), Step::default());
        assert!(!ime.reported());
    }

    #[test]
    fn an_input_method_layout_composes_until_it_answers_closed() {
        let mut ime = Ime::new(false);
        // First key: nothing is known, so composing, and the question goes out.
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), Some(1)));
        assert!(ime.reported());
        // Another key before the answer: still composing, asked again later.
        assert_eq!(ime.key(Some(EDITOR), true), step(None, None));
        // The answer: closed. Keys now go straight to the application.
        assert_eq!(ime.answer(1, false), step(Some(false), Some(2)));
        assert_eq!(ime.answer(2, false), Step::default());
        // Typing goes on: one question per key, no reports.
        assert_eq!(ime.key(Some(EDITOR), true), step(None, Some(3)));
        assert_eq!(ime.answer(3, false), Step::default());
        assert!(!ime.reported());
    }

    #[test]
    fn an_open_answer_is_reported_at_once() {
        let mut ime = Ime::new(false);
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), Some(1)));
        assert_eq!(ime.answer(1, false), step(Some(false), None));
        // The input method is switched on with the mouse: noticed with the next key's answer.
        assert_eq!(ime.key(Some(EDITOR), true), step(None, Some(2)));
        assert_eq!(ime.answer(2, true), step(Some(true), None));
        assert_eq!(ime.key(Some(EDITOR), true), step(None, Some(3)));
        assert_eq!(ime.answer(3, true), Step::default());
        assert!(ime.reported());
    }

    #[test]
    fn a_closed_answer_is_not_trusted_across_a_possible_switch() {
        let mut ime = Ime::new(false);
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), Some(1)));
        assert_eq!(ime.answer(1, false), step(Some(false), None));

        // A mode key: asked at once, nothing reported yet.
        assert_eq!(ime.unsettle(), step(None, Some(2)));
        // The next key comes before the answer: composing.
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), None));
        assert_eq!(ime.answer(2, true), step(None, Some(3)));
        assert_eq!(ime.answer(3, true), Step::default());

        // Switched off again, and this time the answer arrives before the next key.
        assert_eq!(ime.unsettle(), step(None, Some(4)));
        assert_eq!(ime.answer(4, false), step(Some(false), None));
        assert_eq!(ime.key(Some(EDITOR), true), step(None, Some(5)));
        assert_eq!(ime.answer(5, false), Step::default());
    }

    #[test]
    fn a_closed_answer_that_was_asked_before_the_switch_proves_nothing() {
        let mut ime = Ime::new(false);
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), Some(1)));
        assert_eq!(ime.answer(1, false), step(Some(false), None));
        assert_eq!(ime.key(Some(EDITOR), true), step(None, Some(2)));
        // A click while question 2 is on its way.
        assert_eq!(ime.unsettle(), step(None, None));
        // Its "closed" may describe the time before the click: ask again, report nothing.
        assert_eq!(ime.answer(2, false), step(None, Some(3)));
        // The next key is still under doubt.
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), None));
        assert_eq!(ime.answer(3, false), step(Some(false), Some(4)));
        // An "open" answer is believed even when it is old.
        assert_eq!(ime.unsettle(), step(None, None));
        assert_eq!(ime.answer(4, true), step(Some(true), Some(5)));
    }

    #[test]
    fn a_command_chord_unsettles_like_a_mode_key() {
        let mut ime = Ime::new(false);
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), Some(1)));
        assert_eq!(ime.answer(1, false), step(Some(false), None));
        // Alt+` on a Japanese layout.
        assert_eq!(ime.key(Some(EDITOR), false), step(Some(true), Some(2)));
        assert_eq!(ime.answer(2, false), step(Some(false), None));
        // A chord while a question is on its way makes its answer worthless.
        assert_eq!(ime.key(Some(EDITOR), true), step(None, Some(3)));
        assert_eq!(ime.key(Some(EDITOR), false), step(Some(true), None));
        assert_eq!(ime.answer(3, false), step(None, Some(4)));
        assert_eq!(ime.answer(4, false), step(Some(false), None));
    }

    #[test]
    fn another_window_starts_from_nothing_and_ignores_the_old_answer() {
        let mut ime = Ime::new(false);
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), Some(1)));
        assert_eq!(ime.answer(1, false), step(Some(false), None));
        assert_eq!(ime.key(Some(EDITOR), true), step(None, Some(2)));
        // The browser comes to the front before question 2 is answered.
        assert_eq!(ime.key(Some(BROWSER), true), step(Some(true), Some(3)));
        assert_eq!(ime.answer(2, false), Step::default(), "the editor's answer");
        assert!(ime.reported());
        assert_eq!(ime.answer(3, false), step(Some(false), None));
    }

    #[test]
    fn leaving_the_input_method_layout_ends_composition() {
        let mut ime = Ime::new(false);
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), Some(1)));
        assert_eq!(ime.key(None, true), step(Some(false), None));
        assert_eq!(ime.answer(1, true), Step::default(), "no longer relevant");
        assert!(!ime.reported());
    }

    #[test]
    fn a_question_that_could_not_be_sent_leaves_the_doubt() {
        let mut ime = Ime::new(false);
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), Some(1)));
        ime.unasked(1);
        ime.unasked(99);
        // Asked again with the next key; still composing meanwhile.
        assert_eq!(ime.key(Some(EDITOR), true), step(None, Some(2)));
        assert!(ime.reported());
    }

    #[test]
    fn a_process_key_means_an_input_method_took_the_key() {
        // On an ordinary layout: composing until the next ordinary key.
        let mut ime = Ime::new(false);
        assert_eq!(ime.process_key(), step(Some(true), None));
        assert_eq!(ime.process_key(), Step::default());
        assert_eq!(ime.key(None, true), step(Some(false), None));

        // On an input-method layout: open, whatever was answered before.
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), Some(1)));
        assert_eq!(ime.answer(1, false), step(Some(false), None));
        assert_eq!(ime.process_key(), step(Some(true), None));
        assert_eq!(ime.key(Some(EDITOR), true), step(None, Some(2)));
        assert_eq!(ime.answer(2, false), step(Some(false), None));
    }

    #[test]
    fn the_reported_value_survives_a_restart_and_a_reset() {
        // The engine was told `true` before the hook was installed again.
        let mut ime = Ime::new(true);
        assert_eq!(ime.key(None, true), step(Some(false), None));

        let mut ime = Ime::new(false);
        assert_eq!(ime.key(Some(EDITOR), true), step(Some(true), Some(1)));
        ime.reset();
        assert!(ime.reported());
        assert_eq!(ime.answer(1, false), Step::default());
        // Same window, but nothing is known any more.
        assert_eq!(ime.key(Some(EDITOR), true), step(None, Some(2)));
    }
}
