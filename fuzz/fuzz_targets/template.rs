//! Fuzz target: parse an arbitrary template, resolve it against a fixed small library and render
//! it with a fixed context. Besides crashes, it checks that every error comes from the phase that
//! may find it and that successful output is well formed.
//!
//! Seed the corpus with the spec example templates, e.g.
//! `python3 -c 'import tomllib,pathlib,hashlib; [pathlib.Path("corpus/template", hashlib.sha1(e["template"].encode()).hexdigest()).write_text(e["template"]) for p in pathlib.Path("../spec/template/examples").glob("*.toml") for e in tomllib.loads(p.read_text())["example"]]'`

#![no_main]

use iamb_core::datetime::{Format, LocalDateTime, Locale};
use iamb_core::template::{FormValues, RenderContext, RenderedAction, Template};
use libfuzzer_sys::fuzz_target;
use std::collections::HashMap;

fn library() -> HashMap<String, String> {
    let mut lib: HashMap<String, String> = [
        ("sig", "Best,\n{field name=\"Name\"}\n{date fmt=\"%A %-d %B\" shift=\"+1bd\"}"),
        ("two", "{snippet abbr=\"sig\"}{cursor}{snippet abbr=\"sig\"}"),
        ("wide", "{snippet abbr=\"two\"}{snippet abbr=\"two\"}{snippet abbr=\"two\"}"),
        ("loop", "{if field=\"t\"}{snippet abbr=\"loop\"}{endif}"),
        ("bad", "ok {endif}"),
        ("keys", "{key name=tab times=3}{wait ms=500}"),
        ("clip", "[{clipboard trim=yes}]"),
        ("form", "{field name=\"t\" type=toggle on=\"T\"}{field name=\"c\" type=choice options=\"x|y\"}"),
        ("a", "A"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    for i in 1..=9 {
        lib.insert(format!("d{i}"), format!("{i}{{snippet abbr=\"d{}\"}}", i + 1));
    }
    lib.insert("d10".to_owned(), "end".to_owned());
    lib
}

fuzz_target!(|data: &[u8]| {
    let source = String::from_utf8_lossy(data);
    let template = match Template::parse(&source) {
        Ok(t) => t,
        Err(e) => {
            assert!(!e.kind().is_render_time(), "{} at parse", e.code());
            assert!(e.position().is_some());
            return;
        }
    };
    let tokens = template.tokens();
    assert!(!tokens.is_empty() || source.is_empty());

    let lib = library();
    let resolved = match template.resolve(&lib) {
        Ok(r) => r,
        Err(e) => {
            assert!(!e.kind().is_render_time(), "{} at resolve", e.code());
            return;
        }
    };
    let date_format = Format::parse("%Y-%m-%d").expect("date format");
    let time_format = Format::parse("%H:%M").expect("time format");
    let ctx = RenderContext {
        now: LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("now"),
        locale: Locale::from_tag("fr-CA"),
        date_format: &date_format,
        time_format: &time_format,
        clipboard: Some(" clip\r\nboard "),
    };
    let values: FormValues = [("Name", "Kim"), ("t", "yes"), ("a", "x"), ("c", "y")]
        .into_iter()
        .collect();
    match resolved.render(&ctx, &values) {
        Err(e) => assert!(e.kind().is_render_time(), "{} at render", e.code()),
        Ok(r) => {
            let mut previous_text = false;
            for a in &r.actions {
                if let RenderedAction::Text { text, verbatim } = a {
                    assert!(!text.is_empty() && !previous_text && !text.contains('\r'));
                    for range in verbatim {
                        assert!(range.start < range.end && range.end <= text.len());
                        assert!(text.is_char_boundary(range.start));
                        assert!(text.is_char_boundary(range.end));
                    }
                    previous_text = true;
                } else {
                    previous_text = false;
                }
            }
            assert!(r.cursor_back.is_none() || resolved.uses_cursor());
        }
    }
});
