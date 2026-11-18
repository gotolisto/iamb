//! From injection steps to batches of key events (architecture §4.6).
//!
//! Nothing here calls the system. A [`KeyEvent`] is one synthetic key event as `SendInput`
//! takes it, minus the scan code the Windows layer looks up; a [`Plan`] is what one expansion
//! sends, in order:
//!
//! - **Batches.** The leading deletion (the expansion's own and the `k` characters typed after
//!   the trigger), the text, key presses and caret moves are collected into one batch. A
//!   `Wait` and a paste end a batch, because the system can only deliver a single `SendInput`
//!   call without interleaving real keys. Everything after the last `Wait` or paste is
//!   [`Plan::last`]: the Windows layer sends it in one call together with the retyped
//!   characters and the end marker.
//! - **Text.** Each character becomes Unicode key events of its UTF-16 code units, a press and
//!   a release per unit, so a character outside the Basic Multilingual Plane (an emoji, a rare
//!   CJK ideograph) is two units and four events. Combining marks, skin-tone modifiers,
//!   zero-width joiners and regional indicators are characters like any other: an emoji
//!   sequence arrives exactly as written. A line break (LF, CR LF or a lone CR) is not a
//!   character to the applications that matter: it presses Enter, or Shift+Enter where the
//!   strategy says Enter sends the message.
//! - **Type or paste.** A text step longer than the strategy's threshold, or any text step
//!   under a paste strategy, becomes a paste of that text; a `Paste` step under a typing-only
//!   strategy types its plain text.
//! - **Pacing.** For slow applications the strategy can ask for a pause between batches, and
//!   a pause after every key, which splits the batches into one key (or chord) each.
//!
//! [`release_held`] adds the releases of modifier keys the user is still holding (the Shift
//! of a `:` delimiter, say) at the front of a batch, so that Backspace, Left or Enter are not
//! read as chords.

use std::borrow::Cow;
use std::fmt;

use iamb_platform::core_types::KeyName;
use iamb_platform::{ClipboardContent, Compensated, InjectionStep, Marker};

use super::strategy::{LineBreak, PasteKeys, Strategy};

pub(crate) const VK_BACK: u16 = 0x08;
pub(crate) const VK_TAB: u16 = 0x09;
pub(crate) const VK_RETURN: u16 = 0x0D;
pub(crate) const VK_SHIFT: u16 = 0x10;
pub(crate) const VK_CONTROL: u16 = 0x11;
pub(crate) const VK_ESCAPE: u16 = 0x1B;
pub(crate) const VK_SPACE: u16 = 0x20;
pub(crate) const VK_PRIOR: u16 = 0x21;
pub(crate) const VK_NEXT: u16 = 0x22;
pub(crate) const VK_END: u16 = 0x23;
pub(crate) const VK_HOME: u16 = 0x24;
pub(crate) const VK_LEFT: u16 = 0x25;
pub(crate) const VK_UP: u16 = 0x26;
pub(crate) const VK_RIGHT: u16 = 0x27;
pub(crate) const VK_DOWN: u16 = 0x28;
pub(crate) const VK_INSERT: u16 = 0x2D;
pub(crate) const VK_DELETE: u16 = 0x2E;
pub(crate) const VK_V: u16 = 0x56;
pub(crate) const VK_LWIN: u16 = 0x5B;
pub(crate) const VK_RWIN: u16 = 0x5C;
pub(crate) const VK_LSHIFT: u16 = 0xA0;
pub(crate) const VK_RSHIFT: u16 = 0xA1;
pub(crate) const VK_LCONTROL: u16 = 0xA2;
pub(crate) const VK_RCONTROL: u16 = 0xA3;
pub(crate) const VK_LMENU: u16 = 0xA4;
pub(crate) const VK_RMENU: u16 = 0xA5;
/// An unassigned virtual key: no keyboard has it and no application reacts to it. The
/// injection markers are releases of it (as is the listener's self-test probe), and a tap of
/// it keeps a released Alt or Windows key from opening a menu.
pub(crate) const VK_UNASSIGNED: u16 = 0xE8;

/// The modifier keys checked before each batch, by side.
pub(crate) const MODIFIERS: [u16; 8] = [
    VK_LSHIFT,
    VK_RSHIFT,
    VK_LCONTROL,
    VK_RCONTROL,
    VK_LMENU,
    VK_RMENU,
    VK_LWIN,
    VK_RWIN,
];

