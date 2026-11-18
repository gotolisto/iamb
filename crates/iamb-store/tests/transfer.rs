//! Tests for `iamb_store::transfer` (task 1.7): round trips, conflicts, malformed input.

mod common;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use common::{T0, TestResult, profile};
use iamb_core::model::{
    Abbreviation, CaseMode, Folder, FolderDefaults, Id, Snippet, SnippetKind, SnippetTag, SyncMeta,
    Tag, Trigger,
};
use iamb_store::transfer::{self, Format, ImportSummary, OnConflict, TransferError};
use iamb_store::{Library, Store};
use serde_json::Value;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/transfer");

const CSV_HEADER: &str =
    "abbreviation,label,body,folder,tags,trigger,case_mode,word_start,enabled,kind";

/// Deterministic UUID version 7 ids: `n` gives a distinct id for each `n`.
fn id(n: u64) -> Id {
    Id::v7(n, [0; 10])
}

/// A generator for import ids, starting after `start`.
fn ids_from(start: u64) -> impl FnMut() -> Id {
    let mut n = start;
    move || {
        n += 1;
        id(n)
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(FIXTURES).join(name)
}

fn put_folder(
    store: &mut Store,
    next: &mut impl FnMut() -> Id,
    profile_id: Id,
    parent: Option<Id>,
    name: &str,
    case_mode: Option<CaseMode>,
) -> Res<Id> {
    let folder_id = next();
    let folder = Folder {
        meta: SyncMeta::new(folder_id, T0),
        profile_id,
        parent_id: parent,
        name: name.to_owned(),
        position: 0,
        member_permission: None,
        defaults: FolderDefaults {
            case_mode,
            ..FolderDefaults::default()
        },
    };
    store.put_folder(&folder, T0)?;
    Ok(folder_id)
}

/// A store with one profile (`profile_id`) and a folder "Keep" holding snippet "keep".
fn store_with_keep(profile_id: Id) -> Res<Store> {
    let mut store = Store::open_in_memory()?;
    store.put_profile(&profile(profile_id, profile_id), T0)?;
    let mut next = ids_from(500);
    let folder = put_folder(&mut store, &mut next, profile_id, None, "Keep", None)?;
    let snippet = Snippet::new_defaults(
        SyncMeta::new(next(), T0),
        folder,
        Abbreviation::new("keep")?,
        "Keep",
        "kept",
        None,
    );
    store.put_snippet(&snippet, T0)?;
    Ok(store)
}

fn abbreviation(i: usize) -> String {
    match i % 5 {
        0 => format!("sig{i}"),
        1 => format!("/x{i}"),
        2 => format!("Ü{i}"),
        3 => format!("Mixed{i}"),
        _ => format!("🙂{i}"),
    }
}

fn body(i: usize) -> String {
    match i % 3 {
        0 => format!("Dear {{name}},\n\nThanks, \"friend\"\n{i}"),
        1 => format!("Line one\r\nLine two 🙂 {i}"),
        _ => format!("single line, with comma {i}"),
    }
}

/// Folders three levels deep and more, 50 snippets with every trigger, case mode, kind and
/// enabled state, and three tags. Every folder holds snippets.
fn build_library(store: &mut Store, profile_id: Id) -> TestResult {
    let mut next = ids_from(0);
    let work = put_folder(
        store,
        &mut next,
        profile_id,
        None,
        "Work",
        Some(CaseMode::Adaptive),
    )?;
    let clients = put_folder(store, &mut next, profile_id, Some(work), "Clients", None)?;
    let acme = put_folder(store, &mut next, profile_id, Some(clients), "Acme Co", None)?;
    let personal = put_folder(store, &mut next, profile_id, None, "Personal", None)?;
    let home = put_folder(
        store,
        &mut next,
        profile_id,
        Some(personal),
        "Home ☕",
        None,
    )?;
    let misc = put_folder(store, &mut next, profile_id, None, "A/B notes", None)?;
    let folders = [work, clients, acme, personal, home, misc];

    let mut tags = Vec::new();
    for (name, color) in [
        ("urgent", Some("#aabbcc")),
        ("email, work", None),
        ("home", None),
    ] {
        let tag = Tag {
            meta: SyncMeta::new(next(), T0),
            profile_id,
            name: name.to_owned(),
            color: color.map(str::to_owned),
        };
        store.put_tag(&tag, T0)?;
        tags.push(tag.meta.id);
    }

    for i in 0..50 {
        let trigger = if i % 2 == 0 {
            Trigger::Immediate
        } else {
            Trigger::Delimiter
        };
        let mut snippet = Snippet::new_defaults(
            SyncMeta::new(next(), T0),
            folders[i % folders.len()],
            Abbreviation::new(&abbreviation(i))?,
            format!("Label \"{i}\", with comma"),
            body(i),
            None,
        );
        snippet.trigger = trigger;
        snippet.word_start = i % 3 != 0;
        snippet.case_mode = [CaseMode::Exact, CaseMode::Insensitive, CaseMode::Adaptive][i % 3];
        snippet.kind = if i % 4 == 0 {
            SnippetKind::Rich
        } else {
            SnippetKind::Plain
        };
        snippet.enabled = i % 7 != 0;
        store.put_snippet(&snippet, T0)?;
        for (tag, wanted) in [(0, i % 3 == 0), (1, i % 4 == 1), (2, i % 5 == 2)] {
            if wanted {
                let link = SnippetTag {
                    meta: SyncMeta::new(next(), T0),
                    snippet_id: snippet.meta.id,
                    tag_id: tags[tag],
                };
                store.put_snippet_tag(&link, T0)?;
            }
        }
    }
    Ok(())
}

/// A canonical description of a library that ignores ids and timestamps.
fn fingerprint(lib: &Library) -> Vec<String> {
    let by_id: HashMap<Id, &Folder> = lib.folders.iter().map(|f| (f.meta.id, f)).collect();
    let path = |folder: Id| -> String {
        let mut names = Vec::new();
        let mut cur = by_id.get(&folder).copied();
        while let Some(f) = cur {
            names.push(f.name.clone());
            cur = f.parent_id.and_then(|p| by_id.get(&p).copied());
        }
        names.reverse();
        names.join(" / ")
    };
    let abbr: HashMap<Id, String> = lib
        .snippets
        .iter()
        .map(|s| (s.meta.id, s.abbreviation.to_string()))
        .collect();
    let tag_name: HashMap<Id, &str> = lib
        .tags
        .iter()
        .map(|t| (t.meta.id, t.name.as_str()))
        .collect();
    let mut out = Vec::new();
    for f in &lib.folders {
        out.push(format!(
            "folder {} pos={} member={:?} defaults={:?}",
            path(f.meta.id),
            f.position,
            f.member_permission,
            f.defaults
        ));
    }
    for s in &lib.snippets {
        out.push(format!(
            "snippet {:?} in {}: label={:?} kind={:?} body={:?} trigger={:?} case={:?} word_start={} enabled={}",
            s.abbreviation.as_str(),
            path(s.folder_id),
            s.label,
            s.kind,
            s.body,
            s.trigger,
            s.case_mode,
            s.word_start,
            s.enabled
        ));
    }
    for t in &lib.tags {
        out.push(format!("tag {:?} color={:?}", t.name, t.color));
    }
    for l in &lib.snippet_tags {
        out.push(format!(
            "link {:?} -> {:?}",
            abbr.get(&l.snippet_id).cloned().unwrap_or_default(),
            tag_name.get(&l.tag_id).copied().unwrap_or_default()
        ));
    }
    out.sort();
    out
}

fn rows<'a>(doc: &'a Value, section: &str) -> &'a [Value] {
    doc.get(section)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

