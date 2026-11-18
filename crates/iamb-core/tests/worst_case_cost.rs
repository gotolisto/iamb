//! Worst-case cost of parsing and rendering library content other people can write (review 1.9
//! findings 1, 3 and 10). The bounds are for release builds (`cargo test --release`); debug
//! builds get 25 times the room, so the tests still catch a return to quadratic or worse.

use iamb_core::datetime::{Format, LocalDateTime, Locale};
use iamb_core::template::{
    ErrorKind, FormValues, NoSnippets, RenderContext, Rendered, SnippetSource, Template,
    TemplateError,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

fn budget(release_ms: u64) -> Duration {
    let factor = if cfg!(debug_assertions) { 25 } else { 1 };
    Duration::from_millis(release_ms * factor)
}

/// Parses, resolves and renders `source`, returning the result and the time all three took.
fn timed(
    source: &str,
    lib: &dyn SnippetSource,
    clipboard: Option<&str>,
) -> (Result<Rendered, TemplateError>, Duration) {
    let (date, time) = (
        Format::parse("%Y-%m-%d").expect("format"),
        Format::parse("%H:%M").expect("format"),
    );
    let ctx = RenderContext {
        now: LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("now"),
        locale: Locale::from_tag("en"),
        date_format: &date,
        time_format: &time,
        clipboard,
    };
    let start = Instant::now();
    let out = Template::parse(source)
        .and_then(|t| t.resolve(lib))
        .and_then(|r| r.render(&ctx, &FormValues::new()));
    (out, start.elapsed())
}

/// Finding 1: an 80 KB body of business-day shifts, each its own date node.
#[test]
fn many_business_day_shifts_render_within_budget() {
    let source = "{date shift=+9999bd}".repeat(4000);
    assert_eq!(source.len(), 80_000);
    let one = timed("{date shift=+9999bd}", &NoSnippets, None)
        .0
        .expect("renders")
        .text();
    let (out, elapsed) = timed(&source, &NoSnippets, None);
    match out {
        Ok(r) => assert!(r.text() == one.repeat(4000), "wrong text"),
        Err(e) => assert_eq!(e.kind(), ErrorKind::OutputTooLarge),
    }
    assert!(elapsed < budget(25), "{elapsed:?}");
}

/// Finding 1: one shift of 10,000 business-day terms (80 KB).
#[test]
fn long_business_day_shift_renders_within_budget() {
    let source = format!(
        "{{date shift=\"{}\"}}",
        "+9999bd -9999bd ".repeat(5000).trim_end()
    );
    let (out, elapsed) = timed(&source, &NoSnippets, None);
    assert_eq!(out.expect("renders").text(), "2026-03-18");
    assert!(elapsed < budget(25), "{elapsed:?}");
}

/// Finding 3: `{clipboard trim=yes}` reached 10,000 times through nested snippets, with a
/// 64 KiB clipboard that trims to one letter. The clipboard is normalised and trimmed once.
#[test]
fn repeated_trimmed_clipboard_renders_within_budget() {
    let lib: HashMap<String, String> = [
        ("b".to_owned(), "{snippet abbr=c}".repeat(10)),
        ("c".to_owned(), "{clipboard trim=yes}".repeat(100)),
    ]
    .into_iter()
    .collect();
    let clipboard = format!("\r\n{}x{}", " ".repeat(32 * 1024), " ".repeat(32 * 1024));
    let (out, elapsed) = timed(&"{snippet abbr=b}".repeat(10), &lib, Some(&clipboard));
    assert_eq!(out.expect("renders").text(), "x".repeat(10_000));
    assert!(elapsed < budget(50), "{elapsed:?}");
}

/// Finding 10: the duplicate check of 60,000 `choice` options is linear.
#[test]
fn many_choice_options_parse_within_budget() {
    let options: Vec<String> = (0..60_000).map(|i| format!("o{i}")).collect();
    let source = format!(
        "{{field name=x type=choice options=\"{}\"}}",
        options.join("|")
    );
    let start = Instant::now();
    let parsed = Template::parse(&source);
    let elapsed = start.elapsed();
    assert!(parsed.is_ok());
    assert!(elapsed < budget(100), "{elapsed:?}");

    // A duplicate at the very end is still found.
    let source = format!(
        "{{field name=x type=choice options=\"{}|o0\"}}",
        options.join("|")
    );
    let e = Template::parse(&source).expect_err("duplicate option");
    assert_eq!(e.kind(), ErrorKind::BadValue);
}
