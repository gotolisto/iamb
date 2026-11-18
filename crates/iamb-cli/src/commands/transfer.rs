//! `iamb import` and `iamb export`.

use std::path::{Path, PathBuf};

use iamb_ipc::{ConflictPolicy, RequestBody, ResponseBody, TransferFormat};
use iamb_store::transfer::{self, Format};

use crate::cli::{ExportArgs, ImportArgs};
use crate::error::CliError;
use crate::ids;
use crate::output;
use crate::service::{self, Backend, unexpected};

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

/// The format on the wire.
const fn wire_format(format: Format) -> TransferFormat {
    match format {
        Format::Json => TransferFormat::Json,
        Format::Csv => TransferFormat::Csv,
    }
}

/// The service runs in another directory: bulk data travels as an absolute path (§6.1).
fn absolute(path: &Path) -> Result<PathBuf, CliError> {
    std::path::absolute(path)
        .map_err(|e| CliError::new(format!("cannot resolve {}: {e}", path.display())))
}

/// Runs `iamb import`.
pub fn import(data_dir: &Path, args: &ImportArgs) -> Result<(), CliError> {
    let format = pick_format(args.format, &args.path)?;
    let (imported, skipped, folders, tags) = match service::open(data_dir)? {
        Backend::Local(mut s) => {
            let summary = transfer::import(
                &mut s.store,
                s.profile,
                &args.path,
                format,
                s.now,
                &mut ids::new_id,
                args.on_conflict.into(),
            )?;
            let n = |v: usize| u64::try_from(v).unwrap_or(u64::MAX);
            (
                n(summary.snippets_imported),
                n(summary.snippets_skipped),
                n(summary.folders_created),
                n(summary.tags_created),
            )
        }
        Backend::Service(mut r) => match r.call(RequestBody::Import {
            path: absolute(&args.path)?,
            format: wire_format(format),
            on_conflict: match args.on_conflict {
                crate::cli::ConflictArg::Skip => ConflictPolicy::Skip,
                crate::cli::ConflictArg::Fail => ConflictPolicy::Fail,
            },
        })? {
            ResponseBody::Imported {
                snippets_imported,
                snippets_skipped,
                folders_created,
                tags_created,
            } => (
                snippets_imported,
                snippets_skipped,
                folders_created,
                tags_created,
            ),
            _ => return Err(unexpected()),
        },
    };
    output::line(&format!(
        "Imported {imported} snippets ({skipped} skipped), {folders} folders and {tags} tags created"
    ));
    Ok(())
}

/// Runs `iamb export`.
pub fn export(data_dir: &Path, args: &ExportArgs) -> Result<(), CliError> {
    let format = pick_format(args.format, &args.path)?;
    let snippets = match service::open(data_dir)? {
        Backend::Local(s) => {
            let summary = transfer::export(&s.store, s.profile, &args.path, format)?;
            u64::try_from(summary.snippets).unwrap_or(u64::MAX)
        }
        Backend::Service(mut r) => match r.call(RequestBody::Export {
            path: absolute(&args.path)?,
            format: wire_format(format),
        })? {
            ResponseBody::Exported { snippets } => snippets,
            _ => return Err(unexpected()),
        },
    };
    output::line(&format!(
        "Exported {snippets} snippets to {}",
        args.path.display()
    ));
    Ok(())
}
