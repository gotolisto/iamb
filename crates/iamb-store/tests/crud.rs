//! CRUD round trips, soft deletes, purge, uniqueness, folder depth, outbox and usage.

mod common;

use common::{
    Ids, T0, TestResult, attachment, folder, folder_share, group, membership, profile, snippet,
    snippet_attachment, snippet_tag, t, tag,
};
use iamb_core::model::{Id, Permission, Role, SnippetKind, SyncMeta, Trigger};
use iamb_store::{
    AppExclusion, EntityKind, ListOptions, MAX_FOLDER_DEPTH, OutboxEntry, OutboxOp, Record, Store,
    StoreError,
};

/// A store with one personal profile and one top-level folder.
fn setup() -> Result<(Store, Ids, Id, Id), StoreError> {
    let mut store = Store::open_in_memory()?;
    let mut ids = Ids::default();
    let user = ids.next();
    let p = ids.next();
    let f = ids.next();
    store.put_profile(&profile(p, user), T0)?;
    store.put_folder(&folder(f, p, None), T0)?;
    Ok((store, ids, p, f))
}

/// Puts `value` at `t(1)`, reads it back and checks equality, then updates it at `t(2)` and
/// checks that `created_at` is kept and `updated_at` bumped.
fn round_trip<E>(store: &mut Store, mut value: E, edit: impl FnOnce(&mut E)) -> TestResult
where
    E: iamb_store::Entity + PartialEq + std::fmt::Debug,
    E: HasMeta,
{
    let meta = store.put(&value, t(1))?;
    *value.meta_mut() = meta.clone();
    assert_eq!(meta.created_at, t(1));
    assert_eq!(meta.updated_at, t(1));
    assert_eq!(meta.rev, 0);
    let got: Option<E> = store.get(meta.id)?;
    assert_eq!(got.as_ref(), Some(&value));

    edit(&mut value);
    let meta2 = store.put(&value, t(2))?;
    assert_eq!(meta2.created_at, t(1));
    assert_eq!(meta2.updated_at, t(2));
    *value.meta_mut() = meta2;
    let got: Option<E> = store.get(meta.id)?;
    assert_eq!(got.as_ref(), Some(&value));
    assert!(store.list::<E>(ListOptions::LIVE)?.contains(&value));

    store.delete::<E>(meta.id, t(3))?;
    assert_eq!(store.get::<E>(meta.id)?, None);
    assert!(
        !store
            .list::<E>(ListOptions::LIVE)?
            .iter()
            .any(|v| v.meta().id == meta.id)
    );
    let deleted = store.get_any::<E>(meta.id)?.ok_or("deleted row vanished")?;
    assert_eq!(deleted.meta().deleted_at, Some(t(3)));
    assert_eq!(deleted.meta().updated_at, t(3));
    assert!(
        store
            .list::<E>(ListOptions::ALL)?
            .iter()
            .any(|v| v.meta().id == meta.id)
    );
    Ok(())
}

trait HasMeta {
    fn meta_mut(&mut self) -> &mut SyncMeta;
}

macro_rules! has_meta {
    ($($ty:ty),*) => { $(impl HasMeta for $ty { fn meta_mut(&mut self) -> &mut SyncMeta { &mut self.meta } })* };
}
has_meta!(
    iamb_core::model::Profile,
    iamb_core::model::Membership,
    iamb_core::model::Folder,
    iamb_core::model::FolderShare,
    iamb_core::model::Snippet,
    iamb_core::model::Tag,
    iamb_core::model::SnippetTag,
    iamb_core::model::Attachment,
    iamb_core::model::SnippetAttachment
);

