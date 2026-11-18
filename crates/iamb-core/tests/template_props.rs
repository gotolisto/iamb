//! Property tests: the template pipeline never panics, every failure is a known code from the
//! right phase, the output is well formed, and text without `{` is typed unchanged.

use iamb_core::datetime::{Format, LocalDateTime, Locale};
use iamb_core::template::{
    ErrorKind, FormValues, NoSnippets, RenderContext, Rendered, RenderedAction, Template,
    render_template,
};
use proptest::prelude::*;
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;

const PIECES: &[&str] = &[
    "{",
    "}",
    "{date",
    "{time",
    "{clipboard",
    "{cursor",
    "{snippet",
    "{field",
    "{key",
    "{wait",
    "{if",
    "{else",
    "{endif",
    "{lbrace",
    " ",
    "\n",
    "\r\n",
    "\r",
    "\t",
    "=",
    "\"",
    "\\",
    "\\\"",
    "fmt",
    "shift",
    "trim",
    "abbr",
    "name",
    "type",
    "default",
    "options",
    "on",
    "off",
    "times",
    "ms",
    "field",
    "is",
    "not",
    "%",
    "%Y",
    "%-d",
    "%Q",
    "%%",
    "+",
    "-",
    "0",
    "1",
    "50",
    "9999",
    "10000",
    "d",
    "mo",
    "bd",
    "min",
    "m",
    "y",
    "yes",
    "no",
    "text",
    "area",
    "choice",
    "toggle",
    "|",
    "a",
    "b",
    "s1",
    "s2",
    "s3",
    "enter",
    "tab",
    "é",
    "👍",
    "\u{301}",
    "{cursor}",
    "{key name=tab}",
    "{wait ms=100}",
    "{endif}",
    "{else}",
    "{if field=a}",
    "{if field=a is=x}",
    "{field name=a}",
    "{field name=t type=toggle on=Y}",
    "{snippet abbr=s1}",
    "{snippet abbr=s2}",
    "{date}",
    "{date fmt=\"%A %j\" shift=\"+3bd -1mo\"}",
    "{time shift=+9999y}",
    "{lbrace}",
    "{clipboard}",
];

fn macro_heavy(max: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(PIECES), 0..max).prop_map(|v| v.concat())
}

fn context_parts() -> (Format, Format) {
    (
        Format::parse("%Y-%m-%d").expect("date format"),
        Format::parse("%H:%M").expect("time format"),
    )
}

/// Checks the invariants every successful render must satisfy.
fn check_output(r: &Rendered) -> Result<(), TestCaseError> {
    let mut previous_was_text = false;
    let mut total = String::new();
    for a in &r.actions {
        match a {
            RenderedAction::Text { text, verbatim } => {
                prop_assert!(!text.is_empty(), "empty text action");
                prop_assert!(!previous_was_text, "adjacent text actions were not merged");
                let mut end = 0;
                for range in verbatim {
                    prop_assert!(range.start < range.end, "empty verbatim range");
                    prop_assert!(range.start > end || (end == 0 && range.start == 0));
                    prop_assert!(range.end <= text.len());
                    prop_assert!(text.is_char_boundary(range.start));
                    prop_assert!(text.is_char_boundary(range.end));
                    end = range.end;
                }
                total.push_str(text);
                previous_was_text = true;
            }
            RenderedAction::Key { times, .. } => {
                prop_assert!((1..=50).contains(times));
                previous_was_text = false;
            }
            RenderedAction::Wait { ms } => {
                prop_assert!((1..=10_000).contains(ms));
                previous_was_text = false;
            }
        }
    }
    prop_assert!(!total.contains('\r'), "CR in output");
    if let Some(back) = r.cursor_back {
        prop_assert!(back <= total.graphemes(true).count());
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn parse_never_panics_on_arbitrary_strings(s in any::<String>()) {
        if let Ok(t) = Template::parse(&s) {
            let _ = t.tokens();
        }
    }

    #[test]
    fn parse_never_panics_on_macro_heavy_strings(s in macro_heavy(40)) {
        match Template::parse(&s) {
            Ok(t) => {
                // Tokens tile the source exactly.
                let mut at = 0;
                for tok in t.tokens() {
                    let range = match tok {
                        iamb_core::template::Token::Text { range }
                        | iamb_core::template::Token::Macro { range, .. } => range,
                    };
                    prop_assert_eq!(range.start, at);
                    prop_assert!(range.end > range.start);
                    at = range.end;
                }
                prop_assert_eq!(at, s.chars().count());
            }
            Err(e) => {
                prop_assert!(!e.kind().is_render_time());
                prop_assert!(e.position().is_some_and(|p| p <= s.chars().count()));
            }
        }
    }

    #[test]
    fn pipeline_never_panics_and_fails_only_with_known_codes(
        template in macro_heavy(30),
        bodies in prop::collection::vec(macro_heavy(10), 3),
        present in prop::collection::vec(any::<bool>(), 3),
        values in prop::collection::vec(prop::sample::select(&["", "yes", "no", "x", "a\r\nb"][..]), 3),
        clipboard in prop::option::of(prop::sample::select(&["", " c\r\n", "{date}"][..])),
        now in prop::sample::select(&["2026-03-18T14:05:09", "9999-12-31T23:59:59", "0001-01-01T00:00:00"][..]),
    ) {
        let library: HashMap<String, String> = bodies
            .iter()
            .zip(&present)
            .enumerate()
            .filter(|(_, (_, here))| **here)
            .map(|(i, (body, _))| (format!("s{}", i + 1), body.clone()))
            .collect();
        let values: FormValues = ["a", "t", "b"].iter().copied().zip(values).collect();
        let (date_format, time_format) = context_parts();
        let ctx = RenderContext {
            now: LocalDateTime::parse_iso(now).expect("now"),
            locale: Locale::from_tag("de-AT"),
            date_format: &date_format,
            time_format: &time_format,
            clipboard,
        };
        match Template::parse(&template).and_then(|t| t.resolve(&library)) {
            Err(e) => {
                prop_assert!(ErrorKind::from_code(e.code()) == Some(e.kind()));
                prop_assert!(!e.kind().is_render_time(), "{} before render", e.code());
                prop_assert!(e.position().is_some());
            }
            Ok(resolved) => match resolved.render(&ctx, &values) {
                Err(e) => {
                    prop_assert!(e.kind().is_render_time(), "{} at render", e.code());
                }
                Ok(r) => {
                    check_output(&r)?;
                    prop_assert!(r.cursor_back.is_none() || resolved.uses_cursor());
                }
            },
        }
    }

    #[test]
    fn text_without_braces_is_typed_unchanged(s in "[^{]*") {
        let (date_format, time_format) = context_parts();
        let ctx = RenderContext {
            now: LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("now"),
            locale: Locale::from_tag("en"),
            date_format: &date_format,
            time_format: &time_format,
            clipboard: None,
        };
        let r = render_template(&s, &NoSnippets, &ctx, &FormValues::new()).expect("renders");
        let expected = s.replace("\r\n", "\n").replace('\r', "\n");
        if expected.is_empty() {
            prop_assert!(r.actions.is_empty());
        } else {
            prop_assert_eq!(
                r.actions,
                vec![RenderedAction::Text { text: expected, verbatim: vec![] }]
            );
        }
        prop_assert_eq!(r.cursor_back, None);
        let t = Template::parse(&s).expect("parses").resolve(&NoSnippets).expect("resolves");
        prop_assert!(t.form().is_empty());
    }
}
