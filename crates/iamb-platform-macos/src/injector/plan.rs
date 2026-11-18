//! Injection steps into what the injector posts: key presses, text chunks, pauses and pastes.
//!
//! [`Planner`] turns the steps that follow an expansion's leading deletion into [`Burst`]s, using
//! the [`Strategy`] for the application:
//!
//! | Step | Becomes |
//! | --- | --- |
//! | `DeleteBack(n)` | `n` presses of Delete (Backspace) |
//! | `TypeText` sent as keys | text chunks ([`chunk`](super::chunk)); a line break is the strategy's Return; a tab is Tab |
//! | `TypeText` sent as a paste | a [`Burst::Paste`] of the plain text |
//! | `PressKey { key, shift, times }` | `times` presses of that key, with Shift when asked |
//! | `MoveCaretBack(n)` | `n` presses of Left |
//! | `Wait(ms)` | a [`Burst::Wait`] |
//! | `Paste(content)` | a [`Burst::Paste`] |
//!
//! Consecutive key operations form one [`Burst::Events`]; a wait or a paste ends a burst. macOS
//! cannot post a batch that real keys cannot interleave (architecture §4.6), so a burst is
//! posted event by event; what the burst gives is one place where the strategy's key delay
//! applies and the boundary at which the injector sleeps or pastes.
//!
//! Text goes by paste when the method is `paste`; with `auto`, when it has more characters
//! than the threshold, or when it has a line break and the line-break setting is `paste`.
//!
//! The leading deletion (with the compensation for characters typed after the trigger) and
//! the characters typed again are added by the driver ([`run`](super::run)), because they are
//! known only after the start marker.

use std::time::Duration;

use iamb_platform::core_types::KeyName;
use iamb_platform::{ClipboardContent, InjectionStep};

use super::chunk::{Piece, TextChunk, chunk_chars, has_line_break, pieces};
use super::strategy::{LineBreak, Method, Mods, Strategy};

/// macOS virtual key codes (the `kVK_` constants of the system headers). They name physical
/// key positions, which is what a key press needs; text never travels as a key code.
pub(crate) mod keycode {
    /// The key whose code carries text events: `kVK_ANSI_A`. Applications read the Unicode
    /// string, not the code.
    pub(crate) const TEXT: u16 = 0x00;
    /// `kVK_ANSI_V`: the paste shortcut, with Command.
    pub(crate) const V: u16 = 0x09;
    pub(crate) const RETURN: u16 = 0x24;
    pub(crate) const TAB: u16 = 0x30;
    pub(crate) const SPACE: u16 = 0x31;
    /// `kVK_Delete`: Backspace.
    pub(crate) const DELETE: u16 = 0x33;
    pub(crate) const ESCAPE: u16 = 0x35;
    /// `kVK_F20`: the key of the marker events (a release of a key few keyboards have).
    pub(crate) const F20: u16 = 0x5A;
    pub(crate) const HOME: u16 = 0x73;
    pub(crate) const PAGE_UP: u16 = 0x74;
    pub(crate) const FORWARD_DELETE: u16 = 0x75;
    pub(crate) const END: u16 = 0x77;
    pub(crate) const PAGE_DOWN: u16 = 0x79;
    pub(crate) const LEFT: u16 = 0x7B;
    pub(crate) const RIGHT: u16 = 0x7C;
    pub(crate) const DOWN: u16 = 0x7D;
    pub(crate) const UP: u16 = 0x7E;
}

/// One key, pressed and released, with modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyPress {
    pub(crate) code: u16,
    pub(crate) mods: Mods,
}

impl KeyPress {
    pub(crate) const fn plain(code: u16) -> Self {
        Self {
            code,
            mods: Mods::NONE,
        }
    }

    /// Backspace.
    pub(crate) const DELETE: Self = Self::plain(keycode::DELETE);
    /// The paste shortcut, Command+V.
    pub(crate) const PASTE: Self = Self {
        code: keycode::V,
        mods: Mods::COMMAND,
    };

