//! ADR-0008 from inside the crate: no `std::net` in the service or the IPC crate (apart from
//! the `Shutdown` enum that Unix sockets take), and no networking crate anywhere in the
//! service's resolved dependency tree (read from `Cargo.lock`, which covers every target).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Crates that open network connections or implement network protocols.
const DENY: &[&str] = &[
    "async-std",
    "curl",
    "h2",
    "http",
    "hyper",
    "isahc",
    "mio",
    "native-tls",
    "openssl",
    "quinn",
    "reqwest",
    "rustls",
    "smol",
    "socket2",
    "surf",
    "tokio",
    "tungstenite",
    "ureq",
    "url",
    "websocket",
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_std_net_in_the_service_or_the_ipc_crate() {
    let mut files = Vec::new();
    for krate in ["iamb-service", "iamb-ipc"] {
        rust_files(
            &workspace().join("crates").join(krate).join("src"),
            &mut files,
        );
    }
    assert!(files.len() > 5);
    for file in files {
        let text = fs::read_to_string(&file).expect("readable");
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue; // documentation may name what is forbidden
            }
            let rest = line.replace("std::net::Shutdown", "");
            assert!(
                !rest.contains("std::net"),
                "{}:{}: std::net is forbidden (ADR-0008)",
                file.display(),
                n + 1
            );
        }
    }
}

#[test]
fn no_networking_crate_in_the_dependency_tree() {
    let lock = fs::read_to_string(workspace().join("Cargo.lock")).expect("Cargo.lock");
    // name -> dependency names, from the lock file's [[package]] tables.
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut name = String::new();
    let mut in_deps = false;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            in_deps = false;
        } else if let Some(n) = line.strip_prefix("name = \"") {
            name = n.trim_end_matches('"').to_owned();
            deps.entry(name.clone()).or_default();
        } else if line.starts_with("dependencies = [") {
            in_deps = true;
        } else if in_deps && line == "]" {
            in_deps = false;
        } else if in_deps {
            let dep = line.trim_matches(|c| c == '"' || c == ',');
            let dep = dep.split(' ').next().unwrap_or(dep).to_owned();
            deps.entry(name.clone()).or_default().push(dep);
        }
    }
    let mut seen = BTreeSet::new();
    let mut todo = vec!["iamb-service".to_owned()];
    while let Some(next) = todo.pop() {
        if seen.insert(next.clone()) {
            todo.extend(deps.get(&next).cloned().unwrap_or_default());
        }
    }
    assert!(
        seen.contains("iamb-store"),
        "the walk found the service's dependencies"
    );
    for banned in DENY {
        assert!(
            !seen.contains(*banned),
            "{banned} is in the service's dependency tree"
        );
    }
}
