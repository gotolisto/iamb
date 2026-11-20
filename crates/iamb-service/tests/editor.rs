//! What the Phase 3 library window and editor need from the service (`docs/ux/spec.md`
//! §11.2): template parsing and preview, tags, snippet summaries, restoring a deleted
//! snippet, the abbreviation check, search hits with ranges, the palette shortcut setting,
//! named rows in `LibraryChanged`, and the Enter-or-Tab diagnostic. On the mock, through the
//! service endpoint.

mod common;

use std::collections::BTreeMap;

use common::{Harness, LIBRARY, id, wait_event};
use iamb_core::model::{Abbreviation, Snippet, SyncMeta, Tag, Timestamp};
use iamb_ipc::{
    ChangedRows, Client, ErrorCode, EventBody, PreviewPart, RequestBody, ResponseBody, Role,
    SearchField, Secret, SnippetQuery, TagEntry,
};
use iamb_platform::{Chord, HotkeyId};

fn call(c: &mut Client, body: RequestBody) -> ResponseBody {
    c.call(body).expect("answered")
}

fn code(c: &mut Client, body: RequestBody) -> Option<ErrorCode> {
    c.call(body).err().and_then(|e| e.remote_code())
}

fn profile(c: &mut Client) -> iamb_core::model::Id {
    match call(c, RequestBody::ListFolders {}) {
        ResponseBody::Folders { profile, .. } => profile,
        other => panic!("unexpected {other:?}"),
    }
}

fn snippet(c: &mut Client, n: u8) -> (Snippet, Vec<iamb_core::model::Id>) {
    match call(c, RequestBody::GetSnippet { id: id(n) }) {
        ResponseBody::Snippet { snippet, tags } => (snippet, tags),
        other => panic!("unexpected {other:?}"),
    }
}

fn tags(c: &mut Client) -> Vec<TagEntry> {
    match call(c, RequestBody::ListTags {}) {
        ResponseBody::Tags { tags } => tags,
        other => panic!("unexpected {other:?}"),
    }
}

fn tag(profile: iamb_core::model::Id, n: u8, name: &str, color: Option<&str>) -> Tag {
    Tag {
        meta: SyncMeta::new(id(n), Timestamp(1)),
        profile_id: profile,
        name: name.to_owned(),
        color: color.map(str::to_owned),
    }
}

