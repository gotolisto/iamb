//! What the worker knows about the library at one moment: the immutable [`View`], built by the
//! store thread after every change to library content or settings and shared with every other
//! thread through [`SharedView`] (architecture §3.1: "an atomically swapped pointer").

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use iamb_core::datetime::{Format, Locale};
use iamb_core::engine::{EngineSettings, Snapshot};
use iamb_core::matcher::Delimiters;
use iamb_core::model::{
    Folder, FolderDefaults, Id, Profile, ProfileKind, Snippet, SnippetTag, SyncMeta, Tag, Timestamp,
};
use iamb_store::{AppExclusion, ListOptions, Store, StoreError};

/// Name of the root folder created on first use (the command-line tool uses the same).
pub const ROOT_FOLDER: &str = "Snippets";

/// One immutable state of the library.
#[derive(Debug, Clone)]
pub struct View {
    /// Counts up from 1 at worker start; clients hear it in `LibraryChanged`.
    pub revision: u64,
    /// The personal profile.
    pub profile: Id,
    /// The engine's snapshot: the personal profile's snippets in precedence order.
    pub snapshot: Snapshot,
    /// The snippet ids of the snapshot in precedence order (the snippets themselves are read
    /// from the snapshot, so they are held once).
    pub snippet_ids: Arc<Vec<Id>>,
    /// The personal profile's live folders.
    pub folders: Arc<Vec<Folder>>,
    /// The personal profile's live tags, by name (case-insensitive).
    pub tags: Arc<Vec<Tag>>,
    /// The live links between the snippets of the view and its tags.
    pub links: Arc<Vec<SnippetTag>>,
    /// The tags of each snippet that has any, in the order of [`tags`](Self::tags).
    pub snippet_tags: Arc<BTreeMap<Id, Vec<Id>>>,
    /// Every setting.
    pub settings: Arc<BTreeMap<String, String>>,
    /// The app exclusions.
    pub exclusions: Arc<Vec<AppExclusion>>,
    /// The exclusions in the form [`AppIdRule::key_of`] compares, for [`is_excluded`](Self::is_excluded).
    pub exclusion_keys: Arc<BTreeSet<String>>,
}

impl View {
    /// The snippets, in precedence order.
    pub fn snippets(&self) -> impl Iterator<Item = &Snippet> {
        self.snippet_ids
            .iter()
            .filter_map(|id| self.snapshot.snippet(*id))
    }

    /// The tags of `snippet`, in the order of [`tags`](Self::tags).
    #[must_use]
    pub fn tags_of(&self, snippet: Id) -> &[Id] {
        self.snippet_tags.get(&snippet).map_or(&[], Vec::as_slice)
    }

    /// Whether the focus verdict must say `excluded` for `app`, the application id the focus
    /// inspector reported. Compared in the form of [`AppIdRule::this_os`], so an exclusion
    /// stored in another spelling still applies.
    #[must_use]
    pub fn is_excluded(&self, app: &str) -> bool {
        self.exclusion_keys
            .contains(&AppIdRule::this_os().key_of(app))
    }
}

/// How the focus inspector of an operating system names an application, and so the one form
/// an exclusion is stored and compared in (the 2.8 review's finding 6).
///
/// The focus inspectors report a fixed spelling (`crates/iamb-platform-*/src/focus/identity.rs`
/// on Windows, the bundle identifier on macOS); an exclusion is useful only if it is stored in
/// that spelling, so [`normalise`](Self::normalise) is used when one is written and when the
/// stored ones are read, and a value that can never be reported is refused rather than stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppIdRule {
    /// Windows: the executable's file name in lower case (`mybank.exe`). A path is reduced to
    /// its file name; Windows compares file names without regard to case.
    WindowsExecutable,
    /// macOS: what the inspector reports (`crates/iamb-platform-macos/src/focus/identity.rs`):
    /// the bundle identifier as the application spells it (`com.example.bank`), or, for a
    /// program without one, its executable's file name, which may hold spaces, `+`, letters
    /// outside ASCII and anything else but `/` (`My Tool`, `python3.12`). Compared without
    /// regard to ASCII case, as macOS compares bundle identifiers. Only what neither can be
    /// is refused: nothing, a control character, a `/` (a path, which names neither).
    MacBundle,
    /// Any other system (development and the mock): trimmed and in lower case.
    Plain,
}