/// Maps each old id in `a` to the id at the same position in `b` (both documents list their rows
/// in the same content order), and the old profile to the new one.
fn id_map(a: &Value, b: &Value, old_profile: Id, new_profile: Id) -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert(old_profile.to_string(), new_profile.to_string());
    for section in ["folders", "tags", "snippets", "snippet_tags"] {
        for (x, y) in rows(a, section).iter().zip(rows(b, section)) {
            if let (Some(old), Some(new)) = (x["id"].as_str(), y["id"].as_str()) {
                map.insert(old.to_owned(), new.to_owned());
            }
        }
    }
    map
}

fn remap(value: &Value, map: &HashMap<String, String>) -> Value {
    match value {
        Value::String(s) => Value::String(map.get(s).cloned().unwrap_or_else(|| s.clone())),
        Value::Array(items) => Value::Array(items.iter().map(|v| remap(v, map)).collect()),
        Value::Object(obj) => Value::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), remap(v, map)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn read_json(path: &Path) -> Res<Value> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

#[test]
fn json_round_trip_gives_an_identical_library() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (p1, p2) = (id(900), id(901));
    let mut src = Store::open_in_memory()?;
    src.put_profile(&profile(p1, p1), T0)?;
    build_library(&mut src, p1)?;

    let first = dir.path().join("first.json");
    let summary = transfer::export(&src, p1, &first, Format::Json)?;
    assert_eq!(
        (summary.folders, summary.snippets, summary.tags),
        (6, 50, 3)
    );
    let bytes = fs::read(&first)?;
    assert!(
        !bytes.contains(&b'\r'),
        "JSON output must use LF line endings"
    );

    let mut dst = Store::open_in_memory()?;
    dst.put_profile(&profile(p2, p2), T0)?;
    let imported = transfer::import(
        &mut dst,
        p2,
        &first,
        Format::Json,
        T0,
        &mut ids_from(10_000),
        OnConflict::Fail,
    )?;
    assert_eq!(
        imported,
        ImportSummary {
            folders_created: 6,
            snippets_imported: 50,
            snippets_skipped: 0,
            tags_created: 3,
        }
    );

    let second = dir.path().join("second.json");
    transfer::export(&dst, p2, &second, Format::Json)?;
    let a = read_json(&first)?;
    let b = read_json(&second)?;
    assert_eq!(
        remap(&a, &id_map(&a, &b, p1, p2)),
        b,
        "exports differ beyond ids"
    );
    assert_eq!(
        fingerprint(&src.load_library()?),
        fingerprint(&dst.load_library()?)
    );
    Ok(())
}

