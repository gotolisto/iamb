//! The injector end to end: expansions go through the real `Injector` into an edit control
//! on a window of this test, and the control's text is read back and compared exactly. Runs
//! on Windows only (CI's Windows leg).
//!
//! A low-level hook in this process plays the listener's part for the hand-off: it seals the
//! hand-off when the injector's start marker goes by. Keys "typed after the trigger" are
//! given to the hand-off directly and put into the control's text beforehand, which is where
//! the application would have put them.
//!
//! The tests type into whichever window has the focus, so each one first makes sure its own
//! window does. A test that needs something the session does not have (an interactive
//! desktop that delivers synthetic input, the foreground) says `SKIPPED` and why, and fails
//! instead when `IAMB_REQUIRE_DESKTOP` is set.

#![cfg(windows)]

#[path = "injector_support.rs"]
mod support;

use iamb_platform::core_types::{AppId, KeyName};
use iamb_platform::{
    ClipboardAccess, ClipboardContent, InjectionOutcome, InjectionRequest, InjectionStep, Injector,
};
use iamb_platform_windows::Platform;

use support::{
    CF_UNICODETEXT, Clip, HANDOFF, Sealer, TestWindow, alone, arm, registered, skip,
    typed_after_trigger, utf16_bytes,
};

/// The application name the tests inject for: not in the built-in table.
const APP: &str = "iamb-injector-test.exe";

/// A focused test window, the sealing hook and a platform; `None` (with the reason said)
/// when the session cannot run the test.
struct Desk {
    window: TestWindow,
    platform: Platform,
    _sealer: Sealer,
}

impl Desk {
    fn open() -> Option<Self> {
        let Some(sealer) = Sealer::start() else {
            skip("this session does not allow a low-level keyboard hook");
            return None;
        };
        let Some(window) = TestWindow::open() else {
            skip("this session cannot create a window");
            return None;
        };
        if !window.focus() {
            skip("the test window could not take the foreground and the keyboard focus");
            return None;
        }
        let desk = Self {
            window,
            platform: Platform::new(),
            _sealer: sealer,
        };
        // An empty expansion: both markers, nothing typed. It shows whether the desktop
        // delivers synthetic input and the hook sees it.
        match desk.inject_raw(Vec::new(), &[]) {
            InjectionOutcome::Finished { .. } => Some(desk),
            other => {
                skip(&format!(
                    "this session's desktop does not deliver synthetic input ({other:?})"
                ));
                None
            }
        }
    }

    fn inject_raw(&self, steps: Vec<InjectionStep>, typed: &[char]) -> InjectionOutcome {
        let expansion = arm();
        for &ch in typed {
            typed_after_trigger(ch);
        }
        let request = InjectionRequest::new(expansion, steps).in_app(AppId(APP.to_owned()));
        let outcome = self.platform.inject(&request, &HANDOFF);
        HANDOFF.finish(expansion);
        outcome
    }

    /// Starts from `before` (caret at the end), injects, and returns the control's text once
    /// it is `expected` (or after a timeout). `None` when the window lost the focus.
    fn run(
        &self,
        before: &str,
        steps: Vec<InjectionStep>,
        typed: &[char],
        expected: &str,
    ) -> Option<(InjectionOutcome, String)> {
        self.window.set_text(before);
        if !self.window.focus() {
            skip("the test window lost the keyboard focus");
            return None;
        }
        let outcome = self.inject_raw(steps, typed);
        Some((outcome, self.window.wait_for(expected)))
    }
}

fn text(s: &str) -> InjectionStep {
    InjectionStep::TypeText(s.to_owned())
}

#[test]
fn plain_text_replaces_the_abbreviation() {
    let _alone = alone();
    let Some(desk) = Desk::open() else { return };
    let Some((outcome, got)) = desk.run(
        "see you brb",
        vec![InjectionStep::DeleteBack(3), text("be right back")],
        &[],
        "see you be right back",
    ) else {
        return;
    };
    assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
    assert_eq!(got, "see you be right back");
}

#[test]
fn multi_line_text_presses_enter() {
    let _alone = alone();
    let Some(desk) = Desk::open() else { return };
    let Some((outcome, got)) = desk.run(
        ";addr",
        vec![
            InjectionStep::DeleteBack(5),
            text("1 Main Street\nSpringfield\r\n\nEnd"),
        ],
        &[],
        "1 Main Street\r\nSpringfield\r\n\r\nEnd",
    ) else {
        return;
    };
    assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
    assert_eq!(got, "1 Main Street\r\nSpringfield\r\n\r\nEnd");
}

#[test]
fn shift_enter_from_the_strategy_table_still_breaks_the_line() {
    let _alone = alone();
    let Some(desk) = Desk::open() else { return };
    desk.platform
        .set_injection_settings(&format!(
            r#"{{"apps": {{"{APP}": {{"line_break": "shift_enter"}}}}}}"#
        ))
        .expect("valid overrides");
    let Some((outcome, got)) = desk.run("x", vec![text("a\nb")], &[], "xa\r\nb") else {
        return;
    };
    assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
    assert_eq!(got, "xa\r\nb");
}

#[test]
fn accents_emoji_flags_and_cjk_arrive_intact() {
    let _alone = alone();
    let Some(desk) = Desk::open() else { return };
    let sample = "café naïve Ærøskøbing e\u{301} 👍 👍🏽 👨‍👩‍👧‍👦 🏳️‍🌈 🇫🇷🇯🇵 漢字かな 한국어 𝄞 𠜎";
    let Some((outcome, got)) = desk.run(
        "emo",
        vec![InjectionStep::DeleteBack(3), text(sample)],
        &[],
        sample,
    ) else {
        return;
    };
    assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
    assert_eq!(got, sample);
    assert_eq!(
        got.encode_utf16().collect::<Vec<_>>(),
        sample.encode_utf16().collect::<Vec<_>>()
    );
}