/// Why an application id can never be reported by the focus inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidAppId {
    /// Nothing but white space.
    Empty,
    /// A path, where the system reports a bundle identifier.
    Path,
    /// A character that cannot occur in what the system reports.
    Character,
}

impl fmt::Display for InvalidAppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "the app id is empty",
            Self::Path => "the app id is a path; give the application's bundle identifier",
            Self::Character => "the app id has a character no application id of this system has",
        })
    }
}

impl AppIdRule {
    /// The rule of the system this build runs on.
    #[must_use]
    pub const fn this_os() -> Self {
        if cfg!(windows) {
            Self::WindowsExecutable
        } else if cfg!(target_os = "macos") {
            Self::MacBundle
        } else {
            Self::Plain
        }
    }

    /// `raw` in the form the focus inspector reports it.
    ///
    /// # Errors
    ///
    /// [`InvalidAppId`] when no application can ever be reported under `raw`.
    pub fn normalise(self, raw: &str) -> Result<String, InvalidAppId> {
        let raw = raw.trim();
        if raw.chars().any(char::is_control) {
            return Err(InvalidAppId::Character);
        }
        let id = match self {
            Self::WindowsExecutable => {
                let file = raw.rsplit(['\\', '/']).next().unwrap_or(raw).trim();
                // `?` is what the inspector reports for a process it cannot name.
                if file.contains(['<', '>', ':', '"', '|', '?', '*']) {
                    return Err(InvalidAppId::Character);
                }
                file.to_lowercase()
            }
            Self::MacBundle => {
                // The follow-up review's N2: a file name may hold any character but `/`.
                if raw.contains('/') {
                    return Err(InvalidAppId::Path);
                }
                raw.to_ascii_lowercase()
            }
            Self::Plain => raw.to_lowercase(),
        };
        if id.is_empty() {
            Err(InvalidAppId::Empty)
        } else {
            Ok(id)
        }
    }

    /// The form an id the focus inspector reported is compared in. Never matches a stored
    /// exclusion when it cannot be normalised (the Windows inspector's `?`).
    #[must_use]
    pub fn key_of(self, reported: &str) -> String {
        self.normalise(reported)
            .unwrap_or_else(|_| format!("\0{reported}"))
    }
}

/// What [`migrate_exclusions`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExclusionMigration {
    /// Rows rewritten in the form of the rule.
    pub rewritten: usize,
    /// Rows that can never apply, left as they are (the user can still remove them).
    pub unusable: usize,
}

/// Rewrites stored exclusions in the form `rule` compares (worker start). A row whose
/// normalised form is stored already is dropped as a duplicate.
///
/// # Errors
///
/// Store errors.
pub fn migrate_exclusions(
    store: &mut Store,
    rule: AppIdRule,
) -> Result<ExclusionMigration, StoreError> {
    let mut done = ExclusionMigration::default();
    let rows = store.list_app_exclusions()?;
    let stored: BTreeSet<String> = rows.iter().map(|r| r.app_id.clone()).collect();
    for row in rows {
        match rule.normalise(&row.app_id) {
            Ok(key) if key == row.app_id => {}
            Ok(key) => {
                if !stored.contains(&key) && store.get_app_exclusion(&key)?.is_none() {
                    store.put_app_exclusion(&AppExclusion {
                        app_id: key,
                        display_name: row.display_name.clone(),
                        created_at: row.created_at,
                    })?;
                }
                store.delete_app_exclusion(&row.app_id)?;
                done.rewritten += 1;
            }
            Err(_) => done.unusable += 1,
        }
    }
    Ok(done)
}

/// The current [`View`], swapped whole by the store thread. Readers clone the `Arc` and never
/// hold the lock while they work.
#[derive(Debug, Clone)]
pub struct SharedView(Arc<RwLock<Arc<View>>>);