#[test]
fn csv_round_trip_gives_identical_csv() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (p1, p2) = (id(900), id(901));
    let mut src = Store::open_in_memory()?;
    src.put_profile(&profile(p1, p1), T0)?;
    build_library(&mut src, p1)?;

    let first = dir.path().join("first.csv");
    let summary = transfer::export(&src, p1, &first, Format::Csv)?;
    assert_eq!(summary.snippets, 50);
    let text = fs::read_to_string(&first)?;
    assert!(text.starts_with(&format!("{CSV_HEADER}\n")));

    let mut dst = Store::open_in_memory()?;
    dst.put_profile(&profile(p2, p2), T0)?;
    let imported = transfer::import(
        &mut dst,
        p2,
        &first,
        Format::Csv,
        T0,
        &mut ids_from(10_000),
        OnConflict::Fail,
    )?;
    assert_eq!(imported.snippets_imported, 50);
    assert_eq!(imported.snippets_skipped, 0);
    assert_eq!(imported.tags_created, 3);

    let second = dir.path().join("second.csv");
    transfer::export(&dst, p2, &second, Format::Csv)?;
    assert_eq!(fs::read(&first)?, fs::read(&second)?);
    Ok(())
}

#[test]
fn skip_counts_existing_abbreviations_and_fail_rolls_back() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (p1, p2) = (id(900), id(901));
    let mut src = Store::open_in_memory()?;
    src.put_profile(&profile(p1, p1), T0)?;
    build_library(&mut src, p1)?;
    let json = dir.path().join("lib.json");
    let csv = dir.path().join("lib.csv");
    transfer::export(&src, p1, &json, Format::Json)?;
    transfer::export(&src, p1, &csv, Format::Csv)?;
    // The target already uses ten of the file's abbreviations, in upper case: matching ignores case.
    let existing: Vec<String> = src
        .load_library()?
        .snippets
        .iter()
        .take(10)
        .map(|s| s.abbreviation.as_str().to_uppercase())
        .collect();

    for (path, format) in [(&json, Format::Json), (&csv, Format::Csv)] {
        let mut dst = Store::open_in_memory()?;
        dst.put_profile(&profile(p2, p2), T0)?;
        let mut next = ids_from(20_000);
        let pre = put_folder(&mut dst, &mut next, p2, None, "Pre", None)?;
        for abbr in &existing {
            let snippet = Snippet::new_defaults(
                SyncMeta::new(next(), T0),
                pre,
                Abbreviation::new(abbr)?,
                "Existing",
                "existing",
                None,
            );
            dst.put_snippet(&snippet, T0)?;
        }
        let before = dst.load_library()?;

        let err = transfer::import(&mut dst, p2, path, format, T0, &mut next, OnConflict::Fail)
            .err()
            .ok_or("Fail import unexpectedly succeeded")?;
        assert!(
            matches!(err, TransferError::Conflict { .. }),
            "{format:?}: {err}"
        );
        assert_eq!(
            dst.load_library()?,
            before,
            "{format:?}: Fail must roll back"
        );

        let summary =
            transfer::import(&mut dst, p2, path, format, T0, &mut next, OnConflict::Skip)?;
        assert_eq!(summary.snippets_skipped, 10, "{format:?}");
        assert_eq!(summary.snippets_imported, 40, "{format:?}");
    }
    Ok(())
}

