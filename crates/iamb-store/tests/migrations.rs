//! Migration from every earlier schema, and refusal of a newer one.
//!
//! For every prefix of the migration list (including the empty one), the test builds a database
//! the way an older release would have left it: after each migration `k` it inserts fixture
//! `k`, written against schema `k`. It then opens the database with the current code and runs
//! every fixture's check. Adding a migration therefore requires adding a fixture here (enforced
//! by `every_migration_has_a_fixture`).

mod common;

use std::path::Path;

use common::{TestResult, temp_dir};
use iamb_core::model::{
    CaseMode, Id, Permission, ProfileKind, Role, SnippetKind, Timestamp, Trigger,
};
use iamb_store::migrations::{LATEST_VERSION, MIGRATIONS, migrate_to};
use iamb_store::{EntityKind, ListOptions, OutboxOp, Store, StoreError};
use rusqlite::Connection;

struct Fixture {
    /// Schema version the SQL is written against.
    version: u32,
    /// Rows to insert right after migration `version`.
    sql: &'static str,
    /// Checks, through the current API, that the rows survived every later migration.
    check: fn(&Store) -> TestResult,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        version: 1,
        sql: include_str!("fixtures/v0001.sql"),
        check: check_v1,
    },
    Fixture {
        version: 2,
        sql: include_str!("fixtures/v0002.sql"),
        check: check_v2,
    },
];

/// `X'01900000000070008000000000000bNN'` from the version 2 fixture.
fn fid2(n: u8) -> Id {
    let mut b = fid(n).as_bytes().to_owned();
    b[14] = 0x0b;
    Id::from_bytes(b)
}

fn check_v2(store: &Store) -> TestResult {
    let road = store.get_snippet(fid2(0x20))?.ok_or("version 2 snippet")?;
    assert_eq!(road.abbreviation.as_str(), "ΔΡΟΜΟΣ");
    assert_eq!(road.abbreviation.fold_key(), "δρομοσ");
    Ok(())
}

/// Reads the stored folding key of a snippet row directly.
fn stored_key(conn: &Connection, n: u8) -> Result<String, rusqlite::Error> {
    conn.query_row(
        "SELECT abbreviation_key FROM snippet WHERE id = ?1",
        [fid(n).as_bytes().as_slice()],
        |r| r.get(0),
    )
}

#[test]
fn version_2_rewrites_final_sigma_keys() -> TestResult {
    let root = temp_dir();
    let path = root.path().join(iamb_store::LIBRARY_FILE);
    build_at(&path, 1)?;
    {
        // Rows as version 1 stored them: `str::to_lowercase` keeps the final form ς.
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "INSERT INTO snippet (id, created_at, updated_at, updated_by, rev, deleted_at, folder_id,
                                  profile_id, abbreviation, abbreviation_key, label, kind, body,
                                  trigger_mode, case_mode, word_start, enabled)
             VALUES
             (X'01900000000070008000000000000a50', 1, 1, NULL, 0, NULL, X'01900000000070008000000000000a10',
              X'01900000000070008000000000000a01', 'ΟΔΟΣ', 'οδος', '', 'plain', 'a', 'delimiter',
              'insensitive', 1, 1),
             (X'01900000000070008000000000000a51', 1, 1, NULL, 0, NULL, X'01900000000070008000000000000a10',
              X'01900000000070008000000000000a01', 'ΟΣ', 'ος', '', 'plain', 'b', 'delimiter',
              'insensitive', 1, 1),
             (X'01900000000070008000000000000a52', 1, 1, NULL, 0, NULL, X'01900000000070008000000000000a10',
              X'01900000000070008000000000000a01', 'οσ', 'οσ', '', 'plain', 'c', 'delimiter',
              'insensitive', 1, 1),
             (X'01900000000070008000000000000a53', 1, 1, NULL, 0, 5, X'01900000000070008000000000000a10',
              X'01900000000070008000000000000a01', 'ΚΟΣ', 'κος', '', 'plain', 'd', 'delimiter',
              'insensitive', 1, 1);",
        )?;
    }
    let mut store = Store::open(root.path())?;
    drop(store);
    let conn = Connection::open(&path)?;
    assert_eq!(stored_key(&conn, 0x50)?, "οδοσ", "live row is rewritten");
    assert_eq!(stored_key(&conn, 0x53)?, "κοσ", "deleted row is rewritten");
    // `ος` and `οσ` now fold alike; the older key stays so the uniqueness index holds.
    assert_eq!(stored_key(&conn, 0x51)?, "ος");
    assert_eq!(stored_key(&conn, 0x52)?, "οσ");
    drop(conn);

    // The rewritten key now blocks the other spelling, as the matcher would treat them as one.
    store = Store::open(root.path())?;
    let clash = common::snippet(common::id_from(900), fid(0x10), "οδοσ");
    match store.put(&clash, Timestamp(10)) {
        Err(StoreError::AbbreviationConflict { existing, .. }) => assert_eq!(existing, fid(0x50)),
        other => return Err(format!("expected a conflict, got {other:?}").into()),
    }
    Ok(())
}

