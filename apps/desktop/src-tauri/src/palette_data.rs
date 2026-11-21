//! What the palette's rows are made of (task 3.6): `Search` through the bridge, and the rows of
//! its hits drawn from the snippet list this side holds, as the flow says ("rows are drawn from
//! the snippet list the UI host holds", `docs/ux/flows/5-search.md` step 2).
//!
//! The service ranks (`iamb_core::search`, architecture §6.2: `Search` → ids, scores, the matched
//! field and its ranges in UTF-16 code units); this module turns each hit into a row: label,
//! abbreviation, folder, first line, badges. The snippet list (every summary, the folders' names,
//! the tags' names, the revisions waiting for review) is read in pages the first time a query
//! needs it, kept in memory, and read again after a `LibraryChanged`. Nothing of it is written
//! anywhere; the log has no word of it.
//!
//! # Privacy (architecture §8, spec §9)
//!
//! The query is the user's text. It travels in one `Search` frame ([`Secret`], which prints as
//! `<redacted>`), is never logged and is not kept: not here, not in the front end's storage.
//! Nothing in this module logs.
//!
//! | Command | Request sent | Answer |
//! | --- | --- | --- |
//! | `palette_search` | `Search` | [`PaletteResults`] |
//! | `palette_recent` | `RecentSnippets` | [`PaletteResults`] |
//! | `palette_preview` | `GetSnippet`, `RenderPreview` | [`PreviewView`] |

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use iamb_ipc::{RequestBody, ResponseBody, SearchField, Secret};
use serde::Serialize;

use crate::bridge::{Bridge, command};
use crate::contract::{CommandError, parse_id};
use crate::library::{self, FolderView, PAGE_LIMIT, SummaryView};
use crate::template::{self, PreviewView};

/// How many hits one query asks for (flow 5, step 2).
pub const HIT_LIMIT: u32 = 50;

/// How many recently used snippets the palette lists with nothing typed: its eight rows
/// (spec §4.5).
pub const RECENT_LIMIT: u32 = 8;

/// The requests this module sends, by wire name.
pub const REQUESTS: &[&str] = &[
    "search",
    "recent_snippets",
    "list_snippet_summaries",
    "list_folders",
    "list_tags",
    "list_pending_reviews",
    "get_snippet",
    "render_preview",
];

type Res<T> = Result<T, CommandError>;

/// One row of the palette: a snippet, with what its line shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteRow {
    /// Its id.
    pub id: String,
    /// What the user types.
    pub abbreviation: String,
    /// The label (may be empty).
    pub label: String,
    /// The folder, with its parents: "Replies › Returns".
    pub folder: String,
    /// The first line of the content.
    pub first_line: String,
    /// `plain` or `rich`.
    pub kind: String,
    /// Whether the abbreviation expands (a switched-off snippet can still be picked).
    pub enabled: bool,
    /// Why it does not expand although enabled: `duplicate` or a template error code (`E_…`).
    pub broken: Option<String>,
    /// Expanding it asks for values (a `{field}` or `{if}` anywhere in it).
    pub has_fields: bool,
    /// A revision by someone else waits for the user's approval.
    pub needs_review: bool,
    /// The names of its tags.
    pub tags: Vec<String>,
}

/// A hit: the row, and what matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteHit {
    /// The snippet's row.
    pub row: PaletteRow,
    /// `abbreviation`, `label`, `body` or `tag`: the field the ranking matched.
    pub field: Option<String>,
    /// The matched ranges `[start, end)` in that field, in UTF-16 code units (for a tag, in the
    /// tag's name).
    pub ranges: Vec<[usize; 2]>,
}

/// The answer of `palette_search`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteResults {
    /// How many snippets there are in all ("4 of 76").
    pub total: u64,
    /// The hits, best first.
    pub hits: Vec<PaletteHit>,
}

/// The name of a [`SearchField`] on the front end's side.
#[must_use]
pub const fn field_name(field: SearchField) -> &'static str {
    match field {
        SearchField::Abbreviation => "abbreviation",
        SearchField::Label => "label",
        SearchField::Body => "body",
        SearchField::Tag => "tag",
    }
}

/// The snippet list as of one revision.
#[derive(Debug, Default)]
struct Loaded {
    total: u64,
    rows: HashMap<String, SummaryView>,
    /// Folder id → "Parent › Child".
    folders: HashMap<String, String>,
    /// Tag id → name.
    tags: HashMap<String, String>,
    /// Snippets with a revision waiting for review.
    pending: HashSet<String>,
}

