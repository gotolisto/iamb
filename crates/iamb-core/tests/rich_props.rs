//! Property tests of rich rendering (task 3.8): whatever the Markdown, the HTML is balanced,
//! carries only the allowed elements and attributes, links only to `http`, `https` and
//! `mailto`, shows only attachment images, and stays bounded; escaped text renders as itself.

use std::time::Instant;

use iamb_core::datetime::{Format, LocalDateTime, Locale};
use iamb_core::rich::{self, MAX_NESTING};
use iamb_core::template::{FormValues, RenderContext, RenderedAction, SnippetSource, Template};
use proptest::prelude::*;

const PIECES: &[&str] = &[
    "*",
    "**",
    "_",
    "~~",
    "`",
    "```\n",
    "#",
    "- ",
    "1. ",
    "> ",
    "\n",
    "\n\n",
    "  ",
    "    ",
    "|",
    "| --- |",
    "[",
    "]",
    "(",
    ")",
    "<",
    ">",
    "!",
    "&",
    "&#58;",
    "\"",
    "'",
    "\\",
    "text",
    "é",
    "👍",
    "<script>",
    "</p>",
    "<img src=x onerror=y>",
    "javascript:",
    "data:",
    "https://a.example",
    "mailto:a@b.example",
    "attachment:",
    "aa00000000000000000000000000000000000000000000000000000000000001",
    "{",
    "}",
    "<a@b.example>",
    "<https://x.example>",
    "\t",
    "=",
    "---",
];

fn arb_markdown() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(PIECES), 0..60).prop_map(|v| v.concat())
}

/// The tags the writer may produce, with whether they have a closing tag.
const ALLOWED: &[(&str, bool)] = &[
    ("p", true),
    ("br", false),
    ("h1", true),
    ("h2", true),
    ("h3", true),
    ("h4", true),
    ("h5", true),
    ("h6", true),
    ("em", true),
    ("strong", true),
    ("del", true),
    ("sup", true),
    ("sub", true),
    ("code", true),
    ("pre", true),
    ("ul", true),
    ("ol", true),
    ("li", true),
    ("blockquote", true),
    ("table", true),
    ("thead", true),
    ("tr", true),
    ("th", true),
    ("td", true),
    ("hr", false),
    ("a", true),
    ("img", false),
];

/// Checks the HTML tag by tag: every `<` starts an allowed tag with allowed attributes, the
/// tags nest properly and never deeper than the limit.
fn check_html(html: &str) -> Result<(), String> {
    let mut stack: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        rest = &rest[at + 1..];
        let end = rest.find('>').ok_or("unterminated tag")?;
        let tag = &rest[..end];
        rest = &rest[end + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            let open = stack.pop().ok_or_else(|| format!("stray </{name}>"))?;
            if open != name {
                return Err(format!("</{name}> closes <{open}>"));
            }
            continue;
        }
        let (name, attrs) = tag.split_once(' ').unwrap_or((tag, ""));
        let &(_, closes) = ALLOWED
            .iter()
            .find(|(n, _)| *n == name)
            .ok_or_else(|| format!("tag <{name}> is not allowed"))?;
        check_attributes(name, attrs)?;
        if closes {
            stack.push(name.to_owned());
            if stack.len() > MAX_NESTING {
                return Err("nested too deep".to_owned());
            }
        }
    }
    if stack.is_empty() {
        Ok(())
    } else {
        Err(format!("unclosed {stack:?}"))
    }
}

