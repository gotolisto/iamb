//! The immutable library snapshot the engine matches and renders against (architecture §4.4).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::datetime::{Format, Locale};
use crate::matcher::{Delimiters, MatchEntry, Matcher, Shadowing};
use crate::model::{Id, Snippet};
use crate::template::{Resolved, SnippetSource, Template, TemplateError};

/// The engine settings that are part of a snapshot (architecture §4.3: settings are not in the
/// environment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineSettings {
    /// Characters that fire delimiter-mode snippets.
    pub delimiters: Delimiters,
    /// The date-format setting, used by `{date}` without `fmt`.
    pub date_format: Format,
    /// The time-format setting, used by `{time}` without `fmt`.
    pub time_format: Format,
    /// Locale for month and weekday names.
    pub locale: Locale,
}

impl Default for EngineSettings {
    /// The default delimiters, `%Y-%m-%d`, `%H:%M` and `en`.
    fn default() -> Self {
        Self {
            delimiters: Delimiters::default(),
            date_format: Format::parse("%Y-%m-%d").expect("valid date format"),
            time_format: Format::parse("%H:%M").expect("valid time format"),
            locale: Locale::from_tag("en"),
        }
    }
}

/// Why a snippet of the reference set is not in the matchable set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExclusionReason {
    /// An earlier snippet holds the same abbreviation (case-folded). The loser neither triggers
    /// nor resolves in `{snippet}`.
    Duplicate {
        /// The snippet that holds the abbreviation.
        kept: Id,
    },
    /// The template does not parse or resolve. The snippet can still be found by `{snippet}`
    /// lookups of other templates (which then fail with the same error).
    Template(TemplateError),
}

/// A snippet that is not matchable for a reason other than being disabled, for library health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exclusion {
    /// The snippet.
    pub snippet: Id,
    /// Why it is excluded.
    pub reason: ExclusionReason,
}

/// One snippet of the reference set, with its template compiled once at build.
struct Entry {
    snippet: Snippet,
    /// `None` for duplicate losers, which are never compiled.
    compiled: Option<Result<Resolved, TemplateError>>,
}

struct Inner {
    settings: EngineSettings,
    entries: Vec<Entry>,
    by_id: HashMap<Id, usize>,
    /// Fold key → index of the snippet that holds it.
    by_fold_key: HashMap<String, usize>,
    matcher: Matcher,
    excluded: Vec<Exclusion>,
}

/// An immutable library snapshot: the reference set, the matchable set and the settings.
///
/// Cheap to clone and share between threads (one `Arc` inside). Built by the store's snapshot
/// builder after every change to library content or engine settings, and handed to the engine
/// with `Event::SnapshotChanged`.
///
/// - The **reference set** is every non-deleted snippet passed to [`build`](Self::build), in
///   precedence order: of two snippets whose abbreviations fold to the same key, the first
///   holds the abbreviation and the later one neither triggers nor resolves. `{snippet}` lookups
///   use it ([`SnippetSource`] is implemented on the snapshot), including disabled snippets.
/// - The **matchable set** is the enabled subset whose template parses and resolves, minus
///   duplicates. Typing matches against it.
/// - Every template is parsed and resolved once, here.
///
/// Rich snippets (`SnippetKind::Rich`) are compiled and expanded as plain templates through
/// `TypeText` until task 3.8 adds Markdown rendering and `Paste`.
#[derive(Clone)]
pub struct Snapshot {
    inner: Arc<Inner>,
}

