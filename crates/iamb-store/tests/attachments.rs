//! Attachments (task 3.8): content-addressed bytes beside the metadata row, checked on the way
//! in and on the way out, linked to the rich snippets that reference them, and carried through
//! a JSON export and import.

mod common;

use common::{Ids, T0, TestResult, folder, profile, snippet, temp_dir};
use iamb_core::model::{Attachment, Id, SnippetAttachment, SnippetKind};
use iamb_core::rich;
use iamb_store::attachments::{self, MAX_IMAGE_BYTES};
use iamb_store::transfer::{self, Format, OnConflict};
use iamb_store::{AttachmentError, ListOptions, Store, StoreError};

/// A tiny valid-looking PNG (only its signature matters to the store).
fn png(tag: u8) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend_from_slice(&[tag; 32]);
    b
}

struct Lib {
    store: Store,
    ids: Ids,
    profile: Id,
    folder: Id,
}

fn library(dir: &std::path::Path, base: u64) -> Result<Lib, Box<dyn std::error::Error>> {
    let mut store = Store::open(dir)?;
    let mut ids = Ids::default();
    for _ in 0..base {
        ids.next();
    }
    let p = ids.next();
    store.put_profile(&profile(p, ids.next()), T0)?;
    let f = ids.next();
    store.put_folder(&folder(f, p, None), T0)?;
    Ok(Lib {
        store,
        ids,
        profile: p,
        folder: f,
    })
}

#[test]
fn bytes_are_content_addressed_and_deduplicated() -> TestResult {
    let dir = temp_dir();
    let mut lib = library(dir.path(), 0)?;
    let bytes = png(1);
    let id = lib.ids.next();
    let a = lib
        .store
        .add_attachment(id, lib.profile, &bytes, "/home/kim/Logo.png", T0)?;
    assert_eq!(a.sha256, attachments::sha256_of(&bytes));
    assert_eq!(a.mime, "image/png");
    assert_eq!(a.size, bytes.len() as u64);
    assert_eq!(a.file_name, "Logo.png");
    let path = lib
        .store
        .attachment_path(lib.profile, &a.sha256)
        .ok_or("path")?;
    assert!(path.ends_with(format!("attachments/{}/{}", lib.profile, a.sha256)));
    assert_eq!(std::fs::read(&path)?, bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600
        );
        let parent = path.parent().ok_or("parent")?;
        assert_eq!(
            std::fs::metadata(parent)?.permissions().mode() & 0o777,
            0o700
        );
    }

    // The same content again gives back the same row.
    let again = lib
        .store
        .add_attachment(lib.ids.next(), lib.profile, &bytes, "other.png", T0)?;
    assert_eq!(again.meta.id, a.meta.id);
    assert_eq!(lib.store.list_attachments(ListOptions::LIVE)?.len(), 1);
    assert_eq!(
        lib.store.read_attachment(lib.profile, &a.sha256)?,
        Some(bytes)
    );
    Ok(())
}

#[test]
fn damaged_or_missing_bytes_read_as_missing() -> TestResult {
    let dir = temp_dir();
    let mut lib = library(dir.path(), 0)?;
    let a = lib
        .store
        .add_attachment(lib.ids.next(), lib.profile, &png(2), "a.png", T0)?;
    let path = lib
        .store
        .attachment_path(lib.profile, &a.sha256)
        .ok_or("path")?;
    std::fs::write(&path, b"\x89PNG\r\n\x1a\ntampered")?;
    assert_eq!(lib.store.read_attachment(lib.profile, &a.sha256)?, None);
    std::fs::remove_file(&path)?;
    assert_eq!(lib.store.read_attachment(lib.profile, &a.sha256)?, None);
    // Adding the content again repairs the file.
    lib.store
        .add_attachment(lib.ids.next(), lib.profile, &png(2), "a.png", T0)?;
    assert!(lib.store.read_attachment(lib.profile, &a.sha256)?.is_some());
    Ok(())
}

#[test]
fn types_and_sizes_are_refused_before_anything_is_written() -> TestResult {
    let dir = temp_dir();
    let mut lib = library(dir.path(), 0)?;
    let svg = b"<svg onload=alert(1)>".to_vec();
    match lib
        .store
        .add_attachment(lib.ids.next(), lib.profile, &svg, "x.svg", T0)
    {
        Err(StoreError::Attachment(AttachmentError::UnsupportedType)) => {}
        other => return Err(format!("expected UnsupportedType, got {other:?}").into()),
    }
    let mut big = png(3);
    big.resize(usize::try_from(MAX_IMAGE_BYTES)? + 1, 0);
    assert!(matches!(
        lib.store
            .add_attachment(lib.ids.next(), lib.profile, &big, "big.png", T0),
        Err(StoreError::Attachment(AttachmentError::TooLarge { .. }))
    ));
    assert!(lib.store.list_attachments(ListOptions::LIVE)?.is_empty());
    assert!(!dir.path().join("attachments").exists());

    let mut memory = Store::open_in_memory()?;
    let mut ids = Ids::default();
    let p = ids.next();
    memory.put_profile(&profile(p, ids.next()), T0)?;
    assert!(matches!(
        memory.add_attachment(ids.next(), p, &png(4), "a.png", T0),
        Err(StoreError::Attachment(AttachmentError::NoDataDirectory))
    ));
    Ok(())
}

