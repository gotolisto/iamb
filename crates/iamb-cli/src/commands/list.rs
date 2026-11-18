//! `iamb list`.

use std::path::Path;

use iamb_core::model::{CaseMode, Snippet, Trigger};
use iamb_store::ListOptions;

use super::folders::{SEP, folder_map, parse_path, path_of};
use crate::cli::ListArgs;
use crate::error::CliError;
use crate::output::{json_string, line};
use crate::session::Session;

struct Row {
    snippet: Snippet,
    folder: String,
}

fn trigger_word(t: Trigger) -> &'static str {
    match t {
        Trigger::Immediate => "immediate",
        Trigger::Delimiter => "delimiter",
    }
}

fn case_word(c: CaseMode) -> &'static str {
    match c {
        CaseMode::Exact => "exact",
        CaseMode::Insensitive => "insensitive",
        CaseMode::Adaptive => "adaptive",
    }
}

/// The label, or the first 40 characters of the body on one line.
fn display_label(s: &Snippet) -> String {
    if !s.label.is_empty() {
        return one_line(&s.label);
    }
    let flat = one_line(&s.body);
    flat.chars().take(40).collect()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Runs the command.
pub fn run(data_dir: &Path, args: &ListArgs) -> Result<(), CliError> {
    let wanted = args
        .folder
        .as_deref()
        .map(|f| parse_path(f).map(|p| p.join(SEP)))
        .transpose()?;
    let s = Session::open(data_dir)?;
    let folders = folder_map(&s)?;
    let mut rows: Vec<Row> = s
        .store
        .list_snippets(ListOptions::LIVE)?
        .into_iter()
        .filter(|sn| folders.contains_key(&sn.folder_id))
        .map(|sn| Row {
            folder: path_of(&folders, sn.folder_id),
            snippet: sn,
        })
        .filter(|r| {
            wanted
                .as_ref()
                .is_none_or(|w| r.folder == *w || r.folder.starts_with(&format!("{w}{SEP}")))
        })
        .collect();
    rows.sort_by_cached_key(|r| {
        (
            r.snippet.abbreviation.fold_key(),
            r.snippet.abbreviation.as_str().to_owned(),
        )
    });
    if args.json {
        print_json(&rows);
    } else {
        print_table(&rows);
    }
    Ok(())
}

fn print_json(rows: &[Row]) {
    if rows.is_empty() {
        line("[]");
        return;
    }
    line("[");
    for (i, r) in rows.iter().enumerate() {
        let sn = &r.snippet;
        let comma = if i + 1 < rows.len() { "," } else { "" };
        line(&format!(
            "  {{\"id\":{},\"abbreviation\":{},\"label\":{},\"body\":{},\"folder\":{},\"trigger\":\"{}\",\"case\":\"{}\",\"enabled\":{}}}{comma}",
            json_string(&sn.meta.id.to_string()),
            json_string(sn.abbreviation.as_str()),
            json_string(&sn.label),
            json_string(&sn.body),
            json_string(&r.folder),
            trigger_word(sn.trigger),
            case_word(sn.case_mode),
            sn.enabled,
        ));
    }
    line("]");
}

fn print_table(rows: &[Row]) {
    let head = ["ABBREVIATION", "LABEL", "FOLDER", "TRIGGER", "ENABLED"];
    let body: Vec<[String; 5]> = rows
        .iter()
        .map(|r| {
            [
                one_line(r.snippet.abbreviation.as_str()),
                display_label(&r.snippet),
                r.folder.clone(),
                trigger_word(r.snippet.trigger).to_owned(),
                if r.snippet.enabled { "yes" } else { "no" }.to_owned(),
            ]
        })
        .collect();
    let mut widths = head.map(str::len);
    for row in &body {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let fmt = |cells: [&str; 5]| {
        let mut out = String::new();
        for (i, (cell, w)) in cells.iter().zip(widths).enumerate() {
            out.push_str(cell);
            if i + 1 < cells.len() {
                let pad = w.saturating_sub(cell.chars().count()) + 2;
                out.push_str(&" ".repeat(pad));
            }
        }
        out
    };
    line(&fmt(head));
    for row in &body {
        line(&fmt([&row[0], &row[1], &row[2], &row[3], &row[4]]));
    }
}