/// What a key event presses.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    /// One UTF-16 code unit of text (`KEYEVENTF_UNICODE`).
    Unicode(u16),
    /// A virtual key.
    Vk(u16),
}

/// One synthetic key event.
///
/// `Debug` never shows a Unicode unit: it is text from a snippet, a form or the clipboard.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyEvent {
    pub(crate) key: Key,
    pub(crate) up: bool,
    pub(crate) marker: Marker,
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unicode(_) => f.write_str("Unicode"),
            Self::Vk(vk) => write!(f, "Vk({vk:#04x})"),
        }
    }
}

impl fmt::Debug for KeyEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dir = if self.up { "up" } else { "down" };
        write!(f, "{:?} {dir}", self.key)
    }
}

impl KeyEvent {
    const fn output(key: Key, up: bool) -> Self {
        Self {
            key,
            up,
            marker: Marker::Output,
        }
    }

    /// A press of a virtual key, marked as output.
    pub(crate) const fn down(vk: u16) -> Self {
        Self::output(Key::Vk(vk), false)
    }

    /// A release of a virtual key, marked as output.
    pub(crate) const fn up(vk: u16) -> Self {
        Self::output(Key::Vk(vk), true)
    }

    /// The no-op event that carries an injection marker.
    pub(crate) const fn marker(marker: Marker) -> Self {
        Self {
            key: Key::Vk(VK_UNASSIGNED),
            up: true,
            marker,
        }
    }
}

/// True for the virtual keys that need `KEYEVENTF_EXTENDEDKEY`, or applications that read
/// scan codes take them for the numeric keypad.
pub(crate) const fn is_extended(vk: u16) -> bool {
    matches!(
        vk,
        VK_PRIOR
            | VK_NEXT
            | VK_END
            | VK_HOME
            | VK_LEFT
            | VK_UP
            | VK_RIGHT
            | VK_DOWN
            | VK_INSERT
            | VK_DELETE
            | VK_LWIN
            | VK_RWIN
            | VK_RCONTROL
            | VK_RMENU
    )
}

/// The virtual key of a named key.
pub(crate) const fn vk_of(key: KeyName) -> u16 {
    match key {
        KeyName::Enter => VK_RETURN,
        KeyName::Tab => VK_TAB,
        KeyName::Escape => VK_ESCAPE,
        KeyName::Backspace => VK_BACK,
        KeyName::Delete => VK_DELETE,
        KeyName::Space => VK_SPACE,
        KeyName::Up => VK_UP,
        KeyName::Down => VK_DOWN,
        KeyName::Left => VK_LEFT,
        KeyName::Right => VK_RIGHT,
        KeyName::Home => VK_HOME,
        KeyName::End => VK_END,
        KeyName::PageUp => VK_PRIOR,
        KeyName::PageDown => VK_NEXT,
    }
}

/// Appends `times` presses and releases of `vk`.
pub(crate) fn push_taps(out: &mut Vec<KeyEvent>, vk: u16, times: usize) {
    out.reserve(times.saturating_mul(2));
    for _ in 0..times {
        out.push(KeyEvent::down(vk));
        out.push(KeyEvent::up(vk));
    }
}

/// Appends `times` presses of `vk`, with Shift held around them when `shift`.
pub(crate) fn push_key(out: &mut Vec<KeyEvent>, vk: u16, shift: bool, times: usize) {
    if shift {
        out.push(KeyEvent::down(VK_SHIFT));
    }
    push_taps(out, vk, times);
    if shift {
        out.push(KeyEvent::up(VK_SHIFT));
    }
}

/// Appends the press and release of each UTF-16 unit of `ch`.
pub(crate) fn push_char(out: &mut Vec<KeyEvent>, ch: char) {
    let mut units = [0; 2];
    for &unit in ch.encode_utf16(&mut units).iter() {
        out.push(KeyEvent::output(Key::Unicode(unit), false));
        out.push(KeyEvent::output(Key::Unicode(unit), true));
    }
    // Do not leave a copy of the character on the stack.
    std::hint::black_box(&mut units).fill(0);
}

/// Appends a line break.
pub(crate) fn push_line_break(out: &mut Vec<KeyEvent>, line_break: LineBreak) {
    push_key(
        out,
        VK_RETURN,
        matches!(line_break, LineBreak::ShiftEnter),
        1,
    );
}

