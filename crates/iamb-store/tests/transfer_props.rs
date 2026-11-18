//! Property test for import and export (review 1.9 row 9e): random small libraries, exported and
//! imported again as JSON and as CSV, give the same library.
//!
//! The libraries have folders up to depth 4 and tags with names drawn from an alphabet of
//! separators, quotes, backslashes, spaces and line breaks (never empty: empty names are
//! unsupported in CSV), and snippets with random abbreviations, labels, bodies, triggers, case
//! modes and tags. "The same" ignores ids and timestamps. An import writes only the folders on
//! the way to a snippet and the tags a snippet carries, so folders without snippets and unused
//! tags are not expected back. CSV carries snippets with their folder path and tag names; JSON
//! also carries folder defaults and tag colors.

mod common;

use std::collections::{HashMap, HashSet};

use common::{T0, profile};
use iamb_core::model::{
    Abbreviation, CaseMode, Folder, FolderDefaults, Id, Snippet, SnippetKind, SnippetTag, SyncMeta,
    Tag, Trigger,
};
use iamb_store::transfer::{self, Format, OnConflict};
use iamb_store::{Library, Store};
use proptest::prelude::*;

const NAME_CHARS: &[char] = &[
    'a', 'B', '1', ' ', ' ', '/', '\\', ';', ';', '"', '\'', ',', '\n', '\r', '\t', 'é', 'Σ', 'ς',
    '☕',
];
const ABBREVIATION_CHARS: &[char] = &[
    'a', 'B', '1', '/', '\\', ';', '"', '\'', ',', '.', 'é', 'Σ', 'σ', 'ς', '☕', '{', '}',
];
const TEXT_CHARS: &[char] = &[
    'a', ' ', '\n', '\r', '\t', '"', '\'', ',', ';', '/', '\\', '{', '}', 'é', 'ς', '☕', '#',
];

fn text(
    chars: &'static [char],
    len: std::ops::RangeInclusive<usize>,
) -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(chars), len).prop_map(|v| v.into_iter().collect())
}

#[derive(Debug, Clone)]
#[expect(clippy::struct_excessive_bools, reason = "one flag per snippet column")]
struct SnippetSpec {
    abbreviation: String,
    label: String,
    body: String,
    folder: usize,
    tags: Vec<usize>,
    trigger: bool,
    case_mode: usize,
    word_start: bool,
    enabled: bool,
    rich: bool,
}

#[derive(Debug, Clone)]
struct Spec {
    folders: Vec<(usize, String, usize)>,
    tags: Vec<(String, bool)>,
    snippets: Vec<SnippetSpec>,
}

fn spec() -> impl Strategy<Value = Spec> {
    let snippet = (
        text(ABBREVIATION_CHARS, 1..=8),
        text(TEXT_CHARS, 0..=12),
        text(TEXT_CHARS, 0..=24),
        any::<usize>(),
        prop::collection::vec(any::<usize>(), 0..=3),
        any::<bool>(),
        0..3_usize,
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(
                abbreviation,
                label,
                body,
                folder,
                tags,
                trigger,
                case_mode,
                word_start,
                enabled,
                rich,
            )| {
                SnippetSpec {
                    abbreviation,
                    label,
                    body,
                    folder,
                    tags,
                    trigger,
                    case_mode,
                    word_start,
                    enabled,
                    rich,
                }
            },
        );
    (
        prop::collection::vec((any::<usize>(), text(NAME_CHARS, 1..=6), 0..4_usize), 1..=7),
        prop::collection::vec((text(NAME_CHARS, 1..=6), any::<bool>()), 0..=4),
        prop::collection::vec(snippet, 0..=8),
    )
        .prop_map(|(folders, tags, snippets)| Spec {
            folders,
            tags,
            snippets,
        })
}

fn id(n: u64) -> Id {
    Id::v7(n, [0; 10])
}

const CASES: [CaseMode; 3] = [CaseMode::Exact, CaseMode::Insensitive, CaseMode::Adaptive];

/// Puts the library of `spec` into `store` under `profile_id`. Returns how many snippets it holds.
fn build(
    store: &mut Store,
    profile_id: Id,
    spec: &Spec,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut n = 0_u64;
    let mut next = move || {
        n += 1;
        id(n)
    };
    // Folders: parent chosen among the earlier ones, depth at most 4, sibling names distinct.
    let mut built: Vec<(Id, usize)> = Vec::new();
    let mut taken: HashSet<(Option<Id>, String)> = HashSet::new();
    for (i, (parent_hint, name, case)) in spec.folders.iter().enumerate() {
        let pick = parent_hint % (i + 1);
        let parent = built.get(pick).copied().filter(|&(_, depth)| depth < 4);
        let parent_id = parent.map(|(p, _)| p);
        if !taken.insert((parent_id, name.clone())) {
            continue;
        }
        let folder = Folder {
            meta: SyncMeta::new(next(), T0),
            profile_id,
            parent_id,
            name: name.clone(),
            position: 0,
            member_permission: None,
            defaults: FolderDefaults {
                case_mode: (*case > 0).then(|| CASES[*case - 1]),
                ..FolderDefaults::default()
            },
        };
        store.put_folder(&folder, T0)?;
        built.push((folder.meta.id, parent.map_or(1, |(_, d)| d + 1)));
    }
    let mut tags: Vec<Id> = Vec::new();
    let mut tag_names = HashSet::new();
    for (name, colored) in &spec.tags {
        if !tag_names.insert(name.clone()) {
            continue;
        }
        let tag = Tag {
            meta: SyncMeta::new(next(), T0),
            profile_id,
            name: name.clone(),
            color: colored.then(|| "#336699".to_owned()),
        };
        store.put_tag(&tag, T0)?;
        tags.push(tag.meta.id);
    }
    let mut keys = HashSet::new();
    let mut count = 0;
    for s in &spec.snippets {
        let abbreviation = Abbreviation::new(&s.abbreviation)?;
        if !keys.insert(abbreviation.fold_key()) {
            continue;
        }
        let folder = built[s.folder % built.len()].0;
        let mut snippet = Snippet::new_defaults(
            SyncMeta::new(next(), T0),
            folder,
            abbreviation,
            s.label.clone(),
            s.body.clone(),
            None,
        );
        snippet.trigger = if s.trigger {
            Trigger::Immediate
        } else {
            Trigger::Delimiter
        };
        snippet.case_mode = CASES[s.case_mode];
        snippet.word_start = s.word_start;
        snippet.enabled = s.enabled;
        snippet.kind = if s.rich {
            SnippetKind::Rich
        } else {
            SnippetKind::Plain
        };
        store.put_snippet(&snippet, T0)?;
        count += 1;
        let mut linked = HashSet::new();
        for t in &s.tags {
            if tags.is_empty() || !linked.insert(t % tags.len()) {
                continue;
            }
            let link = SnippetTag {
                meta: SyncMeta::new(next(), T0),
                snippet_id: snippet.meta.id,
                tag_id: tags[t % tags.len()],
            };
            store.put_snippet_tag(&link, T0)?;
        }
    }
    Ok(count)
}

