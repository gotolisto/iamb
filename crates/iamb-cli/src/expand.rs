//! Dry-run expansion: build a snapshot from the store and simulate typing into a text field.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use iamb_core::datetime::{Format, LocalDateTime, Locale, civil_from_days};
use iamb_core::engine::{EngineSettings, Environment, ExclusionReason, Snapshot, dry_run};
use iamb_core::matcher::Delimiters;
use iamb_core::model::{Id, ProfileKind, Snippet, Timestamp, fold_abbreviation};
use iamb_core::template::{FormValues, KeyName};
use iamb_ipc::{WireAction, key_named};
use iamb_store::{ListOptions, Store, StoreError};

/// The clock and clipboard of a dry run.
#[derive(Debug, Clone)]
pub struct DryEnv {
    /// The time `{date}` and `{time}` see.
    pub now: LocalDateTime,
    /// The clipboard text, if any.
    pub clipboard: Option<String>,
}

impl Environment for DryEnv {
    fn now(&self) -> LocalDateTime {
        self.now
    }
    fn clipboard_text(&self) -> Option<String> {
        self.clipboard.clone()
    }
}

/// The current time in UTC (no time-zone data is available without a dependency); `None` if
/// the system clock is out of range.
#[must_use]
pub fn utc_now() -> Option<LocalDateTime> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    let part = |v: i64| u8::try_from(v).unwrap_or(0);
    LocalDateTime::new(
        y,
        m,
        d,
        part(rem / 3600),
        part(rem % 3600 / 60),
        part(rem % 60),
    )
    .ok()
}

/// The id of the personal profile, if the store has one.
///
/// # Errors
///
/// Store errors.
pub fn personal_profile(store: &Store) -> Result<Option<Id>, StoreError> {
    Ok(store
        .list_profiles(ListOptions::LIVE)?
        .into_iter()
        .find(|p| p.kind == ProfileKind::Personal)
        .map(|p| p.meta.id))
}

/// Engine settings from the store's `date_format`, `time_format`, `locale` and `delimiters`
/// settings (the delimiters setting is the list of characters); defaults where absent.
///
/// # Errors
///
/// A readable message when a stored format is invalid.
pub fn settings_from(map: &BTreeMap<String, String>) -> Result<EngineSettings, String> {
    let mut s = EngineSettings::default();
    if let Some(v) = map.get("date_format") {
        s.date_format =
            Format::parse(v).map_err(|_| format!("invalid date_format setting {v:?}"))?;
    }
    if let Some(v) = map.get("time_format") {
        s.time_format =
            Format::parse(v).map_err(|_| format!("invalid time_format setting {v:?}"))?;
    }
    if let Some(v) = map.get("locale") {
        s.locale = Locale::from_tag(v);
    }
    if let Some(v) = map.get("delimiters") {
        s.delimiters = Delimiters::new(v.chars());
    }
    Ok(s)
}

/// Builds the snapshot of the personal profile: every live snippet in its folders, ordered by
/// creation time then id (the precedence order).
///
/// # Errors
///
/// Store errors, or an invalid setting (as [`StoreError::InvalidData`]).
pub fn snapshot_from_store(store: &Store, profile: Id) -> Result<Snapshot, StoreError> {
    let lib = store.load_library()?;
    let folders: BTreeSet<Id> = lib
        .folders
        .iter()
        .filter(|f| f.profile_id == profile)
        .map(|f| f.meta.id)
        .collect();
    let mut snippets: Vec<Snippet> = lib
        .snippets
        .into_iter()
        .filter(|s| folders.contains(&s.folder_id))
        .collect();
    snippets.sort_by_key(|s| (s.meta.created_at, s.meta.id));
    let settings = settings_from(&store.list_settings()?).map_err(StoreError::InvalidData)?;
    Ok(Snapshot::build(snippets, settings))
}

/// The result of a dry run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The text field after all actions, without a caret marker.
    pub text: String,
    /// The caret position, in characters from the start of `text`.
    pub caret: usize,
    /// How many expansions happened.
    pub expansions: usize,
    /// Reason codes of the diagnostics, in order.
    pub diagnostics: Vec<String>,
    /// Readable action lines (see [`describe`]).
    pub action_lines: Vec<String>,
}

impl Outcome {
    /// The text with `|` inserted at the caret.
    #[must_use]
    pub fn text_with_cursor(&self) -> String {
        let mut out: String = self.text.chars().take(self.caret).collect();
        out.push('|');
        out.extend(self.text.chars().skip(self.caret));
        out
    }
}

