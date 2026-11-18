//! Folder paths: names joined by " / ", starting at a top-level folder.

use std::collections::BTreeMap;

use iamb_core::model::{Folder, Id};
use iamb_store::ListOptions;

use crate::error::CliError;
use crate::session::{Session, new_folder};

/// Separator between folder names in a path.
pub const SEP: &str = " / ";

/// Splits and validates a user-typed folder path.
pub fn parse_path(text: &str) -> Result<Vec<String>, CliError> {
    let parts: Vec<String> = text.split('/').map(|s| s.trim().to_owned()).collect();
    if parts.iter().any(String::is_empty) {
        return Err(CliError::new(format!(
            "invalid folder path {text:?}: use names separated by \" / \""
        )));
    }
    Ok(parts)
}

/// The personal profile's live folders by id.
pub fn folder_map(s: &Session) -> Result<BTreeMap<Id, Folder>, CliError> {
    Ok(s.store
        .list_folders(ListOptions::LIVE)?
        .into_iter()
        .filter(|f| f.profile_id == s.profile)
        .map(|f| (f.meta.id, f))
        .collect())
}

/// The path text of a folder.
pub fn path_of(map: &BTreeMap<Id, Folder>, id: Id) -> String {
    let mut names = Vec::new();
    let mut cur = map.get(&id);
    while let Some(f) = cur {
        names.push(f.name.as_str());
        if names.len() > 16 {
            break;
        }
        cur = f.parent_id.and_then(|p| map.get(&p));
    }
    names.reverse();
    names.join(SEP)
}

/// Finds the folder at `path`, creating missing levels. Returns its id.
pub fn ensure_path(s: &mut Session, path: &[String]) -> Result<Id, CliError> {
    let mut map = folder_map(s)?;
    let mut parent: Option<Id> = None;
    for name in path {
        let found = map
            .values()
            .find(|f| f.parent_id == parent && &f.name == name)
            .map(|f| f.meta.id);
        let id = if let Some(id) = found {
            id
        } else {
            let position = i64::try_from(map.values().filter(|f| f.parent_id == parent).count())
                .unwrap_or(i64::MAX);
            let folder = new_folder(s.profile, parent, name, position, s.now);
            s.store.put_folder(&folder, s.now)?;
            let id = folder.meta.id;
            map.insert(id, folder);
            id
        };
        parent = Some(id);
    }
    parent.ok_or_else(|| CliError::new("empty folder path"))
}
