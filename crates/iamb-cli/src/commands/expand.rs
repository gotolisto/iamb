//! `iamb expand`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use iamb_cli::expand::{DryEnv, apply, expand, snapshot_from_store, utc_now};
use iamb_core::datetime::LocalDateTime;
use iamb_core::template::FormValues;
use iamb_ipc::{RequestBody, ResponseBody, Secret};

use crate::cli::ExpandArgs;
use crate::error::CliError;
use crate::output;
use crate::service::{self, Backend, unexpected};

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
    let mut wire_values = BTreeMap::new();
    for f in &args.field {
        // The argument may hold user text, so it is never echoed.
        let Some((name, value)) = f.split_once('=') else {
            return Err(CliError::new("invalid --field value, expected NAME=VALUE"));
        };
        values.insert(name, value);
        wire_values.insert(name.to_owned(), value.to_owned());
    }
    let outcome = match service::open(data_dir)? {
        Backend::Local(s) => {
            let snapshot = snapshot_from_store(&s.store, s.profile)?;
            let env = DryEnv {
                now,
                clipboard: args.clipboard.clone(),
            };
            expand(&snapshot, &args.typed, &env, &values)
        }
        Backend::Service(mut r) => match r.call(RequestBody::DryRun {
            typed: Secret::new(args.typed.clone()),
            now: args.now.clone(),
            clipboard: args.clipboard.clone().map(Secret::new),
            values: Some(Secret::new(wire_values)),
        })? {
            ResponseBody::DryRun { actions, hints } => {
                let mut outcome = apply(&args.typed, &actions);
                if outcome.expansions == 0 && outcome.diagnostics.is_empty() {
                    outcome
                        .diagnostics
                        .extend(hints.into_iter().map(|h| h.code));
                }
                outcome
            }
            _ => return Err(unexpected()),
        },
    };
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
