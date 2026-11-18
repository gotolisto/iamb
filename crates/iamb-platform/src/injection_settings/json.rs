//! A small, strict JSON reader for the injection settings (RFC 8259).
//!
//! The settings arrive as a JSON string. This crate has no JSON dependency, and the document is
//! tiny (a few dozen fields), so a reader of about two hundred lines is enough:
//! objects, arrays, strings with every escape, `true`, `false`, `null`, and numbers. Limits
//! keep a damaged setting from costing memory or stack: at most [`MAX_INPUT`] bytes and
//! [`MAX_DEPTH`] levels of nesting. Duplicate object keys are an error, because which one wins
//! is not defined.
//!
//! Errors carry a `&'static str` and a byte offset, never the input itself.

use std::fmt;

/// The longest settings document accepted, in bytes.
pub(crate) const MAX_INPUT: usize = 64 * 1024;
/// The deepest nesting accepted.
pub(crate) const MAX_DEPTH: usize = 16;

/// A JSON value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    /// A number, as written (validated, not converted).
    Number(String),
    String(String),
    Array(Vec<Value>),
    /// An object, in document order.
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The value as a non-negative integer, if it is one that fits.
    pub(crate) fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(text) if text.bytes().all(|b| b.is_ascii_digit()) => text.parse().ok(),
            _ => None,
        }
    }
}

/// Why a document was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct JsonError {
    /// What was wrong. Never user data.
    pub(crate) what: &'static str,
    /// The byte offset where it was found.
    pub(crate) at: usize,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.what, self.at)
    }
}

