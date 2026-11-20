//! The relevance fixtures of the palette's search (`spec/search/relevance.toml`, task 3.6):
//! a small library and the queries whose first results are fixed, which together say what
//! "fuzzy search by abbreviation, label, content and tag" means.

use std::collections::HashMap;
use std::path::PathBuf;

use iamb_core::model::Id;
use iamb_core::search::{Doc, Field, Hit, search};
use toml::Value;

struct Snippet {
    name: String,
    id: Id,
    abbreviation: String,
    label: String,
    body: String,
    tags: Vec<Id>,
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("`{key}` missing in {v}"))
        .to_owned()
}

fn id_of(n: usize) -> Id {
    let mut bytes = [0_u8; 16];
    bytes[8..].copy_from_slice(&(n as u64 + 1).to_be_bytes());
    Id::from_bytes(bytes)
}

fn load() -> (Vec<Snippet>, HashMap<Id, String>, Vec<Value>) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/search/relevance.toml");
    let doc = Value::Table(
        std::fs::read_to_string(&path)
            .expect("fixture file")
            .parse::<toml::Table>()
            .expect("valid toml"),
    );
    let mut tag_ids: HashMap<String, Id> = HashMap::new();
    let mut tag_names: HashMap<Id, String> = HashMap::new();
    let mut snippets = Vec::new();
    for (n, s) in doc["snippet"]
        .as_array()
        .expect("snippets")
        .iter()
        .enumerate()
    {
        let tags = s["tags"]
            .as_array()
            .expect("tags")
            .iter()
            .map(|t| {
                let name = t.as_str().expect("tag").to_owned();
                let next = id_of(1000 + tag_ids.len());
                let id = *tag_ids.entry(name.clone()).or_insert(next);
                tag_names.insert(id, name);
                id
            })
            .collect();
        snippets.push(Snippet {
            name: text(s, "name"),
            id: id_of(n),
            abbreviation: text(s, "abbreviation"),
            label: text(s, "label"),
            body: text(s, "body"),
            tags,
        });
    }
    let queries = doc["query"].as_array().expect("queries").clone();
    (snippets, tag_names, queries)
}

fn run(snippets: &[Snippet], tag_names: &HashMap<Id, String>, query: &str) -> Vec<Hit> {
    search(
        query,
        50,
        snippets.iter().map(|s| Doc {
            id: s.id,
            abbreviation: &s.abbreviation,
            label: &s.label,
            body: &s.body,
            tags: &s.tags,
        }),
        &|id| tag_names.get(&id).map(String::as_str),
    )
}

fn field_name(f: Field) -> &'static str {
    match f {
        Field::Abbreviation => "abbreviation",
        Field::Label => "label",
        Field::Body => "body",
        Field::Tag => "tag",
    }
}

#[test]
fn every_query_gives_its_expected_first_results() {
    let (snippets, tag_names, queries) = load();
    assert!(queries.len() >= 25, "the fixtures are not thinned out");
    let name_of = |id: Id| {
        snippets
            .iter()
            .find(|s| s.id == id)
            .map_or_else(|| "?".to_owned(), |s| s.name.clone())
    };
    let mut failures = Vec::new();
    for q in &queries {
        let query = text(q, "text");
        let why = text(q, "why");
        let hits = run(&snippets, &tag_names, &query);
        let got: Vec<String> = hits.iter().map(|h| name_of(h.id)).collect();
        let want: Vec<String> = q["top"]
            .as_array()
            .expect("top")
            .iter()
            .map(|v| v.as_str().expect("name").to_owned())
            .collect();
        let only = q.get("only").and_then(Value::as_bool).unwrap_or(false);
        let ok = if only {
            got == want
        } else {
            got.len() >= want.len() && got[..want.len()] == want[..]
        };
        if !ok {
            failures.push(format!("{query:?} ({why}): wanted {want:?}, got {got:?}"));
            continue;
        }
        let (Some(first), Some(_)) = (hits.first(), q.get("field")) else {
            continue;
        };
        let field = text(q, "field");
        if field_name(first.field) != field {
            failures.push(format!(
                "{query:?}: the first hit matched the {}, not the {field}",
                field_name(first.field)
            ));
        }
        if q.get("marked").is_some() {
            let s = snippets.iter().find(|s| s.id == first.id).expect("snippet");
            // A tag hit's ranges are in the name of one of the snippet's tags.
            let sources: Vec<String> = match first.field {
                Field::Abbreviation => vec![s.abbreviation.clone()],
                Field::Label => vec![s.label.clone()],
                Field::Body => vec![s.body.clone()],
                Field::Tag => s.tags.iter().map(|t| tag_names[t].clone()).collect(),
            };
            let marked_in = |source: &String| {
                let units: Vec<u16> = source.encode_utf16().collect();
                first
                    .ranges
                    .iter()
                    .filter(|r| r[1] <= units.len())
                    .map(|r| String::from_utf16_lossy(&units[r[0]..r[1]]))
                    .collect::<String>()
            };
            let want = text(q, "marked");
            let marked = sources
                .iter()
                .map(marked_in)
                .find(|m| *m == want)
                .unwrap_or_else(|| marked_in(&sources[0]));
            if marked != want {
                failures.push(format!("{query:?}: marked {marked:?}, wanted {want:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn the_same_query_always_gives_the_same_list() {
    let (snippets, tag_names, queries) = load();
    for q in &queries {
        let query = text(q, "text");
        assert_eq!(
            run(&snippets, &tag_names, &query),
            run(&snippets, &tag_names, &query),
            "{query:?}"
        );
    }
}

#[test]
fn scores_never_rise_down_the_list() {
    let (snippets, tag_names, queries) = load();
    for q in &queries {
        let hits = run(&snippets, &tag_names, &text(q, "text"));
        assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    }
}
