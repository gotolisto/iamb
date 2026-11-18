//! The injector against a real application: `TextEdit`, the reference row of the compatibility
//! matrix (M1). Each test opens a new `TextEdit` document, injects into it through the public
//! `Injector`, reads the document's text back and compares it exactly.
//!
//! # What these tests need
//!
//! | Need | Why | Without it |
//! | --- | --- | --- |
//! | Accessibility for the process that runs the tests (the terminal or IDE running `cargo test`) | the system drops synthetic key events from processes without it | every test is skipped |
//! | Automation permission to control `TextEdit` (asked by the system on first use) | the tests open a document and read its text through `AppleScript` | every test is skipped |
//! | An unlocked screen where `TextEdit` can come to the front | key events go to the frontmost window | the test is skipped |
//!
//! A hosted CI runner cannot grant Accessibility, so there every test is skipped. A skip is
//! never silent: it writes `SKIPPED <test>: <reason>` straight to standard error, which the
//! test harness does not capture. These tests run on the owner's Mac.
//!
//! Do not type or click while they run: real keys would land in the document.
//!
//! `TextEdit`'s own substitutions (smart quotes and dashes, spelling correction, text
//! replacement) would change some text after it arrives; the texts below avoid what they
//! act on: no quotes, no double hyphens, only dictionary words around the accents.

#![cfg(target_os = "macos")]

use std::io::Write as _;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use iamb_platform::core_types::{AppId, ExpansionId, KeyName, Sensitive};
use iamb_platform::{
    ClipboardAccess, ClipboardContent, ClipboardFormat, InjectionOutcome, InjectionRequest,
    InjectionStep, Injector, ListenerEvent, PermissionKind, PlatformError, TypedHandoff,
};
use iamb_platform_macos::Platform;
use objc2_core_graphics::CGPreflightPostEventAccess;

const TEXTEDIT: &str = "com.apple.TextEdit";
const TRANSIENT: &str = "org.nspasteboard.TransientType";

/// Writes past the harness's capture, so a skip is always seen.
fn skip(test: &str, why: &str) {
    let _ = writeln!(
        std::io::stderr().lock(),
        "SKIPPED injector_textedit::{test}: {why}"
    );
}