impl SharedView {
    /// A shared view starting at `view`.
    #[must_use]
    pub fn new(view: View) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(view))))
    }

    /// The current view.
    #[must_use]
    pub fn get(&self) -> Arc<View> {
        Arc::clone(&self.0.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Replaces the view.
    pub fn set(&self, view: View) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(view);
    }
}

/// Engine settings from the store's `date_format`, `time_format`, `locale` and `delimiters`
/// settings; defaults where absent (the command-line tool reads them the same way).
///
/// # Errors
///
/// A readable message when a stored value is invalid.
pub fn engine_settings(map: &BTreeMap<String, String>) -> Result<EngineSettings, String> {
    let mut s = EngineSettings::default();
    if let Some(v) = map.get("date_format") {
        s.date_format = Format::parse(v).map_err(|_| "invalid date_format setting".to_owned())?;
    }
    if let Some(v) = map.get("time_format") {
        s.time_format = Format::parse(v).map_err(|_| "invalid time_format setting".to_owned())?;
    }
    if let Some(v) = map.get("locale") {
        s.locale = Locale::from_tag(v);
    }
    if let Some(v) = map.get("delimiters") {
        s.delimiters = Delimiters::new(v.chars());
    }
    Ok(s)
}

/// Makes sure a personal profile and its root folder exist, and returns the profile's id.
///
/// # Errors
///
/// Store errors.
pub fn ensure_personal_profile(
    store: &mut Store,
    new_id: &mut dyn FnMut() -> Id,
    now: Timestamp,
) -> Result<Id, StoreError> {
    let existing = store
        .list_profiles(ListOptions::LIVE)?
        .into_iter()
        .find(|p| p.kind == ProfileKind::Personal);
    let profile = if let Some(p) = existing {
        p.meta.id
    } else {
        let id = new_id();
        store.put_profile(
            &Profile {
                meta: SyncMeta::new(id, now),
                kind: ProfileKind::Personal,
                name: "Personal".to_owned(),
                owner_user_id: new_id(),
            },
            now,
        )?;
        id
    };
    let has_root = store
        .list_folders(ListOptions::LIVE)?
        .iter()
        .any(|f| f.profile_id == profile && f.parent_id.is_none());
    if !has_root {
        let folder = Folder {
            meta: SyncMeta::new(new_id(), now),
            profile_id: profile,
            parent_id: None,
            name: ROOT_FOLDER.to_owned(),
            position: 0,
            member_permission: None,
            defaults: FolderDefaults::default(),
        };
        store.put_folder(&folder, now)?;
    }
    Ok(profile)
}

/// Builds the view of the personal profile: every live snippet in its folders, ordered by
/// creation time then id (the precedence order the command-line tool uses), with the settings
/// and exclusions.
///
/// # Errors
///
/// Store errors; an invalid setting is reported as [`StoreError::InvalidData`].
pub fn build_view(store: &Store, profile: Id, revision: u64) -> Result<View, StoreError> {
    let lib = store.load_library()?;
    let folders: Vec<Folder> = lib
        .folders
        .into_iter()
        .filter(|f| f.profile_id == profile)
        .collect();
    let ids: BTreeSet<Id> = folders.iter().map(|f| f.meta.id).collect();
    let mut snippets: Vec<Snippet> = lib
        .snippets
        .into_iter()
        .filter(|s| ids.contains(&s.folder_id))
        .collect();
    snippets.sort_by_key(|s| (s.meta.created_at, s.meta.id));
    let snippet_ids: Vec<Id> = snippets.iter().map(|s| s.meta.id).collect();
    let mut tags: Vec<Tag> = lib
        .tags
        .into_iter()
        .filter(|t| t.profile_id == profile)
        .collect();
    tags.sort_by_cached_key(|t| (t.name.to_lowercase(), t.meta.id));
    let tag_order: BTreeMap<Id, usize> = tags
        .iter()
        .enumerate()
        .map(|(i, t)| (t.meta.id, i))
        .collect();
    let in_view: BTreeSet<Id> = snippet_ids.iter().copied().collect();
    let links: Vec<SnippetTag> = lib
        .snippet_tags
        .into_iter()
        .filter(|l| in_view.contains(&l.snippet_id) && tag_order.contains_key(&l.tag_id))
        .collect();
    let mut snippet_tags: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
    for link in &links {
        let list = snippet_tags.entry(link.snippet_id).or_default();
        if !list.contains(&link.tag_id) {
            list.push(link.tag_id);
        }
    }
    for list in snippet_tags.values_mut() {
        list.sort_by_key(|t| tag_order.get(t).copied().unwrap_or(usize::MAX));
    }
    let settings = store.list_settings()?;
    let engine = engine_settings(&settings).map_err(StoreError::InvalidData)?;
    let exclusions = store.list_app_exclusions()?;
    let rule = AppIdRule::this_os();
    let exclusion_keys = exclusions
        .iter()
        .filter_map(|e| rule.normalise(&e.app_id).ok())
        .collect();
    Ok(View {
        revision,
        profile,
        snapshot: Snapshot::build(snippets, engine),
        snippet_ids: Arc::new(snippet_ids),
        folders: Arc::new(folders),
        tags: Arc::new(tags),
        links: Arc::new(links),
        snippet_tags: Arc::new(snippet_tags),
        settings: Arc::new(settings),
        exclusions: Arc::new(exclusions),
        exclusion_keys: Arc::new(exclusion_keys),
    })
}

