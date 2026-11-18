//! The CI stand-in for the lab's packet capture (`docs/test-strategy.md` §5.3): the service
//! binary links no networking crate.
//!
//! It asks Cargo for the normal (non-dev, non-build) dependency tree of `iamb-service` on every
//! target (`cargo tree -p iamb-service -e normal --target all`) and checks each package name
//! against [`DENY`]. `crates/iamb-service/tests/no_network.rs` checks the same list against
//! `Cargo.lock` and the sources; this check reads what Cargo actually resolves for the binary,
//! features included. The lab run adds the capture of the machine's traffic.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Crates that open network connections or implement network protocols (the list of
/// `crates/iamb-service/tests/no_network.rs`).
pub const DENY: &[&str] = &[
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

/// What the check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetCheck {
    /// The command run.
    pub command: String,
    /// Packages in the tree.
    pub packages: usize,
    /// Denied packages found.
    pub denied: Vec<String>,
    /// Why the check could not run, if it could not.
    pub error: Option<String>,
}

impl NetCheck {
    /// The check ran and found nothing.
    pub fn passed(&self) -> bool {
        self.error.is_none() && self.denied.is_empty() && self.packages > 0
    }
}

/// Package names in `cargo tree --prefix none --format {p}` output.
pub fn package_names(tree: &str) -> BTreeSet<String> {
    tree.lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The names of `names` that are on [`DENY`].
pub fn denied(names: &BTreeSet<String>) -> Vec<String> {
    DENY.iter()
        .filter(|d| names.contains(**d))
        .map(|d| (*d).to_owned())
        .collect()
}

/// Runs the check in the workspace at `workspace`.
pub fn run(workspace: &Path) -> NetCheck {
    let args = [
        "tree",
        "--locked",
        "-p",
        "iamb-service",
        "-e",
        "normal",
        "--target",
        "all",
        "--prefix",
        "none",
        "--format",
        "{p}",
    ];
    let command = format!("cargo {}", args.join(" "));
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .args(args)
        .current_dir(workspace)
        .output();
    let mut check = NetCheck {
        command,
        packages: 0,
        denied: Vec::new(),
        error: None,
    };
    match out {
        Ok(out) if out.status.success() => {
            let names = package_names(&String::from_utf8_lossy(&out.stdout));
            check.packages = names.len();
            check.denied = denied(&names);
            if !names.contains("iamb-store") {
                check.error = Some("the tree lacks iamb-store: not the service's tree".to_owned());
            }
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            check.error = Some(format!(
                "cargo tree failed ({}): {}",
                out.status,
                err.lines().last().unwrap_or("")
            ));
        }
        Err(e) => check.error = Some(format!("cannot run cargo: {e}")),
    }
    check
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_the_first_word() {
        let names = package_names(
            "iamb-service v0.0.0 (/w/crates/iamb-service)\nserde v1.0.229\n\nserde v1.0.229\n",
        );
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            ["iamb-service", "serde"]
        );
    }

    #[test]
    fn a_networking_crate_is_found() {
        let names = package_names("iamb-service v0.0.0\ntokio v1.0.0\nmio v1.0.0\nserde v1.0.0\n");
        assert_eq!(denied(&names), ["mio", "tokio"]);
    }

    #[test]
    fn the_list_matches_the_service_test() {
        let test = include_str!("../../../iamb-service/tests/no_network.rs");
        for name in DENY {
            assert!(
                test.contains(&format!("\"{name}\"")),
                "{name} is in both lists"
            );
        }
    }
}
