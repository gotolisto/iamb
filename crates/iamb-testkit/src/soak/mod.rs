//! The long-run (soak) harness of task 2.6 and the canary privacy scan of task 2.5
//! (`docs/test-strategy.md` §5), CI form.
//!
//! Three processes, all started from the `iamb-soak` binary:
//!
//! | Process | Runs | Module |
//! | --- | --- | --- |
//! | Driver (`iamb-soak run`) | Seeds the library, types the key stream, edits the library, samples, kills the worker, scans for canaries, applies the pass rules | [`driver`] |
//! | Supervisor (`iamb-soak serve-supervisor`) | [`iamb_service::supervisor::run`], unchanged, with this binary as the worker executable | [`host`] |
//! | Worker (`iamb-soak serve-worker`) | [`iamb_service::worker::Worker`], unchanged, on the mock platform, serving the real service endpoint through an IPC tap, plus a control endpoint the driver types through | [`host`] |
//!
//! The service's own binary cannot be used as is: its mock platform lives inside the worker
//! process, so another process cannot type into it, and its supervisor always starts its own
//! executable. The two `serve-*` commands therefore do what `iamb-service --supervise` and
//! `iamb-service --worker --platform mock --heartbeat-stdout` do, with the same library code
//! (supervisor loop, backoff, heartbeat, worker threads, store, IPC server, log), and add only
//! the test hooks: the control endpoint and the frame tap. Nothing in `iamb-service`,
//! `iamb-ipc` or `iamb-platform` changes for them.
//!
//! | Module | Does |
//! | --- | --- |
//! | [`rng`] | The seeded generator |
//! | [`fixture`] | The fixture library and the key stream, with the exact text each step must leave |
//! | [`control`] | The driver ↔ worker control protocol (type, focus, settle, read the field) |
//! | [`tap`] | A [`LocalEndpoint`](iamb_platform::LocalEndpoint) wrapper that copies every byte of every IPC connection to files |
//! | [`canary`] | Canaries, their UTF-8, UTF-16LE and Base64 forms, and the scanner |
//! | [`probe`] | Process samples: memory, CPU time, handles or descriptors, threads |
//! | [`rules`] | The pass rules of §5.2, scaled to the run length |
//! | [`netcheck`] | The service links no networking crate (`cargo tree`) |
//! | [`host`] | The supervisor and worker processes |
//! | [`driver`] | The run |
//! | [`cli`] | The `iamb-soak` command line |

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    reason = "test tooling: errors are readable strings for the person running the harness"
)]

pub mod canary;
pub mod cli;
pub mod control;
pub mod driver;
pub mod fixture;
pub mod host;
pub mod netcheck;
pub mod probe;
pub mod rng;
pub mod rules;
pub mod tap;

use std::time::Duration;

/// Parses a duration such as `30m`, `72h`, `90s`, `1500ms` or `1h30m` (no unit: seconds).
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("empty duration".to_owned());
    }
    if let Ok(secs) = text.parse::<u64>() {
        return Ok(Duration::from_secs(secs));
    }
    let mut total = Duration::ZERO;
    let mut rest = text;
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits == 0 {
            return Err(format!("{text:?}: expected a number"));
        }
        let n: u64 = rest[..digits]
            .parse()
            .map_err(|_| format!("{text:?}: number too large"))?;
        rest = &rest[digits..];
        let unit = rest
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len());
        let part = match &rest[..unit] {
            "ms" => Duration::from_millis(n),
            "s" => Duration::from_secs(n),
            "m" | "min" => Duration::from_secs(n.saturating_mul(60)),
            "h" => Duration::from_secs(n.saturating_mul(3600)),
            "d" => Duration::from_secs(n.saturating_mul(86_400)),
            other => return Err(format!("{text:?}: unknown unit {other:?} (ms, s, m, h, d)")),
        };
        total = total.saturating_add(part);
        rest = &rest[unit..];
    }
    Ok(total)
}

/// Milliseconds of `d`, saturating.
pub(crate) fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Milliseconds since the Unix epoch.
pub(crate) fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("90"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("30m"), Ok(Duration::from_mins(30)));
        assert_eq!(parse_duration("72h"), Ok(Duration::from_hours(72)));
        assert_eq!(parse_duration("1h30m"), Ok(Duration::from_mins(90)));
        assert_eq!(parse_duration("1500ms"), Ok(Duration::from_millis(1500)));
        assert!(parse_duration("").is_err());
        assert!(parse_duration("m").is_err());
        assert!(parse_duration("3w").is_err());
    }
}
