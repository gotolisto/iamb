//! Command-line entry point for Iamb (`iamb`).
//!
//! Each command first tries the service endpoint (role `cli`); when no service answers it opens
//! the local store under its lock, and if the lock stays taken for 3 s it stops with "the
//! service is starting, try again" (architecture §6.3, `service.rs`). To add a subcommand: add
//! a variant to [`cli::Command`], write a `run` function in a new module under `commands/` that
//! handles both [`service::Backend`]s, and add one arm to the `match` in [`run`].

mod cli;
mod commands;
mod error;
mod ids;
mod output;
mod service;
mod session;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::error::CliError;

fn run(cli: Cli) -> Result<ExitCode, CliError> {
    let data_dir = session::resolve_data_dir(cli.data_dir.as_deref())?;
    let done = |r: Result<(), CliError>| r.map(|()| ExitCode::SUCCESS);
    match cli.command {
        Command::Add(args) => done(commands::add::run(&data_dir, args)),
        Command::List(args) => done(commands::list::run(&data_dir, &args)),
        Command::Remove(args) => done(commands::remove::run(&data_dir, &args)),
        Command::Import(args) => done(commands::transfer::import(&data_dir, &args)),
        Command::Export(args) => done(commands::transfer::export(&data_dir, &args)),
        Command::Expand(args) => commands::expand::run(&data_dir, &args),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("iamb: {e}");
            ExitCode::from(1)
        }
    }
}