#[test]
fn templates_are_parsed_and_previewed_against_the_library() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    match call(
        &mut ui,
        RequestBody::ParseTemplate {
            source: "Dear {field name=Name}, {snippet abbr=\";sig\"}{cursor}".to_owned(),
        },
    ) {
        ResponseBody::TemplateTokens { tokens, error } => {
            assert_eq!(error, None);
            let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
            assert_eq!(kinds, ["text", "field", "text", "snippet", "cursor"]);
            assert_eq!(tokens[1].attributes[0].value, "Name");
            assert_eq!((tokens[1].start, tokens[1].end), (5, 22));
        }
        other => panic!("unexpected {other:?}"),
    }
    match call(
        &mut ui,
        RequestBody::ParseTemplate {
            source: "{snippet abbr=\";gone\"}".to_owned(),
        },
    ) {
        ResponseBody::TemplateTokens { error, .. } => {
            assert_eq!(
                error.map(|e| e.code),
                Some("E_SNIPPET_NOT_FOUND".to_owned())
            );
        }
        other => panic!("unexpected {other:?}"),
    }
    match call(
        &mut ui,
        RequestBody::RenderPreview {
            source: "Dear {field name=Name}, {snippet abbr=\";sig\"} {date}".to_owned(),
            values: Secret::new(BTreeMap::new()),
            rich: false,
        },
    ) {
        ResponseBody::Preview {
            parts,
            fields,
            error,
            ..
        } => {
            assert_eq!(error, None);
            assert_eq!(fields.len(), 1);
            let [PreviewPart::Text { text, blanks, .. }] = parts.as_slice() else {
                panic!("one text part: {parts:?}");
            };
            // The fixed clock of the harness; the included snippet's text; a sample blank.
            assert_eq!(text, "Dear Name, Kind regards 2026-03-18");
            assert!(blanks[0].sample);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
#[allow(clippy::too_many_lines, reason = "one tag's whole life, step by step")]
fn tags_are_listed_created_renamed_attached_and_deleted() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    let me = profile(&mut ui);
    assert!(tags(&mut ui).is_empty());
    // The colour is one of the eight names.
    for color in [None, Some("#aabbcc"), Some("pink")] {
        assert_eq!(
            code(
                &mut ui,
                RequestBody::PutTag {
                    tag: tag(me, 60, "Work", color)
                }
            ),
            Some(ErrorCode::Invalid),
            "{color:?}"
        );
    }
    assert_eq!(
        code(
            &mut ui,
            RequestBody::PutTag {
                tag: tag(me, 60, "  ", Some("blue"))
            }
        ),
        Some(ErrorCode::Invalid)
    );
    call(
        &mut ui,
        RequestBody::PutTag {
            tag: tag(me, 60, "Work", Some("blue")),
        },
    );
    call(
        &mut ui,
        RequestBody::PutTag {
            tag: tag(me, 61, "Home", Some("green")),
        },
    );
    assert_eq!(
        code(
            &mut ui,
            RequestBody::PutTag {
                tag: tag(me, 62, "work", Some("red"))
            }
        ),
        Some(ErrorCode::Conflict),
        "names are unique without regard to case"
    );

    // Attach two tags to "brb", in the same write as the snippet; the event names the row.
    let (brb, none) = snippet(&mut ui, 1);
    assert!(none.is_empty());
    call(
        &mut ui,
        RequestBody::PutSnippet {
            snippet: brb.clone(),
            tags: Some(vec![id(60), id(61)]),
        },
    );
    match wait_event(
        &mut ui,
        |e| matches!(e, EventBody::LibraryChanged { changed: Some(c), .. } if c.snippets == [id(1)]),
    ) {
        EventBody::LibraryChanged { changed, .. } => assert_eq!(
            changed,
            Some(ChangedRows {
                snippets: vec![id(1)],
                ..ChangedRows::default()
            })
        ),
        _ => unreachable!("matched above"),
    }
    // By name: Home, Work.
    assert_eq!(snippet(&mut ui, 1).1, [id(61), id(60)]);
    let listed: Vec<(String, u64)> = tags(&mut ui)
        .into_iter()
        .map(|t| (t.tag.name, t.snippets))
        .collect();
    assert_eq!(listed, [("Home".to_owned(), 1), ("Work".to_owned(), 1)]);
    match call(
        &mut ui,
        RequestBody::ListSnippets {
            query: SnippetQuery {
                folder: None,
                tag: Some(id(60)),
            },
        },
    ) {
        ResponseBody::Snippets { snippets, tags } => {
            assert_eq!(snippets.len(), 1);
            assert_eq!(tags.get(&id(1)), Some(&vec![id(61), id(60)]));
        }
        other => panic!("unexpected {other:?}"),
    }
    // Saving without `tags` keeps them; with a list, detaches the rest.
    call(
        &mut ui,
        RequestBody::PutSnippet {
            snippet: brb.clone(),
            tags: None,
        },
    );
    assert_eq!(snippet(&mut ui, 1).1.len(), 2);
    call(
        &mut ui,
        RequestBody::PutSnippet {
            snippet: brb.clone(),
            tags: Some(vec![id(60)]),
        },
    );
    assert_eq!(snippet(&mut ui, 1).1, [id(60)]);
    assert_eq!(
        code(
            &mut ui,
            RequestBody::PutSnippet {
                snippet: brb,
                tags: Some(vec![id(99)])
            }
        ),
        Some(ErrorCode::Invalid),
        "an unknown tag"
    );

    // Renamed and recoloured in place.
    call(
        &mut ui,
        RequestBody::PutTag {
            tag: tag(me, 60, "Office", Some("purple")),
        },
    );
    let office = tags(&mut ui)
        .into_iter()
        .find(|t| t.tag.meta.id == id(60))
        .expect("still there");
    assert_eq!(
        (office.tag.name.as_str(), office.tag.color.as_deref()),
        ("Office", Some("purple"))
    );
    // The palette finds a snippet by its tag's name.
    match call(
        &mut ui,
        RequestBody::Search {
            text: Secret::new("offi".to_owned()),
            limit: 10,
        },
    ) {
        ResponseBody::SearchResults { hits } => {
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].snippet, id(1));
            assert_eq!(hits[0].field, Some(SearchField::Tag));
            assert_eq!(hits[0].ranges, [[0, 4]]);
        }
        other => panic!("unexpected {other:?}"),
    }

    // Deleting a tag detaches it everywhere.
    call(&mut ui, RequestBody::DeleteTag { id: id(60) });
    assert!(snippet(&mut ui, 1).1.is_empty());
    assert_eq!(tags(&mut ui).len(), 1);
    assert_eq!(
        code(&mut ui, RequestBody::DeleteTag { id: id(60) }),
        Some(ErrorCode::NotFound)
    );
}

