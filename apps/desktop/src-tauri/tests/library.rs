//! The library's operations end to end (task 3.3): a real worker on the mock platform, reached
//! through the bridge, as the library window's commands reach it. Create, edit and delete a
//! snippet (with Undo), folders and tags, the abbreviation check, and the events with their
//! changed rows. Also: the snippet's text reaches no log.

mod common;

use std::fs;

use common::{Endpoint, LIBRARY, Service, id};
use iamb_desktop::contract::UiEvent;
use iamb_desktop::library::{self, DefaultsView, FolderDraft, SnippetDraft, TagDraft};

fn draft(folder: &str, abbreviation: &str, body: &str) -> SnippetDraft {
    SnippetDraft {
        id: None,
        folder_id: folder.to_owned(),
        abbreviation: abbreviation.to_owned(),
        label: format!("About {abbreviation}"),
        kind: "plain".to_owned(),
        body: body.to_owned(),
        trigger: "delimiter".to_owned(),
        case_mode: "insensitive".to_owned(),
        word_start: true,
        enabled: true,
        tags: vec![],
    }
}

fn code<T: std::fmt::Debug>(r: Result<T, iamb_desktop::contract::CommandError>) -> String {
    r.expect_err("an error").code
}

#[test]
#[allow(clippy::too_many_lines, reason = "one scenario, read top to bottom")]
fn create_edit_and_delete_a_snippet_with_undo() {
    const CANARY: &str = "zqxlibrarycanary";
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();

    let folders = library::folders(&bridge).expect("folders");
    let folder = folders.folders[0].id.clone();
    let page = library::summaries(&bridge, 0, 100).expect("page");
    assert_eq!(page.total, u64::try_from(LIBRARY.len()).expect("small"));
    assert_eq!(page.next, None);
    assert!(page.summaries.iter().all(|s| s.broken.is_none()));

    // A tag, then a snippet that carries it.
    library::save_tag(
        &bridge,
        &TagDraft {
            id: None,
            name: "Work".to_owned(),
            color: "teal".to_owned(),
        },
    )
    .expect("tag");
    let tags = library::tags(&bridge).expect("tags");
    assert_eq!(
        (
            tags[0].name.as_str(),
            tags[0].color.as_str(),
            tags[0].snippets
        ),
        ("Work", "teal", 0)
    );
    let tag = tags[0].id.clone();

    // Create. Nothing expands before the save; after it the abbreviation does.
    seen.clear();
    service.type_text(";thx ");
    assert_eq!(service.text(), ";thx ", "a draft never expands");
    let mut new = draft(&folder, ";thx", &format!("Thank you {CANARY}"));
    new.tags = vec![tag.clone()];
    let saved = library::save_snippet(&bridge, &new).expect("saved");
    let event = seen.wait_event(|e| matches!(e, UiEvent::LibraryChanged { changed: Some(c), .. } if c.snippets.contains(&saved.id)));
    let UiEvent::LibraryChanged { changed, .. } = event else {
        unreachable!()
    };
    assert_eq!(changed.expect("rows").tags, Vec::<String>::new());
    service.type_text(";thx ");
    assert_eq!(service.text(), format!(";thx Thank you {CANARY} "));

    let one = library::snippet(&bridge, &saved.id).expect("one");
    assert_eq!(
        (one.abbreviation.as_str(), one.tags.as_slice()),
        (";thx", &[tag.clone()][..])
    );
    assert_eq!(library::tags(&bridge).expect("tags")[0].snippets, 1);
    let listed = library::summaries(&bridge, 0, 100).expect("page");
    assert_eq!(listed.total, 4);
    let row = listed
        .summaries
        .iter()
        .find(|s| s.id == saved.id)
        .expect("row");
    assert_eq!(row.first_line, format!("Thank you {CANARY}"));
    assert_eq!(row.tags, std::slice::from_ref(&tag));

    // Edit: the same id, new text and a new abbreviation; the old one stops expanding.
    let mut edit = draft(&folder, ";thanks", "Many thanks");
    edit.id = Some(saved.id.clone());
    edit.tags = vec![];
    library::save_snippet(&bridge, &edit).expect("edited");
    let one = library::snippet(&bridge, &saved.id).expect("one");
    assert_eq!((one.body.as_str(), one.tags.len()), ("Many thanks", 0));
    assert_eq!(library::tags(&bridge).expect("tags")[0].snippets, 0);
    service.type_text(";thx ");
    assert!(
        service.text().ends_with(";thx "),
        "the old abbreviation is gone"
    );
    service.type_text(";thanks ");
    assert!(service.text().ends_with("Many thanks "));

    // Turn off and on again; its tags and the rest stay.
    library::set_enabled(&bridge, &saved.id, false).expect("off");
    let summary = |bridge: &_| {
        library::summaries(bridge, 0, 100)
            .expect("page")
            .summaries
            .into_iter()
            .find(|s| s.id == saved.id)
    };
    assert!(!summary(&bridge).expect("row").enabled);
    library::set_enabled(&bridge, &saved.id, true).expect("on");
    assert!(summary(&bridge).expect("row").enabled);

    // Delete, then Undo with the deleted snippet's own id.
    library::delete_snippet_by_id(&bridge, &saved.id).expect("deleted");
    assert_eq!(code(library::snippet(&bridge, &saved.id)), "not_found");
    assert_eq!(
        code(library::delete_snippet_by_id(&bridge, &saved.id)),
        "not_found"
    );
    assert!(summary(&bridge).is_none());
    let restored = library::save_snippet(&bridge, &edit).expect("restored");
    assert_eq!(restored.id, saved.id);
    assert_eq!(
        library::snippet(&bridge, &saved.id).expect("back").body,
        "Many thanks"
    );

    // A tag that is deleted leaves its snippets.
    library::delete_tag_by_id(&bridge, &tag).expect("tag deleted");
    assert!(library::tags(&bridge).expect("tags").is_empty());

    bridge.stop();
    for entry in fs::read_dir(service.dirs().logs()).expect("logs") {
        let path = entry.expect("entry").path();
        let text = fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !text.contains(CANARY),
            "{} holds a snippet's text",
            path.display()
        );
    }
}

