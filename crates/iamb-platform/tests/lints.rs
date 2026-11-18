//! Guards on the manifests of the platform crates (architecture §2):
//!
//! - `unsafe` is forbidden workspace-wide and allowed only in the two OS crates, which must
//!   otherwise carry exactly the workspace lints (a crate cannot inherit the workspace lints
//!   and override one, so they keep a copy);
//! - the OS crates' dependencies are gated by target, so both crates build as empty shells
//!   everywhere;
//! - no networking namespace of the Windows bindings is enabled.

use std::fs;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

const OS_CRATES: [&str; 2] = ["iamb-platform-windows", "iamb-platform-macos"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/iamb-platform has a grandparent")
        .to_path_buf()
}

fn manifest(path: &Path) -> Table {
    let text =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    text.parse()
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
}

fn table<'a>(parent: &'a Table, key: &str) -> &'a Table {
    parent
        .get(key)
        .and_then(Value::as_table)
        .unwrap_or_else(|| panic!("missing table `{key}`"))
}

fn member_manifests(root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(root.join("crates"))
        .expect("crates directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("Cargo.toml"))
        .filter(|path| path.is_file())
        .collect();
    found.push(root.join("xtask").join("Cargo.toml"));
    found.sort();
    found
}

#[test]
fn the_os_crates_mirror_the_workspace_lints_except_for_unsafe() {
    let root = workspace_root();
    let workspace = manifest(&root.join("Cargo.toml"));
    let workspace_lints = table(table(&workspace, "workspace"), "lints");

    let mut expected = workspace_lints.clone();
    let rust = expected
        .get_mut("rust")
        .and_then(Value::as_table_mut)
        .expect("workspace rust lints");
    assert_eq!(
        rust.get("unsafe_code").and_then(Value::as_str),
        Some("forbid"),
        "the workspace forbids unsafe code"
    );
    rust.insert("unsafe_code".to_owned(), Value::from("allow"));
    rust.insert("unsafe_op_in_unsafe_fn".to_owned(), Value::from("deny"));
    expected
        .get_mut("clippy")
        .and_then(Value::as_table_mut)
        .expect("workspace clippy lints")
        .insert("undocumented_unsafe_blocks".to_owned(), Value::from("deny"));

    for name in OS_CRATES {
        let own = manifest(&root.join("crates").join(name).join("Cargo.toml"));
        assert_eq!(
            table(&own, "lints"),
            &expected,
            "{name}: [lints] must be the workspace lints with unsafe allowed and documented"
        );
    }
}

#[test]
fn every_other_crate_inherits_the_workspace_lints() {
    let root = workspace_root();
    let mut checked = 0;
    for path in member_manifests(&root) {
        let own = manifest(&path);
        let name = table(&own, "package")
            .get("name")
            .and_then(Value::as_str)
            .expect("package name")
            .to_owned();
        if OS_CRATES.contains(&name.as_str()) {
            continue;
        }
        assert_eq!(
            table(&own, "lints").get("workspace"),
            Some(&Value::Boolean(true)),
            "{name}: only the two OS crates may leave the workspace lints (unsafe is forbidden)"
        );
        checked += 1;
    }
    assert!(checked >= 6, "found only {checked} other crates");
}

#[test]
fn os_dependencies_are_gated_by_target() {
    let root = workspace_root();
    for (name, target, bindings) in [
        (OS_CRATES[0], "cfg(windows)", "windows-sys"),
        (OS_CRATES[1], "cfg(target_os = \"macos\")", "objc2"),
    ] {
        let own = manifest(&root.join("crates").join(name).join("Cargo.toml"));
        let everywhere: Vec<&String> = table(&own, "dependencies").keys().collect();
        assert_eq!(
            everywhere,
            ["iamb-platform"],
            "{name}: OS bindings belong under [target.'{target}'.dependencies]"
        );
        let targets = table(&own, "target");
        assert_eq!(targets.len(), 1, "{name}: one target table");
        let gated = table(table(targets, target), "dependencies");
        assert!(gated.contains_key(bindings), "{name}: {bindings} is gated");
    }
}

#[test]
fn no_networking_namespace_of_the_windows_bindings_is_enabled() {
    let root = workspace_root();
    let own = manifest(&root.join("crates").join(OS_CRATES[0]).join("Cargo.toml"));
    let windows_sys = table(table(table(&own, "target"), "cfg(windows)"), "dependencies")
        .get("windows-sys")
        .and_then(Value::as_table)
        .expect("windows-sys with a feature list");
    assert_eq!(
        windows_sys.get("default-features"),
        None,
        "windows-sys has no default features to turn off"
    );
    let features: Vec<&str> = windows_sys
        .get("features")
        .and_then(Value::as_array)
        .expect("features")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(features.contains(&"Win32_Foundation"));
    let mut sorted = features.clone();
    sorted.sort_unstable();
    assert_eq!(features, sorted, "keep the feature list sorted");
    for feature in features {
        let banned = [
            "Network",
            "WinSock",
            "WinHttp",
            "WinInet",
            "RemoteDesktop",
            "Bluetooth",
            "Rpc",
            "Web",
        ]
        .iter()
        .any(|part| feature.contains(part));
        assert!(
            !banned,
            "{feature} is a networking namespace (architecture §2)"
        );
        assert!(
            feature.starts_with("Win32_"),
            "{feature}: only Win32 namespaces"
        );
    }
}

#[test]
fn the_macos_bindings_have_default_features_off() {
    let root = workspace_root();
    let own = manifest(&root.join("crates").join(OS_CRATES[1]).join("Cargo.toml"));
    let gated = table(
        table(table(&own, "target"), "cfg(target_os = \"macos\")"),
        "dependencies",
    );
    for (name, spec) in gated {
        // `objc2` and `libc` have small defaults (`std`); every framework crate enables its
        // whole framework by default, which must be turned off.
        if name == "objc2" || name == "libc" {
            continue;
        }
        let spec = spec.as_table().unwrap_or_else(|| panic!("{name}: a table"));
        assert_eq!(
            spec.get("default-features"),
            Some(&Value::Boolean(false)),
            "{name}: default features pull in the whole framework"
        );
        let features: Vec<&str> = spec
            .get("features")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("{name}: a feature list"))
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(features.contains(&"std"), "{name}: std");
        for banned in [
            "Network",
            "NSURLSession",
            "NSURLConnection",
            "SecureTransport",
        ] {
            assert!(
                !features.iter().any(|f| f.contains(banned)),
                "{name}: {banned} is networking (architecture §2)"
            );
        }
    }
}