#[test]
fn a_deleted_snippet_is_restored_with_its_usage_and_tags() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    let me = profile(&mut ui);
    call(
        &mut ui,
        RequestBody::PutTag {
            tag: tag(me, 60, "Work", Some("blue")),
        },
    );
    let (brb, _) = snippet(&mut ui, 1);
    call(
        &mut ui,
        RequestBody::PutSnippet {
            snippet: brb.clone(),
            tags: Some(vec![id(60)]),
        },
    );
    h.os.type_text("brb ");
    h.settle();
    call(&mut ui, RequestBody::DeleteSnippet { id: id(1) });
    assert_eq!(
        code(&mut ui, RequestBody::GetSnippet { id: id(1) }),
        Some(ErrorCode::NotFound)
    );
    // Undo: the same row again.
    call(
        &mut ui,
        RequestBody::PutSnippet {
            snippet: brb,
            tags: None,
        },
    );
    let (back, tags) = snippet(&mut ui, 1);
    assert_eq!(back.meta.deleted_at, None);
    assert_eq!(tags, [id(60)]);
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back be right back ");
    let dir = h.stop();
    let store = iamb_store::Store::open(dir.path()).expect("store");
    assert_eq!(
        store.usage(id(1)).expect("usage").map(|u| u.count),
        Some(2),
        "the usage before the delete is kept"
    );
}

#[test]
fn summaries_come_in_pages_without_bodies() {
    let h = Harness::start(&[
        (1, "brb", "be right back\nsecond line", false),
        (2, "omw", "on my way", false),
        (3, "bad", "{date fmt=\"%Q\"}", false),
        (4, ";sig", "Kind regards", true),
    ]);
    let mut ui = h.client(Role::Ui);
    let page = |ui: &mut Client, offset, limit| match call(
        ui,
        RequestBody::ListSnippetSummaries {
            query: SnippetQuery::default(),
            offset,
            limit,
        },
    ) {
        ResponseBody::SnippetSummaries {
            total,
            summaries,
            next,
            ..
        } => (total, summaries, next),
        other => panic!("unexpected {other:?}"),
    };
    let (total, first, next) = page(&mut ui, 0, 3);
    assert_eq!((total, first.len(), next), (4, 3, Some(3)));
    assert_eq!(first[0].first_line, "be right back");
    assert_eq!(first[0].trigger, "delimiter");
    assert_eq!(first[2].broken.as_deref(), Some("E_BAD_FORMAT"));
    assert_eq!(first[1].broken, None);
    let (_, rest, next) = page(&mut ui, 3, 3);
    assert_eq!((rest.len(), next), (1, None));
    assert_eq!(rest[0].abbreviation, ";sig");
    let (_, empty, next) = page(&mut ui, 10, 3);
    assert!(empty.is_empty() && next.is_none());
    assert_eq!(
        code(
            &mut ui,
            RequestBody::ListSnippetSummaries {
                query: SnippetQuery::default(),
                offset: 0,
                limit: 0
            }
        ),
        Some(ErrorCode::Invalid)
    );
}

