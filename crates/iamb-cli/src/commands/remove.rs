//! `iamb remove`.

use std::path::Path;

use std::collections::BTreeMap;

use iamb_core::model::{Abbreviation, Folder, Id, Snippet};
use iamb_ipc::{RequestBody, ResponseBody};
use iamb_store::ListOptions;

use super::folders::{folder_map, folder_map_of};
use crate::cli::RemoveArgs;
use crate::error::CliError;
use crate::output;
use crate::service::{self, Backend, unexpected};

/// Runs the command.
pub fn run(data_dir: &Path, args: &RemoveArgs) -> Result<(), CliError> {
    let key = Abbreviation::new(&args.abbreviation)
        .map_err(|e| CliError::new(format!("invalid abbreviation {:?}: {e}", args.abbreviation)))?
        .fold_key();
    let find = |folders: &BTreeMap<Id, Folder>, snippets: Vec<Snippet>| {
        snippets
            .into_iter()
            .find(|sn| folders.contains_key(&sn.folder_id) && sn.abbreviation.fold_key() == key)
            .ok_or_else(|| {
                CliError::new(format!(
                    "no snippet with abbreviation {:?}",
                    args.abbreviation
                ))
            })
    };
    let snippet = match service::open(data_dir)? {
        Backend::Local(mut s) => {
            let folders = folder_map(&s)?;
            let snippet = find(&folders, s.store.list_snippets(ListOptions::LIVE)?)?;
            s.store.delete_snippet(snippet.meta.id, s.now)?;
            snippet
        }
        Backend::Service(mut r) => {
            let folders = folder_map_of(r.profile, r.folders()?);
            let snippet = find(&folders, r.snippets()?)?;
            match r.call(RequestBody::DeleteSnippet {
                id: snippet.meta.id,
            })? {
                ResponseBody::Done {} => {}
                _ => return Err(unexpected()),
            }
            snippet
        }
    };
    output::line(&format!("Removed {}", snippet.abbreviation.as_str()));
    Ok(())
}