impl Snapshot {
    /// Builds a snapshot from the reference set (in precedence order) and the settings.
    ///
    /// Deleted snippets are skipped. Never fails: snippets that cannot be matched are listed in
    /// [`excluded`](Self::excluded).
    #[must_use]
    pub fn build(reference: impl IntoIterator<Item = Snippet>, settings: EngineSettings) -> Self {
        let mut entries: Vec<Entry> = Vec::new();
        let mut by_id = HashMap::new();
        let mut by_fold_key: HashMap<String, usize> = HashMap::new();
        let mut excluded = Vec::new();

        for snippet in reference {
            if snippet.meta.is_deleted() || by_id.contains_key(&snippet.meta.id) {
                continue;
            }
            let index = entries.len();
            let key = snippet.abbreviation.fold_key();
            if let Some(&holder) = by_fold_key.get(&key) {
                excluded.push(Exclusion {
                    snippet: snippet.meta.id,
                    reason: ExclusionReason::Duplicate {
                        kept: entries[holder].snippet.meta.id,
                    },
                });
            } else {
                by_fold_key.insert(key, index);
            }
            by_id.insert(snippet.meta.id, index);
            entries.push(Entry {
                snippet,
                compiled: None,
            });
        }

        // Compile every snippet that holds its abbreviation, against the reference set.
        let compiled: Vec<Option<Result<Resolved, TemplateError>>> = {
            let source = RefSource {
                entries: &entries,
                by_fold_key: &by_fold_key,
            };
            entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let holds = by_fold_key.get(&e.snippet.abbreviation.fold_key()) == Some(&i);
                    holds.then(|| Template::parse(&e.snippet.body)?.resolve(&source))
                })
                .collect()
        };
        for (entry, c) in entries.iter_mut().zip(compiled) {
            entry.compiled = c;
        }

        let mut matchable = Vec::new();
        for entry in &entries {
            match &entry.compiled {
                Some(Ok(_)) if entry.snippet.enabled => {
                    matchable.push(MatchEntry::from_snippet(&entry.snippet));
                }
                Some(Err(e)) if entry.snippet.enabled => excluded.push(Exclusion {
                    snippet: entry.snippet.meta.id,
                    reason: ExclusionReason::Template(e.clone()),
                }),
                _ => {}
            }
        }
        let matcher = Matcher::build(matchable, settings.delimiters.clone());
        excluded.extend(matcher.duplicates().iter().map(|d| Exclusion {
            snippet: d.dropped,
            reason: ExclusionReason::Duplicate { kept: d.kept },
        }));

        Self {
            inner: Arc::new(Inner {
                settings,
                entries,
                by_id,
                by_fold_key,
                matcher,
                excluded,
            }),
        }
    }

    /// A snapshot with no snippets and default settings.
    #[must_use]
    pub fn empty() -> Self {
        Self::build([], EngineSettings::default())
    }

    /// The settings.
    #[must_use]
    pub fn settings(&self) -> &EngineSettings {
        &self.inner.settings
    }

    /// The matcher over the matchable set.
    #[must_use]
    pub fn matcher(&self) -> &Matcher {
        &self.inner.matcher
    }

    /// Snippets of the reference set that are not matchable for a reason other than being
    /// disabled: duplicate losers (whether enabled or not) and enabled snippets whose template
    /// fails to parse or resolve. Losers first in input order, then template errors, then
    /// duplicates only the matcher detects (abbreviations that differ in fold key but match
    /// alike, such as final and medial sigma).
    #[must_use]
    pub fn excluded(&self) -> &[Exclusion] {
        &self.inner.excluded
    }

    /// Immediate abbreviations that hide longer ones (library health).
    #[must_use]
    pub fn shadowed(&self) -> Vec<Shadowing> {
        self.inner.matcher.shadowed()
    }

    /// The snippet with this id in the reference set, if any (duplicate losers included).
    #[must_use]
    pub fn snippet(&self, id: Id) -> Option<&Snippet> {
        self.inner
            .by_id
            .get(&id)
            .map(|&i| &self.inner.entries[i].snippet)
    }

    /// The compiled template of `id`: `None` if the snippet is unknown or a duplicate loser.
    #[must_use]
    pub fn compiled(&self, id: Id) -> Option<&Result<Resolved, TemplateError>> {
        let &i = self.inner.by_id.get(&id)?;
        self.inner.entries[i].compiled.as_ref()
    }

    /// Number of snippets in the reference set (duplicate losers included).
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.entries.len()
    }

    /// True when the reference set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.entries.is_empty()
    }
}

impl Default for Snapshot {
    fn default() -> Self {
        Self::empty()
    }
}

impl SnippetSource for Snapshot {
    fn body(&self, abbreviation: &str) -> Option<&str> {
        lookup_body(&self.inner.entries, &self.inner.by_fold_key, abbreviation)
    }
}

impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Snapshot")
            .field("snippets", &self.inner.entries.len())
            .field("matchable", &self.inner.matcher.len())
            .field("excluded", &self.inner.excluded.len())
            .finish_non_exhaustive()
    }
}

/// `{snippet}` lookups over the reference set: case-sensitive on the abbreviation, and only the
/// snippet that holds the folded abbreviation resolves.
struct RefSource<'a> {
    entries: &'a [Entry],
    by_fold_key: &'a HashMap<String, usize>,
}

impl SnippetSource for RefSource<'_> {
    fn body(&self, abbreviation: &str) -> Option<&str> {
        lookup_body(self.entries, self.by_fold_key, abbreviation)
    }
}

fn lookup_body<'a>(
    entries: &'a [Entry],
    by_fold_key: &HashMap<String, usize>,
    abbreviation: &str,
) -> Option<&'a str> {
    let &i = by_fold_key.get(&abbreviation.to_lowercase())?;
    let snippet = &entries[i].snippet;
    (snippet.abbreviation.as_str() == abbreviation).then_some(snippet.body.as_str())
}