#[test]
fn round_trip_every_entity() -> TestResult {
    let (mut store, mut ids, prof, fold) = setup()?;
    let user = ids.next();
    let grp = ids.next();
    store.put_profile(&group(grp, user), T0)?;

    round_trip(&mut store, profile(ids.next(), user), |v| {
        v.name = "Renamed".into();
    })?;
    round_trip(&mut store, membership(ids.next(), grp, user), |v| {
        v.role = Role::Admin;
    })?;
    let mut sub = folder(ids.next(), prof, Some(fold));
    sub.defaults.trigger = Some(Trigger::Immediate);
    sub.defaults.abbreviation_prefix = Some(";".into());
    sub.member_permission = Some(Permission::Edit);
    round_trip(&mut store, sub, |v| {
        v.name = "Moved".into();
        v.parent_id = None;
        v.position = 7;
    })?;
    round_trip(&mut store, folder_share(ids.next(), fold, grp), |v| {
        v.permission = Permission::Edit;
    })?;
    round_trip(&mut store, snippet(ids.next(), fold, "sig"), |v| {
        v.body = "new body".into();
        v.kind = SnippetKind::Rich;
        v.enabled = false;
    })?;
    round_trip(&mut store, tag(ids.next(), prof, "work"), |v| {
        v.color = None;
    })?;

    let snip = ids.next();
    store.put_snippet(&snippet(snip, fold, "addr"), T0)?;
    let tg = ids.next();
    store.put_tag(&tag(tg, prof, "home"), T0)?;
    round_trip(&mut store, snippet_tag(ids.next(), snip, tg), |v| {
        v.meta.updated_by = Some(user);
    })?;
    round_trip(&mut store, attachment(ids.next(), prof), |v| {
        v.file_name = "other.png".into();
    })?;
    let att = ids.next();
    store.put_attachment(&attachment(att, prof), T0)?;
    round_trip(&mut store, snippet_attachment(ids.next(), snip, att), |v| {
        v.meta.updated_by = Some(user);
    })?;
    Ok(())
}