/// Appends `text`: Unicode events for its characters, the strategy's key for each line break
/// (LF, CR LF or a lone CR).
pub(crate) fn push_text(out: &mut Vec<KeyEvent>, text: &str, line_break: LineBreak) {
    out.reserve(text.len().saturating_mul(2));
    let mut after_cr = false;
    for ch in text.chars() {
        match ch {
            '\n' if after_cr => {}
            '\r' | '\n' => push_line_break(out, line_break),
            _ => push_char(out, ch),
        }
        after_cr = ch == '\r';
    }
}

/// The paste shortcut.
pub(crate) fn paste_chord(keys: PasteKeys) -> Vec<KeyEvent> {
    let (modifier, key) = match keys {
        PasteKeys::CtrlV => (VK_CONTROL, VK_V),
        PasteKeys::ShiftInsert => (VK_SHIFT, VK_INSERT),
    };
    vec![
        KeyEvent::down(modifier),
        KeyEvent::down(key),
        KeyEvent::up(key),
        KeyEvent::up(modifier),
    ]
}

/// The events that release the modifier keys in `held` (virtual keys from [`MODIFIERS`])
/// before a batch.
///
/// Releasing Alt or a Windows key on its own opens a menu, so a tap of an unassigned key goes
/// first, while they are still down. The keys are not pressed again afterwards: the user may
/// let go of them at any moment, and a key pressed again by Iamb after that would stay stuck
/// down. A key the user still holds comes back with its auto-repeat.
pub(crate) fn release_held(held: &[u16]) -> Vec<KeyEvent> {
    let mut out = Vec::new();
    if held
        .iter()
        .any(|vk| matches!(*vk, VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN))
    {
        out.push(KeyEvent::down(VK_UNASSIGNED));
        out.push(KeyEvent::up(VK_UNASSIGNED));
    }
    out.extend(
        MODIFIERS
            .iter()
            .filter(|vk| held.contains(vk))
            .map(|vk| KeyEvent::up(*vk)),
    );
    out
}

/// One thing the injector does before the final batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op<'a> {
    /// One `SendInput` call.
    Keys(Vec<KeyEvent>),
    /// A pause, in milliseconds.
    Sleep(u32),
    /// A paste through the clipboard.
    Paste(Cow<'a, ClipboardContent>),
}

/// What one expansion sends, between its two markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan<'a> {
    /// Everything before the final batch, in order.
    pub(crate) ops: Vec<Op<'a>>,
    /// The final batch: sent in the same call as the retyped characters and the end marker.
    pub(crate) last: Vec<KeyEvent>,
    /// Characters typed after the trigger, to type again after `last`.
    pub(crate) retype: usize,
}

impl<'a> Plan<'a> {
    /// The plan for compensated steps under `strategy`.
    pub(crate) fn new(steps: &Compensated<'a>, strategy: &Strategy) -> Self {
        let mut ops = Vec::new();
        let mut batch = Vec::new();
        push_taps(&mut batch, VK_BACK, steps.delete_first);
        for step in steps.rest {
            match step {
                InjectionStep::DeleteBack(n) => push_taps(&mut batch, VK_BACK, *n),
                InjectionStep::TypeText(text) if text.is_empty() => {}
                InjectionStep::TypeText(text) => {
                    if strategy.pastes_text(text.chars().count()) {
                        flush(&mut ops, &mut batch);
                        ops.push(Op::Paste(Cow::Owned(ClipboardContent::text(text.clone()))));
                    } else {
                        push_text(&mut batch, text, strategy.line_break);
                    }
                }
                InjectionStep::Paste(content) => match &content.text {
                    Some(text) if strategy.types_pastes() => {
                        push_text(&mut batch, text, strategy.line_break);
                    }
                    _ if content.is_empty() => {}
                    _ => {
                        flush(&mut ops, &mut batch);
                        ops.push(Op::Paste(Cow::Borrowed(content)));
                    }
                },
                InjectionStep::PressKey { key, shift, times } => {
                    push_key(&mut batch, vk_of(*key), *shift, usize::from(*times));
                }
                InjectionStep::MoveCaretBack(n) => push_taps(&mut batch, VK_LEFT, *n),
                InjectionStep::Wait(ms) => {
                    flush(&mut ops, &mut batch);
                    ops.push(Op::Sleep(*ms));
                }
            }
        }
        let mut plan = Self {
            ops,
            last: batch,
            retype: steps.retype,
        };
        if strategy.key_delay_ms > 0 {
            plan.pace_keys(strategy.key_delay_ms);
        }
        if strategy.batch_delay_ms > 0 {
            plan.pace_batches(strategy.batch_delay_ms);
        }
        plan
    }

