//! A tiny JSON reader/writer (no serde): enough for the snapshot, the socket requests and `state.json`.
use std::collections::BTreeMap;

/// Quote a string as JSON.
pub fn q(s: &str) -> String {
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

/// A number with at most one decimal, or `null` when not finite.
pub fn num(v: f64) -> String {
    if !v.is_finite() {
        "null".into()
    } else if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v:.1}")
    }
}

/// A parsed value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(BTreeMap<String, Value>),
}

impl Value {
    pub fn get(&self, k: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.get(k),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Str(s) if s == "on" || s == "true" => Some(true),
            Value::Str(s) if s == "off" || s == "false" => Some(false),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> &[Value] {
        match self {
            Value::Arr(v) => v,
            _ => &[],
        }
    }
    /// A string field.
    pub fn s(&self, k: &str) -> Option<&str> {
        self.get(k).and_then(Value::as_str)
    }
}

/// Parse one JSON document.
pub fn parse(s: &str) -> Result<Value, String> {
    let b = s.as_bytes();
    let mut i = 0;
    let v = value(b, &mut i, 0)?;
    ws(b, &mut i);
    if i != b.len() {
        return Err(format!("trailing data at {i}"));
    }
    Ok(v)
}

fn ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
}

fn value(b: &[u8], i: &mut usize, depth: usize) -> Result<Value, String> {
    if depth > 32 {
        return Err("too deep".into());
    }
    ws(b, i);
    match b.get(*i) {
        None => Err("unexpected end".into()),
        Some(b'{') => {
            *i += 1;
            let mut m = BTreeMap::new();
            ws(b, i);
            if b.get(*i) == Some(&b'}') {
                *i += 1;
                return Ok(Value::Obj(m));
            }
            loop {
                ws(b, i);
                let k = string(b, i)?;
                ws(b, i);
                if b.get(*i) != Some(&b':') {
                    return Err("expected :".into());
                }
                *i += 1;
                let v = value(b, i, depth + 1)?;
                m.insert(k, v);
                ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b'}') => {
                        *i += 1;
                        return Ok(Value::Obj(m));
                    }
                    _ => return Err("expected , or }".into()),
                }
            }
        }
        Some(b'[') => {
            *i += 1;
            let mut v = Vec::new();
            ws(b, i);
            if b.get(*i) == Some(&b']') {
                *i += 1;
                return Ok(Value::Arr(v));
            }
            loop {
                v.push(value(b, i, depth + 1)?);
                ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b']') => {
                        *i += 1;
                        return Ok(Value::Arr(v));
                    }
                    _ => return Err("expected , or ]".into()),
                }
            }
        }
        Some(b'"') => Ok(Value::Str(string(b, i)?)),
        Some(_) => {
            let st = *i;
            while *i < b.len() && !matches!(b[*i], b',' | b'}' | b']') && !b[*i].is_ascii_whitespace() {
                *i += 1;
            }
            let raw = std::str::from_utf8(&b[st..*i]).map_err(|_| "bad utf-8")?;
            match raw {
                "null" => Ok(Value::Null),
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => raw.parse::<f64>().map(Value::Num).map_err(|_| format!("bad value {raw:?}")),
            }
        }
    }
}

fn string(b: &[u8], i: &mut usize) -> Result<String, String> {
    if b.get(*i) != Some(&b'"') {
        return Err("expected a string".into());
    }
    *i += 1;
    let mut o: Vec<u8> = Vec::new();
    while let Some(&c) = b.get(*i) {
        *i += 1;
        match c {
            b'"' => return String::from_utf8(o).map_err(|_| "bad utf-8".into()),
            b'\\' => {
                let e = *b.get(*i).ok_or("bad escape")?;
                *i += 1;
                match e {
                    b'n' => o.push(b'\n'),
                    b't' => o.push(b'\t'),
                    b'r' => o.push(b'\r'),
                    b'b' => o.push(8),
                    b'f' => o.push(12),
                    b'u' => {
                        let h = b.get(*i..*i + 4).ok_or("bad \\u")?;
                        *i += 4;
                        let cp = u32::from_str_radix(std::str::from_utf8(h).map_err(|_| "bad \\u")?, 16).map_err(|_| "bad \\u")?;
                        let mut buf = [0u8; 4];
                        o.extend_from_slice(char::from_u32(cp).unwrap_or('?').encode_utf8(&mut buf).as_bytes());
                    }
                    c => o.push(c),
                }
            }
            c => o.push(c),
        }
    }
    Err("unterminated string".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = parse(r#"{"a":1,"b":[true,null,"x\"y"],"c":{"d":-2.5},"e":"é"}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_f64(), Some(1.0));
        assert_eq!(v.get("b").unwrap().as_arr()[2].as_str(), Some("x\"y"));
        assert_eq!(v.get("c").unwrap().get("d").unwrap().as_f64(), Some(-2.5));
        assert_eq!(v.s("e"), Some("é"));
        assert_eq!(parse(&q("a\nb\u{1}")).unwrap().as_str(), Some("a\nb\u{1}"));
        assert!(parse("{\"a\":}").is_err());
        assert!(parse("{} x").is_err());
        assert_eq!(num(2.0), "2");
        assert_eq!(num(2.25), "2.2");
        assert_eq!(num(f64::NAN), "null");
    }
}