#[test]
fn keys_typed_after_the_trigger_are_deleted_and_typed_again() {
    let _alone = alone();
    let Some(desk) = Desk::open() else { return };
    // "brb" was the trigger; "xé" reached the control before the injector started.
    let Some((outcome, got)) = desk.run(
        "ok brbxé",
        vec![InjectionStep::DeleteBack(3), text("be right back")],
        &['x', 'é'],
        "ok be right backxé",
    ) else {
        return;
    };
    assert_eq!(outcome, InjectionOutcome::Finished { retyped: 2 });
    assert_eq!(got, "ok be right backxé");
}

#[test]
fn caret_moves_and_key_presses() {
    let _alone = alone();
    let Some(desk) = Desk::open() else { return };
    // `{cursor}` inside a tag: type both tags, move the caret back between them, then type
    // where it is.
    let Some((outcome, got)) = desk.run(
        ";b",
        vec![
            InjectionStep::DeleteBack(2),
            text("<b></b>"),
            InjectionStep::MoveCaretBack(4),
        ],
        &[],
        "<b></b>",
    ) else {
        return;
    };
    assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
    assert_eq!(got, "<b></b>");
    let outcome = desk.inject_raw(
        vec![
            text("bold"),
            InjectionStep::PressKey {
                key: KeyName::End,
                shift: false,
                times: 1,
            },
            text("!"),
            InjectionStep::Wait(20),
            InjectionStep::PressKey {
                key: KeyName::Backspace,
                shift: false,
                times: 1,
            },
            text("?"),
        ],
        &[],
    );
    assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
    assert_eq!(desk.window.wait_for("<b>bold</b>?"), "<b>bold</b>?");
}

#[test]
fn long_text_is_pasted_and_the_clipboard_comes_back() {
    let _alone = alone();
    let Some(desk) = Desk::open() else { return };
    let Some(clip) = Clip::open() else {
        skip("this session has no clipboard");
        return;
    };
    let custom = registered("Iamb Test Format");
    let original = [
        (CF_UNICODETEXT, utf16_bytes("the user's own clipboard ✓")),
        (custom, vec![0, 1, 2, 3, 254, 255]),
    ];
    clip.put(&original);
    drop(clip);

    let long = format!("{}\nsecond line 👍🏽 ünï", "Long paragraph. ".repeat(20));
    assert!(long.chars().count() > 200);
    let expected = format!("{}\r\nsecond line 👍🏽 ünï", "Long paragraph. ".repeat(20));
    let Some((outcome, got)) = desk.run(
        ";long",
        vec![InjectionStep::DeleteBack(5), text(&long)],
        &[],
        &expected,
    ) else {
        return;
    };
    assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
    assert_eq!(got, expected);

    let clip = Clip::open().expect("the clipboard again");
    for (format, bytes) in &original {
        assert_eq!(clip.get(*format).as_ref(), Some(bytes), "format {format}");
    }
}

#[test]
fn a_rich_paste_step_pastes_and_restores() {
    let _alone = alone();
    let Some(desk) = Desk::open() else { return };
    let Some(clip) = Clip::open() else {
        skip("this session has no clipboard");
        return;
    };
    clip.put(&[(CF_UNICODETEXT, utf16_bytes("before"))]);
    drop(clip);
    // An edit control takes the plain text of a rich paste.
    let Some((outcome, got)) = desk.run(
        "",
        vec![InjectionStep::Paste(ClipboardContent::rich(
            "rich text",
            "<b>rich</b> text",
        ))],
        &[],
        "rich text",
    ) else {
        return;
    };
    assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
    assert_eq!(got, "rich text");
    let restored = desk.platform.read_text().expect("readable").expect("text");
    assert_eq!(restored.expose(), "before");
}

/// KPI: the clipboard is restored in 100% of runs. Known content in two formats, a paste
/// through the injector, and the original back byte for byte, fifty times; the target gets
/// every paste.
#[test]
fn clipboard_is_restored_after_every_one_of_fifty_pastes() {
    let _alone = alone();
    let Some(desk) = Desk::open() else { return };
    let Some(clip) = Clip::open() else {
        skip("this session has no clipboard");
        return;
    };
    let custom = registered("Iamb Test Format 2");
    clip.put(&[
        (CF_UNICODETEXT, utf16_bytes("original ✓ 👍")),
        (custom, (0..=255).collect()),
    ]);
    // The baseline is what the clipboard reports, exactly.
    let baseline: Vec<(u32, Vec<u8>)> = [CF_UNICODETEXT, custom]
        .iter()
        .map(|f| (*f, clip.get(*f).expect("just put")))
        .collect();
    drop(clip);
    // A paste strategy: every text step goes through the clipboard.
    desk.platform
        .set_injection_settings(&format!(
            r#"{{"apps": {{"{APP}": {{"typing": "paste"}}}}}}"#
        ))
        .expect("valid overrides");

    for run in 0..50 {
        let pasted = format!("paste number {run} ✓");
        let Some((outcome, got)) = desk.run("", vec![text(&pasted)], &[], &pasted) else {
            return;
        };
        assert_eq!(
            outcome,
            InjectionOutcome::Finished { retyped: 0 },
            "run {run}"
        );
        assert_eq!(got, pasted, "run {run}: the target did not get the paste");
        let clip = Clip::open().expect("the clipboard");
        for (format, bytes) in &baseline {
            assert_eq!(
                clip.get(*format).as_ref(),
                Some(bytes),
                "run {run}: format {format} not restored"
            );
        }
    }
}