/// Types `typed` into an empty field, runs the engine and applies its actions.
///
/// Characters (not grapheme clusters) are the unit of deletion and caret movement here.
#[must_use]
pub fn expand(snapshot: &Snapshot, typed: &str, env: &DryEnv, values: &FormValues) -> Outcome {
    // A diagnostic among the actions carries the time of the dry run.
    let at = Timestamp(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
    );
    let actions: Vec<WireAction> = dry_run(snapshot, typed, env, Some(values))
        .iter()
        .map(|a| WireAction::from_engine(a, at))
        .collect();
    let mut out = apply(typed, &actions);
    // A snippet whose template does not parse is not matchable (architecture §4.4), so the
    // engine stays silent about it; name its parse error when the typed text ends with its
    // abbreviation. Resolve errors need nothing here: such snippets stay matchable and the
    // engine reports them itself (review 1.9 finding 4). The service's `DryRun` returns the
    // same hints.
    if out.expansions == 0 && out.diagnostics.is_empty() {
        let typed_fold = fold_abbreviation(typed);
        for ex in snapshot.excluded() {
            if let (ExclusionReason::Template(e), Some(sn)) =
                (&ex.reason, snapshot.snippet(ex.snippet))
            {
                let key = sn.abbreviation.fold_key();
                let body = typed_fold.strip_suffix(|c| snapshot.settings().delimiters.contains(c));
                if typed_fold.ends_with(&key) || body.is_some_and(|b| b.ends_with(&key)) {
                    out.diagnostics.push(e.code().to_owned());
                }
            }
        }
    }
    out
}

/// Applies a dry run's actions to a field that holds `typed`, caret at the end: the same for
/// a dry run here and one answered by the service.
#[must_use]
pub fn apply(typed: &str, actions: &[WireAction]) -> Outcome {
    let mut field: Vec<char> = typed.chars().collect();
    let mut caret = field.len();
    let mut out = Outcome::default();
    for action in actions {
        match action {
            WireAction::DeleteBack { n } => {
                let n = (*n).min(caret);
                field.drain(caret - n..caret);
                caret -= n;
            }
            WireAction::TypeText { text, .. } => insert(&mut field, &mut caret, text),
            WireAction::Paste { plain, .. } => insert(&mut field, &mut caret, plain),
            WireAction::PressKey { key, times, .. } => {
                let piece = match key_named(key) {
                    Some(KeyName::Enter) => "\n".to_owned(),
                    Some(KeyName::Tab) => "\t".to_owned(),
                    Some(KeyName::Space) => " ".to_owned(),
                    _ => format!("<{key}>"),
                };
                for _ in 0..*times {
                    insert(&mut field, &mut caret, &piece);
                }
            }
            WireAction::MoveCaretBack { n } => caret = caret.saturating_sub(*n),
            WireAction::Expanded { .. } => out.expansions += 1,
            WireAction::Diagnostic { diagnostic } => out.diagnostics.push(diagnostic.code.clone()),
            WireAction::Wait { .. } | WireAction::ShowForm { .. } => {}
        }
        if let Some(line) = describe(action) {
            out.action_lines.push(line);
        }
    }
    out.text = field.into_iter().collect();
    out.caret = caret;
    out
}

fn insert(field: &mut Vec<char>, caret: &mut usize, text: &str) {
    let new: Vec<char> = text.chars().collect();
    let n = new.len();
    field.splice(*caret..*caret, new);
    *caret += n;
}

/// A readable one-line form of an action; `None` for diagnostics, which go to stderr.
#[must_use]
pub fn describe(action: &WireAction) -> Option<String> {
    Some(match action {
        WireAction::DeleteBack { n } => format!("delete-back {n}"),
        WireAction::TypeText { text, .. } => format!("type-text {}", quote(text)),
        WireAction::Paste { plain, .. } => format!("paste {}", quote(plain)),
        WireAction::PressKey { key, shift, times } => {
            let shift = if *shift { " shift" } else { "" };
            format!("press-key {key} x{times}{shift}")
        }
        WireAction::MoveCaretBack { n } => format!("move-caret-back {n}"),
        WireAction::Wait { ms } => format!("wait {ms}"),
        WireAction::ShowForm { fields, .. } => {
            let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            format!("show-form {}", names.join(", "))
        }
        WireAction::Expanded {
            snippet,
            chars_saved,
            ..
        } => format!("expanded {snippet} saved {chars_saved}"),
        WireAction::Diagnostic { .. } => return None,
    })
}

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
