//! Library health (task 3.10, spec §4.9): what is wrong with the snippets that were written,
//! judged from one engine snapshot and the device-local usage counts.
//!
//! Nothing here reads the clock or the database: the caller passes the snapshot, the snippets in
//! precedence order, the usage rows and `now`, so a fixture library can be checked without a
//! service. The sections of the view and where each comes from:
//!
//! | Section | Rule | Source |
//! | --- | --- | --- |
//! | Will not expand: error in the snippet | An enabled snippet whose template does not parse, or does not resolve (a `{snippet}` that names nothing, a cycle, nesting over 8 levels, a parse error in an included snippet, a form that is too large or conflicts) | [`Snapshot::excluded`] |
//! | Will not expand: abbreviation taken | A snippet whose folded abbreviation another snippet holds | [`Snapshot::excluded`] |
//! | Never reached | An immediate abbreviation that is the beginning of a longer one | [`Snapshot::shadowed`], the matcher the editor's warnings use |
//! | Not used on this computer for 90 days | An enabled snippet that expands and was last used 90 days ago or more, or never since it was created (or since the counts were reset) | `usage` |
//!
//! A `{snippet}` that names a **disabled** snippet is not broken: "disabling only stops a snippet
//! from triggering on its own" (template language §4.4). A disabled snippet is never reported,
//! whatever it holds: it does not expand, and the user has already turned it off. A snippet is
//! reported once, in the first section that applies, so a broken snippet is not also called
//! unused and a hidden one is not also called unused.
//!
//! Usage is device-local (ADR-0015): only counts and times are read, never anything typed.

use std::collections::{HashMap, HashSet};

use iamb_core::engine::{ExclusionReason, Snapshot};
use iamb_core::matcher::Shadowing;
use iamb_core::model::{Id, Snippet, Timestamp};
use iamb_core::template::{self, MacroName, SnippetSource, Token};

use crate::Usage;

/// A snippet nobody used for this many days is reported (spec §4.9).
pub const UNUSED_DAYS: i64 = 90;

const DAY_MS: i64 = 86_400_000;

/// Why a snippet will not expand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// Another snippet holds its abbreviation.
    Duplicate {
        /// The snippet that holds it.
        kept: Id,
    },
    /// Its template does not parse or does not resolve.
    Template {
        /// The error code (`E_…`).
        code: &'static str,
        /// For a reference error: the abbreviation the failing `{snippet}` names.
        reference: Option<String>,
        /// The abbreviation of the included snippet whose body holds the mistake; `None` when it
        /// is in the snippet's own content.
        included: Option<String>,
    },
}

/// A snippet that will not expand although it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excluded {
    /// The snippet.
    pub snippet: Id,
    /// Why.
    pub why: Why,
}

/// A snippet that expands and is not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unused {
    /// The snippet.
    pub snippet: Id,
    /// When it last expanded on this computer; `None`: not since it was created, or since the
    /// counts were reset.
    pub last_used: Option<Timestamp>,
}

/// Everything wrong with a library, by section.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Findings {
    /// "Never reached".
    pub shadowed: Vec<Shadowing>,
    /// "Will not expand".
    pub excluded: Vec<Excluded>,
    /// "Not used on this computer for 90 days", longest unused first.
    pub unused: Vec<Unused>,
}

impl Findings {
    /// The snippets with something to fix, each once: the number the sidebar's badge shows.
    #[must_use]
    pub fn affected(&self) -> usize {
        let mut seen: HashSet<Id> = HashSet::new();
        seen.extend(self.excluded.iter().map(|e| e.snippet));
        seen.extend(self.shadowed.iter().map(|s| s.hidden));
        seen.extend(self.unused.iter().map(|u| u.snippet));
        seen.len()
    }
}