#[test]
fn an_abbreviation_is_checked_without_saving() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    let check = |ui: &mut Client, abbreviation: &str, trigger: &str, snippet| match call(
        ui,
        RequestBody::CheckAbbreviation {
            abbreviation: abbreviation.to_owned(),
            trigger: trigger.to_owned(),
            snippet,
        },
    ) {
        ResponseBody::AbbreviationCheck {
            conflict,
            hides,
            hidden_by,
        } => (conflict, hides, hidden_by),
        other => panic!("unexpected {other:?}"),
    };
    // Case-folded, as the store compares.
    assert_eq!(
        check(&mut ui, "BRB", "delimiter", None),
        (Some(id(1)), vec![], vec![])
    );
    assert_eq!(
        check(&mut ui, "brb", "delimiter", Some(id(1))),
        (None, vec![], vec![])
    );
    // An immediate "om" would hide "omw"; ";sig" (immediate) hides ";sigx".
    assert_eq!(
        check(&mut ui, "om", "immediate", None),
        (None, vec![id(2)], vec![])
    );
    assert_eq!(
        check(&mut ui, ";sigx", "delimiter", None),
        (None, vec![], vec![id(4)])
    );
    assert_eq!(
        code(
            &mut ui,
            RequestBody::CheckAbbreviation {
                abbreviation: "a b".to_owned(),
                trigger: "delimiter".to_owned(),
                snippet: None
            }
        ),
        Some(ErrorCode::Invalid)
    );
    // Nothing was written.
    match call(
        &mut ui,
        RequestBody::ListSnippets {
            query: SnippetQuery::default(),
        },
    ) {
        ResponseBody::Snippets { snippets, .. } => assert_eq!(snippets.len(), 4),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn search_hits_say_which_field_matched_and_where() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    match call(
        &mut ui,
        RequestBody::Search {
            text: Secret::new("RIGHT".to_owned()),
            limit: 5,
        },
    ) {
        ResponseBody::SearchResults { hits } => {
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].field, Some(SearchField::Body));
            assert_eq!(hits[0].ranges, [[3, 8]]);
        }
        other => panic!("unexpected {other:?}"),
    }
    match call(
        &mut ui,
        RequestBody::Search {
            text: Secret::new("om".to_owned()),
            limit: 5,
        },
    ) {
        ResponseBody::SearchResults { hits } => {
            assert_eq!(hits[0].snippet, id(2));
            assert_eq!(hits[0].field, Some(SearchField::Abbreviation));
            assert_eq!(hits[0].ranges, [[0, 2]]);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn the_palette_shortcut_is_registered_when_its_setting_changes() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    let palette = HotkeyId(1);
    let put = |ui: &mut Client, value: Option<&str>| {
        ui.call(RequestBody::PutSetting {
            key: "palette_hotkey".to_owned(),
            value: value.map(str::to_owned),
        })
        .map(|_| ())
        .map_err(|e| e.remote_code())
    };
    let chord = |text: &str| Chord::parse(text).expect("chord");
    assert_eq!(put(&mut ui, Some("Ctrl+Shift+Space")), Ok(()));
    assert_eq!(
        h.os.registered_hotkeys(),
        [(palette, chord("Ctrl+Shift+Space"))]
    );
    // Another application holds this one: refused, and the old one stays.
    h.os.reserve_chord(chord("Ctrl+Alt+K"));
    assert_eq!(
        put(&mut ui, Some("Ctrl+Alt+K")),
        Err(Some(ErrorCode::Conflict))
    );
    assert_eq!(
        h.os.registered_hotkeys(),
        [(palette, chord("Ctrl+Shift+Space"))]
    );
    match call(&mut ui, RequestBody::GetSettings {}) {
        ResponseBody::Settings { settings } => assert_eq!(
            settings.get("palette_hotkey").map(String::as_str),
            Some("Ctrl+Shift+Space"),
            "a refused chord is not stored"
        ),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        put(&mut ui, Some("Ctrl+Nope")),
        Err(Some(ErrorCode::Invalid))
    );
    assert_eq!(put(&mut ui, Some("Ctrl+Alt+J")), Ok(()));
    assert_eq!(h.os.registered_hotkeys(), [(palette, chord("Ctrl+Alt+J"))]);
    assert_eq!(put(&mut ui, None), Ok(()));
    assert!(h.os.registered_hotkeys().is_empty());
}

#[test]
fn an_abbreviation_followed_by_enter_is_named_in_a_timed_diagnostic() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    h.os.type_text("brb\n");
    match wait_event(
        &mut ui,
        |e| matches!(e, EventBody::DiagnosticAdded { entry } if entry.code == "not_a_delimiter"),
    ) {
        EventBody::DiagnosticAdded { entry } => {
            assert_eq!(entry.snippet, Some(id(1)));
            assert!(entry.at.0 > 1_700_000_000_000, "stamped with the time");
        }
        _ => unreachable!("matched above"),
    }
    h.settle();
    assert_eq!(h.text(h.editor), "brb\n", "nothing expanded");
}

#[test]
fn a_new_snippet_written_through_the_endpoint_names_its_row() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    let folder = match call(&mut ui, RequestBody::ListFolders {}) {
        ResponseBody::Folders { folders, .. } => folders[0].meta.id,
        other => panic!("unexpected {other:?}"),
    };
    let new = Snippet::new_defaults(
        SyncMeta::new(id(70), Timestamp(1)),
        folder,
        Abbreviation::new("tyvm").expect("abbreviation"),
        "",
        "thank you very much",
        None,
    );
    call(
        &mut ui,
        RequestBody::PutSnippet {
            snippet: new,
            tags: None,
        },
    );
    wait_event(
        &mut ui,
        |e| matches!(e, EventBody::LibraryChanged { changed: Some(c), .. } if c.snippets == [id(70)]),
    );
}
