//! `iamb expand`.

use std::path::Path;
use std::process::ExitCode;

use iamb_cli::expand::{DryEnv, expand, snapshot_from_store, utc_now};
use iamb_core::datetime::LocalDateTime;
use iamb_core::template::FormValues;

use crate::cli::ExpandArgs;
use crate::error::CliError;
use crate::output;
use crate::session::Session;

/// Exit status when nothing expanded.
const NOTHING_EXPANDED: u8 = 3;

/// Runs the command.
pub fn run(data_dir: &Path, args: &ExpandArgs) -> Result<ExitCode, CliError> {
    let now = match &args.now {
        Some(text) => LocalDateTime::parse_iso(text).map_err(|_| {
            CliError::new(format!("invalid --now {text:?}: use YYYY-MM-DDTHH:MM:SS"))
        })?,
        None => utc_now().ok_or_else(|| CliError::new("the system clock is out of range"))?,
    };
    let mut values = FormValues::new();
    for f in &args.field {
        let Some((name, value)) = f.split_once('=') else {
            return Err(CliError::new(format!(
                "invalid --field {f:?}: use NAME=VALUE"
            )));
        };
        values.insert(name, value);
    }
    let s = Session::open(data_dir)?;
    let snapshot = snapshot_from_store(&s.store, s.profile)?;
    let env = DryEnv {
        now,
        clipboard: args.clipboard.clone(),
    };
    let outcome = expand(&snapshot, &args.typed, &env, &values);
    for code in &outcome.diagnostics {
        eprintln!("iamb: {code}");
    }
    if args.actions {
        for l in &outcome.action_lines {
            output::line(l);
        }
    } else if args.show_cursor {
        output::line(&outcome.text_with_cursor());
    } else {
        output::line(&outcome.text);
    }
    Ok(if outcome.expansions > 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(NOTHING_EXPANDED)
    })
}
