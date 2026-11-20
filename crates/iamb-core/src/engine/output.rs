//! Turning rendered output into actions: adaptive case and what precedes the caret afterwards.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use super::io::Action;
use crate::matcher::CasePattern;

/// Applies the typed case pattern to the `TypeText` actions, outside verbatim ranges
/// (architecture §4.4).
///
/// - [`CasePattern::Capitalized`]: the first cased letter of the typed text is upper-cased,
///   unless that letter lies in a verbatim range, in which case nothing changes.
/// - [`CasePattern::Upper`]: every character outside verbatim ranges is upper-cased.
///
/// Upper-casing can change byte lengths (`ß` → `SS`); verbatim ranges are moved to match.
pub(super) fn apply_case(actions: &mut [Action], case: CasePattern) {
    match case {
        CasePattern::AsWritten => {}
        CasePattern::Upper => {
            for action in actions {
                if let Action::TypeText { text, verbatim } = action {
                    let (t, v) = rewrite(text, verbatim, |piece, out| {
                        out.extend(piece.chars().flat_map(char::to_uppercase));
                    });
                    *text = t;
                    *verbatim = v;
                }
            }
        }
        CasePattern::Capitalized => {
            for action in actions {
                let Action::TypeText { text, verbatim } = action else {
                    continue;
                };
                let Some((at, c)) = text.char_indices().find(|(_, c)| is_cased(*c)) else {
                    continue;
                };
                if verbatim.iter().any(|r| r.contains(&at)) || c.is_uppercase() {
                    return;
                }
                let upper: String = c.to_uppercase().collect();
                let delta = upper.len().cast_signed() - c.len_utf8().cast_signed();
                text.replace_range(at..at + c.len_utf8(), &upper);
                for r in verbatim.iter_mut() {
                    if r.start > at {
                        *r = shift(r.start, delta)..shift(r.end, delta);
                    }
                }
                return;
            }
        }
    }
}

fn is_cased(c: char) -> bool {
    c.is_uppercase() || c.is_lowercase()
}

fn shift(at: usize, delta: isize) -> usize {
    at.saturating_add_signed(delta)
}

/// Rebuilds `text`, passing every non-verbatim piece through `f` and copying verbatim pieces.
/// Returns the new text and the moved verbatim ranges.
fn rewrite(
    text: &str,
    verbatim: &[Range<usize>],
    mut f: impl FnMut(&str, &mut String),
) -> (String, Vec<Range<usize>>) {
    let mut out = String::with_capacity(text.len());
    let mut ranges = Vec::with_capacity(verbatim.len());
    let mut at = 0;
    for r in verbatim {
        f(&text[at..r.start], &mut out);
        let start = out.len();
        out.push_str(&text[r.clone()]);
        ranges.push(start..out.len());
        at = r.end;
    }
    f(&text[at..], &mut out);
    (out, ranges)
}

/// Grapheme clusters of all typed text, and of the plain-text version of pasted content.
pub(super) fn typed_graphemes(actions: &[Action]) -> usize {
    actions
        .iter()
        .map(|a| match a {
            Action::TypeText { text, .. } | Action::Paste { plain: text, .. } => {
                text.graphemes(true).count()
            }
            _ => 0,
        })
        .sum()
}

/// The character just before the caret after the injection actions ran, when the actions
/// alone determine it. `None` when it is unknown (nothing typed after the last key press or
/// deletion, or the caret moved back past everything typed).
pub(super) fn char_before_caret(actions: &[Action]) -> Option<char> {
    let mut tail = String::new();
    let mut back = 0;
    for action in actions {
        match action {
            Action::TypeText { text, .. } => tail.push_str(text),
            Action::PressKey { .. } | Action::DeleteBack(_) | Action::Paste { .. } => {
                tail.clear();
            }
            Action::MoveCaretBack(n) => back = *n,
            _ => {}
        }
    }
    let graphemes: Vec<&str> = tail.graphemes(true).collect();
    let keep = graphemes.len().checked_sub(back)?;
    graphemes[..keep].last()?.chars().last()
}

#[cfg(test)]
#[expect(
    clippy::single_range_in_vec_init,
    clippy::many_single_char_names,
    reason = "verbatim ranges are written as vectors of ranges; short names for cases"
)]
mod tests {
    use super::{Action, CasePattern, apply_case, char_before_caret};

    fn text(t: &str, verbatim: Vec<std::ops::Range<usize>>) -> Action {
        Action::TypeText {
            text: t.to_owned(),
            verbatim,
        }
    }

    #[test]
    fn upper_moves_verbatim_ranges() {
        let mut a = vec![text("straße ŉ KIM ß", vec![11..14])];
        apply_case(&mut a, CasePattern::Upper);
        assert_eq!(a, vec![text("STRASSE ʼN KIM SS", vec![12..15])]);
    }

    #[test]
    fn capitalized_skips_leading_noncased_and_stops_in_verbatim() {
        let mut a = vec![text("12 ab", vec![])];
        apply_case(&mut a, CasePattern::Capitalized);
        assert_eq!(a, vec![text("12 Ab", vec![])]);

        let mut b = vec![text("kim says hi", vec![0..3])];
        apply_case(&mut b, CasePattern::Capitalized);
        assert_eq!(b, vec![text("kim says hi", vec![0..3])]);

        let mut c = vec![text("ŉx yz", vec![4..6])];
        apply_case(&mut c, CasePattern::Capitalized);
        assert_eq!(c, vec![text("ʼNx yz", vec![5..7])]);

        let mut d = vec![text("Already", vec![]), text("second", vec![])];
        apply_case(&mut d, CasePattern::Capitalized);
        assert_eq!(d, vec![text("Already", vec![]), text("second", vec![])]);

        let mut e = vec![text("--", vec![]), text("second", vec![])];
        apply_case(&mut e, CasePattern::Capitalized);
        assert_eq!(e, vec![text("--", vec![]), text("Second", vec![])]);
    }

    #[test]
    fn caret_context() {
        assert_eq!(char_before_caret(&[text("ab", vec![])]), Some('b'));
        assert_eq!(
            char_before_caret(&[text("ab", vec![]), Action::MoveCaretBack(1)]),
            Some('a')
        );
        assert_eq!(
            char_before_caret(&[text("ab", vec![]), Action::MoveCaretBack(2)]),
            None
        );
        assert_eq!(
            char_before_caret(&[text("ab", vec![]), Action::MoveCaretBack(3)]),
            None
        );
        assert_eq!(char_before_caret(&[Action::DeleteBack(2)]), None);
    }
}
