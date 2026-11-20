//! The paste-fidelity fixtures (task 3.8, `docs/paste-fidelity.md`): the importable file of
//! fixture snippets, and the clipboard content Iamb builds for each, which the headless-browser
//! round trip (`docs/paste-fidelity/browser-paste.mjs`) pastes.
//!
//! Both files are generated here and must be current: run with `IAMB_BLESS=1` to rewrite them
//! after a deliberate change, and review the diff.
//!
//! - `fixtures.json` is written by the store's own JSON export from a library built here, so it
//!   is exactly what an export holds (the logo attachment embedded), and the test imports it
//!   into a fresh library to prove it is importable.
//! - `clipboard.json` holds, per fixture, the HTML (images embedded as data URIs) and the plain
//!   text that the service puts on the clipboard, built by the same code as an expansion
//!   (`iamb_core::rich`, `iamb_service::rich`), with each field at its default value.

use std::path::{Path, PathBuf};

use iamb_core::datetime::LocalDateTime;
use iamb_core::model::{Abbreviation, Id, Snippet, SnippetKind, SyncMeta, Timestamp};
use iamb_core::rich;
use iamb_core::template::{FormValues, RenderContext, RenderedAction};
use iamb_platform::{ClipboardContent, InjectionStep};
use iamb_service::library::{build_view, ensure_personal_profile};
use iamb_store::Store;
use iamb_store::transfer::{self, Format, OnConflict};
use serde_json::{Value, json};
use tempfile::TempDir;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/paste-fidelity")
}

fn bless() -> bool {
    std::env::var_os("IAMB_BLESS").is_some()
}

/// The fixtures: abbreviation, label, Markdown. `{logo}` stands for the logo's reference.
pub const FIXTURES: &[(&str, &str, &str)] = &[
    (
        ";fx-head",
        "Heading",
        "# Order update\n\nYour order has shipped.",
    ),
    (";fx-bold", "Bold", "Thanks for your order, **Kim**."),
    (
        ";fx-italic",
        "Italic",
        "We are *really* sorry about the delay.",
    ),
    (
        ";fx-link",
        "Links",
        "Track it on [our site](https://example.com/track) or write to <orders@example.com>.",
    ),
    (
        ";fx-list",
        "List",
        "Please bring:\n\n- the box\n- the tape\n- the label",
    ),
    (
        ";fx-nested",
        "Nested list",
        "1. Pack\n   - the box\n   - the tape\n2. Ship\n3. Confirm",
    ),
    (
        ";fx-image",
        "Inline image",
        "Kim Lee\nOrders team\n\n{logo}",
    ),
    (
        ";fx-field",
        "Field inside formatting",
        "Dear **{field name=\"Name\" default=\"Ada <Lovelace> & *co*\"}**, your *{field name=\"Item\" default=\"blue mug\"}* is ready.",
    ),
    (
        ";fx-sig",
        "Signature (everything)",
        "**Kim Lee** · *Orders team*\n[example.com](https://example.com) · ~~old number~~\n\n{logo}",
    ),
];

fn ids() -> impl FnMut() -> Id {
    let mut n = 0x40_u8;
    move || {
        n += 1;
        Id::from_bytes([n; 16])
    }
}

/// A library holding the logo and the fixtures, in a fresh data root.
fn library() -> (TempDir, Id) {
    let root = TempDir::new().expect("temp dir");
    let mut store = Store::open(root.path()).expect("store");
    let mut new_id = ids();
    let profile = ensure_personal_profile(&mut store, &mut new_id, Timestamp(1)).expect("profile");
    let folder = build_view(&store, profile, 1).expect("view").folders[0]
        .meta
        .id;
    let logo = std::fs::read(dir().join("logo.png")).expect("logo.png");
    let attachment = store
        .add_attachment(new_id(), profile, &logo, "Iamb logo.png", Timestamp(1))
        .expect("attachment");
    let image = rich::image_markdown("Iamb logo", &attachment.sha256);
    for (i, (abbr, label, body)) in FIXTURES.iter().enumerate() {
        let mut s = Snippet::new_defaults(
            SyncMeta::new(
                Id::from_bytes([0x80 + u8::try_from(i).expect("few"); 16]),
                Timestamp(2),
            ),
            folder,
            Abbreviation::new(abbr).expect("abbreviation"),
            *label,
            body.replace("{logo}", &image),
            None,
        );
        s.kind = SnippetKind::Rich;
        store.put_snippet(&s, Timestamp(2)).expect("snippet");
        store
            .sync_snippet_attachments(s.meta.id, &mut new_id, Timestamp(2))
            .expect("links");
    }
    (root, profile)
}

/// Compares `ours` with the committed file, or writes it with `IAMB_BLESS=1`.
fn check_file(name: &str, ours: &str) {
    let path = dir().join(name);
    if bless() {
        std::fs::write(&path, ours).expect("write");
        return;
    }
    let theirs = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        ours == theirs,
        "docs/paste-fidelity/{name} is stale: rerun with IAMB_BLESS=1 and review the diff"
    );
}