#[test]
fn named_methods_match_generic_ones() -> TestResult {
    let (mut store, mut ids, _p, f) = setup()?;
    let s = ids.next();
    store.put_snippet(&snippet(s, f, "hello"), t(5))?;
    assert_eq!(store.get_snippet(s)?, store.get(s)?);
    assert_eq!(store.list_snippets(ListOptions::LIVE)?.len(), 1);
    store.delete_snippet(s, t(6))?;
    assert!(store.list_snippets(ListOptions::LIVE)?.is_empty());
    assert_eq!(store.list_snippets(ListOptions::ALL)?.len(), 1);
    // Deleting again changes nothing and writes no outbox entry.
    let before = store.outbox_len()?;
    store.delete_snippet(s, t(7))?;
    assert_eq!(store.outbox_len()?, before);
    assert!(matches!(
        store.delete_snippet(ids.next(), t(8)),
        Err(StoreError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn put_restores_a_soft_deleted_row() -> TestResult {
    let (mut store, mut ids, _p, f) = setup()?;
    let s = ids.next();
    store.put_snippet(&snippet(s, f, "x1"), t(1))?;
    store.delete_snippet(s, t(2))?;
    let meta = store.put_snippet(&snippet(s, f, "x1"), t(3))?;
    assert_eq!(meta.deleted_at, None);
    assert_eq!(meta.created_at, t(1));
    assert!(store.get_snippet(s)?.is_some());
    Ok(())
}

#[test]
fn references_must_exist_and_be_live() -> TestResult {
    let (mut store, mut ids, p, f) = setup()?;
    let missing = ids.next();
    let err = store.put_snippet(&snippet(ids.next(), missing, "a"), T0);
    assert!(
        matches!(err, Err(StoreError::NotFound { entity: EntityKind::Folder, id }) if id == missing)
    );
    let err = store.put_tag(&tag(ids.next(), missing, "a"), T0);
    assert!(matches!(
        err,
        Err(StoreError::NotFound {
            entity: EntityKind::Profile,
            ..
        })
    ));
    store.delete_folder(f, t(1))?;
    let err = store.put_snippet(&snippet(ids.next(), f, "a"), t(2));
    assert!(matches!(
        err,
        Err(StoreError::NotFound {
            entity: EntityKind::Folder,
            ..
        })
    ));
    // A sub-folder must be in its parent's profile.
    let user = ids.next();
    let other = ids.next();
    store.put_profile(&group(other, user), T0)?;
    let top = ids.next();
    store.put_folder(&folder(top, p, None), T0)?;
    let err = store.put_folder(&folder(ids.next(), other, Some(top)), T0);
    assert!(matches!(err, Err(StoreError::InvalidData(_))));
    Ok(())
}

#[test]
fn folder_delete_cascades_to_subfolders_and_snippets() -> TestResult {
    let (mut store, mut ids, p, f) = setup()?;
    let sub = ids.next();
    let subsub = ids.next();
    let sibling = ids.next();
    store.put_folder(&folder(sub, p, Some(f)), T0)?;
    store.put_folder(&folder(subsub, p, Some(sub)), T0)?;
    store.put_folder(&folder(sibling, p, None), T0)?;
    let (s1, s2, s3, keep) = (ids.next(), ids.next(), ids.next(), ids.next());
    store.put_snippet(&snippet(s1, f, "s1"), T0)?;
    store.put_snippet(&snippet(s2, sub, "s2"), T0)?;
    store.put_snippet(&snippet(s3, subsub, "s3"), T0)?;
    store.put_snippet(&snippet(keep, sibling, "keep"), T0)?;
    let acked: Vec<i64> = store.outbox_read(100)?.iter().map(|e| e.seq).collect();
    store.outbox_ack(&acked)?;

    store.delete_folder(f, t(10))?;

    let lib = store.load_library()?;
    let folder_ids: Vec<Id> = lib.folders.iter().map(|x| x.meta.id).collect();
    let snippet_ids: Vec<Id> = lib.snippets.iter().map(|x| x.meta.id).collect();
    assert_eq!(folder_ids, vec![sibling]);
    assert_eq!(snippet_ids, vec![keep]);
    for id in [s1, s2, s3] {
        let s = store
            .get_any::<iamb_core::model::Snippet>(id)?
            .ok_or("missing")?;
        assert_eq!(s.meta.deleted_at, Some(t(10)));
    }
    let outbox = store.outbox_read(100)?;
    assert_eq!(outbox.len(), 6, "{outbox:?}");
    assert!(
        outbox
            .iter()
            .all(|e| e.op == OutboxOp::Delete && e.created_at == t(10))
    );
    assert_eq!(outbox[0].row_id, f);
    assert_eq!(
        outbox
            .iter()
            .filter(|e| e.entity == EntityKind::Folder)
            .count(),
        3
    );
    assert_eq!(
        outbox
            .iter()
            .filter(|e| e.entity == EntityKind::Snippet)
            .count(),
        3
    );
    Ok(())
}

#[test]
fn purge_removes_only_rows_deleted_before_the_cutoff() -> TestResult {
    let (mut store, mut ids, p, f) = setup()?;
    let sub = ids.next();
    store.put_folder(&folder(sub, p, Some(f)), T0)?;
    let (old, recent, live) = (ids.next(), ids.next(), ids.next());
    store.put_snippet(&snippet(old, sub, "old"), T0)?;
    store.put_snippet(&snippet(recent, f, "recent"), T0)?;
    store.put_snippet(&snippet(live, f, "live"), T0)?;
    let tg = ids.next();
    store.put_tag(&tag(tg, p, "t"), T0)?;
    let link = ids.next();
    store.put_snippet_tag(&snippet_tag(link, old, tg), T0)?;
    store.record_expansion(old, 3, t(1))?;

    store.delete_folder(sub, t(100))?; // deletes `old` too
    store.delete_snippet(recent, t(300))?;
    let outbox_before = store.outbox_len()?;

    let removed = store.purge_deleted_before(t(200))?;
    assert_eq!(removed, 2, "folder `sub` and snippet `old`");
    assert!(store.get_any::<iamb_core::model::Folder>(sub)?.is_none());
    assert!(store.get_any::<iamb_core::model::Snippet>(old)?.is_none());
    assert!(
        store
            .get_any::<iamb_core::model::SnippetTag>(link)?
            .is_none(),
        "link cascades"
    );
    assert!(store.usage(old)?.is_none(), "usage cascades");
    assert!(
        store
            .get_any::<iamb_core::model::Snippet>(recent)?
            .is_some()
    );
    assert!(store.get_snippet(live)?.is_some());
    assert!(store.get_tag(tg)?.is_some());
    assert_eq!(
        store.outbox_len()?,
        outbox_before,
        "purge writes no outbox entries"
    );

    assert_eq!(store.purge_deleted_before(t(400))?, 1);
    assert!(
        store
            .get_any::<iamb_core::model::Snippet>(recent)?
            .is_none()
    );
    assert_eq!(store.integrity_check()?, vec!["ok".to_owned()]);
    Ok(())
}

#[test]
fn abbreviations_are_unique_per_profile_under_case_folding() -> TestResult {
    let (mut store, mut ids, p, f) = setup()?;
    let first = ids.next();
    store.put_snippet(&snippet(first, f, "Sig"), T0)?;

    // Same abbreviation, different case, same profile (even another folder): rejected.
    let f2 = ids.next();
    store.put_folder(&folder(f2, p, None), T0)?;
    let err = store.put_snippet(&snippet(ids.next(), f2, "sIG"), T0);
    match err {
        Err(StoreError::AbbreviationConflict { key, existing }) => {
            assert_eq!(key, "sig");
            assert_eq!(existing, first);
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    // Non-ASCII folding.
    store.put_snippet(&snippet(ids.next(), f, "ÄRGER"), T0)?;
    assert!(matches!(
        store.put_snippet(&snippet(ids.next(), f, "ärger"), T0),
        Err(StoreError::AbbreviationConflict { .. })
    ));
    // Updating the holder itself is fine.
    let mut again = snippet(first, f, "SIG");
    again.body = "changed".into();
    store.put_snippet(&again, t(1))?;

    // Same abbreviation in a different profile: allowed.
    let user = ids.next();
    let other = ids.next();
    store.put_profile(&group(other, user), T0)?;
    let of = ids.next();
    store.put_folder(&folder(of, other, None), T0)?;
    store.put_snippet(&snippet(ids.next(), of, "sig"), T0)?;

    // After the holder is soft-deleted the abbreviation is free again.
    store.delete_snippet(first, t(2))?;
    let reuse = ids.next();
    store.put_snippet(&snippet(reuse, f2, "SIG"), t(3))?;
    // ...and restoring the old one now conflicts.
    assert!(matches!(
        store.put_snippet(&snippet(first, f, "Sig"), t(4)),
        Err(StoreError::AbbreviationConflict { existing, .. }) if existing == reuse
    ));
    Ok(())
}

#[test]
fn moving_a_folder_to_another_profile_checks_abbreviations() -> TestResult {
    let (mut store, mut ids, _p, f) = setup()?;
    store.put_snippet(&snippet(ids.next(), f, "dup"), T0)?;
    let user = ids.next();
    let other = ids.next();
    store.put_profile(&group(other, user), T0)?;
    let of = ids.next();
    store.put_folder(&folder(of, other, None), T0)?;
    let holder = ids.next();
    store.put_snippet(&snippet(holder, of, "DUP"), T0)?;
    let mut moved = folder(f, other, None);
    moved.name = "moved".into();
    assert!(matches!(
        store.put_folder(&moved, t(1)),
        Err(StoreError::AbbreviationConflict { existing, .. }) if existing == holder
    ));
    store.delete_snippet(holder, t(2))?;
    store.put_folder(&moved, t(3))?;
    // The snippet now counts against the new profile.
    assert!(matches!(
        store.put_snippet(&snippet(ids.next(), of, "Dup"), t(4)),
        Err(StoreError::AbbreviationConflict { .. })
    ));
    Ok(())
}

#[test]
fn folders_nest_at_most_four_levels() -> TestResult {
    let (mut store, mut ids, p, f1) = setup()?;
    assert_eq!(MAX_FOLDER_DEPTH, 4);
    let f2 = ids.next();
    let f3 = ids.next();
    let f4 = ids.next();
    store.put_folder(&folder(f2, p, Some(f1)), T0)?;
    store.put_folder(&folder(f3, p, Some(f2)), T0)?;
    store.put_folder(&folder(f4, p, Some(f3)), T0)?;
    let f5 = ids.next();
    match store.put_folder(&folder(f5, p, Some(f4)), T0) {
        Err(StoreError::FolderTooDeep { folder, depth }) => {
            assert_eq!(folder, f5);
            assert_eq!(depth, 5);
        }
        other => panic!("expected FolderTooDeep, got {other:?}"),
    }

    // Moving a two-level subtree under level 3 would make level 5.
    let a = ids.next();
    let b = ids.next();
    store.put_folder(&folder(a, p, None), T0)?;
    store.put_folder(&folder(b, p, Some(a)), T0)?;
    assert!(matches!(
        store.put_folder(&folder(a, p, Some(f3)), t(1)),
        Err(StoreError::FolderTooDeep { depth: 5, .. })
    ));
    // Under level 2 it fits (a = 3, b = 4).
    store.put_folder(&folder(a, p, Some(f2)), t(2))?;

    // A folder cannot be moved under its own descendant.
    assert!(matches!(
        store.put_folder(&folder(f1, p, Some(f3)), t(3)),
        Err(StoreError::InvalidData(_))
    ));
    assert!(matches!(
        store.put_folder(&folder(f1, p, Some(f1)), t(3)),
        Err(StoreError::InvalidData(_))
    ));
    Ok(())
}

#[test]
fn outbox_records_every_change_in_order() -> TestResult {
    let (mut store, mut ids, _p, f) = setup()?;
    let setup_entries = store.outbox_read(100)?;
    assert_eq!(setup_entries.len(), 2, "profile and folder");
    assert!(
        setup_entries
            .iter()
            .all(|e| e.op == OutboxOp::Put && e.base_rev == 0)
    );
    store.outbox_ack(&setup_entries.iter().map(|e| e.seq).collect::<Vec<_>>())?;
    assert_eq!(store.outbox_len()?, 0);

    let s = ids.next();
    store.put_snippet(&snippet(s, f, "o1"), t(1))?;
    store.put_snippet(&snippet(s, f, "o1"), t(2))?;
    // The server assigns revision 7 to the snippet.
    let mut synced = store.get_snippet(s)?.ok_or("missing")?;
    synced.meta.rev = 7;
    store.apply_remote(&[Record::Snippet(synced.clone())])?;
    assert_eq!(store.outbox_len()?, 2, "apply_remote adds no entries");
    store.put_snippet(&synced, t(3))?;
    store.delete_snippet(s, t(4))?;

    let entries = store.outbox_read(100)?;
    let shape: Vec<(EntityKind, Id, OutboxOp, u64, i64)> = entries
        .iter()
        .map(|e: &OutboxEntry| (e.entity, e.row_id, e.op, e.base_rev, e.created_at.0))
        .collect();
    assert_eq!(
        shape,
        vec![
            (EntityKind::Snippet, s, OutboxOp::Put, 0, t(1).0),
            (EntityKind::Snippet, s, OutboxOp::Put, 0, t(2).0),
            (EntityKind::Snippet, s, OutboxOp::Put, 7, t(3).0),
            (EntityKind::Snippet, s, OutboxOp::Delete, 7, t(4).0),
        ]
    );
    assert!(entries.windows(2).all(|w| w[0].seq < w[1].seq));
    // A local put keeps the server revision.
    assert_eq!(
        store
            .get_any::<iamb_core::model::Snippet>(s)?
            .ok_or("missing")?
            .meta
            .rev,
        7
    );

    // `limit` and ack.
    let first_two = store.outbox_read(2)?;
    assert_eq!(first_two, entries[..2].to_vec());
    assert_eq!(
        store.outbox_ack(&[first_two[0].seq, first_two[1].seq, 999_999])?,
        2
    );
    assert_eq!(store.outbox_read(100)?, entries[2..].to_vec());
    Ok(())
}

#[test]
fn a_failed_write_leaves_no_outbox_entry() -> TestResult {
    let (mut store, mut ids, _p, f) = setup()?;
    store.put_snippet(&snippet(ids.next(), f, "same"), T0)?;
    let before = store.outbox_len()?;
    assert!(
        store
            .put_snippet(&snippet(ids.next(), f, "SAME"), T0)
            .is_err()
    );
    assert_eq!(store.outbox_len()?, before);
    // A batch rolls back as a whole.
    let result = store.write(|w| {
        w.put_snippet(&snippet(ids.next(), f, "ok1"), T0)?;
        w.put_snippet(&snippet(ids.next(), f, "same"), T0)?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(store.outbox_len()?, before);
    assert_eq!(store.list_snippets(ListOptions::LIVE)?.len(), 1);
    Ok(())
}

#[test]
fn apply_remote_writes_rows_as_given_in_any_order() -> TestResult {
    let mut store = Store::open_in_memory()?;
    let mut ids = Ids::default();
    let user = ids.next();
    let (p, f, s, tg, link) = (ids.next(), ids.next(), ids.next(), ids.next(), ids.next());
    let mut prof = profile(p, user);
    prof.meta.rev = 1;
    let mut fold = folder(f, p, None);
    fold.meta.rev = 2;
    let mut snip = snippet(s, f, "remote");
    snip.meta = SyncMeta {
        id: s,
        created_at: t(-50),
        updated_at: t(-10),
        updated_by: Some(user),
        rev: 3,
        deleted_at: None,
    };
    let mut tg_row = tag(tg, p, "r");
    tg_row.meta.rev = 4;
    let mut link_row = snippet_tag(link, s, tg);
    link_row.meta.rev = 5;
    // Children before parents on purpose.
    store.apply_remote(&[
        Record::SnippetTag(link_row.clone()),
        Record::Snippet(snip.clone()),
        Record::Tag(tg_row.clone()),
        Record::Folder(fold.clone()),
        Record::Profile(prof.clone()),
    ])?;
    assert_eq!(store.outbox_len()?, 0);
    assert_eq!(store.get_snippet(s)?, Some(snip.clone()));
    assert_eq!(store.get_folder(f)?, Some(fold));
    assert_eq!(store.get_snippet_tag(link)?, Some(link_row));

    // A remote delete is applied as given, still without outbox entries.
    let mut gone = snip;
    gone.meta.deleted_at = Some(t(5));
    gone.meta.rev = 6;
    store.apply_remote(&[Record::Snippet(gone.clone())])?;
    assert_eq!(store.get_snippet(s)?, None);
    assert_eq!(store.get_any::<iamb_core::model::Snippet>(s)?, Some(gone));
    assert_eq!(store.outbox_len()?, 0);

    // A broken reference fails the whole batch.
    let orphan = snippet_tag(ids.next(), ids.next(), tg);
    assert!(
        store
            .apply_remote(&[Record::SnippetTag(orphan.clone())])
            .is_err()
    );
    assert_eq!(
        store.get_any::<iamb_core::model::SnippetTag>(orphan.meta.id)?,
        None
    );
    Ok(())
}

#[test]
fn usage_does_not_touch_snippets_or_outbox() -> TestResult {
    let (mut store, mut ids, _p, f) = setup()?;
    let s = ids.next();
    store.put_snippet(&snippet(s, f, "u"), t(1))?;
    let before_row = store.get_snippet(s)?;
    let before_outbox = store.outbox_read(100)?;
    assert_eq!(store.usage(s)?, None);

    store.record_expansion(s, 10, t(5))?;
    store.record_expansion(s, 15, t(9))?;
    store.record_expansion(s, 1, t(7))?; // out of order: last_used stays at t(9)

    let u = store.usage(s)?.ok_or("no usage")?;
    assert_eq!((u.count, u.chars_saved, u.last_used), (3, 26, Some(t(9))));
    assert_eq!(store.list_usage()?, vec![u]);
    assert_eq!(store.get_snippet(s)?, before_row);
    assert_eq!(store.outbox_read(100)?, before_outbox);
    assert!(matches!(
        store.record_expansion(ids.next(), 1, t(1)),
        Err(StoreError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn settings_exclusions_and_device_state() -> TestResult {
    let (mut store, mut ids, _p, f) = setup()?;
    let outbox = store.outbox_len()?;
    assert_eq!(store.get_setting("delimiters")?, None);
    store.put_setting("delimiters", " .,")?;
    store.put_setting("delimiters", " .,;")?;
    store.put_setting("locale", "en-GB")?;
    assert_eq!(store.get_setting("delimiters")?.as_deref(), Some(" .,;"));
    assert_eq!(store.list_settings()?.len(), 2);
    assert!(store.delete_setting("locale")?);
    assert!(!store.delete_setting("locale")?);

    let ex = AppExclusion {
        app_id: "com.example.terminal".into(),
        display_name: "Terminal".into(),
        created_at: t(1),
    };
    store.put_app_exclusion(&ex)?;
    store.put_app_exclusion(&AppExclusion {
        app_id: "a.first".into(),
        display_name: "First".into(),
        created_at: t(2),
    })?;
    assert_eq!(
        store.get_app_exclusion("com.example.terminal")?,
        Some(ex.clone())
    );
    let renamed = AppExclusion {
        display_name: "Term".into(),
        ..ex.clone()
    };
    store.put_app_exclusion(&renamed)?;
    let all = store.list_app_exclusions()?;
    assert_eq!(
        all.iter().map(|e| e.app_id.as_str()).collect::<Vec<_>>(),
        ["a.first", "com.example.terminal"]
    );
    assert_eq!(all[1], renamed);
    assert!(store.delete_app_exclusion("a.first")?);
    assert_eq!(store.list_app_exclusions()?.len(), 1);
    assert!(
        store
            .put_app_exclusion(&AppExclusion {
                app_id: String::new(),
                ..ex
            })
            .is_err()
    );

    let s = ids.next();
    store.put_snippet(&snippet(s, f, "appr"), T0)?;
    let outbox = outbox + 1;
    assert_eq!(store.approved_rev(s)?, None);
    store.put_approval(s, 4, t(3))?;
    assert_eq!(store.approved_rev(s)?, Some(4));

    assert_eq!(store.sync_position()?, None);
    store.put_sync_position("feed:42", t(4))?;
    store.put_sync_position("feed:43", t(5))?;
    assert_eq!(store.sync_position()?.as_deref(), Some("feed:43"));

    let mut features = std::collections::BTreeMap::new();
    features.insert("sync".to_owned(), "true".to_owned());
    store.put_entitlements(&features, t(6))?;
    assert_eq!(store.entitlements()?, features);
    store.put_entitlements(&std::collections::BTreeMap::new(), t(7))?;
    assert!(store.entitlements()?.is_empty());

    assert_eq!(
        store.outbox_len()?,
        outbox,
        "device-local writes are never in the outbox"
    );
    Ok(())
}

#[test]
fn load_library_excludes_deleted_rows_and_dangling_links() -> TestResult {
    let (mut store, mut ids, p, f) = setup()?;
    let (s1, s2, t1, t2) = (ids.next(), ids.next(), ids.next(), ids.next());
    store.put_snippet(&snippet(s1, f, "one"), T0)?;
    store.put_snippet(&snippet(s2, f, "two"), T0)?;
    store.put_tag(&tag(t1, p, "a"), T0)?;
    store.put_tag(&tag(t2, p, "b"), T0)?;
    let (l1, l2, l3) = (ids.next(), ids.next(), ids.next());
    store.put_snippet_tag(&snippet_tag(l1, s1, t1), T0)?;
    store.put_snippet_tag(&snippet_tag(l2, s2, t1), T0)?;
    store.put_snippet_tag(&snippet_tag(l3, s1, t2), T0)?;
    store.delete_snippet(s2, t(1))?;
    store.delete_tag(t2, t(1))?;
    let lib = store.load_library()?;
    assert_eq!(lib.folders.len(), 1);
    assert_eq!(
        lib.snippets.iter().map(|s| s.meta.id).collect::<Vec<_>>(),
        vec![s1]
    );
    assert_eq!(
        lib.tags.iter().map(|t| t.meta.id).collect::<Vec<_>>(),
        vec![t1]
    );
    assert_eq!(
        lib.snippet_tags
            .iter()
            .map(|l| l.meta.id)
            .collect::<Vec<_>>(),
        vec![l1]
    );
    Ok(())
}