    /// Splits every batch into single keys (or chords) with a pause after each. The final
    /// batch keeps only its last key, which goes with the retyped characters and the end
    /// marker.
    fn pace_keys(&mut self, delay: u32) {
        let mut ops = Vec::with_capacity(self.ops.len());
        for op in std::mem::take(&mut self.ops) {
            match op {
                Op::Keys(events) => {
                    for group in groups(&events) {
                        ops.push(Op::Keys(group.to_vec()));
                        ops.push(Op::Sleep(delay));
                    }
                }
                other => ops.push(other),
            }
        }
        let last = std::mem::take(&mut self.last);
        let mut last_groups = groups(&last);
        let final_group = last_groups.pop().map(<[KeyEvent]>::to_vec);
        for group in last_groups {
            ops.push(Op::Keys(group.to_vec()));
            ops.push(Op::Sleep(delay));
        }
        self.ops = ops;
        self.last = final_group.unwrap_or_default();
    }

    /// Adds a pause after every batch and paste that something else follows (the final batch
    /// always does). A pause next to another pause is not doubled.
    fn pace_batches(&mut self, delay: u32) {
        let mut ops = Vec::with_capacity(self.ops.len() * 2);
        for op in std::mem::take(&mut self.ops) {
            let sends = !matches!(op, Op::Sleep(_));
            ops.push(op);
            if sends {
                ops.push(Op::Sleep(delay));
            }
        }
        // Merge a pause the batch delay added with a `Wait` right after it.
        let mut merged: Vec<Op<'a>> = Vec::with_capacity(ops.len());
        for op in ops {
            match (merged.last_mut(), op) {
                (Some(Op::Sleep(a)), Op::Sleep(b)) => *a = (*a).max(b),
                (_, op) => merged.push(op),
            }
        }
        self.ops = merged;
    }
}

fn flush(ops: &mut Vec<Op<'_>>, batch: &mut Vec<KeyEvent>) {
    if !batch.is_empty() {
        ops.push(Op::Keys(std::mem::take(batch)));
    }
}

