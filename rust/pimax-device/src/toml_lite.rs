//! A tiny reader for the TOML subset the device descriptors use. No dependency, no serde.
//!
//! Supported: `# comments`, `[table]`, `[[array-of-tables]]`, `key = value` on one line, where value is a basic
//! string `"..."` (escapes `\"`, `\\`, `\n`), an integer (decimal or `0x` hex, optional `-`), a float, `true`/`false`,
//! or a one-line array of those. Anything else is an error with its line number, so a descriptor that uses more TOML
//! than this fails loudly in the tests instead of being half read.

use std::fmt;

/// A value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Arr(Vec<Value>),
}

/// One `[name]` or `[[name]]` section (the root section has the name "").
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Section {
    pub name: String,
    /// True for `[[name]]`.
    pub array: bool,
    pub entries: Vec<(String, Value)>,
    /// 1-based line of the header (0 for the root).
    pub line: usize,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        }
    }
    pub fn int(&self, key: &str) -> Option<i64> {
        match self.get(key) {
            Some(Value::Int(i)) => Some(*i),
            _ => None,
        }
    }
    pub fn float(&self, key: &str) -> Option<f64> {
        match self.get(key) {
            Some(Value::Float(f)) => Some(*f),
            Some(Value::Int(i)) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key) {
            Some(Value::Bool(b)) => Some(*b),
            _ => None,
        }
    }
    pub fn ints(&self, key: &str) -> Vec<i64> {
        match self.get(key) {
            Some(Value::Arr(a)) => a.iter().filter_map(|v| if let Value::Int(i) = v { Some(*i) } else { None }).collect(),
            _ => Vec::new(),
        }
    }
    pub fn strs(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            Some(Value::Arr(a)) => a.iter().filter_map(|v| if let Value::Str(s) = v { Some(s.clone()) } else { None }).collect(),
            _ => Vec::new(),
        }
    }
}

/// Parse error with its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for ParseError {}

/// Parse a document into its sections, in file order (root first).
pub fn parse(src: &str) -> Result<Vec<Section>, ParseError> {
    let mut out = vec![Section::default()];
    for (i, raw) in src.lines().enumerate() {
        let n = i + 1;
        let err = |msg: &str| ParseError { line: n, msg: msg.to_string() };
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("[[") {
            let name = rest.strip_suffix("]]").ok_or_else(|| err("unclosed [[table]]"))?.trim();
            check_key(name).map_err(|m| err(&m))?;
            out.push(Section { name: name.into(), array: true, entries: Vec::new(), line: n });
        } else if let Some(rest) = line.strip_prefix('[') {
            let name = rest.strip_suffix(']').ok_or_else(|| err("unclosed [table]"))?.trim();
            check_key(name).map_err(|m| err(&m))?;
            if out.iter().any(|s| !s.array && s.name == name) {
                return Err(err("table defined twice"));
            }
            out.push(Section { name: name.into(), array: false, entries: Vec::new(), line: n });
        } else {
            let (k, v) = line.split_once('=').ok_or_else(|| err("expected key = value"))?;
            let k = k.trim();
            check_key(k).map_err(|m| err(&m))?;
            let v = value(v.trim()).map_err(|m| err(&m))?;
            let sec = out.last_mut().expect("root section");
            if sec.get(k).is_some() {
                return Err(err("key defined twice"));
            }
            sec.entries.push((k.into(), v));
        }
    }
    Ok(out)
}

fn check_key(k: &str) -> Result<(), String> {
    if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        Ok(())
    } else {
        Err(format!("bad key {k:?} (bare keys only)"))
    }
}

/// Cut a `#` comment that is not inside a string.
fn strip_comment(l: &str) -> &str {
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in l.char_indices() {
        match c {
            _ if esc => esc = false,
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '#' if !in_str => return &l[..i],
            _ => {}
        }
    }
    l
}

fn value(s: &str) -> Result<Value, String> {
    if let Some(inner) = s.strip_prefix('[') {
        let inner = inner.strip_suffix(']').ok_or("arrays must close on the same line")?;
        let mut items = Vec::new();
        for part in split_top(inner)? {
            let p = part.trim();
            if p.is_empty() {
                continue; // trailing comma
            }
            if p.starts_with('[') {
                return Err("nested arrays are not supported".into());
            }
            items.push(value(p)?);
        }
        return Ok(Value::Arr(items));
    }
    if let Some(inner) = s.strip_prefix('"') {
        let inner = inner.strip_suffix('"').ok_or("unterminated string")?;
        return unescape(inner).map(Value::Str);
    }
    match s {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s),
    };
    let body = body.replace('_', "");
    if let Some(h) = body.strip_prefix("0x") {
        let v = i64::from_str_radix(h, 16).map_err(|_| format!("bad hex {s:?}"))?;
        return Ok(Value::Int(if neg { -v } else { v }));
    }
    if let Ok(v) = body.parse::<i64>() {
        return Ok(Value::Int(if neg { -v } else { v }));
    }
    if let Ok(v) = body.parse::<f64>() {
        return Ok(Value::Float(if neg { -v } else { v }));
    }
    Err(format!("unsupported value {s:?}"))
}

/// Split on commas outside strings.
fn split_top(s: &str) -> Result<Vec<&str>, String> {
    let mut parts = Vec::new();
    let (mut start, mut in_str, mut esc) = (0, false, false);
    for (i, c) in s.char_indices() {
        match c {
            _ if esc => esc = false,
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            ',' if !in_str => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if in_str {
        return Err("unterminated string in array".into());
    }
    parts.push(&s[start..]);
    Ok(parts)
}

fn unescape(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            if c == '"' {
                return Err("unescaped quote in string".into());
            }
            out.push(c);
            continue;
        }
        match it.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            other => return Err(format!("unsupported escape \\{}", other.map(String::from).unwrap_or_default())),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_subset() {
        let d = parse(
            "top = 1 # c\n[device]\nid = \"a#b\"\nvid = 0x34a4\nf = 58.5\nok = true\nl = [0x44, 2, ]\ns = [\"x, y\", \"z\"]\n[[hid]]\nid = \"one\"\n[[hid]]\nid = \"two\"\n",
        )
        .unwrap();
        assert_eq!(d[0].int("top"), Some(1));
        let dev = &d[1];
        assert_eq!(dev.str("id"), Some("a#b"));
        assert_eq!(dev.int("vid"), Some(0x34a4));
        assert_eq!(dev.float("f"), Some(58.5));
        assert_eq!(dev.bool("ok"), Some(true));
        assert_eq!(dev.ints("l"), vec![0x44, 2]);
        assert_eq!(dev.strs("s"), vec!["x, y".to_string(), "z".to_string()]);
        assert_eq!(d.iter().filter(|s| s.name == "hid" && s.array).count(), 2);
    }

    #[test]
    fn refuses_what_it_does_not_know() {
        for bad in ["a.b = 1", "a = { x = 1 }", "a = [1,\n", "[t]\n[t]", "a = 1\na = 2", "a = 'lit'"] {
            assert!(parse(bad).is_err(), "{bad:?} parsed");
        }
    }
}