/// Checks a library.
///
/// `snippets` are the library's snippets in precedence order (the ones `snapshot` was built
/// from). `usage_since` is when the counts were last reset: a snippet that was never used counts
/// from the later of its creation and that time, so a reset does not make everything unused at
/// once.
#[must_use]
pub fn analyse<'a>(
    snapshot: &Snapshot,
    snippets: impl IntoIterator<Item = &'a Snippet>,
    usage: &[Usage],
    usage_since: Option<Timestamp>,
    now: Timestamp,
) -> Findings {
    let shadowed = snapshot.shadowed();
    let template_why = |id: Id, error: &template::TemplateError| {
        let own = snapshot.snippet(id);
        let (reference, included) = reference_of(snapshot, own.map(|s| s.body.as_str()), error);
        // The error names the snippet whose body holds it, which may be the snippet itself.
        let included = included.filter(|i| own.is_none_or(|s| s.abbreviation.as_str() != i));
        Why::Template {
            code: error.code(),
            reference,
            included,
        }
    };
    // Duplicates and templates that do not parse come from the snapshot's exclusions.
    let mut excluded: Vec<Excluded> = snapshot
        .excluded()
        .iter()
        .map(|e| Excluded {
            snippet: e.snippet,
            why: match &e.reason {
                ExclusionReason::Duplicate { kept } => Why::Duplicate { kept: *kept },
                ExclusionReason::Template(error) => template_why(e.snippet, error),
            },
        })
        .collect();
    // A template that parses but does not resolve (a `{snippet}` that names nothing, a cycle,
    // nesting over 8 levels, an error in an included snippet) stays matchable, so that typing
    // it reports the error instead of letting a shorter abbreviation fire; it is still broken.
    let listed: HashSet<Id> = excluded.iter().map(|e| e.snippet).collect();
    let snippets: Vec<&Snippet> = snippets.into_iter().collect();
    for s in &snippets {
        let id = s.meta.id;
        if !s.enabled || listed.contains(&id) {
            continue;
        }
        if let Some(Err(error)) = snapshot.compiled(id) {
            excluded.push(Excluded {
                snippet: id,
                why: template_why(id, error),
            });
        }
    }

    // A snippet appears in one section: the ones above come first.
    let reported: HashSet<Id> = excluded
        .iter()
        .map(|e| e.snippet)
        .chain(shadowed.iter().map(|s| s.hidden))
        .collect();
    let usage: HashMap<Id, &Usage> = usage.iter().map(|u| (u.snippet_id, u)).collect();
    let limit = UNUSED_DAYS * DAY_MS;
    let mut unused: Vec<(i64, Unused)> = Vec::new();
    for s in snippets {
        let id = s.meta.id;
        if !s.enabled || s.meta.is_deleted() || reported.contains(&id) {
            continue;
        }
        let last_used = usage.get(&id).and_then(|u| u.last_used);
        let since = last_used.map_or_else(
            || {
                let created = s.meta.created_at.0;
                usage_since.map_or(created, |t| created.max(t.0))
            },
            |t| t.0,
        );
        if now.0.saturating_sub(since) >= limit {
            unused.push((
                since,
                Unused {
                    snippet: id,
                    last_used,
                },
            ));
        }
    }
    unused.sort_by(|a, b| (a.0, a.1.snippet.as_bytes()).cmp(&(b.0, b.1.snippet.as_bytes())));

    Findings {
        shadowed,
        excluded,
        unused: unused.into_iter().map(|(_, u)| u).collect(),
    }
}

/// For a reference error: the abbreviation the failing `{snippet}` names, and the included
/// snippet that holds it.
fn reference_of(
    snapshot: &Snapshot,
    own_source: Option<&str>,
    error: &template::TemplateError,
) -> (Option<String>, Option<String>) {
    let included = error.snippet().map(str::to_owned);
    let source = match error.snippet() {
        Some(abbreviation) => snapshot.body(abbreviation),
        None => own_source,
    };
    let reference = (|| {
        let (source, at) = (source?, error.position()?);
        // Only the resolve errors that stand at a `{snippet}`.
        if !matches!(
            error.kind(),
            template::ErrorKind::SnippetNotFound
                | template::ErrorKind::SnippetCycle
                | template::ErrorKind::TooDeep
        ) {
            return None;
        }
        template::tokens(source).into_iter().find_map(|t| match t {
            Token::Macro {
                name: MacroName::Snippet,
                attributes,
                range,
            } if range.contains(&at) => attributes
                .into_iter()
                .find(|a| a.name == "abbr")
                .map(|a| a.value),
            _ => None,
        })
    })();
    (reference, included)
}
