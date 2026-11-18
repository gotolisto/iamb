//! Unit tests for the template API beyond the spec examples: output limits, verbatim ranges,
//! snippet graphs, positions, static flags, tokens and redaction.

#![expect(
    clippy::single_range_in_vec_init,
    reason = "verbatim ranges are compared as slices of ranges"
)]

use iamb_core::datetime::{Format, LocalDateTime, Locale};
use iamb_core::template::{
    ErrorKind, FieldKind, FormValues, KeyName, MAX_OUTPUT_CHARS, MacroName, NoSnippets,
    RenderContext, Rendered, RenderedAction, SnippetSource, Template, TemplateError, Token,
    render_template,
};
use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

struct Settings {
    date: Format,
    time: Format,
}

impl Settings {
    fn new() -> Self {
        Self {
            date: Format::parse("%Y-%m-%d").expect("date format"),
            time: Format::parse("%H:%M").expect("time format"),
        }
    }

    fn ctx<'a>(&'a self, clipboard: Option<&'a str>) -> RenderContext<'a> {
        RenderContext {
            now: LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("now"),
            locale: Locale::from_tag("en"),
            date_format: &self.date,
            time_format: &self.time,
            clipboard,
        }
    }
}

fn library(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn render(source: &str, lib: &dyn SnippetSource, values: &[(&str, &str)]) -> Rendered {
    try_render(source, lib, values, None).expect("renders")
}

fn try_render(
    source: &str,
    lib: &dyn SnippetSource,
    values: &[(&str, &str)],
    clipboard: Option<&str>,
) -> Result<Rendered, TemplateError> {
    let settings = Settings::new();
    let values: FormValues = values.iter().copied().collect();
    render_template(source, lib, &settings.ctx(clipboard), &values)
}

fn error_of(source: &str, lib: &dyn SnippetSource) -> TemplateError {
    match Template::parse(source).and_then(|t| t.resolve(lib)) {
        Ok(_) => panic!("expected a parse or resolve error for {source:?}"),
        Err(e) => e,
    }
}

fn texts(r: &Rendered) -> Vec<(&str, &[Range<usize>])> {
    r.actions
        .iter()
        .filter_map(|a| match a {
            RenderedAction::Text { text, verbatim } => Some((text.as_str(), verbatim.as_slice())),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ E_OUTPUT_TOO_LARGE

#[test]
fn output_limit_is_exactly_100000_characters() {
    let at_limit = "x".repeat(MAX_OUTPUT_CHARS);
    let r = render(&at_limit, &NoSnippets, &[]);
    assert_eq!(r.text().len(), MAX_OUTPUT_CHARS);

    let over = "x".repeat(MAX_OUTPUT_CHARS + 1);
    let e = try_render(&over, &NoSnippets, &[], None).expect_err("too large");
    assert_eq!(e.kind(), ErrorKind::OutputTooLarge);
    assert_eq!(e.code(), "E_OUTPUT_TOO_LARGE");
    assert_eq!(e.position(), None);
}

#[test]
fn output_limit_counts_grapheme_clusters() {
    // "e" + combining acute accent is one character of three bytes.
    let accented = "e\u{301}".repeat(MAX_OUTPUT_CHARS);
    let r = try_render(
        "{field name=\"V\" type=area}",
        &NoSnippets,
        &[("V", &accented)],
        None,
    );
    assert!(r.is_ok(), "100,000 accented letters fit");

    let more = format!("{accented}e");
    let e = try_render(
        "{field name=\"V\" type=area}",
        &NoSnippets,
        &[("V", &more)],
        None,
    )
    .expect_err("one more character is too many");
    assert_eq!(e.kind(), ErrorKind::OutputTooLarge);
}

#[test]
fn output_limit_counts_values_clipboard_and_nested_snippets() {
    let block = "y".repeat(1000);
    let lib = library(&[
        ("a", &block),
        ("b", &"{snippet abbr=\"a\"}".repeat(10)),
        ("c", &"{snippet abbr=\"b\"}".repeat(10)),
    ]);
    // 100 KB exactly from snippets is fine; one more character from the clipboard is not.
    assert!(try_render("{snippet abbr=\"c\"}", &lib, &[], None).is_ok());
    let e = try_render("{snippet abbr=\"c\"}{clipboard}", &lib, &[], Some("!")).expect_err("big");
    assert_eq!(e.kind(), ErrorKind::OutputTooLarge);
}

#[test]
fn output_limit_takes_precedence_over_later_render_errors() {
    let source = format!(
        "{{cursor}}{}{{key name=enter}}",
        "z".repeat(MAX_OUTPUT_CHARS + 1)
    );
    let e = try_render(&source, &NoSnippets, &[], None).expect_err("error");
    assert_eq!(e.kind(), ErrorKind::OutputTooLarge);

    // ...but not over earlier ones.
    let source = format!(
        "{{cursor}}{{key name=enter}}{}",
        "z".repeat(MAX_OUTPUT_CHARS + 1)
    );
    let e = try_render(&source, &NoSnippets, &[], None).expect_err("error");
    assert_eq!(e.kind(), ErrorKind::CursorBeforeKey);
}

// ------------------------------------------------------------------ verbatim ranges

#[test]
fn field_values_and_clipboard_are_verbatim() {
    let r = try_render(
        "Hi {field name=\"N\"}, see {clipboard}!",
        &NoSnippets,
        &[("N", "Ana")],
        Some("doc"),
    )
    .expect("renders");
    assert_eq!(texts(&r), vec![("Hi Ana, see doc!", &[3..6, 12..15][..])]);
}

#[test]
fn touching_verbatim_ranges_merge_and_empty_ones_vanish() {
    let r = try_render(
        "<{field name=\"A\"}{field name=\"Empty\"}{clipboard trim=yes}>",
        &NoSnippets,
        &[("A", "é")],
        Some("  x  "),
    )
    .expect("renders");
    assert_eq!(texts(&r), vec![("<éx>", &[1..4][..])]);
}

#[test]
fn verbatim_ranges_shift_when_text_merges_across_snippets() {
    let lib = library(&[("in", "[{field name=\"V\"}]")]);
    let r = render(
        "ab{snippet abbr=\"in\"}{snippet abbr=\"in\"}",
        &lib,
        &[("V", "vv")],
    );
    assert_eq!(texts(&r), vec![("ab[vv][vv]", &[3..5, 7..9][..])]);
}

#[test]
fn verbatim_ranges_are_relative_to_their_text_action() {
    let r = try_render(
        "{clipboard}{key name=tab}x{clipboard}",
        &NoSnippets,
        &[],
        Some("cb"),
    )
    .expect("renders");
    assert_eq!(texts(&r), vec![("cb", &[0..2][..]), ("xcb", &[1..3][..])]);
    assert_eq!(
        r.actions[1],
        RenderedAction::Key {
            key: KeyName::Tab,
            shift: false,
            times: 1
        }
    );
}

#[test]
fn template_text_dates_and_toggle_texts_are_not_verbatim() {
    let r = render(
        "{date fmt=\"%B\"} {field name=\"T\" type=toggle on=\"On\" off=\"Off\"} {snippet abbr=\"s\"}",
        &library(&[("s", "nested")]),
        &[("T", "yes")],
    );
    assert_eq!(texts(&r), vec![("March On nested", &[][..])]);
}

#[test]
fn choice_values_are_verbatim() {
    let r = render(
        "{field name=\"C\" type=choice options=\"Mr|Ms\"}.",
        &NoSnippets,
        &[],
    );
    assert_eq!(texts(&r), vec![("Mr.", &[0..2][..])]);
}

/// What the matcher's adaptive case mode will do (architecture §4.4): change case outside the
/// verbatim ranges only.
fn upper_outside(text: &str, verbatim: &[Range<usize>]) -> String {
    let mut out = String::new();
    let mut at = 0;
    for r in verbatim {
        out.push_str(&text[at..r.start].to_uppercase());
        out.push_str(&text[r.clone()]);
        at = r.end;
    }
    out.push_str(&text[at..].to_uppercase());
    out
}

#[test]
fn adaptive_case_can_skip_values() {
    let r = try_render(
        "dear {field name=\"Name\"}, re: {clipboard}",
        &NoSnippets,
        &[("Name", "de la Cruz")],
        Some("order #a1"),
    )
    .expect("renders");
    let [(text, verbatim)] = texts(&r)[..] else {
        panic!("one text action");
    };
    for r in verbatim {
        assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
    }
    assert_eq!(
        upper_outside(text, verbatim),
        "DEAR de la Cruz, RE: order #a1"
    );
}

// ------------------------------------------------------------------ snippet graphs

fn chain_library(levels: usize, fan_out: usize, leaf: &str) -> HashMap<String, String> {
    (1..=levels)
        .map(|i| {
            let body = if i == levels {
                leaf.to_owned()
            } else {
                format!("{{snippet abbr=\"s{}\"}}", i + 1).repeat(fan_out)
            };
            (format!("s{i}"), body)
        })
        .collect()
}

#[test]
fn doubling_library_eight_deep_is_fast() {
    let lib = chain_library(8, 2, "x{field name=\"F\"}");
    let start = Instant::now();
    let resolved = Template::parse("{snippet abbr=\"s1\"}")
        .and_then(|t| t.resolve(&lib))
        .expect("resolves");
    assert_eq!(resolved.form().len(), 1);
    let settings = Settings::new();
    let values: FormValues = [("F", "!")].into_iter().collect();
    let r = resolved
        .render(&settings.ctx(None), &values)
        .expect("renders");
    assert_eq!(r.text(), "x!".repeat(128));
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn wide_library_resolves_fast_and_render_is_bounded() {
    // 50^7 inclusions if expanded as a tree.
    let lib = chain_library(8, 50, "{cursor}");
    let start = Instant::now();
    let resolved = Template::parse("{snippet abbr=\"s1\"}")
        .and_then(|t| t.resolve(&lib))
        .expect("resolves");
    assert!(resolved.uses_cursor());
    let settings = Settings::new();
    let e = resolved
        .render(&settings.ctx(None), &FormValues::new())
        .expect_err("bounded");
    assert_eq!(e.kind(), ErrorKind::OutputTooLarge);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn errors_in_untaken_branches_of_wide_graphs_are_found_once() {
    let mut lib = chain_library(7, 40, "{if field=\"X\"}{snippet abbr=\"missing\"}{endif}");
    lib.insert("top".to_owned(), "{snippet abbr=\"s1\"}".to_owned());
    let start = Instant::now();
    let e = error_of("{snippet abbr=\"top\"}", &lib);
    assert_eq!(e.kind(), ErrorKind::SnippetNotFound);
    assert_eq!(e.snippet(), Some("s7"));
    assert_eq!(e.position(), Some(14));
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn shared_body_included_deeper_later_is_too_deep() {
    // `a` (height 2) is fine at the top, but the chain reaches it again 7 levels down.
    let mut lib = chain_library(7, 1, "{snippet abbr=\"a\"}");
    lib.insert("a".to_owned(), "{snippet abbr=\"b\"}".to_owned());
    lib.insert("b".to_owned(), "b".to_owned());
    let e = error_of("{snippet abbr=\"a\"}{snippet abbr=\"s1\"}", &lib);
    assert_eq!(e.kind(), ErrorKind::TooDeep);
    assert_eq!(e.snippet(), Some("s7"));

    // Six levels down it still fits.
    let mut lib = chain_library(6, 1, "{snippet abbr=\"a\"}");
    lib.insert("a".to_owned(), "{snippet abbr=\"b\"}".to_owned());
    lib.insert("b".to_owned(), "b".to_owned());
    assert_eq!(
        render("{snippet abbr=\"a\"}{snippet abbr=\"s1\"}", &lib, &[]).text(),
        "bb"
    );
}

#[test]
fn repeated_dates_in_shared_bodies_are_cheap() {
    let lib = chain_library(5, 10, "{date shift=\"+9999bd -9999bd +2bd\" fmt=\"%a\"}");
    let start = Instant::now();
    let r = render("{snippet abbr=\"s1\"}", &lib, &[]);
    assert_eq!(r.text(), "Fri".repeat(10_000));
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
}

// ------------------------------------------------------------------ positions

fn parse_error(source: &str) -> (ErrorKind, Option<usize>) {
    let e = Template::parse(source).expect_err("parse error");
    assert_eq!(e.snippet(), None);
    (e.kind(), e.position())
}

#[test]
fn parse_errors_report_char_positions() {
    use ErrorKind as K;
    assert_eq!(parse_error("{date fmt}"), (K::BadSyntax, Some(9)));
    assert_eq!(parse_error("{date fmt = \"%Y\"}"), (K::BadSyntax, Some(9)));
    assert_eq!(
        parse_error("{date fmt=\"%Y\"shift=x}"),
        (K::BadSyntax, Some(14))
    );
    assert_eq!(parse_error("ab {date"), (K::UnclosedMacro, Some(3)));
    assert_eq!(
        parse_error("ab {date fmt=\"%Y"),
        (K::UnclosedMacro, Some(13))
    );
    assert_eq!(
        parse_error("{date fmt=%Y shift=+1d fmt=%Y}"),
        (K::DuplicateAttr, Some(23))
    );
    assert_eq!(parse_error("ñ {date fmt=\"%Q\"}"), (K::BadFormat, Some(12)));
    assert_eq!(parse_error("{date shift=\"+1m\"}"), (K::BadShift, Some(12)));
    assert_eq!(parse_error("x {key}"), (K::MissingAttr, Some(2)));
    assert_eq!(parse_error("{key name=f1}"), (K::BadValue, Some(10)));
    assert_eq!(
        parse_error("{field name=a options=b}"),
        (K::UnknownAttr, Some(14))
    );
    assert_eq!(
        parse_error("{if field=a}x{else}y{else}"),
        (K::UnbalancedIf, Some(20))
    );
    assert_eq!(
        parse_error("{if field=a}{if field=b}{endif}"),
        (K::UnbalancedIf, Some(0))
    );
    assert_eq!(parse_error("{wait ms=0}"), (K::BadValue, Some(9)));
    let deep = "{if field=a}".repeat(9);
    assert_eq!(parse_error(&deep), (K::TooDeep, Some(96)));
}

#[test]
fn positions_count_source_characters_before_newline_normalisation() {
    // a CR LF b CR LF {date x=1}: the `x` is the 13th source character.
    assert_eq!(
        parse_error("a\r\nb\r\n{date x=1}"),
        (ErrorKind::UnknownAttr, Some(12))
    );
    // A lone CR is one character either way.
    assert_eq!(
        parse_error("a\rb\r{date x=1}"),
        (ErrorKind::UnknownAttr, Some(10))
    );
    // Emoji are one `char` each (not UTF-16 units).
    assert_eq!(
        parse_error("👍👍{date x=1}"),
        (ErrorKind::UnknownAttr, Some(8))
    );
}

#[test]
fn nested_errors_name_the_snippet_that_contains_them() {
    let lib = library(&[
        ("outer", "ok {snippet abbr=\"inner\"}"),
        ("inner", "\r\n{endif}"),
    ]);
    let e = error_of("x{snippet abbr=\"outer\"}", &lib);
    assert_eq!(e.kind(), ErrorKind::UnbalancedIf);
    assert_eq!(e.snippet(), Some("inner"));
    assert_eq!(e.position(), Some(2));
    assert!(e.to_string().contains("E_UNBALANCED_IF"));

    let e = error_of(
        "x{snippet abbr=\"outer\"}",
        &library(&[("outer", "ok {snippet abbr=\"nope\"}")]),
    );
    assert_eq!(
        (e.kind(), e.snippet(), e.position()),
        (ErrorKind::SnippetNotFound, Some("outer"), Some(3))
    );

    let e = error_of("ab{snippet abbr=\"nope\"}", &NoSnippets);
    assert_eq!((e.snippet(), e.position()), (None, Some(2)));
}

#[test]
fn resolve_errors_point_at_the_macro() {
    let e = error_of("{field name=a} {field name=a type=area}", &NoSnippets);
    assert_eq!(
        (e.kind(), e.position()),
        (ErrorKind::FieldConflict, Some(15))
    );
    let e = error_of(
        "{if field=a}x{endif} {if field=a is=b}y{endif}",
        &NoSnippets,
    );
    assert_eq!(
        (e.kind(), e.position()),
        (ErrorKind::FieldUndefined, Some(21))
    );
    let lib = library(&[("a", "{snippet abbr=\"a\"}")]);
    let e = error_of("..{snippet abbr=\"a\"}", &lib);
    assert_eq!(
        (e.kind(), e.snippet(), e.position()),
        (ErrorKind::SnippetCycle, Some("a"), Some(0))
    );
}

#[test]
fn conflicts_are_found_before_undefined_fields() {
    let e = error_of(
        "{if field=u is=x}{endif}{field name=c}{field name=c type=toggle}",
        &NoSnippets,
    );
    assert_eq!(e.kind(), ErrorKind::FieldConflict);
}

// ------------------------------------------------------------------ static flags and form

#[test]
fn needs_clipboard_sees_untaken_branches_and_nested_snippets() {
    let resolve = |s: &str, lib: &dyn SnippetSource| {
        Template::parse(s)
            .and_then(|t| t.resolve(lib))
            .expect("resolves")
    };
    assert!(!resolve("plain {date}", &NoSnippets).needs_clipboard());
    assert!(resolve("{if field=x}{clipboard}{endif}", &NoSnippets).needs_clipboard());
    let lib = library(&[("c", "[{clipboard trim=yes}]"), ("p", "plain")]);
    assert!(resolve("{snippet abbr=\"c\"}", &lib).needs_clipboard());
    assert!(!resolve("{snippet abbr=\"p\"}", &lib).needs_clipboard());
    // A clipboard macro in a value is not a macro.
    assert!(!resolve("{field name=v default=\"{clipboard}\"}", &NoSnippets).needs_clipboard());
}

#[test]
fn uses_cursor_is_static_and_cursor_back_is_rendered() {
    let resolved = Template::parse("a{if field=x}{cursor}{endif}b")
        .and_then(|t| t.resolve(&NoSnippets))
        .expect("resolves");
    assert!(resolved.uses_cursor());
    let settings = Settings::new();
    let off = resolved
        .render(&settings.ctx(None), &FormValues::new())
        .expect("renders");
    assert_eq!(off.cursor_back, None);
    let on: FormValues = [("x", "yes")].into_iter().collect();
    let on = resolved.render(&settings.ctx(None), &on).expect("renders");
    assert_eq!(on.cursor_back, Some(1));
    assert!(
        !Template::parse("abc")
            .and_then(|t| t.resolve(&NoSnippets))
            .expect("resolves")
            .uses_cursor()
    );
}

#[test]
fn cursor_back_counts_grapheme_clusters_across_waits() {
    let r = render("{cursor}e{wait ms=5}\u{301}👍🏽🇪🇸", &NoSnippets, &[]);
    assert_eq!(r.cursor_back, Some(3));
}

#[test]
fn form_lists_fields_with_kinds_and_defaults() {
    let resolved = Template::parse(
        "{if field=\"Rush\"}!{endif}{field name=\"Size\" type=choice options=\"S|M\" default=M}{field name=\"Rush\" type=toggle default=yes}{field name=\"Notes\" type=area default=\"a\nb\"}",
    )
    .and_then(|t| t.resolve(&NoSnippets))
    .expect("resolves");
    let form: Vec<_> = resolved
        .form()
        .iter()
        .map(|f| (f.name.as_str(), f.kind.clone(), f.default.as_str()))
        .collect();
    assert_eq!(
        form,
        vec![
            ("Rush", FieldKind::Toggle, "yes"),
            ("Size", FieldKind::Choice(vec!["S".into(), "M".into()]), "M"),
            ("Notes", FieldKind::Area, "a\nb"),
        ]
    );
}

#[test]
fn values_and_clipboard_line_endings_are_normalised() {
    let r = try_render(
        "{field name=v type=area}|{clipboard}",
        &NoSnippets,
        &[("v", "1\r\n2\r3")],
        Some("4\r\n5"),
    )
    .expect("renders");
    assert_eq!(r.text(), "1\n2\n3|4\n5");
}

#[test]
fn clipboard_trim_matches_the_reference_white_space() {
    let r = try_render(
        "[{clipboard trim=yes}]",
        &NoSnippets,
        &[],
        Some("\u{1f}\u{3000} x \u{a0}\u{1c}"),
    )
    .expect("renders");
    assert_eq!(r.text(), "[x]");
}

// ------------------------------------------------------------------ tokens

#[test]
fn tokens_give_text_runs_and_macros_with_char_ranges() {
    let t = Template::parse("Hé {field name=\"N\" default=\"a\\\"b\"}{lbrace}date}\r\n{endif x=1}")
        .err()
        .map(|e| e.kind());
    assert_eq!(t, Some(ErrorKind::UnknownAttr));

    let t = Template::parse("Hé {field name=\"N\" default=\"a\\\"b\"}{lbrace}date}\r\n{cursor}")
        .expect("parses");
    assert_eq!(
        t.tokens(),
        vec![
            Token::Text { range: 0..3 },
            Token::Macro {
                name: MacroName::Field,
                attributes: vec![
                    iamb_core::template::Attribute {
                        name: "name",
                        value: "N".to_owned()
                    },
                    iamb_core::template::Attribute {
                        name: "default",
                        value: "a\"b".to_owned()
                    },
                ],
                range: 3..34,
            },
            Token::Macro {
                name: MacroName::Lbrace,
                attributes: vec![],
                range: 34..42,
            },
            Token::Text { range: 42..49 },
            Token::Macro {
                name: MacroName::Cursor,
                attributes: vec![],
                range: 49..57,
            },
        ]
    );
    assert_eq!(t.source().chars().count(), 57);
}

#[test]
fn tokens_of_plain_and_empty_templates() {
    assert!(Template::parse("").expect("parses").tokens().is_empty());
    assert_eq!(
        Template::parse("{\"json\": {}}").expect("parses").tokens(),
        vec![Token::Text { range: 0..12 }]
    );
}

// ------------------------------------------------------------------ errors and redaction

#[test]
fn every_error_code_round_trips() {
    for kind in ErrorKind::ALL {
        assert_eq!(ErrorKind::from_code(kind.code()), Some(kind));
        assert!(kind.code().starts_with("E_"));
        assert_eq!(TemplateError::new(kind).code(), kind.code());
    }
    let render_time: Vec<_> = ErrorKind::ALL
        .into_iter()
        .filter(|k| k.is_render_time())
        .map(ErrorKind::code)
        .collect();
    assert_eq!(
        render_time,
        [
            "E_DATE_RANGE",
            "E_CURSOR_BEFORE_KEY",
            "E_WAIT_TOTAL",
            "E_OUTPUT_TOO_LARGE"
        ]
    );
    assert_eq!(ErrorKind::from_code("E_NOPE"), None);
}

#[test]
fn debug_output_never_shows_values_or_clipboard() {
    let secret = "hunter2-secret";
    let values: FormValues = [("Password", secret)].into_iter().collect();
    assert!(!format!("{values:?}").contains(secret));

    let settings = Settings::new();
    let ctx = settings.ctx(Some(secret));
    assert!(!format!("{ctx:?}").contains(secret));

    let resolved = Template::parse("{field name=Password}{clipboard}")
        .and_then(|t| t.resolve(&NoSnippets))
        .expect("resolves");
    let r = resolved.render(&ctx, &values).expect("renders");
    let shown = format!("{r:?} {:?}", r.actions);
    assert!(!shown.contains(secret), "{shown}");
    assert!(shown.contains("len"));
}

#[test]
fn keys_and_waits_stay_separate_actions() {
    let r = render(
        "{key name=pageup shift=yes times=3}{key name=enter}{wait ms=10}{wait ms=20}",
        &NoSnippets,
        &[],
    );
    assert_eq!(
        r.actions,
        vec![
            RenderedAction::Key {
                key: KeyName::PageUp,
                shift: true,
                times: 3
            },
            RenderedAction::Key {
                key: KeyName::Enter,
                shift: false,
                times: 1
            },
            RenderedAction::Wait { ms: 10 },
            RenderedAction::Wait { ms: 20 },
        ]
    );
    for key in KeyName::ALL {
        assert_eq!(KeyName::from_name(key.as_str()), Some(key));
    }
}

// ------------------------------------------------------------------ form values

/// Review 1.9 finding 14: values that do not fit their field are corrected at render.
#[test]
fn form_values_are_checked_against_their_fields() {
    let source = "Size: {field name=Size type=choice options=\"S|M|L\" default=M}\
        {if field=T is=maybe} MAYBE{endif}{if field=T} ON{endif}\
        {field name=T type=toggle on=+ off=-}|{field name=One}|{field name=Many type=area}";
    let r = render(
        source,
        &NoSnippets,
        &[
            ("Size", "XXL"),
            ("T", "maybe"),
            ("One", "two\nlines\r\nhere"),
            ("Many", "a\nb"),
        ],
    );
    // XXL is not an option: the default; "maybe" is not "yes": unchecked, also for `is`; the
    // one-line field gets spaces; the area keeps its line break.
    assert_eq!(r.text(), "Size: M-|two lines here|a\nb");
    assert_eq!(
        texts(&r),
        [("Size: M-|two lines here|a\nb", &[6..7, 9..23, 24..27][..])]
    );

    // Values that fit are used as given.
    let r = render(
        source,
        &NoSnippets,
        &[("Size", "L"), ("T", "yes"), ("One", "x"), ("Many", "")],
    );
    assert_eq!(r.text(), "Size: L ON+|x|");
}