fn check_attributes(name: &str, attrs: &str) -> Result<(), String> {
    if attrs.is_empty() {
        return Ok(());
    }
    let mut rest = attrs;
    while !rest.is_empty() {
        let eq = rest
            .find("=\"")
            .ok_or_else(|| format!("bad attributes {attrs:?}"))?;
        let key = rest[..eq].trim();
        let after = &rest[eq + 2..];
        let close = after.find('"').ok_or("unterminated attribute")?;
        let value = &after[..close];
        rest = after[close + 1..].trim_start();
        let ok = match (name, key) {
            ("a", "href") => {
                let lower = value.to_ascii_lowercase();
                lower.starts_with("http:")
                    || lower.starts_with("https:")
                    || lower.starts_with("mailto:")
            }
            ("img", "src") => rich::attachment_of(value).is_some(),
            ("img", "alt") => true,
            ("ol", "start") => value.chars().all(|c| c.is_ascii_digit()),
            ("th" | "td", "style") => {
                ["text-align:left", "text-align:center", "text-align:right"].contains(&value)
            }
            _ => false,
        };
        if !ok {
            return Err(format!("attribute {key}={value:?} on <{name}>"));
        }
        if value.contains(['<', '>']) {
            return Err(format!("unescaped value {value:?}"));
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn html_is_always_safe_and_balanced(md in arb_markdown()) {
        let out = rich::render(&md).expect("small input renders");
        if let Err(e) = check_html(&out.html) {
            return Err(TestCaseError::fail(format!("{e}\nmarkdown {md:?}\nhtml {:?}", out.html)));
        }
        prop_assert_eq!(rich::to_plain(&md).expect("renders"), out.plain);
        for sha in &out.attachments {
            let src = format!("attachment:{sha}");
            prop_assert!(md.to_ascii_lowercase().contains(&src));
        }
    }

    #[test]
    fn arbitrary_text_never_panics(md in "\\PC{0,200}") {
        let out = rich::render(&md).expect("small input renders");
        prop_assert!(check_html(&out.html).is_ok(), "{:?}", out.html);
    }

    #[test]
    fn escaped_text_renders_as_itself(text in "[^\\x00-\\x1f\\x7f]{0,60}") {
        let text = text.trim_end_matches([' ', '\t']).to_owned();
        let out = rich::render(&rich::escape_markdown(&text)).expect("renders");
        prop_assert_eq!(&out.plain, &text);
        let html_tags: Vec<&str> = out.html.match_indices('<').map(|(i, _)| &out.html[i..]).collect();
        for t in html_tags {
            prop_assert!(t.starts_with("<p>") || t.starts_with("</p>"), "{:?}", out.html);
        }
    }
}

#[test]
fn pathological_input_is_fast() {
    let cases = [
        "*a".repeat(20_000),
        "**".repeat(20_000) + "x",
        "[".repeat(20_000),
        "> ".repeat(5_000) + "x",
        "- ".repeat(5_000) + "x",
        "<".repeat(20_000),
        "`".repeat(20_000),
        "![".repeat(10_000),
        "*_".repeat(20_000),
        "a_".repeat(20_000) + "b",
        format!("{}x{}", "[a](".repeat(5_000), ")".repeat(5_000)),
    ];
    for md in &cases {
        let start = Instant::now();
        let out = rich::render(md).expect("renders");
        let took = start.elapsed();
        assert!(took.as_secs_f64() < 2.0, "{} bytes took {took:?}", md.len());
        check_html(&out.html).unwrap_or_else(|e| panic!("{e}"));
    }
}

struct Lib(Vec<(&'static str, &'static str, bool)>);

impl SnippetSource for Lib {
    fn body(&self, abbreviation: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(a, _, _)| *a == abbreviation)
            .map(|(_, b, _)| *b)
    }

    fn is_rich(&self, abbreviation: &str) -> bool {
        self.0
            .iter()
            .any(|(a, _, rich)| *a == abbreviation && *rich)
    }
}

fn ctx_render(
    source: &str,
    lib: &Lib,
    values: &[(&str, &str)],
    clipboard: &str,
) -> Vec<RenderedAction> {
    let (date, time) = (
        Format::parse("%Y-%m-%d").expect("format"),
        Format::parse("%H:%M").expect("format"),
    );
    let ctx = RenderContext {
        now: LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("date"),
        locale: Locale::from_tag("fr"),
        date_format: &date,
        time_format: &time,
        clipboard: Some(clipboard),
    };
    let values: FormValues = values.iter().copied().collect();
    Template::parse(source)
        .expect("parses")
        .resolve(lib)
        .expect("resolves")
        .render_markdown(&ctx, &values)
        .expect("renders")
        .actions
}

#[test]
fn macro_output_in_markdown_is_escaped() {
    let lib = Lib(vec![
        ("p", "1. not a list *x*", false),
        ("r", "**r** {date fmt=\"%b\"}", true),
    ]);
    let actions = ctx_render(
        "# {field name=\"T\"}\n{clipboard} {snippet abbr=\"p\"} {snippet abbr=\"r\"} {date fmt=\"%b\"}",
        &lib,
        &[("T", "_x_ [y](javascript:z)")],
        "<b>\n    code",
    );
    let RenderedAction::Text { text, .. } = &actions[0] else {
        panic!("text expected");
    };
    let out = rich::render(text).expect("renders");
    assert_eq!(
        out.html,
        "<h1>_x_ [y](javascript:z)</h1>\n<p>&lt;b&gt;<br>\n    code 1. not a list *x* <strong>r</strong> mars mars</p>"
    );
}

#[test]
fn render_markdown_ignores_the_cursor() {
    let lib = Lib(vec![]);
    let (date, time) = (
        Format::parse("%Y").expect("format"),
        Format::parse("%H").expect("format"),
    );
    let ctx = RenderContext {
        now: LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("date"),
        locale: Locale::from_tag("en"),
        date_format: &date,
        time_format: &time,
        clipboard: None,
    };
    let rendered = Template::parse("a{cursor}{key name=tab}b")
        .expect("parses")
        .resolve(&lib)
        .expect("resolves")
        .render_markdown(&ctx, &FormValues::new())
        .expect("no E_CURSOR_BEFORE_KEY in Markdown");
    assert_eq!(rendered.cursor_back, None);
    assert_eq!(rendered.actions.len(), 3);
}