    /// The key for a named key of the template language.
    pub(crate) const fn named(key: KeyName, shift: bool) -> Self {
        let code = match key {
            KeyName::Enter => keycode::RETURN,
            KeyName::Tab => keycode::TAB,
            KeyName::Escape => keycode::ESCAPE,
            KeyName::Backspace => keycode::DELETE,
            KeyName::Delete => keycode::FORWARD_DELETE,
            KeyName::Space => keycode::SPACE,
            KeyName::Up => keycode::UP,
            KeyName::Down => keycode::DOWN,
            KeyName::Left => keycode::LEFT,
            KeyName::Right => keycode::RIGHT,
            KeyName::Home => keycode::HOME,
            KeyName::End => keycode::END,
            KeyName::PageUp => keycode::PAGE_UP,
            KeyName::PageDown => keycode::PAGE_DOWN,
        };
        Self {
            code,
            mods: Mods {
                shift,
                ..Mods::NONE
            },
        }
    }
}

/// One key operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    /// Press and release `key`, `times` times.
    Press { key: KeyPress, times: usize },
    /// One key press and release that carries this text.
    Text(TextChunk),
}

/// A run of output between two waits or pastes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Burst {
    /// Key operations, posted in order with `gap` between two key presses.
    Events { ops: Vec<Op>, gap: Duration },
    /// Sleep.
    Wait(Duration),
    /// Paste `content` through the pasteboard; it stays there `settle` before the previous
    /// content is put back.
    Paste {
        content: ClipboardContent,
        settle: Duration,
    },
}

/// Collects bursts, merging consecutive key operations.
pub(crate) struct Planner<'s> {
    strategy: &'s Strategy,
    bursts: Vec<Burst>,
}

impl<'s> Planner<'s> {
    pub(crate) const fn new(strategy: &'s Strategy) -> Self {
        Self {
            strategy,
            bursts: Vec::new(),
        }
    }

    fn ops(&mut self) -> &mut Vec<Op> {
        if !matches!(self.bursts.last(), Some(Burst::Events { .. })) {
            self.bursts.push(Burst::Events {
                ops: Vec::new(),
                gap: self.strategy.key_delay(),
            });
        }
        match self.bursts.last_mut() {
            Some(Burst::Events { ops, .. }) => ops,
            // Pushed just above.
            _ => unreachable!("the last burst is a run of events"),
        }
    }

    /// Presses `key` `times` times; nothing for zero.
    pub(crate) fn press(&mut self, key: KeyPress, times: usize) {
        if times == 0 {
            return;
        }
        let ops = self.ops();
        if let Some(Op::Press {
            key: last,
            times: n,
        }) = ops.last_mut()
            && *last == key
        {
            *n += times;
            return;
        }
        ops.push(Op::Press { key, times });
    }

    /// Types `chunks`.
    pub(crate) fn text(&mut self, chunks: Vec<TextChunk>) {
        if chunks.is_empty() {
            return;
        }
        self.ops().extend(chunks.into_iter().map(Op::Text));
    }

    fn wait(&mut self, ms: u32) {
        self.bursts
            .push(Burst::Wait(Duration::from_millis(u64::from(ms))));
    }

    fn paste(&mut self, content: ClipboardContent) {
        if content.is_empty() {
            return;
        }
        self.bursts.push(Burst::Paste {
            content,
            settle: self.strategy.paste_wait(),
        });
    }