#[test]
fn json_import_rejects_bad_documents_without_writing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let p = id(900);
    let mut lib = Store::open_in_memory()?;
    lib.put_profile(&profile(p, p), T0)?;
    build_library(&mut lib, p)?;
    let good = dir.path().join("good.json");
    transfer::export(&lib, p, &good, Format::Json)?;
    let doc = read_json(&good)?;

    let write = |name: &str, value: &Value| -> Res<PathBuf> {
        let path = dir.path().join(name);
        fs::write(&path, serde_json::to_string_pretty(value)?)?;
        Ok(path)
    };
    let bad_syntax = dir.path().join("syntax.json");
    fs::write(&bad_syntax, "{ \"format\": ")?;
    let mut wrong_format = doc.clone();
    wrong_format["format"] = Value::from("other-app");
    let mut wrong_version = doc.clone();
    wrong_version["version"] = Value::from(2);
    let mut bad_abbreviation = doc.clone();
    bad_abbreviation["snippets"][3]["abbreviation"] = Value::from("has space");

    let cases = [
        (bad_syntax, "Json"),
        (write("format.json", &wrong_format)?, "WrongFormat"),
        (write("version.json", &wrong_version)?, "UnsupportedVersion"),
        (write("abbr.json", &bad_abbreviation)?, "InvalidValue"),
    ];
    for (path, expected) in cases {
        let mut store = store_with_keep(p)?;
        let before = store.load_library()?;
        let err = transfer::import(
            &mut store,
            p,
            &path,
            Format::Json,
            T0,
            &mut ids_from(30_000),
            OnConflict::Skip,
        )
        .err()
        .ok_or("import unexpectedly succeeded")?;
        let kind = format!("{err:?}");
        assert!(kind.starts_with(expected), "{}: got {kind}", path.display());
        if expected == "InvalidValue" {
            assert!(
                err.to_string().starts_with("snippets[3]:"),
                "wrong location: {err}"
            );
        }
        assert_eq!(
            store.load_library()?,
            before,
            "{} changed the store",
            path.display()
        );
    }
    Ok(())
}

/// Imports `path` into a fresh store that holds some data, and returns the error. Fails the test
/// if the import succeeds or changes the store.
fn csv_error(path: &Path) -> Res<TransferError> {
    let p = id(900);
    let mut store = store_with_keep(p)?;
    let before = store.load_library()?;
    let err = transfer::import(
        &mut store,
        p,
        path,
        Format::Csv,
        T0,
        &mut ids_from(30_000),
        OnConflict::Skip,
    )
    .err()
    .ok_or("import unexpectedly succeeded")?;
    assert_eq!(
        store.load_library()?,
        before,
        "{} changed the store",
        path.display()
    );
    Ok(err)
}

