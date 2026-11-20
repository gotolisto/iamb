//! Rich snippets end to end on the mock platform (task 3.8): a formatted snippet expands through
//! `Paste` with its images embedded, the user's clipboard comes back, the plain text is typed
//! where pasting is not possible or an image is missing, and attachments round-trip through the
//! editor's requests.

mod common;

use std::collections::BTreeMap;

use common::{Harness, diagnostic_codes, id, seed};
use iamb_core::model::{Abbreviation, Snippet, SnippetKind, SyncMeta, Timestamp};
use iamb_core::rich;
use iamb_ipc::{PreviewPart, RequestBody, ResponseBody, Role, Secret};
use iamb_platform::ClipboardContent;
use iamb_platform::mock::Injected;
use iamb_service::library::build_view;
use iamb_store::Store;
use tempfile::TempDir;

fn png(tag: u8) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend_from_slice(&[tag; 24]);
    b
}

/// A data root with the standard library plus a rich snippet `;logo` showing an image, and a
/// rich snippet `;bold` without one. Returns the root and the image's hash text.
fn rich_library(with_image_bytes: bool) -> (TempDir, String) {
    let dir = TempDir::new().expect("temp dir");
    seed(dir.path(), common::LIBRARY);
    let mut store = Store::open(dir.path()).expect("store");
    let profile = store
        .list_profiles(iamb_store::ListOptions::LIVE)
        .expect("profiles")[0]
        .meta
        .id;
    let folder = build_view(&store, profile, 1).expect("view").folders[0]
        .meta
        .id;
    let bytes = png(1);
    let sha = iamb_store::attachments::sha256_of(&bytes);
    if with_image_bytes {
        store
            .add_attachment(id(60), profile, &bytes, "logo.png", Timestamp(20))
            .expect("attachment");
    }
    let put = |store: &mut Store, n: u8, abbr: &str, body: String| {
        let mut s = Snippet::new_defaults(
            SyncMeta::new(id(n), Timestamp(30)),
            folder,
            Abbreviation::new(abbr).expect("abbreviation"),
            "",
            body,
            None,
        );
        s.kind = SnippetKind::Rich;
        store.put_snippet(&s, Timestamp(30)).expect("snippet");
    };
    put(
        &mut store,
        61,
        ";logo",
        format!("**Kim Lee**\n{}", rich::image_markdown("Logo", &sha)),
    );
    put(
        &mut store,
        62,
        ";bold",
        "*Thanks* for **your** order".to_owned(),
    );
    (dir, sha.to_string())
}

fn pastes(h: &Harness) -> Vec<ClipboardContent> {
    h.os.injected()
        .into_iter()
        .filter_map(|i| match i {
            Injected::Paste(c) => Some(c),
            _ => None,
        })
        .collect()
}

#[test]
fn a_rich_snippet_pastes_html_with_its_image_and_restores_the_clipboard() {
    let (dir, sha) = rich_library(true);
    let h = Harness::start_in(dir, |_| {});
    h.os.copy(ClipboardContent::text("what the user copied"));
    h.os.type_text("Best, ;logo ");
    h.settle();

    let pasted = pastes(&h);
    assert_eq!(pasted.len(), 1, "{:?}", h.os.injected());
    let html = pasted[0].html.as_deref().expect("html");
    assert!(
        html.starts_with("<p><strong>Kim Lee</strong><br>\n<img src=\"data:image/png;base64,"),
        "{html}"
    );
    assert!(html.ends_with("\" alt=\"Logo\"></p>"), "{html}");
    assert!(
        !html.contains("attachment:"),
        "the reference is resolved: {html}"
    );
    assert!(!html.contains(&sha));
    assert_eq!(pasted[0].text.as_deref(), Some("Kim Lee\nLogo"));
    assert!(pasted[0].files.is_empty());
    // The mock pastes the plain text into its field; the delimiter is typed again after it.
    assert_eq!(h.text(h.editor), "Best, Kim Lee\nLogo ");
    // The clipboard is the user's again, and the paste never entered the history.
    assert_eq!(
        h.os.clipboard(),
        ClipboardContent::text("what the user copied")
    );
    assert_eq!(
        h.os.clipboard_history(),
        [ClipboardContent::text("what the user copied")]
    );
}

#[test]
fn a_missing_image_types_the_plain_text_and_says_why() {
    let (dir, _) = rich_library(false);
    let h = Harness::start_in(dir, |_| {});
    let mut ui = h.client(Role::Ui);
    h.os.type_text(";logo ");
    h.settle();
    assert!(pastes(&h).is_empty(), "{:?}", h.os.injected());
    assert_eq!(h.text(h.editor), "Kim Lee\nLogo ");
    assert!(diagnostic_codes(&mut ui).contains(&"attachment_missing".to_owned()));
}

