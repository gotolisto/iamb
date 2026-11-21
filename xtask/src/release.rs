//! `cargo xtask release-version [--tag <tag>]`: the release version has one source, the
//! workspace's `[workspace.package] version` in the root `Cargo.toml` (docs/release.md).
//!
//! The command checks that nothing else claims a version:
//!
//! - every member crate (`crates/*`, the desktop app's crate, `xtask`) inherits it with
//!   `version.workspace = true`, so the service, the UI host and the other binaries all report
//!   the same version;
//! - `apps/desktop/src-tauri/tauri.conf.json` has no top-level `"version"`, so the bundler takes
//!   the crate's version (Tauri's fallback when the key is absent);
//! - with `--tag`, the tag (`v1.2.3`, or `refs/tags/v1.2.3`) is `v` followed by that version.
//!
//! On success it prints the version alone on standard output, for the release workflow.

use std::fs;
use std::path::Path;

/// Member manifests outside `crates/`.
const OTHER_MEMBERS: &[&str] = &["apps/desktop/src-tauri/Cargo.toml", "xtask/Cargo.toml"];

const TAURI_CONFIG: &str = "apps/desktop/src-tauri/tauri.conf.json";

/// Runs the command; returns the version.
pub(crate) fn run(args: &[String], root: &Path) -> Result<String, String> {
    let tag = match args {
        [] => None,
        [flag, tag] if flag == "--tag" => Some(tag.as_str()),
        _ => return Err("usage: cargo xtask release-version [--tag <tag>]".to_owned()),
    };
    let read = |rel: &str| {
        fs::read_to_string(root.join(rel)).map_err(|e| format!("cannot read {rel}: {e}"))
    };
    let version = workspace_version(&read("Cargo.toml")?)?;
    let mut members: Vec<String> = OTHER_MEMBERS.iter().map(|m| (*m).to_owned()).collect();
    let crates =
        fs::read_dir(root.join("crates")).map_err(|e| format!("cannot list crates/: {e}"))?;
    for entry in crates.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().join("Cargo.toml").is_file() {
            members.push(format!("crates/{name}/Cargo.toml"));
        }
    }
    members.sort();
    for rel in &members {
        if !inherits_version(&read(rel)?) {
            return Err(format!(
                "{rel}: the version must be `version.workspace = true` (one source of truth)"
            ));
        }
    }
    if top_level_has_key(&read(TAURI_CONFIG)?, "version") {
        return Err(format!(
            "{TAURI_CONFIG}: remove the top-level \"version\"; the bundler takes the Cargo version"
        ));
    }
    if let Some(tag) = tag {
        check_tag(tag, &version)?;
    }
    Ok(version)
}

/// The `version` of `[workspace.package]`.
pub(crate) fn workspace_version(cargo_toml: &str) -> Result<String, String> {
    let mut in_section = false;
    for line in cargo_toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[workspace.package]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "version"
        {
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .ok_or("[workspace.package] version is not a plain string")?;
            if !is_semver(value) {
                return Err(format!(
                    "[workspace.package] version {value:?} is not MAJOR.MINOR.PATCH[-PRE]"
                ));
            }
            return Ok(value.to_owned());
        }
    }
    Err("Cargo.toml has no [workspace.package] version".to_owned())
}

/// `MAJOR.MINOR.PATCH`, optionally followed by `-` and a pre-release of ASCII letters, digits
/// and dots. No build metadata: installers drop it.
fn is_semver(v: &str) -> bool {
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (v, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
                && !p.starts_with('.')
                && !p.ends_with('.')
        })
}

/// True when the `[package]` section says `version.workspace = true`.
fn inherits_version(cargo_toml: &str) -> bool {
    let mut in_package = false;
    for line in cargo_toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            if compact == "version.workspace=true" || compact == "version={workspace=true}" {
                return true;
            }
            if compact.starts_with("version=") || compact.starts_with("version.") {
                return false;
            }
        }
    }
    false
}

/// True when the JSON object in `text` has `key` at its top level. Strings are skipped
/// properly (escapes included); anything deeper than the outer object is ignored.
pub(crate) fn top_level_has_key(text: &str, key: &str) -> bool {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            b'"' => {
                let start = i + 1;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                let s = text.get(start..i.min(bytes.len())).unwrap_or("");
                // A key is a string followed by a colon.
                let rest = text.get(i + 1..).unwrap_or("").trim_start();
                if depth == 1 && rest.starts_with(':') && s == key {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// The tag must be `v<version>`.
pub(crate) fn check_tag(tag: &str, version: &str) -> Result<(), String> {
    let name = tag.strip_prefix("refs/tags/").unwrap_or(tag);
    match name.strip_prefix('v') {
        Some(v) if v == version => Ok(()),
        _ => Err(format!(
            "tag {name:?} does not match the workspace version {version:?}: expected \"v{version}\" \
             (bump [workspace.package] version in Cargo.toml, or fix the tag)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_version_is_read() {
        let toml = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"1.2.3\"\nedition = \"2024\"\n\n[package]\nversion = \"9.9.9\"\n";
        assert_eq!(workspace_version(toml).as_deref(), Ok("1.2.3"));
        assert!(workspace_version("[package]\nversion = \"1.0.0\"\n").is_err());
        assert!(workspace_version("[workspace.package]\nversion = \"1.2\"\n").is_err());
        assert_eq!(
            workspace_version("[workspace.package]\nversion = \"1.0.0-rc.1\"\n").as_deref(),
            Ok("1.0.0-rc.1")
        );
        assert!(workspace_version("[workspace.package]\nversion = \"1.0.0+abc\"\n").is_err());
    }

    #[test]
    fn crates_must_inherit() {
        assert!(inherits_version(
            "[package]\nname = \"x\"\nversion.workspace = true\n"
        ));
        assert!(inherits_version(
            "[package]\nname = \"x\"\nversion = { workspace = true }\n"
        ));
        assert!(!inherits_version(
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n"
        ));
        assert!(!inherits_version(
            "[package]\nname = \"x\"\n[dependencies]\nversion.workspace = true\n"
        ));
    }

    #[test]
    fn only_top_level_keys_count() {
        assert!(top_level_has_key(
            r#"{ "a": 1, "version": "1" }"#,
            "version"
        ));
        assert!(!top_level_has_key(
            r#"{ "a": { "version": "1" }, "b": ["version"] }"#,
            "version"
        ));
        assert!(!top_level_has_key(r#"{ "a": "version" }"#, "version"));
        assert!(!top_level_has_key(
            r#"{ "a": "x\"version\": " }"#,
            "version"
        ));
    }

    #[test]
    fn tags_must_match() {
        assert_eq!(check_tag("v1.2.3", "1.2.3"), Ok(()));
        assert_eq!(check_tag("refs/tags/v1.2.3", "1.2.3"), Ok(()));
        assert!(check_tag("v1.2.4", "1.2.3").is_err());
        assert!(check_tag("1.2.3", "1.2.3").is_err());
        assert!(check_tag("v1.2.3-rc.1", "1.2.3").is_err());
    }

    #[test]
    fn this_repository_is_consistent() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask sits in the workspace root");
        let version = run(&[], root).expect("one source of truth");
        assert!(is_semver(&version));
    }
}