#[test]
fn malformed_csv_gives_the_right_error_and_writes_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    assert!(matches!(
        csv_error(&fixture("no_abbreviation.csv"))?,
        TransferError::MissingColumn {
            column: "abbreviation"
        }
    ));
    let unterminated = csv_error(&fixture("unterminated_quote.csv"))?;
    assert!(
        matches!(unterminated, TransferError::Csv { line: Some(2), .. }),
        "{unterminated}"
    );
    let unknown = csv_error(&fixture("unknown_trigger.csv"))?.to_string();
    assert!(
        unknown.starts_with("row 1 (line 2): trigger must be one of immediate, delimiter"),
        "{unknown}"
    );
    let long = csv_error(&fixture("long_abbreviation.csv"))?.to_string();
    assert!(long.starts_with("row 1 (line 2): abbreviation:"), "{long}");
    assert!(long.contains("at most 64"), "{long}");
    assert!(matches!(
        csv_error(&fixture("empty.csv"))?,
        TransferError::MissingColumn {
            column: "abbreviation"
        }
    ));
    let deep = dir.path().join("deep.csv");
    fs::write(&deep, "abbreviation,body,folder\nx,y,A / B / C / D / E\n")?;
    assert!(matches!(
        csv_error(&deep)?,
        TransferError::FolderTooDeep { depth: 5, .. }
    ));
    Ok(())
}

#[test]
fn header_only_file_imports_nothing() -> TestResult {
    let p = id(900);
    let mut store = store_with_keep(p)?;
    let before = store.load_library()?;
    let summary = transfer::import(
        &mut store,
        p,
        &fixture("header_only.csv"),
        Format::Csv,
        T0,
        &mut ids_from(30_000),
        OnConflict::Fail,
    )?;
    assert_eq!(summary, ImportSummary::default());
    assert_eq!(store.load_library()?, before);
    Ok(())
}

#[test]
fn missing_file_is_an_io_error() -> TestResult {
    let p = id(900);
    let mut store = store_with_keep(p)?;
    let err = transfer::import(
        &mut store,
        p,
        &fixture("does_not_exist.csv"),
        Format::Csv,
        T0,
        &mut ids_from(30_000),
        OnConflict::Skip,
    )
    .err()
    .ok_or("import unexpectedly succeeded")?;
    assert!(matches!(err, TransferError::Io { .. }), "{err}");
    Ok(())
}

#[test]
fn csv_with_bom_and_crlf_is_accepted() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("bom.csv");
    let text = "\u{feff}abbreviation,label,body,folder,tags,trigger,enabled\r\n\
                sig,Sig,\"line1\r\nline2\",Work / Clients,\"urgent; home\",immediate,false\r\n\
                sec,Second,plain body,,,,\r\n";
    fs::write(&path, text)?;

    let p = id(900);
    let mut store = Store::open_in_memory()?;
    store.put_profile(&profile(p, p), T0)?;
    let summary = transfer::import(
        &mut store,
        p,
        &path,
        Format::Csv,
        T0,
        &mut ids_from(40_000),
        OnConflict::Fail,
    )?;
    assert_eq!(
        summary,
        ImportSummary {
            folders_created: 3,
            snippets_imported: 2,
            snippets_skipped: 0,
            tags_created: 2,
        }
    );
    let lib = store.load_library()?;
    let sig = lib
        .snippets
        .iter()
        .find(|s| s.abbreviation.as_str() == "sig")
        .ok_or("sig not imported")?;
    assert_eq!(sig.body, "line1\r\nline2");
    assert_eq!(sig.trigger, Trigger::Immediate);
    assert!(!sig.word_start);
    assert!(!sig.enabled);
    let sec = lib
        .snippets
        .iter()
        .find(|s| s.abbreviation.as_str() == "sec")
        .ok_or("sec not imported")?;
    assert_eq!(sec.trigger, Trigger::Delimiter);
    assert!(sec.word_start);
    assert!(sec.enabled);
    assert_eq!(sec.kind, SnippetKind::Plain);
    assert_eq!(sec.case_mode, CaseMode::Insensitive);
    Ok(())
}

#[test]
fn csv_export_has_header_and_quotes_multiline_bodies() -> TestResult {
    let dir = tempfile::tempdir()?;
    let p = id(900);
    let mut store = Store::open_in_memory()?;
    store.put_profile(&profile(p, p), T0)?;
    let mut next = ids_from(0);
    let folder = put_folder(&mut store, &mut next, p, None, "Work", None)?;
    let snippet = Snippet::new_defaults(
        SyncMeta::new(next(), T0),
        folder,
        Abbreviation::new("sig")?,
        "Sig",
        "one, \"two\"\nthree",
        None,
    );
    store.put_snippet(&snippet, T0)?;
    let path = dir.path().join("one.csv");
    transfer::export(&store, p, &path, Format::Csv)?;
    assert_eq!(
        fs::read_to_string(&path)?,
        format!(
            "{CSV_HEADER}\nsig,Sig,\"one, \"\"two\"\"\nthree\",Work,,delimiter,insensitive,true,true,plain\n"
        )
    );
    Ok(())
}

