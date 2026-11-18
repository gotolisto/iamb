//! Minimal JSON for flat objects: the bench result and baseline lines.
//!
//! Both files hold one object per line whose values are strings, numbers, booleans or `null`,
//! so a small hand-written parser is enough and xtask needs no serialisation dependency.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A scalar JSON value. Numbers keep their source text and are converted on use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
}

/// One parsed line: field name to value.
pub(crate) type Object = BTreeMap<String, Value>;

/// Parses one JSON object whose values are all scalars. Nested objects and arrays are refused.
pub(crate) fn parse_flat_object(text: &str) -> Result<Object, String> {
    let mut p = Parser { s: text, pos: 0 };
    p.ws();
    p.expect('{')?;
    let mut out = Object::new();
    p.ws();
    if p.eat('}') {
        return p.end(out);
    }
    loop {
        p.ws();
        let key = p.string()?;
        p.ws();
        p.expect(':')?;
        p.ws();
        let value = p.value()?;
        if out.insert(key.clone(), value).is_some() {
            return Err(format!("duplicate field `{key}`"));
        }
        p.ws();
        if p.eat(',') {
            continue;
        }
        p.expect('}')?;
        return p.end(out);
    }
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\r' | '\n')) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(format!("expected `{c}` at byte {}", self.pos))
        }
    }

    fn end(&mut self, out: Object) -> Result<Object, String> {
        self.ws();
        if self.pos == self.s.len() {
            Ok(out)
        } else {
            Err(format!("trailing text at byte {}", self.pos))
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            Some('"') => self.string().map(Value::Str),
            Some('n') => self.word("null", Value::Null),
            Some('t') => self.word("true", Value::Bool(true)),
            Some('f') => self.word("false", Value::Bool(false)),
            Some(c) if c == '-' || c.is_ascii_digit() => Ok(self.number()),
            Some('{' | '[') => Err(format!(
                "nested value at byte {}: only flat objects are supported",
                self.pos
            )),
            _ => Err(format!("expected a value at byte {}", self.pos)),
        }
    }

    fn word(&mut self, w: &str, v: Value) -> Result<Value, String> {
        if self.s[self.pos..].starts_with(w) {
            self.pos += w.len();
            Ok(v)
        } else {
            Err(format!("expected `{w}` at byte {}", self.pos))
        }
    }

    fn number(&mut self) -> Value {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'))
        {
            self.pos += 1;
        }
        Value::Num(self.s[start..self.pos].to_owned())
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err("unterminated string".to_owned()),
                Some('"') => return Ok(out),
                Some('\\') => {
                    let c = match self.bump() {
                        Some('"') => '"',
                        Some('\\') => '\\',
                        Some('/') => '/',
                        Some('b') => '\u{8}',
                        Some('f') => '\u{c}',
                        Some('n') => '\n',
                        Some('r') => '\r',
                        Some('t') => '\t',
                        Some('u') => self.unicode_escape()?,
                        _ => return Err(format!("bad escape at byte {}", self.pos)),
                    };
                    out.push(c);
                }
                Some(c) => out.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let hex = self
            .s
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| "short \\u escape".to_owned())?;
        let v = u32::from_str_radix(hex, 16).map_err(|_| format!("bad \\u escape `{hex}`"))?;
        self.pos += 4;
        Ok(v)
    }

    fn unicode_escape(&mut self) -> Result<char, String> {
        let hi = self.hex4()?;
        let code = if (0xD800..0xDC00).contains(&hi) {
            if !(self.eat('\\') && self.eat('u')) {
                return Err("unpaired surrogate in \\u escape".to_owned());
            }
            let lo = self.hex4()?;
            if !(0xDC00..0xE000).contains(&lo) {
                return Err("unpaired surrogate in \\u escape".to_owned());
            }
            0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
        } else {
            hi
        };
        char::from_u32(code).ok_or_else(|| format!("bad \\u escape {code:#x}"))
    }
}

/// Quotes and escapes a string as a JSON string literal.
pub(crate) fn quote(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::{Value, parse_flat_object, quote};

    #[test]
    fn parses_a_result_line() {
        let o = parse_flat_object(
            r#"{"schema":1,"bench":"a\/b \"x\" é😀","kpi":null,"pass":true,"p50":-1.5e3}"#,
        )
        .expect("valid");
        assert_eq!(o["schema"], Value::Num("1".to_owned()));
        assert_eq!(
            o["bench"],
            Value::Str("a/b \"x\" \u{e9}\u{1f600}".to_owned())
        );
        assert_eq!(o["kpi"], Value::Null);
        assert_eq!(o["pass"], Value::Bool(true));
        assert_eq!(o["p50"], Value::Num("-1.5e3".to_owned()));
    }

    #[test]
    fn refuses_bad_input() {
        for bad in [
            "",
            "{",
            r#"{"a":1,}"#,
            r#"{"a":[1]}"#,
            r#"{"a":1} x"#,
            r#"{"a":1,"a":2}"#,
            r#"{"a":"\q"}"#,
        ] {
            assert!(parse_flat_object(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn quote_round_trips() {
        let s = "a\"b\\c\nd\u{1}é";
        let o = parse_flat_object(&format!("{{\"k\":{}}}", quote(s))).expect("valid");
        assert_eq!(o["k"], Value::Str(s.to_owned()));
    }
}
