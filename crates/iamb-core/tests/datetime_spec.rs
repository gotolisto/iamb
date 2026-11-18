//! Cross-checks the date and time API against `spec/template/examples/{date,time}.toml`.

use iamb_core::datetime::{DateError, Format, LocalDateTime, Locale, Shift};

/// Parses the attributes of a template that is exactly one `{name attr=value ...}` macro.
/// Returns `None` when the template is anything else.
fn single_macro(template: &str, name: &str) -> Option<Vec<(String, String)>> {
    let inner = template.strip_prefix('{')?.strip_suffix('}')?;
    let rest = inner.strip_prefix(name)?;
    if !(rest.is_empty() || rest.starts_with(' ')) || inner.contains('{') || inner.contains('}') {
        return None;
    }
    let chars: Vec<char> = rest.chars().collect();
    let mut i = 0;
    let mut attrs = Vec::new();
    loop {
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        if i >= chars.len() {
            return Some(attrs);
        }
        let start = i;
        while i < chars.len() && chars[i] != '=' {
            i += 1;
        }
        let key: String = chars[start..i].iter().collect();
        i += 1; // '='
        if i > chars.len() {
            return None;
        }
        let mut value = String::new();
        if chars.get(i) == Some(&'"') {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                value.push(chars[i]);
                i += 1;
            }
            i += 1;
        } else {
            while i < chars.len() && chars[i] != ' ' {
                value.push(chars[i]);
                i += 1;
            }
        }
        attrs.push((key, value));
    }
}

fn error_code(e: DateError) -> &'static str {
    match e {
        DateError::BadFormat => "E_BAD_FORMAT",
        DateError::BadShift => "E_BAD_SHIFT",
        DateError::DateRange => "E_DATE_RANGE",
        DateError::EmptyFormat => "E_BAD_VALUE",
        DateError::InvalidDate => "E_INVALID_DATE",
    }
}

fn check_file(file: &str, name: &str, default_key: &str, default_fmt: &str) -> usize {
    let path = format!(
        "{}/../../spec/template/examples/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).expect("read examples");
    let doc: toml::Table = text.parse().expect("parse toml");
    let examples = doc["example"].as_array().expect("examples");
    let mut checked = 0;
    for ex in examples {
        let id = ex["id"].as_str().unwrap_or("?");
        let template = ex["template"].as_str().expect("template");
        let Some(attrs) = single_macro(template, name) else {
            continue;
        };
        if attrs.iter().any(|(k, _)| k != "fmt" && k != "shift") {
            continue;
        }
        let attr = |k: &str| {
            attrs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        if attrs.len()
            != attrs.iter().filter(|(k, _)| k == "fmt").count()
                + attrs.iter().filter(|(k, _)| k == "shift").count()
            || attrs.iter().filter(|(k, _)| k == "fmt").count() > 1
            || attrs.iter().filter(|(k, _)| k == "shift").count() > 1
        {
            continue;
        }
        let ctx = ex.get("ctx").and_then(|c| c.as_table());
        let ctx_str = |k: &str| ctx.and_then(|c| c.get(k)).and_then(|v| v.as_str());
        let now = ctx_str("now").unwrap_or("2026-03-18T14:05:09");
        let locale = Locale::from_tag(ctx_str("locale").unwrap_or("en"));
        let fmt = attr("fmt")
            .or_else(|| ctx_str(default_key).map(str::to_owned))
            .unwrap_or_else(|| default_fmt.to_owned());
        let shift = attr("shift").unwrap_or_default();

        let result = (|| {
            let t = LocalDateTime::parse_iso(now)?;
            let format = Format::parse(&fmt)?;
            let shifted = Shift::parse(&shift)?.apply(t)?;
            Ok::<_, DateError>(format.render(&shifted, &locale))
        })();

        let expect = ex["expect"].as_table().expect("expect");
        if let Some(text) = expect.get("text").and_then(|v| v.as_str()) {
            assert_eq!(result.as_deref(), Ok(text), "{id}");
        } else {
            let code = expect["error"].as_str().expect("error code");
            let got = result.expect_err(id);
            assert_eq!(error_code(got), code, "{id}");
        }
        checked += 1;
    }
    checked
}

#[test]
fn spec_examples() {
    let n = check_file("date.toml", "date", "date_format", "%Y-%m-%d")
        + check_file("time.toml", "time", "time_format", "%H:%M");
    assert!(n >= 25, "only {n} examples checked");
}
