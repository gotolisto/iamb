//! A short soak through the real binary, so the harness keeps working (`docs/test-strategy.md`
//! §5): a 12-second run with a small library and fast typing passes every rule, its files are
//! where the docs say, the IPC tap really holds the service's traffic, and the scan finds a
//! canary planted afterwards, reporting it by number, form and location only.

#![cfg(feature = "soak")]

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_iamb-soak");

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn read_dir_bytes(dir: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).expect("dir").flatten() {
        if e.path().is_file() {
            out.extend(std::fs::read(e.path()).expect("readable"));
        }
    }
    out
}

#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "the CI soak runs on Linux; on Windows and macOS run `iamb-soak run` by hand (docs/test-strategy.md §5)"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end run, checked step by step"
)]
fn a_short_soak_passes_and_its_scan_can_fail() {
    // Under target/ (not the system temp directory), so the socket paths stay short.
    let dir = tempfile::TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).expect("temp dir");
    let run = Command::new(BIN)
        .current_dir(dir.path())
        .args([
            "run",
            "--duration",
            "12s",
            "--library",
            "300",
            "--keys-per-second",
            "60",
            "--sample-every",
            "1s",
            "--edit-every",
            "2s",
            "--canary-rounds",
            "1",
            "--seed",
            "42",
            "--quiet",
            "--out",
            "out",
        ])
        .output()
        .expect("iamb-soak runs");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "the smoke soak failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let out = dir.path().join("out");

    let report: Value =
        serde_json::from_slice(&std::fs::read(out.join("report.json")).expect("report"))
            .expect("json");
    assert_eq!(report["passed"], true);
    assert_eq!(report["schema"], "iamb-soak-report/1");
    let recoveries = report["recoveries"].as_array().expect("recoveries");
    assert_eq!(recoveries.len(), 1, "one injected kill");
    assert_eq!(recoveries[0]["recovered"], true);
    assert!(recoveries[0]["probe_ms"].as_u64().expect("ms") < 2000);
    assert!(report["totals"]["expansions_ok"].as_u64().expect("n") > 5);
    assert!(report["totals"]["edits"].as_u64().expect("n") >= 3);
    assert_eq!(report["canaries"].as_array().expect("canaries").len(), 5);
    for rule in report["rules"].as_array().expect("rules") {
        assert_ne!(rule["verdict"], "fail", "{rule}");
    }
    let rule = |name: &str| {
        report["rules"]
            .as_array()
            .expect("rules")
            .iter()
            .find(|r| r["rule"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no rule {name}"))
    };
    // Every shape of every canary was planted and found, and no core file was left.
    let control = rule("canary-scanner-control");
    assert!(
        control["value"]
            .as_str()
            .expect("value")
            .contains("145 of 145 planted shapes found"),
        "{control}"
    );
    assert_eq!(rule("no-crash-dumps")["verdict"], "pass");
    assert!(
        !report["git_sha_source"]
            .as_str()
            .expect("source")
            .is_empty()
    );

    let samples = std::fs::read_to_string(out.join("samples.jsonl")).expect("samples");
    let lines: Vec<Value> = samples
        .lines()
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect();
    assert!(lines.len() >= 10, "{} samples", lines.len());
    assert!(
        lines.iter().any(|s| s["worker_life"] == 2),
        "a sample after the kill"
    );
    assert!(lines[0]["worker"]["rss_kb"].as_u64().expect("rss") > 0);
    assert!(lines[0]["worker"]["threads"].as_u64().expect("threads") > 3);

    // The tap holds the service's frames, both ways, and the log names them.
    let tap = read_dir_bytes(&out.join("work/ipc-tap"));
    for name in [
        &b"\"hello\""[..],
        b"get_status",
        b"put_snippet",
        b"put_exclusion",
        b"welcome",
    ] {
        assert!(
            contains(&tap, name),
            "the tap lacks {}",
            String::from_utf8_lossy(name)
        );
    }
    let log = read_dir_bytes(&out.join("work/data/logs"));
    assert!(contains(&log, b"ipc in put_snippet"));
    // The worker and the supervisor opted out of core-file memory, as the service does.
    assert!(
        contains(&log, b"crash dumps: coredump-filter-0 ok"),
        "the crash-dump line"
    );
    assert!(
        contains(&log, b"exited abnormally"),
        "the supervisor logged the kill"
    );

    // Nothing the harness wrote outside its input holds a canary.
    let canaries: Vec<Value> =
        serde_json::from_slice(&std::fs::read(out.join("input/canaries.json")).expect("input"))
            .expect("json");
    let texts: Vec<String> = canaries
        .iter()
        .map(|c| c["text"].as_str().expect("text").to_owned())
        .collect();
    assert_eq!(texts.len(), 5);
    for file in [
        "report.json",
        "canary-scan.json",
        "summary.txt",
        "samples.jsonl",
    ] {
        let bytes = std::fs::read(out.join(file)).expect("file");
        for t in &texts {
            assert!(!contains(&bytes, t.as_bytes()), "{file} holds a canary");
        }
    }
    for t in &texts {
        assert!(!stdout.contains(t.as_str()), "stdout holds a canary");
    }

    // Plant canary 3 as Base64 of UTF-16LE in a log: the scan must find it and say only where.
    let utf16: Vec<u8> = texts[2].encode_utf16().flat_map(u16::to_le_bytes).collect();
    let planted = base64(&[b"x: ".as_slice(), &utf16].concat());
    std::fs::write(out.join("work/data/logs/planted.log"), &planted).expect("plant");
    // And canary 4 one key per line, as a log that records every key would hold it.
    let mut per_key = String::new();
    for (n, c) in texts[3].chars().enumerate() {
        let _ = writeln!(
            per_key,
            "2026-11-18T22:35:55.{n:03}Z INFO worker[7]: key {c:?} seq {n}"
        );
    }
    std::fs::write(out.join("work/data/logs/per-key.log"), per_key).expect("plant");
    let scan = Command::new(BIN)
        .current_dir(dir.path())
        .args([
            "scan",
            "--canaries",
            "out/input/canaries.json",
            "--location",
            "data-root=out/work/data",
            "--location",
            "ipc-tap=out/work/ipc-tap",
        ])
        .output()
        .expect("scan runs");
    let printed = String::from_utf8_lossy(&scan.stdout);
    assert_eq!(scan.status.code(), Some(1), "{printed}");
    assert!(
        printed.contains(
            "HIT canary #3 (secure-field) as base64-utf16le in data-root/logs/planted.log"
        ),
        "{printed}"
    );
    assert!(
        printed.contains("HIT canary #4 (excluded-app) as per-line in data-root/logs/per-key.log"),
        "{printed}"
    );
    assert_eq!(printed.matches("HIT").count(), 2, "{printed}");
    for t in &texts {
        assert!(!printed.contains(t.as_str()), "the scan printed a canary");
    }
}

/// Standard Base64 with padding.
fn base64(bytes: &[u8]) -> Vec<u8> {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[usize::try_from((n >> (18 - 6 * i)) & 63).expect("small")]);
            } else {
                out.push(b'=');
            }
        }
    }
    out
}

#[test]
fn the_network_check_finds_no_networking_crate() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new(BIN)
        .args(["net-check", "--workspace"])
        .arg(&workspace)
        .output()
        .expect("net-check runs");
    let printed = String::from_utf8_lossy(&out.stdout);
    // Without the registry (an offline machine) cargo tree cannot run: that is not a finding.
    if printed.contains("could not check") {
        return;
    }
    assert!(out.status.success(), "{printed}");
    assert!(printed.contains("no networking crate"), "{printed}");
}