#[test]
fn abbreviations_are_checked_before_saving() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let folder = library::folders(&bridge).expect("folders").folders[0]
        .id
        .clone();

    // `brb` is taken by a snippet of the seed library (case does not matter).
    let check = library::check(&bridge, "BRB", "delimiter", None).expect("check");
    assert_eq!(check.conflict, Some(id(1).to_string()));
    // Editing that snippet itself is no conflict.
    let own = library::check(&bridge, "brb", "delimiter", Some(&id(1).to_string())).expect("check");
    assert_eq!(own.conflict, None);
    // Saving it anyway is refused, and nothing is written.
    let e = library::save_snippet(&bridge, &draft(&folder, "brb", "x")).expect_err("conflict");
    assert_eq!(e.code, "conflict");
    assert_eq!(library::summaries(&bridge, 0, 10).expect("page").total, 3);

    // Shadowing: an immediate `;r` hides a later `;rlate`, and the check says which.
    let mut late = draft(&folder, ";rlate", "late");
    let late_id = library::save_snippet(&bridge, &late).expect("saved").id;
    late.id = Some(late_id.clone());
    let hides = library::check(&bridge, ";r", "immediate", None).expect("check");
    assert_eq!(hides.hides, std::slice::from_ref(&late_id));
    assert_eq!(hides.conflict, None);
    let delimited = library::check(&bridge, ";r", "delimiter", None).expect("check");
    assert!(delimited.hides.is_empty());
    // The other way round: `;rlate` is hidden by an immediate `;rl`.
    let mut rl = draft(&folder, ";rl", "rl");
    rl.trigger = "immediate".to_owned();
    rl.word_start = false;
    let rl_id = library::save_snippet(&bridge, &rl).expect("saved").id;
    let hidden = library::check(&bridge, ";rlate", "delimiter", Some(&late_id)).expect("check");
    assert_eq!(hidden.hidden_by, [rl_id]);
    assert_eq!(
        code(library::check(&bridge, ";rl", "sometimes", None)),
        "invalid"
    );
    assert_eq!(
        code(library::save_snippet(&bridge, &draft(&folder, "a b", "x"))),
        "invalid"
    );
    bridge.stop();
}