    fn type_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if sends_by_paste(text, self.strategy) {
            self.paste(ClipboardContent::text(text));
            return;
        }
        let line_break = match self.strategy.line_break {
            LineBreak::Return(mods) => KeyPress {
                code: keycode::RETURN,
                mods,
            },
            // Key events were forced on text that would have been pasted.
            LineBreak::Paste => KeyPress::plain(keycode::RETURN),
        };
        let limit = usize::from(self.strategy.chunk_units);
        for piece in pieces(text) {
            match piece {
                Piece::Run(run) => self.text(chunk_chars(run.chars(), limit)),
                Piece::LineBreak => self.press(line_break, 1),
                Piece::Tab => self.press(KeyPress::plain(keycode::TAB), 1),
            }
        }
    }

    /// Adds one step.
    pub(crate) fn step(&mut self, step: &InjectionStep) {
        match step {
            InjectionStep::DeleteBack(n) => self.press(KeyPress::DELETE, *n),
            InjectionStep::TypeText(text) => self.type_text(text),
            InjectionStep::Paste(content) => self.paste(content.clone()),
            InjectionStep::PressKey { key, shift, times } => {
                self.press(KeyPress::named(*key, *shift), usize::from(*times));
            }
            InjectionStep::MoveCaretBack(n) => {
                self.press(KeyPress::plain(keycode::LEFT), *n);
            }
            InjectionStep::Wait(ms) => self.wait(*ms),
        }
    }

    pub(crate) fn finish(self) -> Vec<Burst> {
        self.bursts
    }
}

/// True when the strategy sends `text` by paste rather than key events.
pub(crate) fn sends_by_paste(text: &str, strategy: &Strategy) -> bool {
    match strategy.method {
        Method::Keys => false,
        Method::Paste => true,
        Method::Auto => {
            text.chars().count() > strategy.paste_threshold
                || (strategy.line_break == LineBreak::Paste && has_line_break(text))
        }
    }
}