#[cfg(test)]
mod tests {
    use iamb_core::model::Abbreviation;

    use super::*;

    #[test]
    fn a_fresh_store_gets_a_profile_and_a_view() {
        let mut store = Store::open_in_memory().expect("store");
        let mut n = 0_u8;
        let mut new_id = || {
            n += 1;
            Id::from_bytes([n; 16])
        };
        let profile =
            ensure_personal_profile(&mut store, &mut new_id, Timestamp(1)).expect("profile");
        let again = ensure_personal_profile(&mut store, &mut new_id, Timestamp(2)).expect("again");
        assert_eq!(profile, again);
        let view = build_view(&store, profile, 1).expect("view");
        assert_eq!(view.folders.len(), 1);
        let root = view.folders[0].meta.id;
        let s = Snippet::new_defaults(
            SyncMeta::new(Id::from_bytes([50; 16]), Timestamp(3)),
            root,
            Abbreviation::new("brb").expect("abbr"),
            "",
            "be right back",
            None,
        );
        store.put_snippet(&s, Timestamp(3)).expect("put");
        store.put_setting("locale", "de").expect("setting");
        let view = build_view(&store, profile, 2).expect("view");
        assert_eq!(view.snippets().count(), 1);
        assert_eq!(view.snapshot.len(), 1);
        assert_eq!(view.settings.get("locale").map(String::as_str), Some("de"));
        assert!(!view.is_excluded("org.example"));
        let shared = SharedView::new(view);
        assert_eq!(shared.get().revision, 2);
        store.put_setting("date_format", "%Q%").expect("setting");
        assert!(build_view(&store, profile, 3).is_err());
    }

