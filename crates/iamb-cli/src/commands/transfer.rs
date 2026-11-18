//! `iamb import` and `iamb export`.

use std::path::Path;

use iamb_store::transfer::{self, Format};

use crate::cli::{ExportArgs, ImportArgs};
use crate::error::CliError;
use crate::ids;
use crate::output;
use crate::session::Session;

fn pick_format(explicit: Option<crate::cli::FormatArg>, path: &Path) -> Result<Format, CliError> {
    if let Some(f) = explicit {
        return Ok(f.into());
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("json") => Ok(Format::Json),
        Some("csv") => Ok(Format::Csv),
        _ => Err(CliError::new(
            "cannot tell the file format from the extension; pass --format json or --format csv",
        )),
    }
}

/// Runs `iamb import`.
pub fn import(data_dir: &Path, args: &ImportArgs) -> Result<(), CliError> {
    let format = pick_format(args.format, &args.path)?;
    let mut s = Session::open(data_dir)?;
    let summary = transfer::import(
        &mut s.store,
        s.profile,
        &args.path,
        format,
        s.now,
        &mut ids::new_id,
        args.on_conflict.into(),
    )?;
    output::line(&format!(
        "Imported {} snippets ({} skipped), {} folders and {} tags created",
        summary.snippets_imported,
        summary.snippets_skipped,
        summary.folders_created,
        summary.tags_created
    ));
    Ok(())
}

/// Runs `iamb export`.
pub fn export(data_dir: &Path, args: &ExportArgs) -> Result<(), CliError> {
    let format = pick_format(args.format, &args.path)?;
    let s = Session::open(data_dir)?;
    let summary = transfer::export(&s.store, s.profile, &args.path, format)?;
    output::line(&format!(
        "Exported {} snippets to {}",
        summary.snippets,
        args.path.display()
    ));
    Ok(())
}
