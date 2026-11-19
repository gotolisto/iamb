//! ADR-0008 from inside the crate. Two of the checks of architecture §2:
//!
//! - check 3, `std::net` and the socket calls of `libc`: a lint, clippy's `disallowed-types`
//!   and `disallowed-methods`, configured in a `clippy.toml` in each of the six crates §2 names
//!   (the service, the IPC crate, the store and the three platform crates). The lint resolves
//!   paths, so it sees `use std::{io, net::TcpStream}` and a fully qualified call alike, which
//!   a search of the source text did not (the 2.8 review's finding 8). CI runs clippy with
//!   `-D warnings`; the test here makes sure the configuration is there, the same in every
//!   crate, and names everything it must;
//! - check 1, no networking crate anywhere in the service's resolved dependency tree (read
//!   from `Cargo.lock`, which covers every target).
//!
//! Check 4, the release binary built with `-p iamb-service` alone, is a step of the `lint` job
//! of `.github/workflows/ci.yml`.

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

/// The crates in which a lint forbids network code (architecture §2, check 3).
const GUARDED: &[&str] = &[
    "iamb-service",
    "iamb-ipc",
    "iamb-store",
    "iamb-platform",
    "iamb-platform-windows",
    "iamb-platform-macos",
];

/// What the lint must forbid: every type of `std::net` but `Shutdown`, the calls that open a
/// socket or resolve a name, and the same calls of `libc`.
const FORBIDDEN_TYPES: &[&str] = &[
    "std::net::TcpStream",
    "std::net::TcpListener",
    "std::net::UdpSocket",
    "std::net::SocketAddr",
    "std::net::SocketAddrV4",
    "std::net::SocketAddrV6",
    "std::net::IpAddr",
    "std::net::Ipv4Addr",
    "std::net::Ipv6Addr",
    "std::net::ToSocketAddrs",
];
const FORBIDDEN_METHODS: &[&str] = &[
    "std::net::TcpStream::connect",
    "std::net::TcpStream::connect_timeout",
    "std::net::TcpListener::bind",
    "std::net::UdpSocket::bind",
    "std::net::ToSocketAddrs::to_socket_addrs",
    "libc::socket",
    "libc::connect",
    "libc::bind",
    "libc::listen",
    "libc::accept",
    "libc::sendto",
    "libc::sendmsg",
    "libc::recvfrom",
    "libc::recvmsg",
    "libc::getaddrinfo",
    "libc::gethostbyname",
    "libc::getnameinfo",
];

/// The `path = "..."` entries of the array `key` in a `clippy.toml`.
fn entries(config: &str, key: &str) -> BTreeSet<String> {
    let Some(start) = config.find(&format!("{key} = [")) else {
        return BTreeSet::new();
    };
    let rest = &config[start..];
    let body = &rest[..rest.find("\n]").unwrap_or(rest.len())];
    body.split("path = \"")
        .skip(1)
        .filter_map(|e| e.split('"').next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_lint_forbids_network_code_in_the_six_crates() {
    let mut first: Option<String> = None;
    for krate in GUARDED {
        let dir = workspace().join("crates").join(krate);
        let path = dir.join("clippy.toml");
        let config =
            fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing", path.display()));
        let types = entries(&config, "disallowed-types");
        let methods = entries(&config, "disallowed-methods");
        for wanted in FORBIDDEN_TYPES {
            assert!(
                types.contains(*wanted),
                "{krate}: {wanted} is not forbidden"
            );
        }
        for wanted in FORBIDDEN_METHODS {
            assert!(
                methods.contains(*wanted),
                "{krate}: {wanted} is not forbidden"
            );
        }
        assert!(
            !types.iter().chain(&methods).any(|p| p.contains("Shutdown")),
            "{krate}: Unix sockets need std::net::Shutdown"
        );
        // One configuration, copied: a crate's own file hides any file above it.
        match &first {
            None => first = Some(config),
            Some(first) => assert_eq!(first, &config, "{krate}/clippy.toml differs"),
        }
        // The lints are not switched off for the crate.
        let manifest = fs::read_to_string(dir.join("Cargo.toml")).expect("manifest");
        for lint in ["disallowed_types", "disallowed_methods"] {
            assert!(!manifest.contains(lint), "{krate} configures {lint}");
        }
        assert!(
            manifest.contains("[lints]\nworkspace = true")
                || manifest.contains("all = { level = \"warn\", priority = -1 }"),
            "{krate} does not take the workspace's clippy lints"
        );
    }
    let root = fs::read_to_string(workspace().join("Cargo.toml")).expect("manifest");
    assert!(root.contains("all = { level = \"warn\", priority = -1 }"));
    assert!(!root.contains("disallowed_types") && !root.contains("disallowed_methods"));
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
