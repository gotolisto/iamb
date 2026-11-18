//! Integration tests: run the built `iamb` binary against a temporary data directory.

#![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn iamb(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_iamb"));
    c.arg("--data-dir").arg(dir).env_remove("IAMB_DATA_DIR");
    c
}

fn run(dir: &Path, args: &[&str]) -> Output {
    iamb(dir).args(args).output().expect("run iamb")
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "iamb {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8")
}

fn fail(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, args);
    assert_eq!(out.status.code(), Some(1), "iamb {args:?} should exit 1");
    let err = String::from_utf8(out.stderr).expect("utf8");
    assert!(err.starts_with("iamb: "), "stderr was {err:?}");
    assert_eq!(err.lines().count(), 1, "stderr was {err:?}");
    err
}

/// Minimal JSON well-formedness check (the crate takes no JSON dependency).
fn is_valid_json(text: &str) -> bool {
    struct P<'a>(&'a [u8], usize);
    impl P<'_> {
        fn ws(&mut self) {
            while self.0.get(self.1).is_some_and(u8::is_ascii_whitespace) {
                self.1 += 1;
            }
        }
        fn eat(&mut self, c: u8) -> bool {
            self.ws();
            if self.0.get(self.1) == Some(&c) {
                self.1 += 1;
                true
            } else {
                false
            }
        }
        fn string(&mut self) -> bool {
            if !self.eat(b'"') {
                return false;
            }
            while let Some(&c) = self.0.get(self.1) {
                self.1 += 1;
                match c {
                    b'"' => return true,
                    b'\\' => self.1 += 1,
                    c if c < 0x20 => return false,
                    _ => {}
                }
            }
            false
        }
        fn value(&mut self) -> bool {
            self.ws();
            match self.0.get(self.1) {
                Some(b'"') => self.string(),
                Some(b'[') => {
                    self.1 += 1;
                    if self.eat(b']') {
                        return true;
                    }
                    loop {
                        if !self.value() {
                            return false;
                        }
                        if self.eat(b']') {
                            return true;
                        }
                        if !self.eat(b',') {
                            return false;
                        }
                    }
                }
                Some(b'{') => {
                    self.1 += 1;
                    if self.eat(b'}') {
                        return true;
                    }
                    loop {
                        if !self.string() || !self.eat(b':') || !self.value() {
                            return false;
                        }
                        if self.eat(b'}') {
                            return true;
                        }
                        if !self.eat(b',') {
                            return false;
                        }
                    }
                }
                Some(_) => {
                    for lit in ["true", "false", "null"] {
                        if self.0[self.1..].starts_with(lit.as_bytes()) {
                            self.1 += lit.len();
                            return true;
                        }
                    }
                    false
                }
                None => false,
            }
        }
    }
    let mut p = P(text.as_bytes(), 0);
    let good = p.value();
    p.ws();
    good && p.1 == text.len()
}