#[test]
fn the_fixture_file_is_current_and_importable() {
    let (root, profile) = library();
    let store = Store::open(root.path()).expect("store");
    let out = root.path().join("fixtures.json");
    transfer::export(&store, profile, &out, Format::Json).expect("export");
    drop(store);
    let text = std::fs::read_to_string(&out).expect("exported");
    check_file("fixtures.json", &text);

    // A fresh library imports it, with the logo.
    let fresh = TempDir::new().expect("temp dir");
    let mut store = Store::open(fresh.path()).expect("store");
    let mut new_id = ids();
    let me = ensure_personal_profile(&mut store, &mut new_id, Timestamp(1)).expect("profile");
    let mut more = {
        let mut n = 0u64;
        move || {
            n += 1;
            let mut b = [0x22_u8; 16];
            b[..8].copy_from_slice(&n.to_be_bytes());
            Id::from_bytes(b)
        }
    };
    let summary = transfer::import(
        &mut store,
        me,
        &dir().join("fixtures.json"),
        Format::Json,
        Timestamp(5),
        &mut more,
        OnConflict::Fail,
    )
    .expect("import");
    assert_eq!(summary.snippets_imported, FIXTURES.len());
    assert_eq!(
        store
            .list_attachments(iamb_store::ListOptions::LIVE)
            .expect("attachments")
            .len(),
        1
    );
}

/// The clipboard content of each fixture, as the service builds it for a paste.
fn clipboard() -> Vec<(String, Vec<ClipboardContent>)> {
    let (root, profile) = library();
    let store = Store::open(root.path()).expect("store");
    let view = build_view(&store, profile, 1).expect("view");
    let settings = view.snapshot.settings().clone();
    let ctx = RenderContext {
        now: LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("date"),
        locale: settings.locale,
        date_format: &settings.date_format,
        time_format: &settings.time_format,
        clipboard: None,
    };
    let mut out = Vec::new();
    for (abbr, _, _) in FIXTURES {
        let snippet = view
            .snippets()
            .find(|s| s.abbreviation.as_str() == *abbr)
            .expect("fixture snippet");
        let resolved = view
            .snapshot
            .compiled(snippet.meta.id)
            .expect("compiled")
            .as_ref()
            .expect("resolves");
        let rendered = resolved
            .render_markdown(&ctx, &FormValues::new())
            .expect("renders");
        let mut steps: Vec<InjectionStep> = rendered
            .actions
            .iter()
            .filter_map(|a| match a {
                RenderedAction::Text { text, .. } => {
                    let content = rich::render(text).expect("rich");
                    Some(InjectionStep::Paste(ClipboardContent::rich(
                        content.plain,
                        content.html,
                    )))
                }
                _ => None,
            })
            .collect();
        let missing = iamb_service::rich::prepare_steps(&mut steps, root.path(), profile);
        assert!(missing.is_empty(), "{abbr}: missing {missing:?}");
        let contents = steps
            .into_iter()
            .map(|s| match s {
                InjectionStep::Paste(c) => c,
                other => panic!("{abbr}: {other:?}"),
            })
            .collect();
        out.push(((*abbr).to_owned(), contents));
    }
    out
}

#[test]
fn the_clipboard_content_is_current_and_well_formed() {
    let all = clipboard();
    let mut doc = Vec::new();
    for (abbr, contents) in &all {
        assert_eq!(contents.len(), 1, "{abbr}: one piece of content");
        let c = &contents[0];
        let html = c.html.as_deref().expect("html");
        let plain = c.text.as_deref().expect("plain");
        assert!(!html.contains("attachment:"), "{abbr}: references resolved");
        assert!(
            c.files.is_empty(),
            "{abbr}: images are not offered as files"
        );
        doc.push(json!({ "abbreviation": abbr, "html": html, "plain": plain }));
    }
    let find = |abbr: &str| &all.iter().find(|(a, _)| a == abbr).expect("fixture").1[0];
    // What each fixture must carry (the browser round trip checks the same after a paste).
    assert!(
        find(";fx-head")
            .html
            .as_deref()
            .is_some_and(|h| h.starts_with("<h1>Order update</h1>"))
    );
    assert_eq!(
        find(";fx-bold").html.as_deref(),
        Some("<p>Thanks for your order, <strong>Kim</strong>.</p>")
    );
    assert_eq!(
        find(";fx-link").text.as_deref(),
        Some("Track it on our site (https://example.com/track) or write to orders@example.com.")
    );
    assert_eq!(
        find(";fx-nested").text.as_deref(),
        Some("1. Pack\n   - the box\n   - the tape\n2. Ship\n3. Confirm")
    );
    let field = find(";fx-field");
    assert_eq!(
        field.html.as_deref(),
        Some(
            "<p>Dear <strong>Ada &lt;Lovelace&gt; &amp; *co*</strong>, your <em>blue mug</em> is ready.</p>"
        )
    );
    assert!(
        find(";fx-image")
            .html
            .as_deref()
            .is_some_and(|h| h.contains("<img src=\"data:image/png;base64,iVBORw0KGgo"))
    );
    let mut text = serde_json::to_string_pretty(&Value::Array(doc)).expect("json");
    text.push('\n');
    check_file("clipboard.json", &text);
}
