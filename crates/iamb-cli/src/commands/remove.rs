//! `iamb remove`.

use std::path::Path;

use iamb_core::model::Abbreviation;
use iamb_store::ListOptions;

use super::folders::folder_map;
use crate::cli::RemoveArgs;
use crate::error::CliError;
use crate::output;
use crate::session::Session;

/// Runs the command.
pub fn run(data_dir: &Path, args: &RemoveArgs) -> Result<(), CliError> {
    let key = Abbreviation::new(&args.abbreviation)
        .map_err(|e| CliError::new(format!("invalid abbreviation {:?}: {e}", args.abbreviation)))?
        .fold_key();
    let mut s = Session::open(data_dir)?;
    let folders = folder_map(&s)?;
    let found = s
        .store
        .list_snippets(ListOptions::LIVE)?
        .into_iter()
        .find(|sn| folders.contains_key(&sn.folder_id) && sn.abbreviation.fold_key() == key);
    let Some(snippet) = found else {
        return Err(CliError::new(format!(
            "no snippet with abbreviation {:?}",
            args.abbreviation
        )));
    };
    s.store.delete_snippet(snippet.meta.id, s.now)?;
    output::line(&format!("Removed {}", snippet.abbreviation.as_str()));
    Ok(())
}
