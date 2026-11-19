//! The control channel between the driver and the probe: one JSON object per line, the driver's
//! commands on the probe's standard input, one reply per command on its standard output.
//!
//! | Command | Does | Reply |
//! | --- | --- | --- |
//! | `{"op":"hello"}` | Who are you | `hello`: process id, surface (`native` or `browser`), the page's URL |
//! | `{"op":"wait_ready","timeout_ms":N}` | Wait until the surface can be typed into (the browser page has connected) | `ok`, or `error` after the timeout |
//! | `{"op":"reset"}` | Cancel any armed trial, empty the control and give it the focus | `ok` with `focused` |
//! | `{"op":"expect","cell":C,"typed":K,"expected":E,"key_wait_ms":N}` | Arm a trial: `K` is about to be typed, then the control must hold exactly `E` | `ok` |
//! | `{"op":"wait","settle_ms":N}` | Wait until the armed trial finishes; after a match, wait `N` ms more and check the text is still `E` | `sample` |
//! | `{"op":"report"}` | Every trial so far, in the `LatencyRecorder` format | `report` |
//! | `{"op":"quit"}` | Close the probe | `ok`, then the probe exits |
//!
//! Any failure is answered with `{"reply":"error","message":…}`. Only the driver's commands
//! carry text (`typed`, `expected`, from its fixture); no reply does, and `Debug` of a command
//! gives lengths only.

use serde::{Deserialize, Serialize};

use super::session::{Report, Sample};

fn default_key_wait_ms() -> u64 {
    u64::try_from(super::session::DEFAULT_KEY_WAIT.as_millis()).unwrap_or(10_000)
}

/// What the driver asks.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// Who are you. → [`Reply::Hello`]
    Hello,
    /// Wait until the surface can be typed into. → [`Reply::Ok`] or [`Reply::Error`]
    WaitReady {
        /// How long to wait, ms.
        timeout_ms: u64,
    },
    /// Cancel any armed trial, empty the control and focus it. → [`Reply::Ok`]
    Reset,
    /// Arm a trial. → [`Reply::Ok`]
    Expect {
        /// The driver's id of the trial.
        cell: String,
        /// The keys the driver is about to type.
        typed: String,
        /// The control's whole text once the expansion is complete.
        expected: String,
        /// How long the last key may take to arrive, ms.
        #[serde(default = "default_key_wait_ms")]
        key_wait_ms: u64,
    },
    /// Wait for the armed trial. → [`Reply::Sample`]
    Wait {
        /// After a match, how long to keep watching the text, ms (0: no check).
        #[serde(default)]
        settle_ms: u64,
    },
    /// Every trial so far. → [`Reply::Report`]
    Report,
    /// Close the probe. → [`Reply::Ok`]
    Quit,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hello => f.write_str("Hello"),
            Self::WaitReady { timeout_ms } => f
                .debug_struct("WaitReady")
                .field("timeout_ms", timeout_ms)
                .finish(),
            Self::Reset => f.write_str("Reset"),
            Self::Expect {
                cell,
                typed,
                expected,
                key_wait_ms,
            } => f
                .debug_struct("Expect")
                .field("cell", cell)
                .field("typed_chars", &typed.chars().count())
                .field("expected_chars", &expected.chars().count())
                .field("key_wait_ms", key_wait_ms)
                .finish(),
            Self::Wait { settle_ms } => f
                .debug_struct("Wait")
                .field("settle_ms", settle_ms)
                .finish(),
            Self::Report => f.write_str("Report"),
            Self::Quit => f.write_str("Quit"),
        }
    }
}

/// What the probe answers. Holds no typed or expected text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    /// The probe.
    Hello {
        /// Its process id.
        pid: u32,
        /// `native` or `browser`.
        surface: String,
        /// The page to open, for the browser surface.
        #[serde(default)]
        url: Option<String>,
    },
    /// Done.
    Ok {
        /// After a reset: whether the control has the keyboard focus.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focused: Option<bool>,
    },
    /// A finished trial.
    Sample {
        /// The trial.
        sample: Sample,
    },
    /// Every trial so far.
    Report {
        /// The trials and the recorder's samples.
        report: Report,
    },
    /// The command failed.
    Error {
        /// Why.
        message: String,
    },
}

/// Parses one command line.
pub fn parse_command(line: &str) -> Result<Command, String> {
    // The error of serde_json can quote the input, which may hold fixture text: say where only.
    serde_json::from_str(line).map_err(|e| {
        format!(
            "not a command (line {}, column {}): {:?}",
            e.line(),
            e.column(),
            e.classify()
        )
    })
}

/// One line of JSON (no newline).
pub fn encode<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| {
        "{\"reply\":\"error\",\"message\":\"cannot encode the reply\"}".to_owned()
    })
}

/// Parses one reply line.
pub fn parse_reply(line: &str) -> Result<Reply, String> {
    serde_json::from_str(line).map_err(|e| format!("not a reply from the probe: {e}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

    use super::*;

    #[test]
    fn commands_round_trip_as_json_lines() {
        let all = [
            Command::Hello,
            Command::WaitReady { timeout_ms: 5 },
            Command::Reset,
            Command::Expect {
                cell: "L0001".to_owned(),
                typed: ";labc".to_owned(),
                expected: "line one\nline \"two\"".to_owned(),
                key_wait_ms: 3000,
            },
            Command::Wait { settle_ms: 300 },
            Command::Report,
            Command::Quit,
        ];
        for c in all {
            let line = encode(&c);
            assert!(!line.contains('\n'), "{line}");
            assert_eq!(parse_command(&line).unwrap(), c);
        }
        assert_eq!(
            encode(&Command::Reset),
            "{\"op\":\"reset\"}",
            "the wire form is documented"
        );
    }

    #[test]
    fn defaults_and_errors() {
        let c = parse_command(r#"{"op":"expect","cell":"c","typed":"k","expected":"v"}"#).unwrap();
        assert_eq!(
            c,
            Command::Expect {
                cell: "c".to_owned(),
                typed: "k".to_owned(),
                expected: "v".to_owned(),
                key_wait_ms: 10_000
            }
        );
        assert_eq!(
            parse_command(r#"{"op":"wait"}"#).unwrap(),
            Command::Wait { settle_ms: 0 }
        );
        let err = parse_command(r#"{"op":"type","text":"qzsecretcanary"}"#).unwrap_err();
        assert!(!err.contains("qzsecret"), "{err}");
        assert!(parse_command("not json").is_err());
    }

    #[test]
    fn replies_round_trip_and_debug_hides_text() {
        let r = Reply::Ok {
            focused: Some(true),
        };
        assert_eq!(parse_reply(&encode(&r)).unwrap(), r);
        assert_eq!(encode(&Reply::Ok { focused: None }), "{\"reply\":\"ok\"}");
        let c = Command::Expect {
            cell: "L1".to_owned(),
            typed: ";secret".to_owned(),
            expected: "secret body".to_owned(),
            key_wait_ms: 1,
        };
        assert!(!format!("{c:?}").contains("secret"));
    }
}
