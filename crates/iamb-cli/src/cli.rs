//! Command-line definitions.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use iamb_core::model::{CaseMode, Trigger};
use iamb_store::transfer::{Format, OnConflict};

/// Iamb, a free and open-source text expander.
#[derive(Debug, Parser)]
#[command(
    name = "iamb",
    version,
    about = "Manage your Iamb snippet library from the command line.",
    after_help = "Example:\n  iamb add ;sig \"Best regards, Ada\" --label Signature"
)]
pub struct Cli {
    /// Data directory (default: `IAMB_DATA_DIR`, else the per-OS location).
    #[arg(long, global = true, value_name = "PATH")]
    pub data_dir: Option<PathBuf>,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Add a snippet and print its id.
    #[command(
        after_help = "Example:\n  iamb add ;sig \"Best regards, Ada\" --folder \"Work / Email\""
    )]
    Add(AddArgs),
    /// List snippets, sorted by abbreviation.
    #[command(after_help = "Example:\n  iamb list --folder Work --json")]
    List(ListArgs),
    /// Remove a snippet (soft delete) by abbreviation.
    #[command(after_help = "Example:\n  iamb remove ;sig")]
    Remove(RemoveArgs),
    /// Import snippets from a JSON or CSV file.
    #[command(after_help = "Example:\n  iamb import library.json --on-conflict skip")]
    Import(ImportArgs),
    /// Export snippets to a JSON or CSV file.
    #[command(after_help = "Example:\n  iamb export library.csv --format csv")]
    Export(ExportArgs),
}

/// Trigger mode choices.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum TriggerArg {
    /// Expand on the last character of the abbreviation.
    Immediate,
    /// Expand when a delimiter follows the abbreviation.
    Delimiter,
}

impl From<TriggerArg> for Trigger {
    fn from(t: TriggerArg) -> Self {
        match t {
            TriggerArg::Immediate => Self::Immediate,
            TriggerArg::Delimiter => Self::Delimiter,
        }
    }
}

/// Case mode choices.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CaseArg {
    /// Case-sensitive match.
    Exact,
    /// Case-insensitive match.
    Insensitive,
    /// Case-insensitive match; the expansion follows the typed case.
    Adaptive,
}

impl From<CaseArg> for CaseMode {
    fn from(c: CaseArg) -> Self {
        match c {
            CaseArg::Exact => Self::Exact,
            CaseArg::Insensitive => Self::Insensitive,
            CaseArg::Adaptive => Self::Adaptive,
        }
    }
}

/// File format choices.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum FormatArg {
    /// The lossless library document.
    Json,
    /// One row per snippet.
    Csv,
}

impl From<FormatArg> for Format {
    fn from(f: FormatArg) -> Self {
        match f {
            FormatArg::Json => Self::Json,
            FormatArg::Csv => Self::Csv,
        }
    }
}

/// Conflict handling choices.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ConflictArg {
    /// Keep the existing snippet and skip the new one.
    Skip,
    /// Stop and write nothing.
    Fail,
}

impl From<ConflictArg> for OnConflict {
    fn from(c: ConflictArg) -> Self {
        match c {
            ConflictArg::Skip => Self::Skip,
            ConflictArg::Fail => Self::Fail,
        }
    }
}

/// Arguments of `iamb add`.
#[derive(Debug, Args)]
pub struct AddArgs {
    /// What you type to trigger the snippet.
    pub abbreviation: String,
    /// The text it expands to (or use `--body-file`).
    #[arg(required_unless_present = "body_file", conflicts_with = "body_file")]
    pub body: Option<String>,
    /// Read the body from a file, or from standard input when the path is `-`.
    #[arg(long, value_name = "PATH")]
    pub body_file: Option<PathBuf>,
    /// A human-readable label.
    #[arg(long)]
    pub label: Option<String>,
    /// Folder path, folder names separated by " / " (created if missing).
    #[arg(long, value_name = "PATH")]
    pub folder: Option<String>,
    /// When the snippet fires.
    #[arg(long, value_enum)]
    pub trigger: Option<TriggerArg>,
    /// How case is matched.
    #[arg(long = "case", value_enum)]
    pub case_mode: Option<CaseArg>,
    /// Create the snippet disabled.
    #[arg(long)]
    pub disabled: bool,
}

/// Arguments of `iamb list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Only snippets in this folder path (and below).
    #[arg(long, value_name = "PATH")]
    pub folder: Option<String>,
    /// Print JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// Arguments of `iamb remove`.
#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// The abbreviation of the snippet to remove (case-insensitive).
    pub abbreviation: String,
}

/// Arguments of `iamb import`.
#[derive(Debug, Args)]
pub struct ImportArgs {
    /// The file to read.
    pub path: PathBuf,
    /// File format (default: from the file extension).
    #[arg(long, value_enum)]
    pub format: Option<FormatArg>,
    /// What to do with an abbreviation that is already in use.
    #[arg(long, value_enum, default_value = "skip")]
    pub on_conflict: ConflictArg,
}

/// Arguments of `iamb export`.
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// The file to write.
    pub path: PathBuf,
    /// File format (default: from the file extension).
    #[arg(long, value_enum)]
    pub format: Option<FormatArg>,
}