/// Removes the `"id":"..."` member from every line of `list --json` output.
fn strip_ids(json: &str) -> String {
    json.lines()
        .map(|l| match l.find("\"id\":\"") {
            Some(start) => {
                let rest = &l[start + 6..];
                let end = rest.find('"').expect("closing quote");
                format!("{}{}", &l[..start], &rest[end + 2..])
            }
            None => l.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn add_then_list_shows_it() {
    let d = TempDir::new().unwrap();
    let id = ok(
        d.path(),
        &["add", ";sig", "Best regards", "--label", "Signature"],
    );
    assert_eq!(id.trim().len(), 36, "id was {id:?}");
    let table = ok(d.path(), &["list"]);
    assert!(table.starts_with("ABBREVIATION"));
    assert!(table.contains(";sig") && table.contains("Signature"));
    assert!(table.contains("Snippets") && table.contains("delimiter") && table.contains("yes"));
}

#[test]
fn list_without_label_shows_start_of_body_and_is_sorted() {
    let d = TempDir::new().unwrap();
    ok(d.path(), &["add", "zz", "last"]);
    let long = format!("line one\nline two {}", "x".repeat(60));
    ok(d.path(), &["add", "aa", &long]);
    let table = ok(d.path(), &["list"]);
    assert!(table.find("aa").unwrap() < table.find("zz").unwrap());
    assert!(table.contains("line one line two "));
    assert!(!table.contains(&"x".repeat(40)));
}

#[test]
fn list_json_parses_and_has_fields() {
    let d = TempDir::new().unwrap();
    ok(
        d.path(),
        &[
            "add",
            "q\"t",
            "say \"hi\"\nback\\slash",
            "--trigger",
            "immediate",
            "--case",
            "exact",
            "--disabled",
        ],
    );
    let json = ok(d.path(), &["list", "--json"]);
    assert!(is_valid_json(&json), "bad JSON: {json}");
    assert!(json.contains("\"trigger\":\"immediate\"") && json.contains("\"case\":\"exact\""));
    assert!(json.contains("\"enabled\":false"));
    let empty = TempDir::new().unwrap();
    assert_eq!(ok(empty.path(), &["list", "--json"]).trim(), "[]");
}

#[test]
fn folder_is_created_by_path_and_filters_list() {
    let d = TempDir::new().unwrap();
    ok(d.path(), &["add", "a1", "one", "--folder", "Work / Email"]);
    ok(d.path(), &["add", "b1", "two"]);
    let all = ok(d.path(), &["list"]);
    assert!(all.contains("Work / Email") && all.contains("b1"));
    let work = ok(d.path(), &["list", "--folder", "Work"]);
    assert!(work.contains("a1") && !work.contains("b1"));
    let exact = ok(d.path(), &["list", "--folder", "Work / Email", "--json"]);
    assert!(exact.contains("\"folder\":\"Work / Email\""));
}

#[test]
fn body_from_stdin_and_from_file() {
    let d = TempDir::new().unwrap();
    let mut child = iamb(d.path())
        .args(["add", "fromin", "--body-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"typed on stdin")
        .unwrap();
    assert!(child.wait_with_output().unwrap().status.success());

    let file = d.path().join("body.txt");
    std::fs::write(&file, "from a file").unwrap();
    ok(
        d.path(),
        &["add", "fromfile", "--body-file", file.to_str().unwrap()],
    );

    let json = ok(d.path(), &["list", "--json"]);
    assert!(json.contains("typed on stdin") && json.contains("from a file"));
}

#[test]
fn conflict_exits_1_and_names_existing() {
    let d = TempDir::new().unwrap();
    let id = ok(d.path(), &["add", ";Sig", "one"]);
    let err = fail(d.path(), &["add", ";sig", "two"]);
    assert!(
        err.contains("already used") && err.contains(id.trim()),
        "{err}"
    );
}

#[test]
fn invalid_abbreviation_exits_1_readably() {
    let d = TempDir::new().unwrap();
    let err = fail(d.path(), &["add", "has space", "x"]);
    assert!(err.contains("invalid abbreviation"), "{err}");
    assert!(
        !err.contains("Abbreviation"),
        "looks like Debug output: {err}"
    );
}

#[test]
fn remove_then_list_and_unknown() {
    let d = TempDir::new().unwrap();
    ok(d.path(), &["add", ";gone", "x"]);
    ok(d.path(), &["add", ";keep", "y"]);
    ok(d.path(), &["remove", ";GONE"]);
    let table = ok(d.path(), &["list"]);
    assert!(!table.contains(";gone") && table.contains(";keep"));
    let err = fail(d.path(), &["remove", ";gone"]);
    assert!(err.contains("no snippet"), "{err}");
    // The abbreviation can be used again after removal.
    ok(d.path(), &["add", ";gone", "again"]);
}

#[test]
fn export_import_round_trips_json_and_csv() {
    for (name, extra) in [
        ("lib.json", None),
        ("lib.csv", None),
        ("lib.dat", Some("csv")),
    ] {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        ok(
            a.path(),
            &[
                "add",
                ";one",
                "First, \"quoted\"\nsecond line",
                "--label",
                "One",
                "--folder",
                "Work / Email",
            ],
        );
        ok(
            a.path(),
            &[
                "add",
                ";two",
                "Two",
                "--trigger",
                "immediate",
                "--case",
                "adaptive",
                "--disabled",
            ],
        );
        let file = a.path().join(name);
        let path = file.to_str().unwrap();
        let mut export = vec!["export", path];
        let mut import = vec!["import", path];
        if let Some(f) = extra {
            export.extend(["--format", f]);
            import.extend(["--format", f]);
        }
        assert!(ok(a.path(), &export).contains("Exported 2 snippets"));
        let summary = ok(b.path(), &import);
        assert!(summary.contains("Imported 2 snippets"), "{summary}");
        let before = strip_ids(&ok(a.path(), &["list", "--json"]));
        let after = strip_ids(&ok(b.path(), &["list", "--json"]));
        assert_eq!(before, after, "round trip of {name}");
        // Importing again skips by default and fails on request.
        assert!(ok(b.path(), &import).contains("2 skipped"));
        import.extend(["--on-conflict", "fail"]);
        fail(b.path(), &import);
    }
}

#[test]
fn import_needs_known_format() {
    let d = TempDir::new().unwrap();
    let f = d.path().join("x.txt");
    std::fs::write(&f, "").unwrap();
    let err = fail(d.path(), &["import", f.to_str().unwrap()]);
    assert!(err.contains("--format"), "{err}");
    fail(
        d.path(),
        &["import", d.path().join("missing.json").to_str().unwrap()],
    );
}

#[test]
fn help_has_an_example_everywhere() {
    let d = TempDir::new().unwrap();
    for args in [
        vec!["--help"],
        vec!["add", "--help"],
        vec!["list", "--help"],
        vec!["remove", "--help"],
        vec!["import", "--help"],
        vec!["export", "--help"],
        vec!["expand", "--help"],
    ] {
        let out = run(d.path(), &args);
        assert!(out.status.success(), "{args:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("Example"), "{args:?}: {text}");
    }
    assert!(ok(d.path(), &["--version"]).starts_with("iamb "));
}

#[test]
fn env_var_is_used_without_flag() {
    let d = TempDir::new().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_iamb"))
        .env("IAMB_DATA_DIR", d.path())
        .args(["add", "viaenv", "x"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(d.path().join("library.db").exists());
    // The flag wins over the variable.
    let other = TempDir::new().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_iamb"))
        .env("IAMB_DATA_DIR", d.path())
        .arg("--data-dir")
        .arg(other.path())
        .arg("list")
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("viaenv"));
}

#[test]
fn held_lock_gives_try_again() {
    let d = TempDir::new().unwrap();
    let _held = iamb_store::Store::open(d.path()).unwrap();
    let start = Instant::now();
    let err = fail(d.path(), &["list"]);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "took {:?}",
        start.elapsed()
    );
    assert!(err.contains("try again"), "{err}");
}

fn expand(dir: &Path, args: &[&str]) -> Output {
    let mut all = vec!["expand"];
    all.extend(args);
    run(dir, &all)
}

fn expand_ok(dir: &Path, args: &[&str]) -> String {
    let out = expand(dir, args);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn expand_plain_delimiter_and_immediate() {
    let d = TempDir::new().unwrap();
    ok(d.path(), &["add", "brb", "be right back"]);
    ok(d.path(), &["add", "ty", "thanks", "--trigger", "immediate"]);
    assert_eq!(expand_ok(d.path(), &["brb "]), "be right back \n");
    assert_eq!(expand_ok(d.path(), &["say ty"]), "say thanks\n");
    // A delimiter snippet waits for its delimiter.
    assert_eq!(expand(d.path(), &["brb"]).status.code(), Some(3));
}

#[test]
fn expand_date_macro_with_now() {
    let d = TempDir::new().unwrap();
    ok(d.path(), &["add", "dd", "{date fmt=\"%Y/%m/%d\"} {time}"]);
    let out = expand_ok(d.path(), &["dd ", "--now", "2026-03-18T14:05:09"]);
    assert_eq!(out, "2026/03/18 14:05 \n");
    let bad = expand(d.path(), &["dd ", "--now", "yesterday"]);
    assert_eq!(bad.status.code(), Some(1));
}

#[test]
fn expand_form_with_field_and_defaults() {
    let d = TempDir::new().unwrap();
    ok(
        d.path(),
        &["add", ";hi", "Hi {field name=Name default=Bob}!"],
    );
    assert_eq!(
        expand_ok(d.path(), &[";hi ", "--field", "Name=Ada"]),
        "Hi Ada! \n"
    );
    assert_eq!(expand_ok(d.path(), &[";hi "]), "Hi Bob! \n");
    assert_eq!(
        expand(d.path(), &[";hi ", "--field", "oops"]).status.code(),
        Some(1)
    );
}

#[test]
fn expand_clipboard() {
    let d = TempDir::new().unwrap();
    ok(d.path(), &["add", "cb", "[{clipboard}]"]);
    assert_eq!(
        expand_ok(d.path(), &["cb ", "--clipboard", "pasted"]),
        "[pasted] \n"
    );
}

#[test]
fn expand_show_cursor_and_actions() {
    let d = TempDir::new().unwrap();
    ok(d.path(), &["add", "cur", "a{cursor}b"]);
    ok(d.path(), &["add", "brb", "be right back"]);
    assert_eq!(expand_ok(d.path(), &["cur ", "--show-cursor"]), "a|b\n");
    assert_eq!(expand_ok(d.path(), &["cur "]), "ab\n");
    let actions = expand_ok(d.path(), &["brb ", "--actions"]);
    let lines: Vec<&str> = actions.lines().collect();
    assert_eq!(lines[0], "delete-back 4");
    assert_eq!(lines[1], "type-text \"be right back\"");
    assert_eq!(lines[2], "type-text \" \"");
    assert!(lines[3].starts_with("expanded "), "{actions}");
}

#[test]
fn expand_nothing_exits_3() {
    let d = TempDir::new().unwrap();
    ok(d.path(), &["add", "brb", "be right back"]);
    let out = expand(d.path(), &["hello "]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello \n");
}

#[test]
fn expand_reports_template_error_code() {
    let d = TempDir::new().unwrap();
    ok(d.path(), &["add", "bad", "{date fmt=\"%Q\"}"]);
    ok(d.path(), &["add", "far", "{date shift=\"+9000y\"}"]);
    let out = expand(d.path(), &["bad "]);
    assert_eq!(out.status.code(), Some(3));
    let err = String::from_utf8(out.stderr).unwrap();
    assert_eq!(err.trim(), "iamb: E_BAD_FORMAT");
    let out = expand(d.path(), &["far "]);
    assert_eq!(out.status.code(), Some(3));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("iamb: E_DATE_RANGE")
    );
}