/// The folders' names, each with the names of its parents.
fn folder_paths(folders: &[FolderView]) -> HashMap<String, String> {
    let by_id: HashMap<&str, &FolderView> = folders.iter().map(|f| (f.id.as_str(), f)).collect();
    folders
        .iter()
        .map(|f| {
            let mut names = vec![f.name.as_str()];
            let mut at = f.parent_id.as_deref();
            // A folder tree has no cycles; the bound only keeps a corrupt one from looping.
            for _ in 0..16 {
                let Some(parent) = at.and_then(|p| by_id.get(p)) else {
                    break;
                };
                names.push(parent.name.as_str());
                at = parent.parent_id.as_deref();
            }
            names.reverse();
            (f.id.clone(), names.join(" › "))
        })
        .collect()
}

/// The palette's snippet list and its searches (module docs).
#[derive(Debug, Default)]
pub struct PaletteData {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    loaded: Option<Loaded>,
    /// A `LibraryChanged` came after the list was read.
    stale: bool,
}

impl PaletteData {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The library changed: the list is read again by the next query.
    pub fn invalidate(&self) {
        self.lock().stale = true;
    }

    /// Whether the list is held and current.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        let state = self.lock();
        state.loaded.is_some() && !state.stale
    }

    /// Forgets the list (the connection to the service ended).
    pub fn forget(&self) {
        let mut state = self.lock();
        state.loaded = None;
        state.stale = false;
    }

    fn read(bridge: &Bridge) -> Res<Loaded> {
        let folders = library::folders(bridge)?;
        let tags = library::tags(bridge)?;
        let first = library::summaries(bridge, 0, PAGE_LIMIT)?;
        let total = first.total;
        let mut rows: HashMap<String, SummaryView> = HashMap::new();
        let mut next = first.next;
        rows.extend(first.summaries.into_iter().map(|s| (s.id.clone(), s)));
        while let Some(offset) = next {
            let page = library::summaries(bridge, offset, PAGE_LIMIT)?;
            next = page.next;
            rows.extend(page.summaries.into_iter().map(|s| (s.id.clone(), s)));
        }
        // Nothing waits for review before sharing exists (task 4.4); a failure here only
        // leaves the badge off.
        let pending = match bridge.call(RequestBody::ListPendingReviews {}) {
            Ok(ResponseBody::PendingReviews { reviews }) => {
                reviews.iter().map(|r| r.snippet.to_string()).collect()
            }
            _ => HashSet::new(),
        };
        Ok(Loaded {
            total,
            rows,
            folders: folder_paths(&folders.folders),
            tags: tags.into_iter().map(|t| (t.id, t.name)).collect(),
            pending,
        })
    }

    /// Reads the list now when it is missing or out of date. Blocks while it is read (a library
    /// of 10,000 snippets is ten pages).
    ///
    /// # Errors
    ///
    /// The calls' errors.
    pub fn refresh(&self, bridge: &Bridge) -> Res<()> {
        {
            let state = self.lock();
            if state.loaded.is_some() && !state.stale {
                return Ok(());
            }
        }
        // Read outside the lock; a change that arrives meanwhile marks the list stale again.
        self.lock().stale = false;
        let loaded = match Self::read(bridge) {
            Ok(loaded) => loaded,
            Err(e) => {
                self.lock().stale = true;
                return Err(e);
            }
        };
        self.lock().loaded = Some(loaded);
        Ok(())
    }

    fn row(loaded: &Loaded, id: &str) -> Option<PaletteRow> {
        let s = loaded.rows.get(id)?;
        Some(PaletteRow {
            id: s.id.clone(),
            abbreviation: s.abbreviation.clone(),
            label: s.label.clone(),
            folder: loaded
                .folders
                .get(&s.folder_id)
                .cloned()
                .unwrap_or_default(),
            first_line: s.first_line.clone(),
            kind: s.kind.clone(),
            enabled: s.enabled,
            broken: s.broken.clone(),
            has_fields: s.has_fields,
            needs_review: loaded.pending.contains(id),
            tags: s
                .tags
                .iter()
                .filter_map(|t| loaded.tags.get(t).cloned())
                .collect(),
        })
    }

    /// The rows of `hits`; with `strict`, none when a hit has no row.
    fn hits_of(
        loaded: &Loaded,
        hits: &[iamb_ipc::SearchHit],
        strict: bool,
    ) -> Option<Vec<PaletteHit>> {
        let one = |h: &iamb_ipc::SearchHit| {
            Some(PaletteHit {
                row: Self::row(loaded, &h.snippet.to_string())?,
                field: h.field.map(|f| field_name(f).to_owned()),
                ranges: h.ranges.clone(),
            })
        };
        if strict {
            hits.iter().map(one).collect()
        } else {
            Some(hits.iter().filter_map(one).collect())
        }
    }

    /// Searches the library for `query` (with nothing typed the palette asks [`recent`](Self::recent) instead).
    ///
    /// # Errors
    ///
    /// The calls' errors (`disconnected`, `timeout`, …).
    pub fn search(&self, bridge: &Bridge, query: &str) -> Res<PaletteResults> {
        let response = bridge
            .call(RequestBody::Search {
                text: Secret::new(query.to_owned()),
                limit: HIT_LIMIT,
            })
            .map_err(command)?;
        let ResponseBody::SearchResults { hits } = response else {
            return Err(CommandError::new(
                "unexpected_response",
                "the service's answer to search was not the one expected",
            ));
        };
        self.refresh(bridge)?;
        // Strictly first: a hit the list does not know was added after the list was read.
        let first = {
            let state = self.lock();
            state
                .loaded
                .as_ref()
                .and_then(|l| Self::hits_of(l, &hits, true).map(|h| (l.total, h)))
        };
        let rows = if first.is_some() {
            first
        } else {
            self.invalidate();
            self.refresh(bridge)?;
            let state = self.lock();
            state
                .loaded
                .as_ref()
                .and_then(|l| Self::hits_of(l, &hits, false).map(|h| (l.total, h)))
        };
        let (total, hits) = rows.unwrap_or_default();
        Ok(PaletteResults { total, hits })
    }

    /// The snippets used most recently on this computer, newest first, for the palette with
    /// nothing typed (spec §4.5). A service that does not report usage (`unknown_request`)
    /// gives an empty list, and the palette shows its hint.
    ///
    /// # Errors
    ///
    /// The calls' errors (`disconnected`, `timeout`, …) other than a service that does not know
    /// the request.
    pub fn recent(&self, bridge: &Bridge) -> Res<PaletteResults> {
        let response = match bridge.call(RequestBody::RecentSnippets {
            limit: RECENT_LIMIT,
        }) {
            Ok(r) => r,
            Err(e) => {
                let e = command(e);
                if e.code == "unknown_request" {
                    return Ok(PaletteResults::default());
                }
                return Err(e);
            }
        };
        let ResponseBody::SearchResults { hits } = response else {
            return Err(CommandError::new(
                "unexpected_response",
                "the service's answer to recent_snippets was not the one expected",
            ));
        };
        self.refresh(bridge)?;
        let rows = {
            let state = self.lock();
            state
                .loaded
                .as_ref()
                .and_then(|l| Self::hits_of(l, &hits, true).map(|h| (l.total, h)))
        };
        let rows = rows.or_else(|| {
            self.invalidate();
            self.refresh(bridge).ok()?;
            let state = self.lock();
            state
                .loaded
                .as_ref()
                .and_then(|l| Self::hits_of(l, &hits, false).map(|h| (l.total, h)))
        });
        let (total, hits) = rows.unwrap_or_default();
        Ok(PaletteResults { total, hits })
    }

    /// What snippet `id` would type now, for the strip under the list ("as it would be typed now",
    /// with its blanks shown by name).
    ///
    /// # Errors
    ///
    /// The calls' errors (`not_found` for a snippet that is gone).
    pub fn preview(&self, bridge: &Bridge, id: &str) -> Res<PreviewView> {
        let snippet = library::snippet(bridge, id)?;
        let _ = parse_id(id)?;
        // A formatted snippet is rendered as one: the strip shows its text, not its Markdown.
        let rich = snippet.kind == "rich";
        template::preview_as(bridge, snippet.body, BTreeMap::new(), rich)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, parent: Option<&str>, name: &str) -> FolderView {
        FolderView {
            id: id.to_owned(),
            parent_id: parent.map(str::to_owned),
            name: name.to_owned(),
            position: 0,
            defaults: library::DefaultsView::default(),
        }
    }

    #[test]
    fn a_folder_is_named_with_its_parents() {
        let paths = folder_paths(&[
            folder("a", None, "Replies"),
            folder("b", Some("a"), "Returns"),
            folder("c", Some("b"), "Parts"),
            folder("x", Some("missing"), "Orphan"),
        ]);
        assert_eq!(paths["a"], "Replies");
        assert_eq!(paths["b"], "Replies › Returns");
        assert_eq!(paths["c"], "Replies › Returns › Parts");
        assert_eq!(paths["x"], "Orphan");
    }

    #[test]
    fn a_cycle_in_the_folders_does_not_loop() {
        let paths = folder_paths(&[folder("a", Some("b"), "A"), folder("b", Some("a"), "B")]);
        assert!(paths["a"].len() < 200);
    }

    #[test]
    fn the_fields_have_the_front_ends_names() {
        assert_eq!(field_name(SearchField::Abbreviation), "abbreviation");
        assert_eq!(field_name(SearchField::Label), "label");
        assert_eq!(field_name(SearchField::Body), "body");
        assert_eq!(field_name(SearchField::Tag), "tag");
    }

    #[test]
    fn the_list_is_stale_after_a_change_and_gone_after_forget() {
        let data = PaletteData::default();
        assert!(!data.is_fresh());
        data.lock().loaded = Some(Loaded::default());
        assert!(data.is_fresh());
        data.invalidate();
        assert!(!data.is_fresh());
        data.forget();
        assert!(data.lock().loaded.is_none());
    }
}