/// `X'01900000000070008000000000000aNN'` from the fixture files.
fn fid(n: u8) -> Id {
    let mut b = [0u8; 16];
    b[0] = 0x01;
    b[1] = 0x90;
    b[6] = 0x70;
    b[8] = 0x80;
    b[14] = 0x0a;
    b[15] = n;
    Id::from_bytes(b)
}

fn check_v1(store: &Store) -> TestResult {
    let me = store.get_profile(fid(0x01))?.ok_or("profile")?;
    assert_eq!(me.kind, ProfileKind::Personal);
    assert_eq!(me.meta.rev, 3);
    assert_eq!(me.meta.updated_by, Some(fid(0xff)));
    let m = store.get_membership(fid(0x03))?.ok_or("membership")?;
    assert_eq!(m.role, Role::Owner);

    let work = store.get_folder(fid(0x10))?.ok_or("folder")?;
    assert_eq!(work.defaults.trigger, Some(Trigger::Immediate));
    assert_eq!(work.defaults.case_mode, Some(CaseMode::Adaptive));
    assert_eq!(work.defaults.abbreviation_prefix.as_deref(), Some(";"));
    assert_eq!(
        store.get_folder(fid(0x11))?.ok_or("sub-folder")?.parent_id,
        Some(fid(0x10))
    );
    assert!(
        store.get_folder(fid(0x12))?.is_none(),
        "deleted folder stays deleted"
    );
    assert_eq!(
        store
            .get_folder_share(fid(0x13))?
            .ok_or("share")?
            .permission,
        Permission::Edit
    );

    let sig = store.get_snippet(fid(0x20))?.ok_or("snippet")?;
    assert_eq!(sig.abbreviation.as_str(), ";Sig");
    assert_eq!(sig.body, "Best regards,\n{cursor}");
    assert_eq!(
        (sig.kind, sig.trigger, sig.case_mode),
        (SnippetKind::Plain, Trigger::Immediate, CaseMode::Adaptive)
    );
    assert!(!sig.word_start && sig.enabled);
    assert_eq!(sig.meta.rev, 5);
    let old = store
        .get_any::<iamb_core::model::Snippet>(fid(0x21))?
        .ok_or("deleted snippet")?;
    assert_eq!(old.meta.deleted_at, Some(Timestamp(2000)));

    assert_eq!(
        store.get_tag(fid(0x30))?.ok_or("tag")?.color.as_deref(),
        Some("#112233")
    );
    assert!(store.get_snippet_tag(fid(0x31))?.is_some());
    let att = store.get_attachment(fid(0x40))?.ok_or("attachment")?;
    assert_eq!((att.size, att.sha256.to_string()), (42, "ab".repeat(32)));
    assert!(store.get_snippet_attachment(fid(0x41))?.is_some());

    let lib = store.load_library()?;
    assert!(lib.snippets.iter().any(|s| s.meta.id == fid(0x20)));
    assert!(lib.snippet_tags.iter().any(|l| l.meta.id == fid(0x31)));

    let u = store.usage(fid(0x20))?.ok_or("usage")?;
    assert_eq!(
        (u.count, u.chars_saved, u.last_used),
        (12, 340, Some(Timestamp(1700)))
    );
    assert_eq!(store.get_setting("delimiters")?.as_deref(), Some(" .,;"));
    assert!(store.get_app_exclusion("com.example.vault")?.is_some());
    assert_eq!(store.approved_rev(fid(0x20))?, Some(5));
    let outbox = store.outbox_read(10)?;
    assert!(outbox.iter().any(|e| e.entity == EntityKind::Snippet
        && e.row_id == fid(0x20)
        && e.op == OutboxOp::Put
        && e.base_rev == 5));
    assert_eq!(store.sync_position()?.as_deref(), Some("feed:7"));
    assert_eq!(
        store.entitlements()?.get("sync").map(String::as_str),
        Some("true")
    );
    assert!(store.list_folders(ListOptions::ALL)?.len() >= 3);
    Ok(())
}

