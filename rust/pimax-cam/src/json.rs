//! Minimal JSON reader and writer helpers (RFC 8259 subset that covers every valid document: objects, arrays,
//! strings with escapes, numbers, `true`/`false`/`null`). Used for the calibration overlay
//! ([`crate::overlay`]); the crate stays free of serde, like the hand-written XML reader in [`crate::xml`].

use std::fmt;

/// A parsed JSON value. Object members keep their order; duplicate keys keep the first occurrence on lookup.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `null`.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// Any number (as f64).
    Num(f64),
    /// String.
    Str(String),
    /// Array.
    Arr(Vec<Value>),
    /// Object.
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// Member of an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    /// Number.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(x) => Some(*x),
            _ => None,
        }
    }
    /// String.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    /// Array.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(a) => Some(a),
            _ => None,
        }
    }
    /// Array of exactly `N` numbers.
    pub fn as_f64_array<const N: usize>(&self) -> Option<[f64; N]> {
        let a = self.as_array()?;
        if a.len() != N {
            return None;
        }
        let mut out = [0.0; N];
        for (o, v) in out.iter_mut().zip(a) {
            *o = v.as_f64()?;
        }
        Some(out)
    }
}

/// Parse error with byte offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    /// Byte offset into the input.
    pub pos: usize,
    /// What went wrong.
    pub msg: &'static str,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON error at byte {}: {}", self.pos, self.msg)
    }
}

impl std::error::Error for JsonError {}