/// Parses one JSON document.
pub(crate) fn parse(input: &str) -> Result<Value, JsonError> {
    if input.len() > MAX_INPUT {
        return Err(JsonError {
            what: "the settings are longer than 64 KiB",
            at: MAX_INPUT,
        });
    }
    let mut reader = Reader {
        bytes: input.as_bytes(),
        at: 0,
    };
    let value = reader.value(0)?;
    reader.skip_space();
    if reader.at != reader.bytes.len() {
        return Err(reader.error("trailing characters"));
    }
    Ok(value)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    const fn error(&self, what: &'static str) -> JsonError {
        JsonError { what, at: self.at }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8, what: &'static str) -> Result<(), JsonError> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.error(what))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, JsonError> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("malformed JSON: unknown literal"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.error("malformed JSON: nested too deeply"));
        }
        self.skip_space();
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::String),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("malformed JSON: unexpected character")),
            None => Err(self.error("malformed JSON: unexpected end")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.at += 1;
        let mut fields: Vec<(String, Value)> = Vec::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Value::Object(fields));
        }
        loop {
            self.skip_space();
            if self.peek() != Some(b'"') {
                return Err(self.error("malformed JSON: expected a key"));
            }
            let key_at = self.at;
            let key = self.string()?;
            if fields.iter().any(|(k, _)| *k == key) {
                return Err(JsonError {
                    what: "malformed JSON: duplicate key",
                    at: key_at,
                });
            }
            self.skip_space();
            self.expect(b':', "expected ':'")?;
            let value = self.value(depth + 1)?;
            fields.push((key, value));
            self.skip_space();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Value::Object(fields));
                }
                _ => return Err(self.error("malformed JSON: expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.at += 1;
        let mut items = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("malformed JSON: expected ',' or ']'")),
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Result<Value, JsonError> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        let int_start = self.at;
        let int_len = self.digits();
        if int_len == 0 || (int_len > 1 && self.bytes[int_start] == b'0') {
            return Err(self.error("malformed JSON: bad number"));
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if self.digits() == 0 {
                return Err(self.error("malformed JSON: bad number"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                return Err(self.error("malformed JSON: bad number"));
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at])
            .map_err(|_| self.error("malformed JSON: bad number"))?;
        Ok(Value::Number(text.to_owned()))
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .ok_or_else(|| self.error("malformed JSON: short \\u escape"))?;
        let mut value = 0;
        for &d in digits {
            let nibble = char::from(d)
                .to_digit(16)
                .ok_or_else(|| self.error("malformed JSON: bad \\u escape"))?;
            value = value * 16 + nibble;
        }
        self.at += 4;
        Ok(value)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let start = self.at;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.at += 1;
            }
            // The input is a `str` and the stops are ASCII, so the run is valid UTF-8.
            out.push_str(
                std::str::from_utf8(&self.bytes[start..self.at])
                    .map_err(|_| self.error("malformed JSON: invalid text"))?,
            );
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escape = self
                        .peek()
                        .ok_or_else(|| self.error("malformed JSON: unexpected end"))?;
                    self.at += 1;
                    let c = match escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.unicode_escape()?,
                        _ => return Err(self.error("malformed JSON: bad escape")),
                    };
                    out.push(c);
                }
                Some(_) => return Err(self.error("malformed JSON: control character in string")),
                None => return Err(self.error("malformed JSON: unterminated string")),
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, JsonError> {
        let first = self.hex4()?;
        let code = if (0xD800..0xDC00).contains(&first) {
            if !self.bytes[self.at..].starts_with(b"\\u") {
                return Err(self.error("malformed JSON: lone surrogate"));
            }
            self.at += 2;
            let second = self.hex4()?;
            if !(0xDC00..0xE000).contains(&second) {
                return Err(self.error("malformed JSON: lone surrogate"));
            }
            0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
        } else {
            first
        };
        char::from_u32(code).ok_or_else(|| self.error("malformed JSON: lone surrogate"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(fields: &[(&str, Value)]) -> Value {
        Value::Object(
            fields
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn reads_every_kind_of_value() {
        let doc = r#" { "a": [1, -2.5e3, true, false, null], "b": {"c": "d\u00e9\n\"\\\/"},
                       "e": "\ud83d\ude00", "f": [] , "g": {} } "#;
        assert_eq!(
            parse(doc),
            Ok(obj(&[
                (
                    "a",
                    Value::Array(vec![
                        Value::Number("1".to_owned()),
                        Value::Number("-2.5e3".to_owned()),
                        Value::Bool(true),
                        Value::Bool(false),
                        Value::Null,
                    ])
                ),
                ("b", obj(&[("c", Value::String("dé\n\"\\/".to_owned()))])),
                ("e", Value::String("😀".to_owned())),
                ("f", Value::Array(Vec::new())),
                ("g", Value::Object(Vec::new())),
            ]))
        );
        assert_eq!(
            parse("\"Café 🇯🇵\""),
            Ok(Value::String("Café 🇯🇵".to_owned()))
        );
    }

    #[test]
    fn integers() {
        assert_eq!(parse("42").ok().and_then(|v| v.as_u64()), Some(42));
        assert_eq!(parse("0").ok().and_then(|v| v.as_u64()), Some(0));
        for not_one in ["-1", "1.5", "1e3", "\"7\"", "99999999999999999999999"] {
            assert_eq!(
                parse(not_one).ok().and_then(|v| v.as_u64()),
                None,
                "{not_one}"
            );
        }
    }

    #[test]
    fn rejects_malformed_documents() {
        for bad in [
            "",
            "{",
            "}",
            "[1,]",
            "{\"a\":1,}",
            "{\"a\" 1}",
            "{a:1}",
            "01",
            "1.",
            "-",
            "1e",
            "tru",
            "nul",
            "\"abc",
            "\"\\x\"",
            "\"\\u12\"",
            "\"\\ud800\"",
            "\"\\udc00\"",
            "\"a\nb\"",
            "1 2",
            "{\"a\":1,\"a\":2}",
            "[1 2]",
            "{\"a\":1}{",
            "'single'",
            "\"\\ud800\\u0041\"",
            "\"tab\there\"",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn limits_depth_and_size() {
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        assert_eq!(
            parse(&deep).map_err(|e| e.what),
            Err("malformed JSON: nested too deeply")
        );
        let fine = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(&fine).is_ok());
        let long = format!("\"{}\"", "a".repeat(MAX_INPUT));
        assert_eq!(
            parse(&long).map_err(|e| e.what),
            Err("the settings are longer than 64 KiB")
        );
    }

    #[test]
    fn errors_name_the_place_not_the_content() {
        let error = parse("{\"secret\": nope}").expect_err("invalid");
        assert_eq!(error.what, "malformed JSON: unknown literal");
        assert_eq!(error.at, 11);
        assert_eq!(
            error.to_string(),
            "malformed JSON: unknown literal at byte 11"
        );
    }
}
