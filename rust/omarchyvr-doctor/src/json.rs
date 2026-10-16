//! Tiny JSON writer (no serde in this workspace).
use std::fmt::Write;

/// A JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// null
    Null,
    /// true/false
    Bool(bool),
    /// integer
    Int(i64),
    /// string
    Str(String),
    /// array
    Arr(Vec<Json>),
    /// object (insertion order)
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// String value.
    pub fn s(v: impl Into<String>) -> Json {
        Json::Str(v.into())
    }

    /// Optional string.
    pub fn opt(v: Option<impl Into<String>>) -> Json {
        v.map_or(Json::Null, |v| Json::Str(v.into()))
    }

    /// Pretty-printed text.
    pub fn render(&self) -> String {
        let mut o = String::new();
        self.write(&mut o, 0);
        o.push('\n');
        o
    }

    fn write(&self, o: &mut String, ind: usize) {
        match self {
            Json::Null => o.push_str("null"),
            Json::Bool(b) => o.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => {
                let _ = write!(o, "{i}");
            }
            Json::Str(s) => quote(s, o),
            Json::Arr(a) if a.is_empty() => o.push_str("[]"),
            Json::Arr(a) => {
                o.push_str("[\n");
                for (i, v) in a.iter().enumerate() {
                    o.push_str(&"  ".repeat(ind + 1));
                    v.write(o, ind + 1);
                    o.push_str(if i + 1 < a.len() { ",\n" } else { "\n" });
                }
                o.push_str(&"  ".repeat(ind));
                o.push(']');
            }
            Json::Obj(m) if m.is_empty() => o.push_str("{}"),
            Json::Obj(m) => {
                o.push_str("{\n");
                for (i, (k, v)) in m.iter().enumerate() {
                    o.push_str(&"  ".repeat(ind + 1));
                    quote(k, o);
                    o.push_str(": ");
                    v.write(o, ind + 1);
                    o.push_str(if i + 1 < m.len() { ",\n" } else { "\n" });
                }
                o.push_str(&"  ".repeat(ind));
                o.push('}');
            }
        }
    }
}

fn quote(s: &str, o: &mut String) {
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_nests() {
        let j = Json::Obj(vec![
            ("a\"b".into(), Json::s("x\ny\\")),
            (
                "l".into(),
                Json::Arr(vec![Json::Int(1), Json::Null, Json::Bool(true)]),
            ),
            ("e".into(), Json::Arr(vec![])),
        ]);
        let t = j.render();
        assert!(t.contains(r#""a\"b": "x\ny\\""#), "{t}");
        assert!(t.contains("\"e\": []"));
        assert!(t.contains("null"));
    }
}
