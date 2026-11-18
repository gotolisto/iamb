//! `iamb add`.

use std::io::{self, Read};
use std::path::Path;

use iamb_core::model::{Snippet, SyncMeta, Trigger};
use iamb_store::{ListOptions, StoreError};

use super::folders::{ensure_path, find_path, folder_map, parse_path};
use crate::cli::AddArgs;
use crate::error::CliError;
use crate::ids;
use crate::output;
use crate::session::{ROOT_FOLDER, Session};

fn read_body(args: &AddArgs) -> Result<String, CliError> {
    match (&args.body, &args.body_file) {
        (Some(body), _) => Ok(body.clone()),
        (None, Some(path)) if path == Path::new("-") => {
            let mut text = String::new();
            io::stdin()
                .read_to_string(&mut text)
                .map_err(|e| CliError::new(format!("cannot read standard input: {e}")))?;
            Ok(text)
        }
        (None, Some(path)) => std::fs::read_to_string(path)
            .map_err(|e| CliError::new(format!("cannot read {}: {e}", path.display()))),
        (None, None) => Err(CliError::new("give a body or --body-file")),
    }
}

/// Runs the command.
pub fn run(data_dir: &Path, args: AddArgs) -> Result<(), CliError> {
    let body = read_body(&args)?;
    let path = match &args.folder {
        Some(text) => parse_path(text)?,
        None => vec![ROOT_FOLDER.to_owned()],
    };
    let mut s = Session::open(data_dir)?;

    // The folder's prefix is part of the abbreviation, so apply it before the conflict check.
    // A folder that does not exist yet has no defaults.
    let folders = folder_map(&s)?;
    let defaults = find_path(&folders, &path)
        .and_then(|id| folders.get(&id))
        .map(|f| f.defaults.clone())
        .unwrap_or_default();
    let abbreviation = defaults
        .prefixed_abbreviation(&args.abbreviation)
        .map_err(|e| CliError::new(format!("invalid abbreviation {:?}: {e}", args.abbreviation)))?;
    let key = abbreviation.fold_key();
    if let Some(existing) = s
        .store
        .list_snippets(ListOptions::LIVE)?
        .into_iter()
        .find(|sn| folders.contains_key(&sn.folder_id) && sn.abbreviation.fold_key() == key)
    {
        return Err(conflict(&existing));
    }

    let id = ids::new_id();
    let (profile, now) = (s.profile, s.now);
    let result = s.store.write(|w| {
        let folder_id = ensure_path(w, profile, now, &path)?;
        let mut snippet = Snippet::new_defaults(
            SyncMeta::new(id, now),
            folder_id,
            abbreviation,
            args.label.unwrap_or_default(),
            body,
            Some(&defaults),
        );
        if let Some(t) = args.trigger {
            snippet.trigger = t.into();
            snippet.word_start = snippet.trigger == Trigger::Delimiter;
        }
        if let Some(c) = args.case_mode {
            snippet.case_mode = c.into();
        }
        snippet.enabled = !args.disabled;
        w.put(&snippet, now)
    });
    match result {
        Ok(_) => {}
        Err(StoreError::AbbreviationConflict { existing, .. }) => {
            let other = s.store.get_snippet(existing)?;
            return Err(other.map_or_else(
                || CliError::new("abbreviation is already in use"),
                |o| conflict(&o),
            ));
        }
        Err(e) => return Err(e.into()),
    }
    output::line(&id.to_string());
    Ok(())
}

fn conflict(existing: &Snippet) -> CliError {
    let label = if existing.label.is_empty() {
        String::new()
    } else {
        format!(" ({})", existing.label)
    };
    CliError::new(format!(
        "abbreviation {:?} is already used by snippet {}{label}",
        existing.abbreviation.as_str(),
        existing.meta.id
    ))
}
