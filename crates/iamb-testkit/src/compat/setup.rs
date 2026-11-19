//! `iamb-compat setup`: puts the fixture library into a data directory.

use std::path::Path;

#[cfg(feature = "store")]
pub use with_store::{Imported, import_fixtures, import_latency_fixtures};

#[cfg(feature = "store")]
mod with_store {
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use iamb_core::model::{Id, Profile, ProfileKind, SyncMeta, Timestamp};
    use iamb_store::transfer::{self, Format, OnConflict};
    use iamb_store::{ListOptions, Store};

    use crate::compat::types::FIXTURE_JSON;

    /// What the import did.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Imported {
        /// Snippets written.
        pub imported: usize,
        /// Snippets already there (a second setup).
        pub skipped: usize,
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }

    fn new_id() -> Id {
        let mut random = [0_u8; 10];
        if getrandom::fill(&mut random).is_err() {
            // Not expected; ids only have to differ inside this one store.
            let n = now_ms().to_le_bytes();
            random[..8].copy_from_slice(&n);
        }
        Id::v7(now_ms(), random)
    }

    /// Imports `fixtures/compat.json` into the store in `data_dir` (created if missing), through
    /// `iamb-store`, into the personal profile (created if missing). Importing twice adds nothing.
    ///
    /// The service must not be running on `data_dir`: it holds the store's lock.
    pub fn import_fixtures(data_dir: &Path) -> Result<Imported, String> {
        import_json(data_dir, FIXTURE_JSON, "compat-fixture.json")
    }

    /// Imports the 1,000 snippets of the latency run ([`crate::compat::latency::library`]) as
    /// [`import_fixtures`] imports the matrix's five.
    pub fn import_latency_fixtures(data_dir: &Path) -> Result<Imported, String> {
        let json = crate::compat::latency::library_json()?;
        import_json(data_dir, &json, "latency-fixture.json")
    }

    fn import_json(data_dir: &Path, json: &str, name: &str) -> Result<Imported, String> {
        fs::create_dir_all(data_dir)
            .map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
        let mut store = Store::open_with_timeout(data_dir, Duration::from_secs(3)).map_err(|e| {
            format!(
                "cannot open the store in {} (is the service running on it? stop it first): {e}",
                data_dir.display()
            )
        })?;
        let now = Timestamp(i64::try_from(now_ms()).unwrap_or(i64::MAX));
        let existing = store
            .list_profiles(ListOptions::LIVE)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|p| p.kind == ProfileKind::Personal);
        let profile = if let Some(p) = existing {
            p.meta.id
        } else {
            let id = new_id();
            store
                .put_profile(
                    &Profile {
                        meta: SyncMeta::new(id, now),
                        kind: ProfileKind::Personal,
                        name: "Personal".to_owned(),
                        owner_user_id: new_id(),
                    },
                    now,
                )
                .map_err(|e| e.to_string())?;
            id
        };
        let file = data_dir.join(name);
        fs::write(&file, json).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        let result = transfer::import(
            &mut store,
            profile,
            &file,
            Format::Json,
            now,
            &mut new_id,
            OnConflict::Skip,
        );
        let _ = fs::remove_file(&file);
        let summary = result.map_err(|e| format!("import failed: {e}"))?;
        Ok(Imported {
            imported: summary.snippets_imported,
            skipped: summary.snippets_skipped,
        })
    }
}

/// How to start the service against `data_dir` on this OS, as lines of text.
pub fn start_instructions(data_dir: &Path) -> Vec<String> {
    let dir = data_dir.display();
    if cfg!(windows) {
        vec![
            "Start the service from a normal (not elevated) Command Prompt, in the folder of your build:".to_owned(),
            format!("  target\\release\\iamb-service.exe --supervise --data-dir \"{dir}\""),
            "Check that it answers:".to_owned(),
            format!("  target\\release\\iamb.exe --data-dir \"{dir}\" list"),
        ]
    } else if cfg!(target_os = "macos") {
        vec![
            "Start the service from a terminal, with a development build that has both permissions (Accessibility and Input Monitoring):".to_owned(),
            format!("  target/release/iamb-service --supervise --data-dir \"{dir}\""),
            "Also allow your terminal under System Settings > Privacy & Security > Accessibility, so the harness can type.".to_owned(),
            "Check that it answers:".to_owned(),
            format!("  target/release/iamb --data-dir \"{dir}\" list"),
        ]
    } else {
        vec![
            "This system has no real platform layer. For a trial against the mock platform:"
                .to_owned(),
            format!("  target/debug/iamb-service --supervise --platform mock --data-dir \"{dir}\""),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_name_the_directory() {
        let lines = start_instructions(Path::new("/tmp/x"));
        assert!(lines.iter().any(|l| l.contains("--data-dir")));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("/tmp/x") || l.contains("x\""))
        );
    }
}
