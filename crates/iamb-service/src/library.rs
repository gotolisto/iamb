//! What the worker knows about the library at one moment: the immutable [`View`], built by the
//! store thread after every change to library content or settings and shared with every other
//! thread through [`SharedView`] (architecture §3.1: "an atomically swapped pointer").

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError, RwLock};

use iamb_core::datetime::{Format, Locale};
use iamb_core::engine::{EngineSettings, Snapshot};
use iamb_core::matcher::Delimiters;
use iamb_core::model::{
    Folder, FolderDefaults, Id, Profile, ProfileKind, Snippet, SyncMeta, Timestamp,
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
    /// Every setting.
    pub settings: Arc<BTreeMap<String, String>>,
    /// The app exclusions.
    pub exclusions: Arc<Vec<AppExclusion>>,
}

impl View {
    /// The snippets, in precedence order.
    pub fn snippets(&self) -> impl Iterator<Item = &Snippet> {
        self.snippet_ids
            .iter()
            .filter_map(|id| self.snapshot.snippet(*id))
    }

    /// Whether the focus verdict must say `excluded` for `app`.
    #[must_use]
    pub fn is_excluded(&self, app: &str) -> bool {
        self.exclusions.iter().any(|e| e.app_id == app)
    }
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
    let snippet_ids = snippets.iter().map(|s| s.meta.id).collect();
    let settings = store.list_settings()?;
    let engine = engine_settings(&settings).map_err(StoreError::InvalidData)?;
    Ok(View {
        revision,
        profile,
        snapshot: Snapshot::build(snippets, engine),
        snippet_ids: Arc::new(snippet_ids),
        folders: Arc::new(folders),
        settings: Arc::new(settings),
        exclusions: Arc::new(store.list_app_exclusions()?),
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
}