    #[test]
    fn windows_ids_are_lower_case_file_names() {
        let rule = AppIdRule::WindowsExecutable;
        // The same spellings `iamb-platform-windows/src/focus/identity.rs` reduces.
        for (raw, id) in [
            (r"C:\Windows\System32\notepad.exe", "notepad.exe"),
            (r"C:\Program Files\Office\EXCEL.EXE", "excel.exe"),
            ("C:/tools/Editor.exe", "editor.exe"),
            (r"\\?\C:\x\WindowsTerminal.exe", "windowsterminal.exe"),
            ("ÄPFEL.EXE", "äpfel.exe"),
            ("  MyBank.exe ", "mybank.exe"),
            ("no-extension", "no-extension"),
        ] {
            assert_eq!(rule.normalise(raw), Ok(id.to_owned()), "{raw}");
            assert_eq!(
                rule.key_of(id),
                id,
                "what the inspector reports is its own key"
            );
        }
        for (raw, why) in [
            ("", InvalidAppId::Empty),
            (r"C:\dir\", InvalidAppId::Empty),
            ("?", InvalidAppId::Character),
            ("bank*.exe", InvalidAppId::Character),
            ("a\u{7}.exe", InvalidAppId::Character),
        ] {
            assert_eq!(rule.normalise(raw), Err(why), "{raw:?}");
        }
        // The name of a process Windows cannot name matches no exclusion.
        assert_ne!(rule.key_of("?"), "?");
    }

    #[test]
    fn mac_ids_are_bundle_identifiers_in_ascii_lower_case() {
        let rule = AppIdRule::MacBundle;
        assert_eq!(
            rule.normalise("Com.Example.Bank"),
            Ok("com.example.bank".to_owned())
        );
        assert_eq!(
            rule.normalise(" org.ex-ample_2 "),
            Ok("org.ex-ample_2".to_owned())
        );
        assert_eq!(
            rule.normalise("/Applications/Bank.app"),
            Err(InvalidAppId::Path)
        );
        // Every name the inspector can report (the follow-up review's N2): bundle identifiers
        // as applications spell them, and executable file names of programs without one.
        for (reported, key) in [
            ("com.apple.TextEdit", "com.apple.textedit"),
            ("some-tool", "some-tool"),
            ("python3.12", "python3.12"),
            ("qemu-system-x86_64", "qemu-system-x86_64"),
            ("My Tool", "my tool"),
            ("com.example.app+beta", "com.example.app+beta"),
            ("com.exämple", "com.exämple"),
            ("Bäckerei Kasse", "bäckerei kasse"),
            (r"back\slash", r"back\slash"),
        ] {
            assert_eq!(rule.normalise(reported), Ok(key.to_owned()), "{reported:?}");
            assert_eq!(rule.key_of(reported), key, "{reported:?}");
        }
        assert_eq!(rule.normalise("Bank App"), Ok("bank app".to_owned()));
        assert_eq!(rule.normalise("a\u{7}b"), Err(InvalidAppId::Character));
        assert_eq!(rule.normalise(""), Err(InvalidAppId::Empty));
        assert_eq!(rule.key_of("com.Example.BANK"), "com.example.bank");
        for why in [
            InvalidAppId::Empty,
            InvalidAppId::Path,
            InvalidAppId::Character,
        ] {
            assert!(why.to_string().starts_with("the app id"));
        }
    }

    #[test]
    fn stored_exclusions_are_compared_and_migrated_in_the_rules_form() {
        let mut store = Store::open_in_memory().expect("store");
        let mut n = 0_u8;
        let mut new_id = || {
            n += 1;
            Id::from_bytes([n; 16])
        };
        let profile =
            ensure_personal_profile(&mut store, &mut new_id, Timestamp(1)).expect("profile");
        let put = |store: &mut Store, app_id: &str| {
            store
                .put_app_exclusion(&AppExclusion {
                    app_id: app_id.to_owned(),
                    display_name: String::new(),
                    created_at: Timestamp(2),
                })
                .expect("stored");
        };
        put(&mut store, "Org.Example.Bank");
        put(&mut store, "org.example\u{1}");
        // Compared in the rule's form even before the rows are rewritten.
        let view = build_view(&store, profile, 1).expect("view");
        assert!(view.is_excluded("org.example.bank"));
        assert!(view.is_excluded("ORG.example.bank"));
        assert!(!view.is_excluded("org.example.editor"));

        put(&mut store, "org.example.bank");
        let done = migrate_exclusions(&mut store, AppIdRule::Plain).expect("migrated");
        assert_eq!(
            done,
            ExclusionMigration {
                rewritten: 1,
                unusable: 1
            }
        );
        let ids: Vec<String> = store
            .list_app_exclusions()
            .expect("rows")
            .into_iter()
            .map(|e| e.app_id)
            .collect();
        assert_eq!(ids, ["org.example\u{1}", "org.example.bank"]);
        assert_eq!(
            migrate_exclusions(&mut store, AppIdRule::Plain).expect("again"),
            ExclusionMigration {
                rewritten: 0,
                unusable: 1
            }
        );
    }
}
