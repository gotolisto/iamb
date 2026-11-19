//! `iamb-probe --browser` end to end, on every system: the binary is started as the driver
//! starts it, a simulated page talks to its loopback server, and the driver's commands go over
//! standard input and output. Needs the `probe` feature
//! (`cargo test -p iamb-testkit --features probe --test probe_browser`).

#![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use iamb_testkit::compat::latency::{ProbeLink, ProbeProcess};
use iamb_testkit::compat::probe::protocol::{Command, Reply};

fn http(port: u16, method: &str, target: &str, body: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req = format!(
        "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn the_browser_probe_times_a_trial_from_a_simulated_page() {
    let dir = tempfile::tempdir().unwrap();
    let samples = dir.path().join("samples.txt");
    let mut probe = ProbeProcess::spawn(
        Path::new(env!("CARGO_BIN_EXE_iamb-probe")),
        &[
            "--browser".to_owned(),
            "--samples".to_owned(),
            samples.display().to_string(),
        ],
    )
    .unwrap();
    let Reply::Hello { surface, url, .. } = probe.call(&Command::Hello).unwrap() else {
        panic!("no hello");
    };
    assert_eq!(surface, "browser");
    let url = url.unwrap();
    let rest = url.strip_prefix("http://127.0.0.1:").unwrap();
    let (port, base) = rest.split_once('/').unwrap();
    let port: u16 = port.parse().unwrap();
    let base = format!("/{base}");
    assert!(http(port, "GET", &base, "").contains("performance.now()"));

    // The page: polls, and acknowledges each reset.
    let page = {
        let base = base.clone();
        std::thread::spawn(move || {
            let mut seen = 0;
            while seen < 1 {
                let answer = http(port, "GET", &format!("{base}poll?seen={seen}"), "");
                let g: u64 = answer
                    .rsplit("\"generation\":")
                    .next()
                    .and_then(|t| t.trim_end_matches('}').parse().ok())
                    .unwrap_or(0);
                if g > seen {
                    seen = g;
                    let ack = format!("[{{\"k\":\"reset\",\"g\":{g}}}]");
                    http(port, "POST", &format!("{base}events"), &ack);
                }
            }
        })
    };
    assert!(matches!(
        probe
            .call(&Command::WaitReady { timeout_ms: 5000 })
            .unwrap(),
        Reply::Ok { .. }
    ));
    assert_eq!(
        probe.call(&Command::Reset).unwrap(),
        Reply::Ok {
            focused: Some(true)
        }
    );
    page.join().unwrap();
    assert!(matches!(
        probe
            .call(&Command::Expect {
                cell: "t0001-s0003".to_owned(),
                typed: ";laad".to_owned(),
                expected: "two\nlines".to_owned(),
                key_wait_ms: 5000,
            })
            .unwrap(),
        Reply::Ok { .. }
    ));
    let events = r#"[{"k":"key","t":10,"c":";"},{"k":"key","t":11,"c":"l"},
        {"k":"key","t":12,"c":"a"},{"k":"key","t":13,"c":"a"},{"k":"key","t":14.5,"c":"d"},
        {"k":"text","t":14.6,"v":";laad"},{"k":"text","t":20,"v":""},
        {"k":"text","t":31.5,"v":"two\nlines"}]"#;
    assert!(http(port, "POST", &format!("{base}events"), events).contains("200 OK"));
    let Reply::Sample { sample } = probe.call(&Command::Wait { settle_ms: 50 }).unwrap() else {
        panic!("no sample");
    };
    assert!(sample.pass);
    assert_eq!(sample.cell, "t0001-s0003");
    assert_eq!(sample.latency_ns, 17_000_000);
    assert_eq!(sample.stable, Some(true));
    let Reply::Report { report } = probe.call(&Command::Report).unwrap() else {
        panic!("no report");
    };
    assert_eq!(report.sorted_ns, vec![17_000_000]);
    assert_eq!(std::fs::read_to_string(&samples).unwrap(), "17000000\n");
    assert_eq!(
        probe.call(&Command::Quit).unwrap(),
        Reply::Ok { focused: None }
    );
    drop(probe);
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "the server is gone"
    );
}
