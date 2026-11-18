//! Writing results to standard output.

use std::fmt::Write as _;
use std::io::{self, Write};

/// Writes `text` and a newline to standard output. A closed pipe is not an error.
pub fn line(text: &str) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{text}");
}

/// Quotes `s` as a JSON string.
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
