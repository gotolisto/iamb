//! The `run/store.lock` rule (architecture §6.3).

mod common;

use std::thread;
use std::time::{Duration, Instant};

use common::{TestResult, temp_dir};
use iamb_store::{Store, StoreError};

#[test]
fn open_creates_the_layout() -> TestResult {
    let root = temp_dir();
    let dir = root.path().join("nested").join("data");
    let store = Store::open(&dir)?;
    assert!(store.holds_lock());
    assert!(dir.join("run").join("store.lock").is_file());
    assert!(dir.join("library.db").is_file());
    assert_eq!(
        store.schema_version()?,
        iamb_store::migrations::LATEST_VERSION
    );
    assert!(!Store::open_in_memory()?.holds_lock());
    Ok(())
}

#[test]
fn second_open_fails_while_the_first_is_alive() -> TestResult {
    let root = temp_dir();
    let first = Store::open(root.path())?;
    match Store::open(root.path()) {
        Err(StoreError::Locked { path }) => assert!(path.ends_with("store.lock")),
        other => panic!("expected Locked, got {other:?}"),
    }
    let started = Instant::now();
    assert!(matches!(
        Store::open_with_timeout(root.path(), Duration::from_millis(200)),
        Err(StoreError::Locked { .. })
    ));
    assert!(started.elapsed() >= Duration::from_millis(200));
    drop(first);
    let second = Store::open(root.path())?;
    drop(second);
    Ok(())
}

#[test]
fn open_with_timeout_waits_for_the_holder() -> TestResult {
    let root = temp_dir();
    let first = Store::open(root.path())?;
    let releaser = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        drop(first);
    });
    let started = Instant::now();
    let second = Store::open_with_timeout(root.path(), Duration::from_secs(5))?;
    assert!(started.elapsed() >= Duration::from_millis(250));
    releaser.join().map_err(|_| "releaser panicked")?;
    assert!(matches!(
        Store::open(root.path()),
        Err(StoreError::Locked { .. })
    ));
    drop(second);
    Ok(())
}

#[test]
fn data_survives_reopen() -> TestResult {
    let root = temp_dir();
    let mut ids = common::Ids::default();
    let (user, p) = (ids.next(), ids.next());
    {
        let mut store = Store::open(root.path())?;
        store.put_profile(&common::profile(p, user), common::T0)?;
        store.put_setting("k", "v")?;
    }
    let store = Store::open(root.path())?;
    assert!(store.get_profile(p)?.is_some());
    assert_eq!(store.get_setting("k")?.as_deref(), Some("v"));
    Ok(())
}

/// Finding 16: private data directory and files on Unix.
#[cfg(unix)]
#[test]
fn data_directory_and_files_are_private_on_unix() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &std::path::Path) -> std::io::Result<u32> {
        Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
    }

    let root = temp_dir();
    let dir = root.path().join("nested").join("data");
    let mut store = Store::open(&dir)?;
    // Write something so that the WAL and shared-memory files exist.
    store.put_setting("k", "v")?;
    assert_eq!(mode(&dir)?, 0o700, "data directory");
    assert_eq!(mode(&dir.join("run"))?, 0o700, "run directory");
    assert_eq!(mode(&dir.join("library.db"))?, 0o600, "library.db");
    for name in ["library.db-wal", "library.db-shm"] {
        let path = dir.join(name);
        assert!(path.exists(), "{name} should exist while the store is open");
        assert_eq!(mode(&path)?, 0o600, "{name}");
    }
    assert_eq!(mode(&dir.join("run").join("store.lock"))?, 0o600, "lock");
    Ok(())
}

/// Review 1.9 row 16: a data directory and files made by an earlier build with loose modes are
/// tightened when the store is opened: directories to 0700, `library.db`, its WAL and
/// shared-memory files and `store.lock` to 0600. Fails when `Store::open` leaves them alone.
#[cfg(unix)]
#[test]
fn existing_directory_and_files_are_tightened_on_unix() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &std::path::Path) -> std::io::Result<u32> {
        Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
    }
    fn set(path: &std::path::Path, mode: u32) -> std::io::Result<()> {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }

    let root = temp_dir();
    let dir = root.path().join("data");
    let db = dir.join("library.db");
    let wal = dir.join("library.db-wal");
    let shm = dir.join("library.db-shm");
    let lock = dir.join("run").join("store.lock");

    // A first run makes the files; a second connection keeps the WAL and shared-memory files
    // alive after the store is dropped, as another process holding the database would.
    let mut store = Store::open(&dir)?;
    store.put_setting("k", "v")?;
    let keeper = rusqlite::Connection::open(&db)?;
    let _: i64 = keeper.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    drop(store);
    assert!(
        wal.exists() && shm.exists(),
        "WAL and shm outlive the store"
    );

    // As an earlier build (or another tool) would have left them.
    set(&dir, 0o755)?;
    set(&dir.join("run"), 0o755)?;
    for file in [&db, &wal, &shm, &lock] {
        set(file, 0o644)?;
    }

    let store = Store::open(&dir)?;
    assert_eq!(mode(&dir)?, 0o700, "data directory");
    assert_eq!(mode(&dir.join("run"))?, 0o700, "run directory");
    assert_eq!(mode(&db)?, 0o600, "library.db");
    assert_eq!(mode(&wal)?, 0o600, "library.db-wal");
    assert_eq!(mode(&shm)?, 0o600, "library.db-shm");
    assert_eq!(mode(&lock)?, 0o600, "store.lock");
    drop(store);
    drop(keeper);
    Ok(())
}