/// Splits events into keys: a group ends when no key it pressed is still down.
fn groups(events: &[KeyEvent]) -> Vec<&[KeyEvent]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut down: Vec<Key> = Vec::new();
    for (i, event) in events.iter().enumerate() {
        if event.up {
            if let Some(at) = down.iter().position(|k| *k == event.key) {
                down.remove(at);
            }
        } else {
            down.push(event.key);
        }
        if down.is_empty() {
            out.push(&events[start..=i]);
            start = i + 1;
        }
    }
    if start < events.len() {
        out.push(&events[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use iamb_platform::compensate;
    use iamb_platform::core_types::ExpansionId;

    use super::super::strategy::Typing;
    use super::*;

    /// The text a batch types into a plain editor: Unicode units decoded, Enter as `\n`,
    /// Shift+Enter as `⇧\n`, Backspace as `⌫`, Left as `←`, other keys as `<vk>`; checks
    /// that every press is released.
    fn render(events: &[KeyEvent]) -> String {
        let mut units = Vec::new();
        let mut out = String::new();
        let mut shift = false;
        let mut pressed: Vec<Key> = Vec::new();
        for event in events {
            assert_eq!(event.marker, Marker::Output);
            if event.up {
                let at = pressed
                    .iter()
                    .position(|k| *k == event.key)
                    .expect("released a key that was not pressed");
                pressed.remove(at);
                if event.key == Key::Vk(VK_SHIFT) {
                    shift = false;
                }
                continue;
            }
            pressed.push(event.key);
            match event.key {
                Key::Unicode(u) => units.push(u),
                Key::Vk(vk) => {
                    out.push_str(&String::from_utf16(&units).expect("whole characters"));
                    units.clear();
                    match vk {
                        VK_SHIFT => shift = true,
                        VK_RETURN if shift => out.push_str("⇧\n"),
                        VK_RETURN => out.push('\n'),
                        VK_BACK => out.push('⌫'),
                        VK_LEFT => out.push('←'),
                        other => write!(out, "<{other:#x}>").expect("a string"),
                    }
                }
            }
        }
        assert!(pressed.is_empty(), "keys left down");
        out.push_str(&String::from_utf16(&units).expect("whole characters"));
        out
    }

    fn typed(text: &str, line_break: LineBreak) -> Vec<KeyEvent> {
        let mut out = Vec::new();
        push_text(&mut out, text, line_break);
        out
    }

    fn unicode_units(events: &[KeyEvent]) -> Vec<u16> {
        events
            .iter()
            .filter(|e| !e.up)
            .map(|e| match e.key {
                Key::Unicode(u) => u,
                Key::Vk(_) => panic!("a virtual key"),
            })
            .collect()
    }

    #[test]
    fn text_becomes_its_utf16_units_pressed_and_released() {
        for text in [
            "plain ascii",
            "café naïve façade Ærøskøbing",
            "e\u{301} (combining acute)",
            "漢字かなカナ 한국어",
            "😀",
            "👍🏽",
            "👨‍👩‍👧‍👦",
            "🏳️‍🌈",
            "🇫🇷🇯🇵",
            "𝄞 𠜎",
            "\t tab and nbsp\u{a0}",
        ] {
            let events = typed(text, LineBreak::Enter);
            let expected: Vec<u16> = text.encode_utf16().collect();
            assert_eq!(unicode_units(&events), expected, "{text}");
            assert_eq!(events.len(), expected.len() * 2, "{text}");
            // Each unit is pressed and then released before the next one.
            for pair in events.chunks(2) {
                assert!(!pair[0].up && pair[1].up);
                assert_eq!(pair[0].key, pair[1].key);
            }
            assert_eq!(render(&events), text);
        }
    }

    #[test]
    fn characters_outside_the_bmp_are_surrogate_pairs() {
        let events = typed("👍", LineBreak::Enter);
        assert_eq!(
            events,
            [
                KeyEvent::output(Key::Unicode(0xD83D), false),
                KeyEvent::output(Key::Unicode(0xD83D), true),
                KeyEvent::output(Key::Unicode(0xDC4D), false),
                KeyEvent::output(Key::Unicode(0xDC4D), true),
            ]
        );
        // Skin tone: two characters, both outside the BMP.
        assert_eq!(typed("👍🏽", LineBreak::Enter).len(), 8);
        // A flag: two regional indicators.
        assert_eq!(
            unicode_units(&typed("🇫🇷", LineBreak::Enter)),
            [0xD83C, 0xDDEB, 0xD83C, 0xDDF7]
        );
    }

    #[test]
    fn line_breaks_press_the_strategy_key() {
        for text in ["one\ntwo", "one\r\ntwo", "one\rtwo"] {
            assert_eq!(
                render(&typed(text, LineBreak::Enter)),
                "one\ntwo",
                "{text:?}"
            );
            assert_eq!(
                render(&typed(text, LineBreak::ShiftEnter)),
                "one⇧\ntwo",
                "{text:?}"
            );
        }
        assert_eq!(render(&typed("a\n\nb", LineBreak::Enter)), "a\n\nb");
        assert_eq!(render(&typed("a\r\n\r\nb", LineBreak::Enter)), "a\n\nb");
        assert_eq!(render(&typed("a\n\rb", LineBreak::Enter)), "a\n\nb");
        assert_eq!(render(&typed("end\n", LineBreak::Enter)), "end\n");
        let enter = typed("\n", LineBreak::Enter);
        assert_eq!(enter, [KeyEvent::down(VK_RETURN), KeyEvent::up(VK_RETURN)]);
        let shifted = typed("\n", LineBreak::ShiftEnter);
        assert_eq!(
            shifted,
            [
                KeyEvent::down(VK_SHIFT),
                KeyEvent::down(VK_RETURN),
                KeyEvent::up(VK_RETURN),
                KeyEvent::up(VK_SHIFT),
            ]
        );
        // No line break leaks through as a Unicode unit.
        assert!(
            typed("a\r\nb\nc\r", LineBreak::Enter)
                .iter()
                .all(|e| !matches!(e.key, Key::Unicode(0x0A | 0x0D)))
        );
    }

    #[test]
    fn named_keys() {
        assert_eq!(vk_of(KeyName::Enter), VK_RETURN);
        assert_eq!(vk_of(KeyName::PageDown), VK_NEXT);
        for key in KeyName::ALL {
            let vk = vk_of(key);
            assert!(vk != 0 && vk != VK_UNASSIGNED);
        }
        let mut out = Vec::new();
        push_key(&mut out, VK_TAB, true, 2);
        assert_eq!(
            out,
            [
                KeyEvent::down(VK_SHIFT),
                KeyEvent::down(VK_TAB),
                KeyEvent::up(VK_TAB),
                KeyEvent::down(VK_TAB),
                KeyEvent::up(VK_TAB),
                KeyEvent::up(VK_SHIFT),
            ]
        );
        assert!(is_extended(VK_LEFT) && is_extended(VK_DELETE) && is_extended(VK_RMENU));
        assert!(!is_extended(VK_BACK) && !is_extended(VK_RETURN) && !is_extended(VK_LMENU));
    }

    #[test]
    fn markers_are_releases_of_the_unassigned_key() {
        let e = ExpansionId::from_raw(9);
        let event = KeyEvent::marker(Marker::InjectionStart(e));
        assert_eq!(event.key, Key::Vk(VK_UNASSIGNED));
        assert!(event.up);
        assert_eq!(event.marker, Marker::InjectionStart(e));
        assert_eq!(KeyEvent::down(VK_BACK).marker, Marker::Output);
    }

    #[test]
    fn debug_never_shows_text() {
        let events = typed("s", LineBreak::Enter);
        let shown = format!("{events:?}");
        assert_eq!(shown, "[Unicode down, Unicode up]");
        assert!(!shown.contains("115") && !shown.contains("0x73"));
        assert_eq!(format!("{:?}", KeyEvent::down(VK_BACK)), "Vk(0x08) down");
    }

    #[test]
    fn paste_chords() {
        assert_eq!(
            paste_chord(PasteKeys::CtrlV),
            [
                KeyEvent::down(VK_CONTROL),
                KeyEvent::down(VK_V),
                KeyEvent::up(VK_V),
                KeyEvent::up(VK_CONTROL),
            ]
        );
        assert_eq!(
            paste_chord(PasteKeys::ShiftInsert)[1],
            KeyEvent::down(VK_INSERT)
        );
    }

    #[test]
    fn held_modifiers_are_released_first() {
        assert!(release_held(&[]).is_empty());
        assert_eq!(release_held(&[VK_LSHIFT]), [KeyEvent::up(VK_LSHIFT)]);
        assert_eq!(
            release_held(&[VK_RCONTROL, VK_LSHIFT]),
            [KeyEvent::up(VK_LSHIFT), KeyEvent::up(VK_RCONTROL)]
        );
        // Alt and Windows: a masking tap first, so that their release opens no menu.
        for vk in [VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN] {
            assert_eq!(
                release_held(&[vk]),
                [
                    KeyEvent::down(VK_UNASSIGNED),
                    KeyEvent::up(VK_UNASSIGNED),
                    KeyEvent::up(vk)
                ]
            );
        }
        // Nothing is pressed again.
        assert!(release_held(&MODIFIERS).iter().skip(2).all(|e| e.up));
    }

    fn plan_for<'a>(steps: &'a [InjectionStep], k: usize, strategy: &Strategy) -> Plan<'a> {
        Plan::new(&compensate(steps, k), strategy)
    }

    fn text(s: &str) -> InjectionStep {
        InjectionStep::TypeText(s.to_owned())
    }

    #[test]
    fn a_plain_expansion_is_one_batch() {
        let steps = [InjectionStep::DeleteBack(4), text("be right back ")];
        let plan = plan_for(&steps, 0, &Strategy::DEFAULT);
        assert!(plan.ops.is_empty());
        assert_eq!(render(&plan.last), "⌫⌫⌫⌫be right back ");
        assert_eq!(plan.retype, 0);
    }

    #[test]
    fn compensation_deletes_the_typed_characters_too() {
        let steps = [InjectionStep::DeleteBack(3), text("be right back")];
        let plan = plan_for(&steps, 2, &Strategy::DEFAULT);
        assert!(plan.ops.is_empty());
        assert_eq!(render(&plan.last), "⌫⌫⌫⌫⌫be right back");
        assert_eq!(plan.retype, 2);

        // A palette pick has nothing of its own to delete.
        let steps = [text("hello")];
        let plan = plan_for(&steps, 1, &Strategy::DEFAULT);
        assert_eq!(render(&plan.last), "⌫hello");
        assert_eq!(plan.retype, 1);
    }

    #[test]
    fn every_step_kind_in_one_batch() {
        let steps = [
            InjectionStep::DeleteBack(2),
            text("<b>"),
            InjectionStep::PressKey {
                key: KeyName::Enter,
                shift: false,
                times: 2,
            },
            text("</b>"),
            InjectionStep::MoveCaretBack(4),
            InjectionStep::DeleteBack(1),
        ];
        let plan = plan_for(&steps, 0, &Strategy::DEFAULT);
        assert!(plan.ops.is_empty());
        assert_eq!(render(&plan.last), "⌫⌫<b>\n\n</b>←←←←⌫");
    }

    #[test]
    fn a_wait_ends_a_batch() {
        let steps = [
            InjectionStep::DeleteBack(1),
            text("a"),
            InjectionStep::Wait(250),
            text("b"),
            InjectionStep::Wait(10),
            InjectionStep::Wait(20),
        ];
        let plan = plan_for(&steps, 1, &Strategy::DEFAULT);
        assert_eq!(plan.ops.len(), 5);
        let Op::Keys(first) = &plan.ops[0] else {
            panic!("a batch first")
        };
        assert_eq!(render(first), "⌫⌫a");
        assert_eq!(plan.ops[1], Op::Sleep(250));
        let Op::Keys(second) = &plan.ops[2] else {
            panic!("a batch")
        };
        assert_eq!(render(second), "b");
        assert_eq!(plan.ops[3], Op::Sleep(10));
        assert_eq!(plan.ops[4], Op::Sleep(20));
        assert!(plan.last.is_empty());
        // ... and the trailing wait: the retyped characters and the end marker come after it,
        // in a final batch of their own.
        let steps = [text("a"), InjectionStep::Wait(20)];
        let plan = plan_for(&steps, 2, &Strategy::DEFAULT);
        assert_eq!(plan.ops.len(), 2);
        assert!(plan.last.is_empty());
        assert_eq!(plan.retype, 2);
    }

    #[test]
    fn long_text_is_pasted_and_ends_a_batch() {
        let long = "x".repeat(201);
        let steps = [InjectionStep::DeleteBack(3), text(&long), text("!")];
        let plan = plan_for(&steps, 0, &Strategy::DEFAULT);
        assert_eq!(plan.ops.len(), 2);
        let Op::Keys(delete) = &plan.ops[0] else {
            panic!("the deletion first")
        };
        assert_eq!(render(delete), "⌫⌫⌫");
        assert_eq!(
            plan.ops[1],
            Op::Paste(Cow::Owned(ClipboardContent::text(long.clone())))
        );
        assert_eq!(render(&plan.last), "!");

        // At the threshold it is still typed; characters, not bytes or units, count.
        let at = "é".repeat(200);
        let steps = [text(&at)];
        assert!(plan_for(&steps, 0, &Strategy::DEFAULT).ops.is_empty());
        let emoji = "👍".repeat(201);
        let steps = [text(&emoji)];
        assert_eq!(plan_for(&steps, 0, &Strategy::DEFAULT).ops.len(), 1);
    }

    #[test]
    fn typing_only_never_pastes_text() {
        let keys = Strategy {
            typing: Typing::Keys,
            ..Strategy::DEFAULT
        };
        let long = "y".repeat(5000);
        let steps = [text(&long)];
        let plan = plan_for(&steps, 0, &keys);
        assert!(plan.ops.is_empty());
        assert_eq!(plan.last.len(), 10_000);

        // A rich paste types its plain text; content without text still pastes.
        let rich = ClipboardContent::rich("line\nnext", "<p>line<br>next</p>");
        let files = ClipboardContent {
            files: vec!["C:\\a.png".into()],
            ..ClipboardContent::default()
        };
        let steps = [
            InjectionStep::Paste(rich),
            InjectionStep::Paste(files.clone()),
            InjectionStep::Paste(ClipboardContent::default()),
        ];
        let plan = plan_for(&steps, 0, &keys);
        assert_eq!(plan.ops.len(), 2);
        let Op::Keys(typed) = &plan.ops[0] else {
            panic!("typed")
        };
        assert_eq!(render(typed), "line\nnext");
        assert_eq!(plan.ops[1], Op::Paste(Cow::Borrowed(&files)));
        assert!(plan.last.is_empty());
    }

    #[test]
    fn paste_strategy_pastes_every_text_step() {
        let paste = Strategy {
            typing: Typing::Paste,
            ..Strategy::DEFAULT
        };
        let rich = ClipboardContent::rich("a", "<b>a</b>");
        let steps = [
            InjectionStep::DeleteBack(1),
            text("hi"),
            InjectionStep::Paste(rich.clone()),
            text(""),
        ];
        let plan = plan_for(&steps, 0, &paste);
        // The empty text step sends nothing.
        assert_eq!(plan.ops.len(), 3);
        assert_eq!(
            plan.ops[1],
            Op::Paste(Cow::Owned(ClipboardContent::text("hi")))
        );
        assert_eq!(plan.ops[2], Op::Paste(Cow::Borrowed(&rich)));
        assert!(plan.last.is_empty());
    }

    #[test]
    fn an_empty_expansion_has_an_empty_final_batch() {
        let plan = plan_for(&[], 0, &Strategy::DEFAULT);
        assert!(plan.ops.is_empty() && plan.last.is_empty());
        assert_eq!(plan.retype, 0);
        let steps = [text("")];
        assert!(plan_for(&steps, 0, &Strategy::DEFAULT).last.is_empty());
    }

    #[test]
    fn shift_enter_strategy_for_typed_line_breaks_only() {
        let chat = Strategy {
            line_break: LineBreak::ShiftEnter,
            ..Strategy::DEFAULT
        };
        let steps = [
            text("hi\nthere"),
            InjectionStep::PressKey {
                key: KeyName::Enter,
                shift: false,
                times: 1,
            },
        ];
        // The line break in the text is Shift+Enter; an explicit Enter stays Enter.
        assert_eq!(render(&plan_for(&steps, 0, &chat).last), "hi⇧\nthere\n");
    }

    #[test]
    fn key_delay_sends_one_key_at_a_time() {
        let slow = Strategy {
            key_delay_ms: 3,
            ..Strategy::DEFAULT
        };
        let steps = [InjectionStep::DeleteBack(1), text("a\nb")];
        let plan = plan_for(&steps, 1, &slow);
        let rendered: Vec<String> = plan
            .ops
            .iter()
            .map(|op| match op {
                Op::Keys(k) => render(k),
                Op::Sleep(ms) => format!("sleep {ms}"),
                Op::Paste(_) => "paste".to_owned(),
            })
            .collect();
        assert_eq!(
            rendered,
            [
                "⌫", "sleep 3", "⌫", "sleep 3", "a", "sleep 3", "\n", "sleep 3"
            ]
        );
        assert_eq!(render(&plan.last), "b");
        assert_eq!(plan.retype, 1);

        // A chord stays together, and a surrogate pair is one character.
        let steps = [text("\n👍")];
        let chat = Strategy {
            line_break: LineBreak::ShiftEnter,
            ..slow
        };
        let plan = plan_for(&steps, 0, &chat);
        let Op::Keys(chord) = &plan.ops[0] else {
            panic!("a chord")
        };
        assert_eq!(chord.len(), 4);
        assert_eq!(plan.last.len(), 2);
        assert_eq!(plan.ops.len(), 4);
    }

    #[test]
    fn batch_delay_separates_batches() {
        let slow = Strategy {
            batch_delay_ms: 30,
            ..Strategy::DEFAULT
        };
        // One batch: nothing to separate.
        let steps = [InjectionStep::DeleteBack(1), text("a")];
        assert!(plan_for(&steps, 0, &slow).ops.is_empty());

        let long = "z".repeat(300);
        let steps = [
            InjectionStep::DeleteBack(1),
            text(&long),
            text("a"),
            InjectionStep::Wait(10),
            text("b"),
            InjectionStep::Wait(100),
        ];
        let plan = plan_for(&steps, 0, &slow);
        let kinds: Vec<String> = plan
            .ops
            .iter()
            .map(|op| match op {
                Op::Keys(k) => render(k),
                Op::Sleep(ms) => format!("sleep {ms}"),
                Op::Paste(_) => "paste".to_owned(),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "⌫",
                "sleep 30",
                "paste",
                "sleep 30",
                "a",
                "sleep 30",
                "b",
                "sleep 100"
            ]
        );
        assert!(plan.last.is_empty());
    }

    #[test]
    fn groups_split_at_released_keys() {
        let mut events = Vec::new();
        push_key(&mut events, VK_RETURN, true, 1);
        push_taps(&mut events, VK_BACK, 2);
        let split = groups(&events);
        assert_eq!(split.len(), 3);
        assert_eq!(split[0].len(), 4);
        // A press never released is a group of its own at the end.
        let dangling = [KeyEvent::down(VK_SHIFT)];
        assert_eq!(groups(&dangling).len(), 1);
        assert!(groups(&[]).is_empty());
    }
}
