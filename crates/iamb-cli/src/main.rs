//! Command-line entry point for Iamb (`iamb`).
//!
//! The tool opens the local store directly (there is no background service yet). To add a
//! subcommand: add a variant to [`cli::Command`], write a `run` function in a new module under
//! `commands/`, and add one arm to the `match` in [`run`].

mod cli;
mod commands;
mod error;
mod ids;
mod output;
mod session;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::error::CliError;

fn run(cli: Cli) -> Result<(), CliError> {
    let data_dir = session::resolve_data_dir(cli.data_dir.as_deref())?;
    match cli.command {
        Command::Add(args) => commands::add::run(&data_dir, args),
        Command::List(args) => commands::list::run(&data_dir, &args),
        Command::Remove(args) => commands::remove::run(&data_dir, &args),
        Command::Import(args) => commands::transfer::import(&data_dir, &args),
        Command::Export(args) => commands::transfer::export(&data_dir, &args),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("iamb: {e}");
            ExitCode::from(1)
        }
    }
}