/// Number of live folders and tags and the names of the tags, for the re-import tests.
fn counts(store: &Store) -> Res<(usize, usize, usize)> {
    let lib = store.load_library()?;
    Ok((lib.folders.len(), lib.tags.len(), lib.snippets.len()))
}

/// Finding 7: importing the same JSON file again and again creates one copy of each folder and
/// tag, and the summaries say so.
#[test]
fn json_reimport_reuses_folders_and_tags() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (p1, p2) = (id(900), id(901));
    let mut src = Store::open_in_memory()?;
    src.put_profile(&profile(p1, p1), T0)?;
    build_library(&mut src, p1)?;
    let json = dir.path().join("lib.json");
    transfer::export(&src, p1, &json, Format::Json)?;

    let mut dst = Store::open_in_memory()?;
    dst.put_profile(&profile(p2, p2), T0)?;
    let mut next = ids_from(10_000);
    let first = transfer::import(
        &mut dst,
        p2,
        &json,
        Format::Json,
        T0,
        &mut next,
        OnConflict::Skip,
    )?;
    assert_eq!(
        first,
        ImportSummary {
            folders_created: 6,
            snippets_imported: 50,
            snippets_skipped: 0,
            tags_created: 3,
        }
    );
    let after_first = dst.load_library()?;
    for _ in 0..2 {
        let again = transfer::import(
            &mut dst,
            p2,
            &json,
            Format::Json,
            T0,
            &mut next,
            OnConflict::Skip,
        )?;
        assert_eq!(
            again,
            ImportSummary {
                folders_created: 0,
                snippets_imported: 0,
                snippets_skipped: 50,
                tags_created: 0,
            }
        );
        assert_eq!(counts(&dst)?, (6, 3, 50));
    }
    assert_eq!(
        dst.load_library()?,
        after_first,
        "a re-import changes nothing"
    );
    Ok(())
}

/// Finding 7: folders and tags of a file are reused when they exist, created only for the
/// snippets that are written, and nothing at all is created when every snippet is skipped.
#[test]
fn json_import_creates_folders_and_tags_only_for_written_snippets() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (p1, p2) = (id(900), id(901));
    let mut src = Store::open_in_memory()?;
    src.put_profile(&profile(p1, p1), T0)?;
    build_library(&mut src, p1)?;
    let json = dir.path().join("lib.json");
    transfer::export(&src, p1, &json, Format::Json)?;
    let abbreviations: Vec<String> = src
        .load_library()?
        .snippets
        .iter()
        .map(|s| s.abbreviation.as_str().to_owned())
        .collect();

    // The target already uses every abbreviation of the file, in another folder.
    let mut dst = Store::open_in_memory()?;
    dst.put_profile(&profile(p2, p2), T0)?;
    let mut next = ids_from(20_000);
    let pre = put_folder(&mut dst, &mut next, p2, None, "Pre", None)?;
    for abbr in &abbreviations {
        let snippet = Snippet::new_defaults(
            SyncMeta::new(next(), T0),
            pre,
            Abbreviation::new(abbr)?,
            "Existing",
            "existing",
            None,
        );
        dst.put_snippet(&snippet, T0)?;
    }
    let before = dst.load_library()?;
    let summary = transfer::import(
        &mut dst,
        p2,
        &json,
        Format::Json,
        T0,
        &mut next,
        OnConflict::Skip,
    )?;
    assert_eq!(
        summary,
        ImportSummary {
            folders_created: 0,
            snippets_imported: 0,
            snippets_skipped: 50,
            tags_created: 0,
        }
    );
    assert_eq!(
        dst.load_library()?,
        before,
        "a fully skipped import writes nothing"
    );
    Ok(())
}

