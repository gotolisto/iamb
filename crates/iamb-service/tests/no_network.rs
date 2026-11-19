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
//!
//! The lint can be switched off where it is used, and it cannot see a function declared by
//! hand (the follow-up review's N6). So the sources of the six crates are also searched:
//!
//! - no `allow` or `expect` attribute (also inside `cfg_attr`) names `disallowed_types`,
//!   `disallowed_methods`, or a group that holds them (`clippy::style`, `clippy::all`);
//! - no `extern` block declares a socket, connection or name-resolution function by hand, and
//!   no `#[link]` attribute or `link!` macro names a networking library.
//!
//! The built binaries' imports are checked by CI on each system (`docs/ci.md`).

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

/// Lints that forbid network code, and the groups that would switch them off with them. Built
/// from parts so that this file does not hold the attribute it looks for.
fn guarding_lints() -> Vec<String> {
    ["disallowed_types", "disallowed_methods", "style", "all"]
        .iter()
        .map(|lint| format!("clippy::{lint}"))
        .collect()
}

/// Socket, connection and name-resolution functions of the C library and the Windows sockets
/// and Internet libraries.
const NETWORK_FUNCTIONS: &[&str] = &[
    "socket",
    "socketpair",
    "connect",
    "bind",
    "listen",
    "accept",
    "accept4",
    "send",
    "sendto",
    "sendmsg",
    "recv",
    "recvfrom",
    "recvmsg",
    "getaddrinfo",
    "freeaddrinfo",
    "gethostbyname",
    "gethostbyaddr",
    "getnameinfo",
    "res_query",
    "WSAStartup",
    "WSASocketA",
    "WSASocketW",
    "WSAConnect",
    "GetAddrInfoW",
    "GetAddrInfoExW",
    "DnsQuery_W",
    "WinHttpOpen",
    "InternetOpenW",
    "URLDownloadToFileW",
    "CFSocketCreate",
    "CFStreamCreatePairWithSocketToHost",
    "nw_connection_create",
];

/// Libraries and frameworks that do networking.
const NETWORK_LIBRARIES: &[&str] = &[
    "ws2_32",
    "wsock32",
    "mswsock",
    "dnsapi",
    "winhttp",
    "wininet",
    "urlmon",
    "iphlpapi",
    "cfnetwork",
    "network",
    "resolv",
];

/// The code of `source` without `//` comments, one string per line.
fn code_lines(source: &str) -> Vec<String> {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or("").to_owned())
        .collect()
}

/// What in `source` switches the network lints off, or declares network code by hand.
fn hand_made_network_code(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let lines = code_lines(source);
    let flat: String = lines.concat().split_whitespace().collect();
    let guarding = guarding_lints();
    for attribute in ["allow(", "expect("] {
        let mut rest = flat.as_str();
        while let Some(at) = rest.find(attribute) {
            let list = &rest[at + attribute.len()..];
            let list = &list[..list.find(')').unwrap_or(list.len())];
            for lint in list.split(',') {
                if guarding.iter().any(|l| l == lint) {
                    found.push(format!("{attribute}{lint})"));
                }
            }
            rest = &rest[at + attribute.len()..];
        }
    }
    // Functions declared in `extern` blocks.
    let mut depth: Option<i32> = None;
    for line in &lines {
        let opens = line.contains("extern \"")
            && line
                .split("extern \"")
                .nth(1)
                .and_then(|after| after.split_once('"'))
                .is_some_and(|(_, after)| after.trim_start().starts_with('{'));
        if depth.is_none() && opens {
            depth = Some(0);
        }
        if let Some(d) = depth.as_mut() {
            let mut words = line
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .filter(|w| !w.is_empty());
            while let Some(word) = words.next() {
                if word == "fn"
                    && let Some(name) = words.next()
                    && NETWORK_FUNCTIONS.contains(&name)
                {
                    found.push(format!("extern fn {name}"));
                }
            }
            if let Some((_, name)) = line.split_once("link_name") {
                let name = name.split('"').nth(1).unwrap_or("");
                if NETWORK_FUNCTIONS.contains(&name) {
                    found.push(format!("link_name {name}"));
                }
            }
            *d += i32::try_from(line.matches('{').count()).unwrap_or(0);
            *d -= i32::try_from(line.matches('}').count()).unwrap_or(0);
            if *d <= 0 && line.contains('}') {
                depth = None;
            }
        }
    }
    // Libraries linked by name.
    for marker in ["#[link(name=\"", "link!(\""] {
        let mut rest = flat.as_str();
        while let Some(at) = rest.find(marker) {
            let name = rest[at + marker.len()..].split('"').next().unwrap_or("");
            let base = name
                .to_ascii_lowercase()
                .trim_end_matches(".dll")
                .to_owned();
            if NETWORK_LIBRARIES.contains(&base.as_str()) {
                found.push(format!("library {name}"));
            }
            rest = &rest[at + marker.len()..];
        }
    }
    found
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The follow-up review's N6: what the lint cannot see in the six crates.
#[test]
fn the_network_lint_is_not_switched_off_and_nothing_declares_network_code_by_hand() {
    let mut checked = 0;
    for krate in GUARDED {
        let mut files = Vec::new();
        rust_files(&workspace().join("crates").join(krate), &mut files);
        for file in files {
            let source = fs::read_to_string(&file).expect("readable");
            let found = hand_made_network_code(&source);
            assert!(found.is_empty(), "{}: {found:?}", file.display());
            checked += 1;
        }
    }
    assert!(
        checked > 100,
        "the six crates' sources were found ({checked} files)"
    );
}

/// The search above finds the follow-up review's two examples, in their usual spellings, and
/// not the trait methods and hand-written declarations the crates have.
#[test]
fn the_search_for_hand_made_network_code_finds_what_it_should() {
    let lints = guarding_lints();
    let (types, methods) = (&lints[0], &lints[1]);
    for source in [
        format!("#[allow({types}, {methods})]\nfn f() {{}}"),
        format!("#[allow(\n    clippy::too_many_lines,\n    {methods}\n)]\nfn f() {{}}"),
        format!("#![cfg_attr(test, expect({types}))]"),
        format!("#[allow({})]\nmod m {{}}", lints[3]),
        "unsafe extern \"C\" {\n    fn socket(d: i32, t: i32, p: i32) -> i32;\n}".to_owned(),
        "extern \"C\" {\n    #[link_name = \"connect\"]\n    fn open_it(fd: i32) -> i32;\n}"
            .to_owned(),
        "#[link(name = \"ws2_32\")]\nunsafe extern \"system\" {}".to_owned(),
        "windows_link::link!(\"WS2_32.dll\" \"system\" fn f() -> i32);".to_owned(),
    ] {
        assert!(
            !hand_made_network_code(&source).is_empty(),
            "not found: {source}"
        );
    }
    for source in [
        "fn bind(&self, name: &EndpointName) -> Result<()> { todo!() }",
        "unsafe extern \"C-unwind\" fn focus_notified(refcon: *mut c_void) {}",
        "#[link(name = \"Carbon\", kind = \"framework\")]\nunsafe extern \"C\" {\n    fn EnableSecureEventInput() -> i32;\n}\nfn connect() {}",
        "#[allow(clippy::too_many_lines, reason = \"long\")]\nfn f() {}",
    ] {
        assert_eq!(
            hand_made_network_code(source),
            Vec::<String>::new(),
            "{source}"
        );
    }
    let commented = format!("// #[allow({})] in a comment", lints[2]);
    assert_eq!(hand_made_network_code(&commented), Vec::<String>::new());
}