#[test]
fn rich_snippets_are_linked_to_the_attachments_they_reference() -> TestResult {
    let dir = temp_dir();
    let mut lib = library(dir.path(), 0)?;
    let a = lib
        .store
        .add_attachment(lib.ids.next(), lib.profile, &png(5), "a.png", T0)?;
    let b = lib
        .store
        .add_attachment(lib.ids.next(), lib.profile, &png(6), "b.png", T0)?;
    let sid = lib.ids.next();
    let mut s = snippet(sid, lib.folder, ";logo");
    s.kind = SnippetKind::Rich;
    s.body = format!(
        "**Hi** {} {}",
        rich::image_markdown("A", &a.sha256),
        rich::image_markdown("B", &b.sha256)
    );
    lib.store.put_snippet(&s, T0)?;
    let mut ids = Ids::default();
    for _ in 0..100 {
        ids.next();
    }
    let mut new_id = || ids.next();
    lib.store.sync_snippet_attachments(sid, &mut new_id, T0)?;
    let linked = |store: &Store| -> Result<Vec<Id>, StoreError> {
        let mut v: Vec<Id> = store
            .list::<SnippetAttachment>(ListOptions::LIVE)?
            .into_iter()
            .filter(|l| l.snippet_id == sid)
            .map(|l| l.attachment_id)
            .collect();
        v.sort();
        Ok(v)
    };
    let mut both = vec![a.meta.id, b.meta.id];
    both.sort();
    assert_eq!(linked(&lib.store)?, both);

    // Syncing again changes nothing; dropping a reference drops its link.
    lib.store.sync_snippet_attachments(sid, &mut new_id, T0)?;
    assert_eq!(
        lib.store.list::<SnippetAttachment>(ListOptions::ALL)?.len(),
        2
    );
    s.body = rich::image_markdown("B", &b.sha256);
    lib.store.put_snippet(&s, T0)?;
    lib.store.sync_snippet_attachments(sid, &mut new_id, T0)?;
    assert_eq!(linked(&lib.store)?, vec![b.meta.id]);

    // A plain snippet has no links, even with the same text.
    s.kind = SnippetKind::Plain;
    lib.store.put_snippet(&s, T0)?;
    lib.store.sync_snippet_attachments(sid, &mut new_id, T0)?;
    assert!(linked(&lib.store)?.is_empty());
    Ok(())
}

#[test]
fn json_export_and_import_carry_the_bytes() -> TestResult {
    let src_dir = temp_dir();
    let mut src = library(src_dir.path(), 0)?;
    let bytes = png(7);
    let a = src
        .store
        .add_attachment(src.ids.next(), src.profile, &bytes, "logo.png", T0)?;
    let mut s = snippet(src.ids.next(), src.folder, ";sigf");
    s.kind = SnippetKind::Rich;
    s.body = format!("Kim\n\n{}", rich::image_markdown("Logo", &a.sha256));
    src.store.put_snippet(&s, T0)?;
    let file = src_dir.path().join("export.json");
    transfer::export(&src.store, src.profile, &file, Format::Json)?;
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&file)?)?;
    let exported = doc["attachments"].as_array().ok_or("attachments array")?;
    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0]["sha256"], a.sha256.to_string());
    assert_eq!(exported[0]["mime"], "image/png");

    let dst_dir = temp_dir();
    let mut dst = library(dst_dir.path(), 500)?;
    let mut ids = Ids::default();
    for _ in 0..1000 {
        ids.next();
    }
    let mut new_id = || ids.next();
    transfer::import(
        &mut dst.store,
        dst.profile,
        &file,
        Format::Json,
        T0,
        &mut new_id,
        OnConflict::Skip,
    )?;
    assert_eq!(
        dst.store.read_attachment(dst.profile, &a.sha256)?,
        Some(bytes)
    );
    let rows: Vec<Attachment> = dst.store.list_attachments(ListOptions::LIVE)?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].profile_id, dst.profile);
    assert_eq!(rows[0].file_name, "logo.png");
    let links = dst.store.list::<SnippetAttachment>(ListOptions::LIVE)?;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attachment_id, rows[0].meta.id);
    Ok(())
}

#[test]
fn an_import_with_a_bad_attachment_writes_nothing() -> TestResult {
    let dir = temp_dir();
    let mut lib = library(dir.path(), 0)?;
    let wrong = attachments::sha256_of(b"something else");
    let doc = serde_json::json!({
        "format": "iamb-library",
        "version": 1,
        "attachments": [{
            "sha256": wrong.to_string(),
            "mime": "image/png",
            "file_name": "a.png",
            "data": "iVBORw0KGgoAAAAA",
        }],
    });
    let file = dir.path().join("bad.json");
    std::fs::write(&file, serde_json::to_vec(&doc)?)?;
    let mut ids = Ids::default();
    let mut new_id = || ids.next();
    let result = transfer::import(
        &mut lib.store,
        lib.profile,
        &file,
        Format::Json,
        T0,
        &mut new_id,
        OnConflict::Skip,
    );
    let message = result.err().ok_or("refused")?.to_string();
    assert!(message.contains("do not match its hash"), "{message}");
    assert!(lib.store.list_attachments(ListOptions::LIVE)?.is_empty());
    Ok(())
}