/// A description of the library that ignores ids and timestamps and leaves out what an import
/// does not write (folders without snippets below them, tags no snippet carries). `full` adds
/// what only JSON carries: folder defaults and tag colors.
fn fingerprint(lib: &Library, full: bool) -> Vec<String> {
    let by_id: HashMap<Id, &Folder> = lib.folders.iter().map(|f| (f.meta.id, f)).collect();
    let path = |folder: Id| -> Vec<String> {
        let mut names = Vec::new();
        let mut cur = by_id.get(&folder).copied();
        while let Some(f) = cur {
            names.push(f.name.clone());
            cur = f.parent_id.and_then(|p| by_id.get(&p).copied());
        }
        names.reverse();
        names
    };
    let tag_by_id: HashMap<Id, &Tag> = lib.tags.iter().map(|t| (t.meta.id, t)).collect();
    let mut tags_of: HashMap<Id, Vec<String>> = HashMap::new();
    let mut used_tags: HashSet<Id> = HashSet::new();
    for l in &lib.snippet_tags {
        if let Some(t) = tag_by_id.get(&l.tag_id) {
            tags_of
                .entry(l.snippet_id)
                .or_default()
                .push(t.name.clone());
            used_tags.insert(t.meta.id);
        }
    }
    let mut used_folders: HashSet<Id> = HashSet::new();
    for s in &lib.snippets {
        let mut cur = by_id.get(&s.folder_id).copied();
        while let Some(f) = cur {
            used_folders.insert(f.meta.id);
            cur = f.parent_id.and_then(|p| by_id.get(&p).copied());
        }
    }
    let mut out = Vec::new();
    for f in lib
        .folders
        .iter()
        .filter(|f| used_folders.contains(&f.meta.id))
    {
        let defaults = if full {
            format!(" defaults={:?}", f.defaults)
        } else {
            String::new()
        };
        out.push(format!("folder {:?}{defaults}", path(f.meta.id)));
    }
    for s in &lib.snippets {
        let mut tags = tags_of.get(&s.meta.id).cloned().unwrap_or_default();
        tags.sort();
        out.push(format!(
            "snippet {:?} in {:?}: label={:?} kind={:?} body={:?} trigger={:?} case={:?} \
             word_start={} enabled={} tags={tags:?}",
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
    if full {
        for t in lib.tags.iter().filter(|t| used_tags.contains(&t.meta.id)) {
            out.push(format!("tag {:?} color={:?}", t.name, t.color));
        }
    }
    out.sort();
    out
}

fn round_trip(spec: &Spec, format: Format) -> Result<(), Box<dyn std::error::Error>> {
    let (p1, p2) = (id(1_000_000), id(1_000_001));
    let mut src = Store::open_in_memory()?;
    src.put_profile(&profile(p1, p1), T0)?;
    let count = build(&mut src, p1, spec)?;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join(match format {
        Format::Json => "lib.json",
        Format::Csv => "lib.csv",
    });
    transfer::export(&src, p1, &file, format)?;

    let mut dst = Store::open_in_memory()?;
    dst.put_profile(&profile(p2, p2), T0)?;
    let mut n = 2_000_000_u64;
    let mut next = move || {
        n += 1;
        id(n)
    };
    let summary = transfer::import(&mut dst, p2, &file, format, T0, &mut next, OnConflict::Fail)?;
    assert_eq!(summary.snippets_imported, count, "{format:?}");
    assert_eq!(summary.snippets_skipped, 0, "{format:?}");

    let full = format == Format::Json;
    let (a, b) = (
        fingerprint(&src.load_library()?, full),
        fingerprint(&dst.load_library()?, full),
    );
    assert_eq!(a, b, "{format:?} round trip changed the library");
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn json_export_then_import_gives_the_same_library(spec in spec()) {
        round_trip(&spec, Format::Json).map_err(|e| TestCaseError::fail(e.to_string()))?;
    }

    #[test]
    fn csv_export_then_import_gives_the_same_library(spec in spec()) {
        round_trip(&spec, Format::Csv).map_err(|e| TestCaseError::fail(e.to_string()))?;
    }
}
