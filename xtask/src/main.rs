//! Repository maintenance tasks, run as `cargo xtask <task>`.
//!
//! Currently provides `banned-terms`, which keeps the repository free of the names of other
//! products without storing those names anywhere: only SHA-256 digests of normalised terms
//! are kept in `xtask/banned-terms.sha256`.

mod scan;

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use scan::{Digest, digest_of, format_hit, normalise, parse_hex, scan_bytes, scan_text, to_hex};

const USAGE: &str = "usage: cargo xtask banned-terms [check | add <term...> | hash <term...>]";
const LIST_HEADER: &str = "# SHA-256 digests of normalised banned terms (lowercase, tokens joined by one space).\n\
# Managed by `cargo xtask banned-terms add`; never write the terms themselves in the repo.\n";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn list_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("banned-terms.sha256")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    if args.first().map(String::as_str) != Some("banned-terms") {
        return Err(USAGE.to_owned());
    }
    let rest = &args[1..];
    match rest.first().map(String::as_str) {
        None | Some("check") => check(),
        Some("add") => add(&rest[1..]),
        Some("hash") => hash(&rest[1..]),
        Some(_) => Err(USAGE.to_owned()),
    }
}

fn say(line: &str) -> Result<(), String> {
    writeln!(std::io::stdout().lock(), "{line}").map_err(|e| e.to_string())
}

/// Parses digest-list text into (comment lines, digests).
fn parse_list(text: &str) -> Result<(Vec<String>, BTreeSet<String>), String> {
    let mut comments = Vec::new();
    let mut digests = BTreeSet::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            comments.push(line.to_owned());
        } else if parse_hex(line).is_some() {
            digests.insert(line.to_ascii_lowercase());
        } else {
            return Err(format!(
                "banned-terms.sha256 line {}: not a SHA-256 hex digest",
                i + 1
            ));
        }
    }
    Ok((comments, digests))
}

fn load_digests(path: &Path) -> Result<HashSet<Digest>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (_, set) = parse_list(&text)?;
    Ok(set.iter().filter_map(|h| parse_hex(h)).collect())
}

fn term_from(args: &[String]) -> Result<String, String> {
    let term = normalise(&args.join(" "));
    if term.is_empty() {
        return Err(USAGE.to_owned());
    }
    Ok(term)
}

fn hash(args: &[String]) -> Result<ExitCode, String> {
    say(&to_hex(&digest_of(&term_from(args)?)))?;
    Ok(ExitCode::SUCCESS)
}

fn add(args: &[String]) -> Result<ExitCode, String> {
    let digest = to_hex(&digest_of(&term_from(args)?));
    let path = list_path();
    let text = fs::read_to_string(&path).unwrap_or_default();
    let (mut comments, mut digests) = parse_list(&text)?;
    if comments.is_empty() {
        comments = LIST_HEADER.lines().map(str::to_owned).collect();
    }
    let added = digests.insert(digest);
    let mut out = comments.join("\n");
    out.push('\n');
    for d in &digests {
        out.push_str(d);
        out.push('\n');
    }
    fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()))?;
    say(if added { "added" } else { "already present" })?;
    Ok(ExitCode::SUCCESS)
}

/// Tracked plus untracked-but-not-ignored files, relative to the repo root.
fn list_files(root: &Path) -> Result<Vec<String>, String> {
    let git = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output();
    if let Ok(o) = git
        && o.status.success()
    {
        let files: BTreeSet<String> = o
            .stdout
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        return Ok(files.into_iter().collect());
    }
    let mut files = Vec::new();
    walk(root, "", &mut files).map_err(|e| format!("walking {}: {e}", root.display()))?;
    files.sort();
    Ok(files)
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if entry.file_type()?.is_dir() {
            if name == ".git" || (rel.is_empty() && name == "target") {
                continue;
            }
            walk(&entry.path(), &child, out)?;
        } else {
            out.push(child);
        }
    }
    Ok(())
}

fn check() -> Result<ExitCode, String> {
    let root = root();
    let digests = load_digests(&list_path())?;
    let files = list_files(&root)?;
    let (mut scanned, mut hit_count) = (0_usize, 0_usize);
    for rel in &files {
        for hit in scan_text(rel, &digests) {
            hit_count += 1;
            say(&format_hit(
                &format!("{rel} (path)"),
                &scan::Hit { line: 0, ..hit },
            ))?;
        }
        if rel.rsplit('/').next() == Some("Cargo.lock") {
            continue;
        }
        let bytes = match fs::read(root.join(rel)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{rel}: {e}")),
        };
        scanned += 1;
        for hit in scan_bytes(&bytes, &digests) {
            hit_count += 1;
            say(&format_hit(rel, &hit))?;
        }
    }
    say(&format!("scanned {scanned} files, {hit_count} hits"))?;
    Ok(if hit_count == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_digest_file_is_well_formed() {
        let text = fs::read_to_string(list_path()).expect("digest file readable");
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        assert!(!lines.is_empty());
        for l in &lines {
            assert!(l.len() == 64 && l.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        }
        let mut sorted = lines.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, lines, "digest file must be sorted and deduplicated");
    }
}