/// The bursts for `steps` (the steps after the leading deletion), with `delete_first`
/// Backspaces in front. The driver plans in two parts, so this is for the tests.
#[cfg(test)]
pub(crate) fn plan(
    delete_first: usize,
    steps: &[InjectionStep],
    strategy: &Strategy,
) -> Vec<Burst> {
    let mut planner = Planner::new(strategy);
    planner.press(KeyPress::DELETE, delete_first);
    for step in steps {
        planner.step(step);
    }
    planner.finish()
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use iamb_platform::compensate;

    use super::*;
    use crate::injector::strategy::StrategyTable;

    /// What a burst list types, as a readable script.
    fn script(bursts: &[Burst]) -> Vec<String> {
        let mut out = Vec::new();
        for burst in bursts {
            match burst {
                Burst::Events { ops, gap } => {
                    let mut line = format!("events(gap {}ms):", gap.as_millis());
                    for op in ops {
                        match op {
                            Op::Press { key, times } => {
                                let _ = write!(line, " key{}{:?}x{times}", key.code, key.mods);
                            }
                            Op::Text(chunk) => {
                                let _ = write!(line, " «{}»", chunk.to_text());
                            }
                        }
                    }
                    out.push(line);
                }
                Burst::Wait(d) => out.push(format!("wait {}ms", d.as_millis())),
                Burst::Paste { content, settle } => out.push(format!(
                    "paste {:?} {:?} settle {}ms",
                    content.text,
                    content.html,
                    settle.as_millis()
                )),
            }
        }
        out
    }

    fn text(s: &str) -> InjectionStep {
        InjectionStep::TypeText(s.to_owned())
    }

    fn press_count(bursts: &[Burst], code: u16) -> usize {
        bursts
            .iter()
            .flat_map(|b| match b {
                Burst::Events { ops, .. } => ops.as_slice(),
                _ => &[],
            })
            .map(|op| match op {
                Op::Press { key, times } if key.code == code => *times,
                _ => 0,
            })
            .sum()
    }

    #[test]
    fn a_plain_expansion_is_one_burst() {
        let strategy = Strategy::DEFAULT;
        let bursts = plan(4, &[text("be right back")], &strategy);
        assert_eq!(bursts.len(), 1);
        assert_eq!(press_count(&bursts, keycode::DELETE), 4);
        assert_eq!(
            script(&bursts),
            [format!(
                "events(gap 0ms): key51{:?}x4 «be right back»",
                Mods::NONE
            )]
        );
    }

    #[test]
    fn waits_and_pastes_split_bursts() {
        let strategy = Strategy::DEFAULT;
        let steps = [
            text("a"),
            InjectionStep::Wait(30),
            text("b"),
            InjectionStep::Paste(ClipboardContent::rich("p", "<b>p</b>")),
            text("c"),
            InjectionStep::Wait(0),
            InjectionStep::Wait(5),
        ];
        let bursts = plan(2, &steps, &strategy);
        let kinds: Vec<&str> = bursts
            .iter()
            .map(|b| match b {
                Burst::Events { .. } => "events",
                Burst::Wait(_) => "wait",
                Burst::Paste { .. } => "paste",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "events", "wait", "events", "paste", "events", "wait", "wait"
            ]
        );
        let lines = script(&bursts);
        assert_eq!(lines[1], "wait 30ms");
        assert_eq!(
            lines[3],
            "paste Some(\"p\") Some(\"<b>p</b>\") settle 250ms"
        );
        assert!(lines[4].ends_with("«c»"));
    }

    #[test]
    fn an_empty_expansion_plans_nothing() {
        assert!(plan(0, &[], &Strategy::DEFAULT).is_empty());
        assert!(
            plan(
                0,
                &[text(""), InjectionStep::DeleteBack(0)],
                &Strategy::DEFAULT
            )
            .is_empty()
        );
        assert!(
            plan(
                0,
                &[InjectionStep::Paste(ClipboardContent::default())],
                &Strategy::DEFAULT
            )
            .is_empty()
        );
    }

    #[test]
    fn keys_caret_and_deletion_steps() {
        let steps = [
            InjectionStep::PressKey {
                key: KeyName::Tab,
                shift: true,
                times: 2,
            },
            InjectionStep::PressKey {
                key: KeyName::Enter,
                shift: false,
                times: 0,
            },
            InjectionStep::MoveCaretBack(3),
            InjectionStep::DeleteBack(1),
        ];
        let bursts = plan(0, &steps, &Strategy::DEFAULT);
        let Burst::Events { ops, .. } = &bursts[0] else {
            panic!("one run of events");
        };
        assert_eq!(
            ops,
            &[
                Op::Press {
                    key: KeyPress {
                        code: keycode::TAB,
                        mods: Mods::SHIFT
                    },
                    times: 2
                },
                Op::Press {
                    key: KeyPress::plain(keycode::LEFT),
                    times: 3
                },
                Op::Press {
                    key: KeyPress::DELETE,
                    times: 1
                },
            ]
        );
    }

    #[test]
    fn every_named_key_has_its_own_code() {
        let mut codes: Vec<u16> = KeyName::ALL
            .iter()
            .map(|&k| KeyPress::named(k, false).code)
            .collect();
        assert_eq!(KeyPress::named(KeyName::Up, true).mods, Mods::SHIFT);
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), KeyName::ALL.len());
        assert!(!codes.contains(&keycode::TEXT) && !codes.contains(&keycode::F20));
    }

    #[test]
    fn line_breaks_follow_the_strategy() {
        let steps = [text("one\ntwo\r\nthree\tfour")];
        let bursts = plan(0, &steps, &Strategy::DEFAULT);
        assert_eq!(press_count(&bursts, keycode::RETURN), 2);
        assert_eq!(press_count(&bursts, keycode::TAB), 1);
        let line = &script(&bursts)[0];
        assert!(line.contains("«one»") && line.contains("«three»") && line.contains("«four»"));

        let table = StrategyTable::built_in();
        let slack = table.resolve(
            Some(&iamb_platform::core_types::AppId(
                "com.tinyspeck.slackmacgap".to_owned(),
            )),
            &iamb_platform::InjectionOverrides::default(),
        );
        let bursts = plan(0, &steps, &slack);
        let Burst::Events { ops, .. } = &bursts[0] else {
            panic!("keys");
        };
        assert!(ops.contains(&Op::Press {
            key: KeyPress {
                code: keycode::RETURN,
                mods: Mods::SHIFT
            },
            times: 1
        }));

        let paste_lines = Strategy {
            line_break: LineBreak::Paste,
            ..Strategy::DEFAULT
        };
        let bursts = plan(1, &steps, &paste_lines);
        assert_eq!(script(&bursts).len(), 2, "the deletion, then the paste");
        assert!(matches!(&bursts[1], Burst::Paste { content, .. }
            if content.text.as_deref() == Some("one\ntwo\r\nthree\tfour") && content.html.is_none()));
        // Single-line text is still typed.
        let bursts = plan(0, &[text("single")], &paste_lines);
        assert!(matches!(&bursts[0], Burst::Events { .. }));
        // Forced keys press plain Return.
        let forced = Strategy {
            method: Method::Keys,
            ..paste_lines
        };
        assert_eq!(press_count(&plan(0, &steps, &forced), keycode::RETURN), 2);
    }

    #[test]
    fn long_text_is_pasted_above_the_threshold() {
        let strategy = Strategy::DEFAULT;
        let at = "é".repeat(strategy.paste_threshold);
        let above = "é".repeat(strategy.paste_threshold + 1);
        assert!(
            !sends_by_paste(&at, &strategy),
            "the threshold counts characters"
        );
        assert!(sends_by_paste(&above, &strategy));
        let bursts = plan(3, &[text(&above)], &strategy);
        assert!(matches!(&bursts[0], Burst::Events { .. }));
        assert!(matches!(&bursts[1], Burst::Paste { .. }));
        let keys = Strategy {
            method: Method::Keys,
            ..strategy
        };
        assert!(!sends_by_paste(&above, &keys));
        let paste = Strategy {
            method: Method::Paste,
            ..strategy
        };
        assert!(sends_by_paste("x", &paste));
    }

    #[test]
    fn the_key_delay_and_chunk_size_come_from_the_strategy() {
        let slow = Strategy {
            key_delay_ms: 3,
            chunk_units: 2,
            ..Strategy::DEFAULT
        };
        let bursts = plan(0, &[text("abcde")], &slow);
        assert_eq!(script(&bursts), ["events(gap 3ms): «ab» «cd» «e»"]);
    }

    #[test]
    fn unicode_text_is_chunked_without_splitting_clusters() {
        let body = "Café déjà vu — naïve façade, Ærøskøbing, 日本語, 👋🏽 👩‍💻 ✅";
        let bursts = plan(0, &[text(body)], &Strategy::DEFAULT);
        let Burst::Events { ops, .. } = &bursts[0] else {
            panic!("keys");
        };
        let typed: String = ops
            .iter()
            .map(|op| match op {
                Op::Text(chunk) => chunk.to_text(),
                Op::Press { .. } => panic!("no keys in single-line text"),
            })
            .collect();
        assert_eq!(typed, body);
        assert!(ops.len() >= 3);
    }

    #[test]
    fn compensation_feeds_the_leading_deletion() {
        // The driver plans with `compensate`; here the same combination.
        let steps = [
            InjectionStep::DeleteBack(4),
            text("be right back"),
            InjectionStep::Wait(10),
            text("!"),
        ];
        let compensated = compensate(&steps, 2);
        let bursts = plan(
            compensated.delete_first,
            compensated.rest,
            &Strategy::DEFAULT,
        );
        assert_eq!(press_count(&bursts, keycode::DELETE), 6);
        assert_eq!(bursts.len(), 3);
        // Without a leading deletion (a palette pick), only the typed characters go.
        let palette = [text("hello")];
        let compensated = compensate(&palette, 3);
        let bursts = plan(
            compensated.delete_first,
            compensated.rest,
            &Strategy::DEFAULT,
        );
        assert_eq!(press_count(&bursts, keycode::DELETE), 3);
        // A deletion later in the steps is its own press run, not merged into the first.
        let later = [text("ab"), InjectionStep::DeleteBack(1)];
        let compensated = compensate(&later, 0);
        let bursts = plan(
            compensated.delete_first,
            compensated.rest,
            &Strategy::DEFAULT,
        );
        assert_eq!(press_count(&bursts, keycode::DELETE), 1);
    }

    #[test]
    fn debug_shows_no_text() {
        let bursts = plan(
            0,
            &[
                text("my secret"),
                InjectionStep::Paste(ClipboardContent::text("other secret")),
            ],
            &Strategy::DEFAULT,
        );
        let shown = format!("{bursts:?}");
        assert!(!shown.contains("secret"), "{shown}");
    }
}