#[test]
fn an_app_that_must_not_be_pasted_into_gets_the_plain_text() {
    let (dir, _) = rich_library(true);
    let h = Harness::start_in(dir, |_| {});
    let mut ui = h.client(Role::Ui);
    let settings = r#"{"apps": {"org.example.editor": {"method": "keys"}}}"#;
    match ui
        .call(RequestBody::PutSetting {
            key: iamb_platform::INJECTION_SETTINGS_KEY.to_owned(),
            value: Some(settings.to_owned()),
        })
        .expect("call")
    {
        ResponseBody::Done {} => {}
        other => panic!("unexpected {other:?}"),
    }
    h.settle();
    h.os.copy(ClipboardContent::text("mine"));
    h.os.type_text(";bold ");
    h.settle();
    assert!(pastes(&h).is_empty(), "{:?}", h.os.injected());
    assert_eq!(h.text(h.editor), "Thanks for your order ");
    assert_eq!(h.os.clipboard(), ClipboardContent::text("mine"));
}

fn call(ui: &mut iamb_ipc::Client, body: RequestBody) -> ResponseBody {
    ui.call(body).expect("call")
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one image from the drop to the saved link, step by step"
)]
fn the_editor_adds_an_image_previews_it_and_saving_links_it() {
    let h = Harness::start(common::LIBRARY);
    let mut ui = h.client(Role::Ui);
    let (profile, folder) = match call(&mut ui, RequestBody::ListFolders {}) {
        ResponseBody::Folders { folders, .. } => (folders[0].profile_id, folders[0].meta.id),
        other => panic!("unexpected {other:?}"),
    };
    let drop_dir = TempDir::new().expect("dir");
    let file = drop_dir.path().join("Logo.png");
    std::fs::write(&file, png(9)).expect("write");
    let attachment = match call(
        &mut ui,
        RequestBody::PutAttachment {
            path: file.clone(),
            file_name: "Logo.png".to_owned(),
            folder,
        },
    ) {
        ResponseBody::Attachment { attachment } => attachment,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(attachment.mime, "image/png");
    assert_eq!(attachment.file_name, "Logo.png");
    let stored = attachment.path.clone().expect("path");
    assert!(stored.starts_with(h.dir.path()));
    assert_eq!(std::fs::read(&stored).expect("bytes"), png(9));

    // A file that is not an accepted type is refused with `invalid`.
    let svg = drop_dir.path().join("x.svg");
    std::fs::write(&svg, "<svg onload=alert(1)/>").expect("write");
    match ui.call(RequestBody::PutAttachment {
        path: svg,
        file_name: "x.svg".to_owned(),
        folder,
    }) {
        Err(e) => assert_eq!(e.remote_code(), Some(iamb_ipc::ErrorCode::Invalid)),
        Ok(other) => panic!("unexpected {other:?}"),
    }

    // The formatted preview shows the image from its file.
    let body = format!(
        "Hi {{field name=Name}} **bold** {}",
        rich::image_markdown("Logo", &attachment.sha256)
    );
    match call(
        &mut ui,
        RequestBody::RenderPreview {
            source: body.clone(),
            values: Secret::new(BTreeMap::new()),
            rich: true,
        },
    ) {
        ResponseBody::Preview {
            parts,
            cursor,
            fields,
            error,
        } => {
            assert_eq!(error, None);
            assert_eq!(cursor, None);
            assert_eq!(fields.len(), 1);
            let [
                PreviewPart::Formatted {
                    html,
                    plain,
                    attachments,
                },
            ] = parts.as_slice()
            else {
                panic!("one formatted part: {parts:?}");
            };
            assert_eq!(
                html,
                &format!(
                    "<p>Hi Name <strong>bold</strong> <img src=\"attachment:{}\" alt=\"Logo\"></p>",
                    attachment.sha256
                )
            );
            assert_eq!(plain, "Hi Name bold Logo");
            assert_eq!(attachments[0].path.as_deref(), Some(stored.as_path()));
            assert_eq!(attachments[0].mime, "image/png");
        }
        other => panic!("unexpected {other:?}"),
    }

    // Saving the snippet links it to the attachment.
    let mut snippet = Snippet::new_defaults(
        SyncMeta::new(id(70), Timestamp(40)),
        folder,
        Abbreviation::new(";img").expect("abbreviation"),
        "",
        body,
        None,
    );
    snippet.kind = SnippetKind::Rich;
    match call(
        &mut ui,
        RequestBody::PutSnippet {
            snippet,
            tags: None,
        },
    ) {
        ResponseBody::Saved { .. } => {}
        other => panic!("unexpected {other:?}"),
    }
    drop(ui);
    let dir = h.stop();
    let store = Store::open(dir.path()).expect("store");
    let links = store
        .list_snippet_attachments(iamb_store::ListOptions::LIVE)
        .expect("links");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].snippet_id, id(70));
    let rows = store
        .list_attachments(iamb_store::ListOptions::LIVE)
        .expect("rows");
    assert_eq!(rows[0].profile_id, profile);
    assert_eq!(links[0].attachment_id, rows[0].meta.id);
}