/// Nesting limit (protects the recursion against hostile input).
const MAX_DEPTH: usize = 64;

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn err<T>(&self, msg: &'static str) -> Result<T, JsonError> {
        Err(JsonError { pos: self.pos, msg })
    }

    fn ws(&mut self) {
        while self.pos < self.s.len() && matches!(self.s[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
    }

    fn eat(&mut self, lit: &[u8]) -> bool {
        if self.s[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, JsonError> {
        if depth > MAX_DEPTH {
            return self.err("nesting too deep");
        }
        self.ws();
        let Some(&c) = self.s.get(self.pos) else {
            return self.err("unexpected end");
        };
        match c {
            b'{' => {
                self.pos += 1;
                let mut m = Vec::new();
                self.ws();
                if self.eat(b"}") {
                    return Ok(Value::Obj(m));
                }
                loop {
                    self.ws();
                    if self.s.get(self.pos) != Some(&b'"') {
                        return self.err("expected a string key");
                    }
                    let k = self.string()?;
                    self.ws();
                    if !self.eat(b":") {
                        return self.err("expected ':'");
                    }
                    let v = self.value(depth + 1)?;
                    m.push((k, v));
                    self.ws();
                    if self.eat(b",") {
                        continue;
                    }
                    if self.eat(b"}") {
                        return Ok(Value::Obj(m));
                    }
                    return self.err("expected ',' or '}'");
                }
            }
            b'[' => {
                self.pos += 1;
                let mut a = Vec::new();
                self.ws();
                if self.eat(b"]") {
                    return Ok(Value::Arr(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(b",") {
                        continue;
                    }
                    if self.eat(b"]") {
                        return Ok(Value::Arr(a));
                    }
                    return self.err("expected ',' or ']'");
                }
            }
            b'"' => Ok(Value::Str(self.string()?)),
            b't' if self.eat(b"true") => Ok(Value::Bool(true)),
            b'f' if self.eat(b"false") => Ok(Value::Bool(false)),
            b'n' if self.eat(b"null") => Ok(Value::Null),
            b'-' | b'0'..=b'9' => self.number(),
            _ => self.err("unexpected character"),
        }
    }

    fn number(&mut self) -> Result<Value, JsonError> {
        let start = self.pos;
        let digits = |p: &mut Parser| {
            let s = p.pos;
            while p.pos < p.s.len() && p.s[p.pos].is_ascii_digit() {
                p.pos += 1;
            }
            p.pos > s
        };
        self.eat(b"-");
        if self.eat(b"0") {
            // no leading zeros
        } else if !digits(self) {
            return self.err("bad number");
        }
        if self.eat(b".") && !digits(self) {
            return self.err("bad fraction");
        }
        if self
            .s
            .get(self.pos)
            .is_some_and(|c| *c == b'e' || *c == b'E')
        {
            self.pos += 1;
            if !self.eat(b"+") {
                self.eat(b"-");
            }
            if !digits(self) {
                return self.err("bad exponent");
            }
        }
        let txt = std::str::from_utf8(&self.s[start..self.pos]).expect("ascii");
        match txt.parse::<f64>() {
            Ok(x) if x.is_finite() => Ok(Value::Num(x)),
            _ => Err(JsonError {
                pos: start,
                msg: "number out of range",
            }),
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let h = self
            .s
            .get(self.pos..self.pos + 4)
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(|t| u32::from_str_radix(t, 16).ok());
        match h {
            Some(v) => {
                self.pos += 4;
                Ok(v)
            }
            None => self.err("bad \\u escape"),
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.pos += 1; // opening quote
        let mut out = Vec::new();
        loop {
            let Some(&c) = self.s.get(self.pos) else {
                return self.err("unterminated string");
            };
            self.pos += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.s.get(self.pos) else {
                        return self.err("unterminated escape");
                    };
                    self.pos += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xd800..0xdc00).contains(&hi) {
                                if !self.eat(b"\\u") {
                                    return self.err("lone surrogate");
                                }
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return self.err("bad surrogate pair");
                                }
                                0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                            } else {
                                hi
                            };
                            match char::from_u32(cp) {
                                Some(ch) => ch,
                                None => return self.err("bad code point"),
                            }
                        }
                        _ => return self.err("bad escape"),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                0..=0x1f => return self.err("control character in string"),
                _ => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|_| JsonError {
            pos: self.pos,
            msg: "invalid UTF-8",
        })
    }
}

/// Parse a complete JSON document.
pub fn parse(text: &str) -> Result<Value, JsonError> {
    let mut p = Parser {
        s: text.as_bytes(),
        pos: 0,
    };
    let v = p.value(0)?;
    p.ws();
    if p.pos != p.s.len() {
        return p.err("trailing characters");
    }
    Ok(v)
}

/// JSON string literal (with quotes) for `s`.
pub fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// JSON number for a finite `x` (shortest representation that round-trips); `null` for NaN/inf.
pub fn num(x: f64) -> String {
    if x.is_finite() {
        format!("{x:?}")
    } else {
        "null".into()
    }
}

/// `[a, b, c]` of numbers.
pub fn num_array(v: &[f64]) -> String {
    let parts: Vec<String> = v.iter().map(|&x| num(x)).collect();
    format!("[{}]", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documents() {
        let v = parse(r#" { "a": [1, -2.5e-3, 0, true, false, null], "b": {"c": "x\"y\\z\u00e9\ud83d\ude00"} } "#)
            .unwrap();
        let a = v.get("a").unwrap().as_array().unwrap();
        assert_eq!(a[0], Value::Num(1.0));
        assert_eq!(a[1], Value::Num(-0.0025));
        assert_eq!(a[3], Value::Bool(true));
        assert_eq!(a[5], Value::Null);
        assert_eq!(
            v.get("b").unwrap().get("c").unwrap().as_str().unwrap(),
            "x\"y\\z\u{e9}\u{1f600}"
        );
        assert_eq!(
            parse("[1,2,3]").unwrap().as_f64_array::<3>(),
            Some([1.0, 2.0, 3.0])
        );
        assert_eq!(parse("[1,2]").unwrap().as_f64_array::<3>(), None);
        assert_eq!(parse("{}").unwrap(), Value::Obj(vec![]));
        assert_eq!(parse("[]").unwrap(), Value::Arr(vec![]));
    }

    #[test]
    fn rejects_bad_documents() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "01",
            "1.",
            "-",
            "1e",
            "\"abc",
            "\"\\x\"",
            "tru",
            "[1] 2",
            "{1: 2}",
            "\"\u{1}\"",
            "1e999",
            "\"\\ud800\"",
        ] {
            assert!(parse(bad).is_err(), "accepted {bad:?}");
        }
        let deep = "[".repeat(100) + &"]".repeat(100);
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn writer_roundtrip() {
        let s = "tab\t quote\" back\\ nl\n ctl\u{2} é";
        assert_eq!(parse(&quote(s)).unwrap().as_str().unwrap(), s);
        for x in [0.0, -1.5, 1e-12, 0.1 + 0.2, 123456789.0, f64::MIN_POSITIVE] {
            assert_eq!(parse(&num(x)).unwrap().as_f64().unwrap(), x);
        }
        assert_eq!(num(f64::NAN), "null");
        assert_eq!(
            parse(&num_array(&[1.0, 2.5])).unwrap().as_f64_array::<2>(),
            Some([1.0, 2.5])
        );
    }
}