#[test]
fn every_migration_has_a_fixture() {
    let versions: Vec<u32> = MIGRATIONS.iter().map(|m| m.version).collect();
    let fixtures: Vec<u32> = FIXTURES.iter().map(|f| f.version).collect();
    assert_eq!(
        versions, fixtures,
        "add a fixture (tests/fixtures/vNNNN.sql) for each new migration"
    );
    assert_eq!(versions.last().copied(), Some(LATEST_VERSION));
}

/// Builds a database at `version` with the fixtures of every version up to it.
fn build_at(path: &Path, version: u32) -> TestResult {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    for f in FIXTURES.iter().filter(|f| f.version <= version) {
        assert_eq!(migrate_to(&mut conn, f.version)?, f.version);
        conn.execute_batch(f.sql)?;
    }
    if version == 0 {
        migrate_to(&mut conn, 0)?;
    }
    Ok(())
}

#[test]
fn upgrade_from_every_earlier_schema() -> TestResult {
    for version in 0..=LATEST_VERSION {
        let root = temp_dir();
        build_at(&root.path().join(iamb_store::LIBRARY_FILE), version)?;

        let store = Store::open(root.path())?;
        assert_eq!(
            store.schema_version()?,
            LATEST_VERSION,
            "from version {version}"
        );
        assert_eq!(store.integrity_check()?, vec!["ok".to_owned()]);
        for f in FIXTURES.iter().filter(|f| f.version <= version) {
            (f.check)(&store).map_err(|e| {
                format!("fixture v{} after upgrade from v{version}: {e}", f.version)
            })?;
        }
        drop(store);

        let conn = Connection::open(root.path().join(iamb_store::LIBRARY_FILE))?;
        let broken: i64 =
            conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })?;
        assert_eq!(broken, 0, "foreign keys after upgrade from v{version}");
    }
    Ok(())
}

#[test]
fn reopening_applies_nothing_twice() -> TestResult {
    let root = temp_dir();
    drop(Store::open(root.path())?);
    drop(Store::open(root.path())?);
    let conn = Connection::open(root.path().join(iamb_store::LIBRARY_FILE))?;
    let rows: i64 = conn.query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))?;
    assert_eq!(rows, i64::from(LATEST_VERSION));
    let applied_at: i64 =
        conn.query_row("SELECT min(applied_at) FROM schema_migrations", [], |r| {
            r.get(0)
        })?;
    assert!(
        applied_at > 1_600_000_000_000,
        "applied_at is in milliseconds"
    );
    Ok(())
}

#[test]
fn a_newer_schema_is_refused() -> TestResult {
    let root = temp_dir();
    drop(Store::open(root.path())?);
    let conn = Connection::open(root.path().join(iamb_store::LIBRARY_FILE))?;
    conn.execute(
        "INSERT INTO schema_migrations (version) VALUES (?1)",
        [LATEST_VERSION + 1],
    )?;
    drop(conn);
    match Store::open(root.path()) {
        Err(StoreError::SchemaTooNew { found, supported }) => {
            assert_eq!(found, LATEST_VERSION + 1);
            assert_eq!(supported, LATEST_VERSION);
        }
        other => panic!("expected SchemaTooNew, got {other:?}"),
    }
    // The refusal leaves the database untouched and the lock released.
    let conn = Connection::open(root.path().join(iamb_store::LIBRARY_FILE))?;
    let max: u32 = conn.query_row("SELECT max(version) FROM schema_migrations", [], |r| {
        r.get(0)
    })?;
    assert_eq!(max, LATEST_VERSION + 1);
    assert!(matches!(
        Store::open(root.path()),
        Err(StoreError::SchemaTooNew { .. })
    ));
    Ok(())
}
