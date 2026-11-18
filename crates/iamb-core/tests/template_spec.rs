//! Runs every example in `spec/template/examples/*.toml` (spec §8) through parse, resolve and
//! render, and compares with the expected error or output.

use iamb_core::datetime::{Format, LocalDateTime, Locale};
use iamb_core::template::{FieldKind, FormValues, RenderContext, RenderedAction, Template};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::PathBuf;
use toml::Value;

/// The groups and counts of the spec §8 table.
const GROUPS: [(&str, usize, usize); 10] = [
    ("date", 25, 5),
    ("time", 16, 4),
    ("clipboard", 10, 2),
    ("cursor", 14, 2),
    ("snippet", 16, 8),
    ("field", 21, 8),
    ("key", 14, 7),
    ("wait", 11, 6),
    ("if", 20, 8),
    ("syntax", 19, 7),
];

#[derive(Debug, PartialEq)]
enum Outcome {
    Error(String),
    Output {
        actions: Vec<Action>,
        cursor_back: Option<usize>,
        form: Vec<Field>,
    },
}

#[derive(Debug, PartialEq)]
enum Action {
    Text(String),
    Key { key: String, shift: bool, times: u8 },
    Wait(u32),
}

#[derive(Debug, PartialEq)]
struct Field {
    name: String,
    kind: String,
    default: String,
    options: Option<Vec<String>>,
}

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/template/examples")
}

fn str_of<'v>(v: &'v Value, key: &str) -> Option<&'v str> {
    v.get(key).and_then(Value::as_str)
}

fn string_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_table)
        .map(|t| {
            t.iter()
                .map(|(k, v)| (k.clone(), v.as_str().expect("string value").to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn expected(exp: &Value) -> Outcome {
    if let Some(code) = str_of(exp, "error") {
        return Outcome::Error(code.to_owned());
    }
    let actions = match exp.get("actions").and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .map(|a| {
                if let Some(t) = str_of(a, "text") {
                    Action::Text(t.to_owned())
                } else if let Some(k) = str_of(a, "key") {
                    Action::Key {
                        key: k.to_owned(),
                        shift: a.get("shift").and_then(Value::as_bool).unwrap_or(false),
                        times: a
                            .get("times")
                            .and_then(Value::as_integer)
                            .map_or(1, |t| u8::try_from(t).expect("times fits u8")),
                    }
                } else {
                    let ms = a.get("wait").and_then(Value::as_integer).expect("wait ms");
                    Action::Wait(u32::try_from(ms).expect("ms fits u32"))
                }
            })
            .collect(),
        None => match str_of(exp, "text") {
            Some(t) if !t.is_empty() => vec![Action::Text(t.to_owned())],
            _ => Vec::new(),
        },
    };
    let form = exp
        .get("form")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|f| Field {
                    name: str_of(f, "name").expect("form name").to_owned(),
                    kind: str_of(f, "type").expect("form type").to_owned(),
                    default: str_of(f, "default").expect("form default").to_owned(),
                    options: f.get("options").and_then(Value::as_array).map(|o| {
                        o.iter()
                            .map(|s| s.as_str().expect("option").to_owned())
                            .collect()
                    }),
                })
                .collect()
        })
        .unwrap_or_default();
    Outcome::Output {
        actions,
        cursor_back: exp
            .get("cursor_back")
            .and_then(Value::as_integer)
            .map(|n| usize::try_from(n).expect("cursor_back fits usize")),
        form,
    }
}

fn actual(template: &str, ctx: Option<&Value>) -> Outcome {
    let get = |key: &str| ctx.and_then(|c| str_of(c, key));
    let now = LocalDateTime::parse_iso(get("now").unwrap_or("2026-03-18T14:05:09")).expect("now");
    let date_format = Format::parse(get("date_format").unwrap_or("%Y-%m-%d")).expect("date fmt");
    let time_format = Format::parse(get("time_format").unwrap_or("%H:%M")).expect("time fmt");
    let snippets: HashMap<String, String> = string_map(ctx.and_then(|c| c.get("snippets")))
        .into_iter()
        .collect();
    let values: FormValues = string_map(ctx.and_then(|c| c.get("values")))
        .into_iter()
        .collect();
    let render_ctx = RenderContext {
        now,
        locale: Locale::from_tag(get("locale").unwrap_or("en")),
        date_format: &date_format,
        time_format: &time_format,
        clipboard: get("clipboard"),
    };
    let resolved = match Template::parse(template).and_then(|t| t.resolve(&snippets)) {
        Ok(r) => r,
        Err(e) => return Outcome::Error(e.code().to_owned()),
    };
    let form = resolved
        .form()
        .iter()
        .map(|f| Field {
            name: f.name.clone(),
            kind: f.kind.as_str().to_owned(),
            default: f.default.clone(),
            options: match &f.kind {
                FieldKind::Choice(o) => Some(o.clone()),
                _ => None,
            },
        })
        .collect();
    match resolved.render(&render_ctx, &values) {
        Err(e) => Outcome::Error(e.code().to_owned()),
        Ok(r) => Outcome::Output {
            actions: r
                .actions
                .into_iter()
                .map(|a| match a {
                    RenderedAction::Text { text, .. } => Action::Text(text),
                    RenderedAction::Key { key, shift, times } => Action::Key {
                        key: key.as_str().to_owned(),
                        shift,
                        times,
                    },
                    RenderedAction::Wait { ms } => Action::Wait(ms),
                })
                .collect(),
            cursor_back: r.cursor_back,
            form,
        },
    }
}

#[test]
fn every_spec_example_passes() {
    let mut files: Vec<PathBuf> = std::fs::read_dir(examples_dir())
        .expect("examples dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();

    let mut total = 0;
    let mut failures = String::new();
    let mut per_group: HashMap<String, (usize, usize)> = HashMap::new();
    let mut ids = std::collections::HashSet::new();
    for path in &files {
        let doc: Value = toml::from_str(&std::fs::read_to_string(path).expect("read"))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let examples = doc
            .get("example")
            .and_then(Value::as_array)
            .expect("[[example]] list");
        for ex in examples {
            total += 1;
            let id = str_of(ex, "id").expect("id");
            assert!(ids.insert(id.to_owned()), "duplicate id {id}");
            let group = str_of(ex, "macro").expect("macro");
            let exp = ex.get("expect").expect("expect");
            let entry = per_group.entry(group.to_owned()).or_default();
            entry.0 += 1;
            if exp.get("error").is_some() {
                entry.1 += 1;
            }
            let want = expected(exp);
            let got = actual(str_of(ex, "template").expect("template"), ex.get("ctx"));
            if want != got {
                let _ = writeln!(
                    failures,
                    "{id} ({}):\n  want {want:?}\n  got  {got:?}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
        }
    }
    assert!(failures.is_empty(), "failing examples:\n{failures}");
    assert!(total >= 166, "only {total} examples");
    for (group, count, errors) in GROUPS {
        let (n, e) = per_group.get(group).copied().unwrap_or_default();
        assert!(
            n >= count && e >= errors,
            "group {group}: {n} examples / {e} errors, spec table says {count} / {errors}"
        );
    }
}