/// Finding 11: names with separators, backslashes and surrounding spaces survive a CSV round
/// trip, and export no longer fails on them.
#[test]
fn csv_round_trip_with_separators_in_names() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (p1, p2) = (id(900), id(901));
    let mut src = Store::open_in_memory()?;
    src.put_profile(&profile(p1, p1), T0)?;
    let mut next = ids_from(0);
    let names = [
        "R / D",
        "ends with /",
        " padded ",
        "C:\\dir\\",
        "TCP/IP",
        "/ / /",
        "back \\/ slash",
    ];
    let top = put_folder(&mut src, &mut next, p1, None, "Top / level", None)?;
    let mut folders = vec![top];
    for name in names {
        folders.push(put_folder(&mut src, &mut next, p1, Some(top), name, None)?);
    }
    let tag_names = [
        "a;b",
        ";",
        " spaced ",
        "back\\slash",
        "ends\\",
        "x\\;y",
        "plain",
    ];
    let mut tags = Vec::new();
    for name in tag_names {
        let tag_id = next();
        let tag = Tag {
            meta: SyncMeta::new(tag_id, T0),
            profile_id: p1,
            name: name.to_owned(),
            color: None,
        };
        src.put_tag(&tag, T0)?;
        tags.push(tag_id);
    }
    for (i, folder) in folders.iter().enumerate() {
        let snippet_id = next();
        let snippet = Snippet::new_defaults(
            SyncMeta::new(snippet_id, T0),
            *folder,
            Abbreviation::new(&format!("s{i}"))?,
            "",
            "body",
            None,
        );
        src.put_snippet(&snippet, T0)?;
        // Every snippet carries all the awkward tags.
        for tag in &tags {
            let link = SnippetTag {
                meta: SyncMeta::new(next(), T0),
                snippet_id,
                tag_id: *tag,
            };
            src.put_snippet_tag(&link, T0)?;
        }
    }

    let first = dir.path().join("first.csv");
    transfer::export(&src, p1, &first, Format::Csv)?;
    let text = fs::read_to_string(&first)?;
    assert!(text.contains("Top \\/ level / R \\/ D"), "{text}");

    let mut dst = Store::open_in_memory()?;
    dst.put_profile(&profile(p2, p2), T0)?;
    let summary = transfer::import(
        &mut dst,
        p2,
        &first,
        Format::Csv,
        T0,
        &mut ids_from(10_000),
        OnConflict::Fail,
    )?;
    assert_eq!(summary.snippets_imported, folders.len());
    assert_eq!(summary.tags_created, tag_names.len());
    assert_eq!(summary.folders_created, folders.len());

    let lib = dst.load_library()?;
    let mut got: Vec<&str> = lib.folders.iter().map(|f| f.name.as_str()).collect();
    got.sort_unstable();
    let mut want: Vec<&str> = names.to_vec();
    want.push("Top / level");
    want.sort_unstable();
    assert_eq!(got, want);
    let mut got: Vec<&str> = lib.tags.iter().map(|t| t.name.as_str()).collect();
    got.sort_unstable();
    let mut want: Vec<&str> = tag_names.to_vec();
    want.sort_unstable();
    assert_eq!(got, want);

    let second = dir.path().join("second.csv");
    transfer::export(&dst, p2, &second, Format::Csv)?;
    assert_eq!(fs::read(&first)?, fs::read(&second)?);
    Ok(())
}

/// Hand-made CSV files with a backslash that is not an escape keep it.
#[test]
fn csv_import_keeps_backslashes_that_are_not_escapes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let p = id(900);
    let mut store = Store::open_in_memory()?;
    store.put_profile(&profile(p, p), T0)?;
    let path = dir.path().join("hand.csv");
    fs::write(
        &path,
        format!("{CSV_HEADER}\nhp,,body,C:\\temp / Sub,a\\b;c,,,,,\n"),
    )?;
    transfer::import(
        &mut store,
        p,
        &path,
        Format::Csv,
        T0,
        &mut ids_from(10),
        OnConflict::Fail,
    )?;
    let lib = store.load_library()?;
    let mut folders: Vec<&str> = lib.folders.iter().map(|f| f.name.as_str()).collect();
    folders.sort_unstable();
    assert_eq!(folders, ["C:\\temp", "Sub"]);
    let mut tags: Vec<&str> = lib.tags.iter().map(|t| t.name.as_str()).collect();
    tags.sort_unstable();
    assert_eq!(tags, ["a\\b", "c"]);
    Ok(())
}
