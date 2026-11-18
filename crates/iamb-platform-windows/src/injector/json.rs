//! A small, strict JSON reader for the strategy overrides kept in settings.
//!
//! The crate has no JSON dependency, and the overrides are a few dozen bytes of settings, so a
//! reader of standard JSON (RFC 8259) is kept here instead: objects, arrays, strings with every
//! escape (surrogate pairs combined, lone surrogates refused), numbers, `true`, `false` and
//! `null`. Duplicate object keys and anything after the value are errors, nesting is limited,
//! and every error is a fixed message that quotes nothing from the input.

/// A JSON value. Objects keep their members in input order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

/// Nesting deeper than this is refused; the overrides need three levels.
const MAX_DEPTH: usize = 16;

/// Parses one JSON document.
pub(crate) fn parse(text: &str) -> Result<Json, &'static str> {
    let mut reader = Reader {
        bytes: text.as_bytes(),
        at: 0,
    };
    let value = reader.value(0)?;
    reader.space();
    if reader.at == reader.bytes.len() {
        Ok(value)
    } else {
        Err("unexpected text after the JSON value")
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, &'static str> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err("invalid JSON literal")
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, &'static str> {
        if depth > MAX_DEPTH {
            return Err("JSON nested too deeply");
        }
        self.space();
        match self.peek() {
            None => Err("unexpected end of JSON"),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Json::String),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err("unexpected character in JSON"),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, &'static str> {
        self.at += 1;
        let mut members: Vec<(String, Json)> = Vec::new();
        self.space();
        if self.eat(b'}') {
            return Ok(Json::Object(members));
        }
        loop {
            self.space();
            if self.peek() != Some(b'"') {
                return Err("expected a string key in a JSON object");
            }
            let key = self.string()?;
            if members.iter().any(|(k, _)| *k == key) {
                return Err("duplicate key in a JSON object");
            }
            self.space();
            if !self.eat(b':') {
                return Err("expected ':' in a JSON object");
            }
            let value = self.value(depth + 1)?;
            members.push((key, value));
            self.space();
            if self.eat(b'}') {
                return Ok(Json::Object(members));
            }
            if !self.eat(b',') {
                return Err("expected ',' or '}' in a JSON object");
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, &'static str> {
        self.at += 1;
        let mut items = Vec::new();
        self.space();
        if self.eat(b']') {
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.space();
            if self.eat(b']') {
                return Ok(Json::Array(items));
            }
            if !self.eat(b',') {
                return Err("expected ',' or ']' in a JSON array");
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, &'static str> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .ok_or("unexpected end of a JSON escape")?;
        let mut value = 0;
        for &d in digits {
            let digit = char::from(d)
                .to_digit(16)
                .ok_or("invalid \\u escape in JSON")?;
            value = value * 16 + digit;
        }
        self.at += 4;
        Ok(value)
    }

    fn string(&mut self) -> Result<String, &'static str> {
        self.at += 1;
        let mut out = String::new();
        loop {
            // Copy the run of plain characters up to the next quote, escape or control byte.
            let start = self.at;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.at += 1;
            }
            // The input is a `str` and the run stops only at ASCII bytes, so it is whole
            // UTF-8.
            out.push_str(
                std::str::from_utf8(&self.bytes[start..self.at])
                    .map_err(|_| "invalid UTF-8 in JSON")?,
            );
            match self.peek() {
                None => return Err("unterminated JSON string"),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escape = self.peek().ok_or("unterminated JSON string")?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        _ => return Err("invalid escape in a JSON string"),
                    }
                }
                Some(_) => return Err("control character in a JSON string"),
            }
        }
    }

    /// The character of a `\u` escape (the `\u` already read), combining a surrogate pair.
    fn unicode_escape(&mut self) -> Result<char, &'static str> {
        let first = self.hex4()?;
        let code = match first {
            0xD800..=0xDBFF => {
                if !(self.eat(b'\\') && self.eat(b'u')) {
                    return Err("lone surrogate in a JSON string");
                }
                let second = self.hex4()?;
                if !(0xDC00..=0xDFFF).contains(&second) {
                    return Err("lone surrogate in a JSON string");
                }
                0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
            }
            0xDC00..=0xDFFF => return Err("lone surrogate in a JSON string"),
            _ => first,
        };
        char::from_u32(code).ok_or("invalid \\u escape in JSON")
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Result<Json, &'static str> {
        let start = self.at;
        self.eat(b'-');
        if self.eat(b'0') {
            if matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err("leading zero in a JSON number");
            }
        } else if self.digits() == 0 {
            return Err("invalid JSON number");
        }
        if self.eat(b'.') && self.digits() == 0 {
            return Err("invalid JSON number");
        }
        if self.eat(b'e') || self.eat(b'E') {
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if self.digits() == 0 {
                return Err("invalid JSON number");
            }
        }
        std::str::from_utf8(&self.bytes[start..self.at])
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|n| n.is_finite())
            .map(Json::Number)
            .ok_or("invalid JSON number")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(members: &[(&str, Json)]) -> Json {
        Json::Object(
            members
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn reads_every_kind_of_value() {
        let parsed = parse(
            r#" { "a": [1, -2.5, 3e2, 0], "b": {"c": true, "d": false, "e": null}, "f": "x" } "#,
        );
        assert_eq!(
            parsed,
            Ok(obj(&[
                (
                    "a",
                    Json::Array(vec![
                        Json::Number(1.0),
                        Json::Number(-2.5),
                        Json::Number(300.0),
                        Json::Number(0.0),
                    ])
                ),
                (
                    "b",
                    obj(&[
                        ("c", Json::Bool(true)),
                        ("d", Json::Bool(false)),
                        ("e", Json::Null)
                    ])
                ),
                ("f", Json::String("x".to_owned())),
            ]))
        );
        assert_eq!(parse("[]"), Ok(Json::Array(Vec::new())));
        assert_eq!(parse("{}"), Ok(Json::Object(Vec::new())));
    }

    #[test]
    fn decodes_string_escapes() {
        assert_eq!(
            parse(r#""a\"b\\c\/d\b\f\n\r\t""#),
            Ok(Json::String("a\"b\\c/d\u{8}\u{c}\n\r\t".to_owned()))
        );
        assert_eq!(
            parse(r#""caf\u00e9 \ud83d\udc4d ü""#),
            Ok(Json::String("café 👍 ü".to_owned()))
        );
    }

    #[test]
    fn refuses_what_is_not_json() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "{\"a\" 1}",
            "{a:1}",
            "{\"a\":1}{",
            "01",
            "1.",
            "-",
            "1e",
            "tru",
            "nul",
            "\"open",
            "\"\\x\"",
            "\"\\ud800\"",
            "\"\\udc00\"",
            "\"\\ud800\\u0041\"",
            "\"\\u12\"",
            "\"tab\there\"",
            "{\"a\":1,\"a\":2}",
            "'single'",
            "1e999",
        ] {
            assert!(parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn limits_nesting() {
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        assert_eq!(parse(&deep), Err("JSON nested too deeply"));
        let fine = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(&fine).is_ok());
    }

    #[test]
    fn errors_quote_nothing_from_the_input() {
        let err = parse("{\"secret-app.exe\": nope}").expect_err("invalid");
        assert!(!err.contains("secret"));
    }
}