/// Runs an `AppleScript`; the text it printed, or why it failed (the system's message, which
/// names the script error, never document text).
fn osascript(lines: &[&str]) -> Result<String, String> {
    let mut command = Command::new("osascript");
    for line in lines {
        command.arg("-e").arg(line);
    }
    let output = command.output().map_err(|e| format!("osascript: {e}"))?;
    if output.status.success() {
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        if text.ends_with('\n') {
            text.pop();
        }
        Ok(text)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// A new `TextEdit` document in front, closed (without saving) when dropped. Holds the turn:
/// one test at a time owns the keyboard focus.
struct Document {
    _turn: MutexGuard<'static, ()>,
}

static TURN: Mutex<()> = Mutex::new(());

impl Document {
    /// Opens a document, or says why the test is skipped.
    fn open(test: &str) -> Option<Self> {
        let turn = TURN.lock().unwrap_or_else(PoisonError::into_inner);
        if !CGPreflightPostEventAccess() {
            skip(
                test,
                "the process that runs the tests lacks the Accessibility permission, without \
                 which macOS drops synthetic key events. Grant Accessibility in System \
                 Settings, Privacy & Security, to the terminal or IDE that runs `cargo test`",
            );
            return None;
        }
        if let Err(why) = osascript(&[
            "tell application \"TextEdit\"",
            "activate",
            "make new document",
            "end tell",
        ]) {
            skip(
                test,
                &format!(
                    "TextEdit could not be controlled ({why}). Allow the terminal or IDE that \
                     runs `cargo test` to control TextEdit in System Settings, Privacy & \
                     Security, Automation"
                ),
            );
            return None;
        }
        let document = Self { _turn: turn };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if osascript(&["tell application \"TextEdit\" to get frontmost"]).as_deref()
                == Ok("true")
            {
                break;
            }
            if Instant::now() >= deadline {
                skip(
                    test,
                    "TextEdit did not come to the front (is the screen locked?)",
                );
                return None;
            }
            thread::sleep(Duration::from_millis(50));
        }
        // Let the new window become key before the first event arrives.
        thread::sleep(Duration::from_millis(400));
        Some(document)
    }

    /// The document's text, with line breaks as `\n`.
    fn text() -> String {
        osascript(&["tell application \"TextEdit\" to get text of front document"])
            .expect("TextEdit answers")
            .replace("\r\n", "\n")
            .replace('\r', "\n")
    }

    /// Waits until the document holds `expected` (the application handles the events after
    /// they were posted), and returns what it holds then.
    fn wait_for(expected: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = Self::text();
            if text == expected || Instant::now() >= deadline {
                return text;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        let _ = osascript(&["tell application \"TextEdit\" to close front document saving no"]);
    }
}

/// A hand-off sealed for `expansion` with `typed_after` (what the listener would have
/// recorded between the trigger and the start marker).
fn sealed(expansion: ExpansionId, typed_after: &str) -> TypedHandoff {
    let typed = TypedHandoff::new();
    typed.arm(expansion);
    for (seq, c) in (1..).zip(typed_after.chars()) {
        typed.observe(&ListenerEvent::char(c, seq));
    }
    typed.observe(&ListenerEvent::InjectionStarted {
        expansion,
        seq: 1000,
    });
    typed
}

fn text(s: &str) -> InjectionStep {
    InjectionStep::TypeText(s.to_owned())
}

/// Injects `steps` as expansion `n` into `TextEdit`, with `typed_after` typed after its
/// trigger.
fn inject(
    platform: &Platform,
    n: u64,
    steps: Vec<InjectionStep>,
    typed_after: &str,
) -> InjectionOutcome {
    let expansion = ExpansionId::from_raw(n);
    let request = InjectionRequest::new(expansion, steps).in_app(AppId(TEXTEDIT.to_owned()));
    let typed = sealed(expansion, typed_after);
    let outcome = platform.inject(&request, &typed);
    typed.finish(expansion);
    outcome
}

fn finished(retyped: usize) -> InjectionOutcome {
    InjectionOutcome::Finished { retyped }
}

#[test]
fn plain_text_arrives_exactly() {
    let Some(_doc) = Document::open("plain_text_arrives_exactly") else {
        return;
    };
    let platform = Platform::new();
    let body = "Kind regards, the Iamb team";
    assert_eq!(inject(&platform, 1, vec![text(body)], ""), finished(0));
    assert_eq!(Document::wait_for(body), body);
}

#[test]
fn line_breaks_and_tabs_arrive_as_keys() {
    let Some(_doc) = Document::open("line_breaks_and_tabs_arrive_as_keys") else {
        return;
    };
    let platform = Platform::new();
    let body = "First line\nSecond line\n\nAfter a blank line\tand a tab";
    assert_eq!(inject(&platform, 2, vec![text(body)], ""), finished(0));
    assert_eq!(Document::wait_for(body), body);
}

#[test]
fn emoji_accents_and_cjk_arrive_intact() {
    let Some(_doc) = Document::open("emoji_accents_and_cjk_arrive_intact") else {
        return;
    };
    let platform = Platform::new();
    // Longer than one key event, with clusters that straddle where a naive cut would fall.
    let body = "Café, naïve façade, résumé, piñata. 日本語のテキスト, 한국어. \
                👋🏽 👩‍💻 ✅ 🇯🇵🇫🇷 👨‍👩‍👧‍👦 🏴\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F} ❤️ 1️⃣";
    assert_eq!(inject(&platform, 3, vec![text(body)], ""), finished(0));
    assert_eq!(Document::wait_for(body), body);
}

#[test]
fn deletion_caret_movement_and_keys() {
    let Some(_doc) = Document::open("deletion_caret_movement_and_keys") else {
        return;
    };
    let platform = Platform::new();
    // Type, delete the last word, type another, then insert in the middle with the caret.
    let steps = vec![
        text("Hello there"),
        InjectionStep::DeleteBack(5),
        text("world"),
        InjectionStep::MoveCaretBack(5),
        text("big "),
        // Right, not End: in a Mac text view End scrolls and leaves the caret where it is.
        InjectionStep::PressKey {
            key: KeyName::Right,
            shift: false,
            times: 5,
        },
        InjectionStep::Wait(30),
        text("!"),
    ];
    assert_eq!(inject(&platform, 4, steps, ""), finished(0));
    let expected = "Hello big world!";
    assert_eq!(Document::wait_for(expected), expected);
}

#[test]
fn characters_typed_after_the_trigger_are_moved_after_the_expansion() {
    let Some(_doc) =
        Document::open("characters_typed_after_the_trigger_are_moved_after_the_expansion")
    else {
        return;
    };
    let platform = Platform::new();
    // On screen: the abbreviation and two characters typed after it (here typed by an
    // earlier injection, as the user's keys would have been).
    // (The abbreviation is a dictionary word, so spelling correction leaves it alone.)
    assert_eq!(
        inject(&platform, 5, vec![text("Say back!?")], ""),
        finished(0)
    );
    assert_eq!(Document::wait_for("Say back!?"), "Say back!?");
    let steps = vec![InjectionStep::DeleteBack(4), text("be right back")];
    assert_eq!(inject(&platform, 6, steps, "!?"), finished(2));
    let expected = "Say be right back!?";
    assert_eq!(Document::wait_for(expected), expected);
}

/// What is on the pasteboard, as formats and lengths, without the history marker a restore
/// adds (the text itself is compared through `read_text`).
fn pasteboard_shape(platform: &Platform) -> Vec<(ClipboardFormat, usize)> {
    platform
        .snapshot()
        .expect("snapshot")
        .flavours()
        .iter()
        .filter(|f| f.format != ClipboardFormat::Name(TRANSIENT.to_owned()))
        .map(|f| (f.format.clone(), f.data.expose().len()))
        .collect()
}

#[test]
fn long_text_is_pasted_and_the_pasteboard_restored() {
    let Some(_doc) = Document::open("long_text_is_pasted_and_the_pasteboard_restored") else {
        return;
    };
    let platform = Platform::new();
    let saved = platform.snapshot().expect("snapshot");
    platform
        .write(
            &ClipboardContent::rich("clip-sentinel-1", "<b>clip-sentinel-1</b>"),
            false,
        )
        .expect("sentinel on the pasteboard");
    let shape_before = pasteboard_shape(&platform);
    let line = "This line of plain prose is long enough to be pasted.";
    let body = [line; 12].join("\n");
    assert!(body.chars().count() > 300, "above the default threshold");
    assert_eq!(inject(&platform, 7, vec![text(&body)], ""), finished(0));
    let got = Document::wait_for(&body);
    let sentinel = platform
        .read_text()
        .expect("read")
        .map(Sensitive::into_inner);
    let shape_after = pasteboard_shape(&platform);
    let _ = platform.restore(saved);
    assert_eq!(got, body);
    assert_eq!(
        sentinel.as_deref(),
        Some("clip-sentinel-1"),
        "the pasteboard is restored"
    );
    assert_eq!(
        shape_after, shape_before,
        "every type is back, at its length"
    );
}

#[test]
fn a_rich_paste_step_arrives_and_the_pasteboard_is_restored() {
    let Some(_doc) = Document::open("a_rich_paste_step_arrives_and_the_pasteboard_is_restored")
    else {
        return;
    };
    let platform = Platform::new();
    let saved = platform.snapshot().expect("snapshot");
    platform
        .write(&ClipboardContent::text("clip-sentinel-2"), false)
        .expect("sentinel on the pasteboard");
    let steps = vec![
        text("Before "),
        InjectionStep::Paste(ClipboardContent::rich("bold words", "<b>bold words</b>")),
        text(" after"),
    ];
    assert_eq!(inject(&platform, 8, steps, ""), finished(0));
    let expected = "Before bold words after";
    let got = Document::wait_for(expected);
    let sentinel = platform
        .read_text()
        .expect("read")
        .map(Sensitive::into_inner);
    let _ = platform.restore(saved);
    assert_eq!(got, expected);
    assert_eq!(sentinel.as_deref(), Some("clip-sentinel-2"));
}

#[test]
fn a_paste_strategy_from_settings_is_used() {
    let Some(_doc) = Document::open("a_paste_strategy_from_settings_is_used") else {
        return;
    };
    let platform = Platform::new();
    platform
        .configure(r#"{"apps": {"com.apple.TextEdit": {"method": "paste"}}}"#)
        .expect("valid settings");
    let saved = platform.snapshot().expect("snapshot");
    platform
        .write(&ClipboardContent::text("clip-sentinel-3"), false)
        .expect("sentinel");
    let body = "Short, but pasted: Ærø 👋🏽";
    assert_eq!(inject(&platform, 9, vec![text(body)], ""), finished(0));
    let got = Document::wait_for(body);
    let sentinel = platform
        .read_text()
        .expect("read")
        .map(Sensitive::into_inner);
    let _ = platform.restore(saved);
    assert_eq!(got, body);
    assert_eq!(sentinel.as_deref(), Some("clip-sentinel-3"));
}

#[test]
fn without_accessibility_injection_is_refused() {
    if CGPreflightPostEventAccess() {
        skip(
            "without_accessibility_injection_is_refused",
            "this process has the Accessibility permission, so the refusal cannot be shown \
             here; the crate's unit tests cover it with a fake check",
        );
        return;
    }
    let platform = Platform::new();
    assert_eq!(
        inject(&platform, 10, vec![text("x")], ""),
        InjectionOutcome::Failed(PlatformError::PermissionDenied(
            PermissionKind::Accessibility
        ))
    );
}