#[test]
fn folders_nest_to_the_limit_and_delete_with_their_snippets() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let root = library::folders(&bridge).expect("folders").folders[0]
        .id
        .clone();
    let folder = |parent: Option<&str>, name: &str| FolderDraft {
        id: None,
        parent_id: parent.map(str::to_owned),
        name: name.to_owned(),
        position: 0,
        defaults: DefaultsView {
            abbreviation_prefix: Some(";r".to_owned()),
            trigger: Some("immediate".to_owned()),
            case_mode: None,
        },
    };

    // The seed folder is level 1; three more reach the limit of four.
    let l2 = library::save_folder(&bridge, &folder(Some(&root), "Replies"))
        .expect("l2")
        .id;
    let l3 = library::save_folder(&bridge, &folder(Some(&l2), "Shop"))
        .expect("l3")
        .id;
    let l4 = library::save_folder(&bridge, &folder(Some(&l3), "Returns"))
        .expect("l4")
        .id;
    assert_eq!(
        code(library::save_folder(
            &bridge,
            &folder(Some(&l4), "Too deep")
        )),
        "invalid"
    );
    assert_eq!(
        code(library::save_folder(&bridge, &folder(None, "  "))),
        "invalid"
    );

    // The defaults come back; a rename keeps the row.
    let listed = library::folders(&bridge).expect("folders");
    assert_eq!(listed.folders.len(), 4);
    let shop = listed.folders.iter().find(|f| f.id == l3).expect("shop");
    assert_eq!(shop.defaults.abbreviation_prefix.as_deref(), Some(";r"));
    assert_eq!(shop.defaults.trigger.as_deref(), Some("immediate"));
    let mut renamed = folder(Some(&l2), "Shop replies");
    renamed.id = Some(l3.clone());
    library::save_folder(&bridge, &renamed).expect("renamed");
    let listed = library::folders(&bridge).expect("folders");
    assert_eq!(listed.folders.len(), 4);
    assert!(listed.folders.iter().any(|f| f.name == "Shop replies"));

    // A snippet in the folder goes with it.
    let mut s = draft(&l3, ";rhi", "hi");
    s.trigger = "immediate".to_owned();
    s.word_start = false;
    let sid = library::save_snippet(&bridge, &s).expect("saved").id;
    seen.clear();
    library::delete_folder_by_id(&bridge, &l3).expect("deleted");
    seen.wait_event(|e| matches!(e, UiEvent::LibraryChanged { .. }));
    assert_eq!(code(library::snippet(&bridge, &sid)), "not_found");
    assert_eq!(
        code(library::delete_folder_by_id(&bridge, &l3)),
        "not_found"
    );
    bridge.stop();
}

#[test]
fn tags_have_eight_colours_and_unique_names() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let tag = |name: &str, color: &str| TagDraft {
        id: None,
        name: name.to_owned(),
        color: color.to_owned(),
    };
    let first = library::save_tag(&bridge, &tag("urgent", "red"))
        .expect("saved")
        .id;
    assert_eq!(
        code(library::save_tag(&bridge, &tag("URGENT", "blue"))),
        "conflict"
    );
    assert_eq!(
        code(library::save_tag(&bridge, &tag("x", "pink"))),
        "invalid"
    );
    assert_eq!(
        code(library::save_tag(&bridge, &tag("  ", "red"))),
        "invalid"
    );
    // Rename and recolour.
    let mut edit = tag("español", "orange");
    edit.id = Some(first.clone());
    library::save_tag(&bridge, &edit).expect("edited");
    let tags = library::tags(&bridge).expect("tags");
    assert_eq!(tags.len(), 1);
    assert_eq!(
        (tags[0].name.as_str(), tags[0].color.as_str()),
        ("español", "orange")
    );
    bridge.stop();
}
