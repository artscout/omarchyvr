//! Minimal XML element/attribute reader, enough for the factory calibration document
//! (`<?xml?>` prolog, comments, nested elements, quoted attributes, the five predefined entities and numeric
//! character references). Text content is ignored. No external dependency.

/// One element with its attributes and child elements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Element {
    /// Tag name.
    pub name: String,
    /// Attributes in document order.
    pub attrs: Vec<(String, String)>,
    /// Child elements in document order.
    pub children: Vec<Element>,
}

impl Element {
    /// Attribute value.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// First child with this tag name.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    /// All children with this tag name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

/// Parse error with byte offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlError {
    /// Byte offset in the input.
    pub at: usize,
    /// What went wrong.
    pub msg: String,
}

impl std::fmt::Display for XmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "xml: {} at byte {}", self.msg, self.at)
    }
}

impl std::error::Error for XmlError {}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

fn err<T>(at: usize, msg: impl Into<String>) -> Result<T, XmlError> {
    Err(XmlError {
        at,
        msg: msg.into(),
    })
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':') || b >= 0x80
}

impl<'a> P<'a> {
    fn starts(&self, pat: &str) -> bool {
        self.s[self.i..].starts_with(pat.as_bytes())
    }

    fn skip_ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn skip_until(&mut self, pat: &str) -> Result<(), XmlError> {
        let start = self.i;
        match self.s[self.i..]
            .windows(pat.len())
            .position(|w| w == pat.as_bytes())
        {
            Some(p) => {
                self.i += p + pat.len();
                Ok(())
            }
            None => err(start, format!("unterminated construct, expected {pat:?}")),
        }
    }

    /// Skip text, comments, processing instructions and DOCTYPE until the next element tag (`<x` or `</`).
    fn skip_misc(&mut self) -> Result<(), XmlError> {
        loop {
            while self.i < self.s.len() && self.s[self.i] != b'<' {
                self.i += 1;
            }
            if self.i >= self.s.len() {
                return Ok(());
            }
            if self.starts("<!--") {
                self.skip_until("-->")?;
            } else if self.starts("<?") {
                self.skip_until("?>")?;
            } else if self.starts("<![CDATA[") {
                self.skip_until("]]>")?;
            } else if self.starts("<!") {
                self.skip_until(">")?;
            } else {
                return Ok(());
            }
        }
    }

    fn name(&mut self) -> Result<String, XmlError> {
        let start = self.i;
        while self.i < self.s.len() && is_name_byte(self.s[self.i]) {
            self.i += 1;
        }
        if self.i == start {
            return err(start, "expected a name");
        }
        Ok(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    fn attr_value(&mut self) -> Result<String, XmlError> {
        let q = match self.s.get(self.i) {
            Some(&q @ (b'"' | b'\'')) => q,
            _ => return err(self.i, "expected a quoted attribute value"),
        };
        self.i += 1;
        let start = self.i;
        while self.i < self.s.len() && self.s[self.i] != q {
            self.i += 1;
        }
        if self.i >= self.s.len() {
            return err(start, "unterminated attribute value");
        }
        let raw = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
        self.i += 1;
        decode_entities(&raw).ok_or(XmlError {
            at: start,
            msg: "bad entity".into(),
        })
    }

    fn element(&mut self, depth: usize) -> Result<Element, XmlError> {
        if depth > 64 {
            return err(self.i, "nesting too deep");
        }
        if self.s.get(self.i) != Some(&b'<') {
            return err(self.i, "expected '<'");
        }
        self.i += 1;
        let name = self.name()?;
        let mut el = Element {
            name,
            ..Default::default()
        };
        loop {
            self.skip_ws();
            match self.s.get(self.i) {
                None => return err(self.i, "unexpected end inside a tag"),
                Some(b'/') => {
                    if self.s.get(self.i + 1) != Some(&b'>') {
                        return err(self.i, "expected '/>'");
                    }
                    self.i += 2;
                    return Ok(el);
                }
                Some(b'>') => {
                    self.i += 1;
                    break;
                }
                Some(_) => {
                    let k = self.name()?;
                    self.skip_ws();
                    if self.s.get(self.i) != Some(&b'=') {
                        return err(self.i, format!("expected '=' after attribute {k}"));
                    }
                    self.i += 1;
                    self.skip_ws();
                    let v = self.attr_value()?;
                    el.attrs.push((k, v));
                }
            }
        }
        // content
        loop {
            self.skip_misc()?;
            if self.i >= self.s.len() {
                return err(self.i, format!("missing </{}>", el.name));
            }
            if self.starts("</") {
                self.i += 2;
                let close = self.name()?;
                if close != el.name {
                    return err(self.i, format!("</{close}> closes <{}>", el.name));
                }
                self.skip_ws();
                if self.s.get(self.i) != Some(&b'>') {
                    return err(self.i, "expected '>'");
                }
                self.i += 1;
                return Ok(el);
            }
            let child = self.element(depth + 1)?;
            el.children.push(child);
        }
    }
}

fn decode_entities(s: &str) -> Option<String> {
    if !s.contains('&') {
        return Some(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let after = &rest[p + 1..];
        let end = after.find(';')?;
        let ent = &after[..end];
        let c = match ent {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                char::from_u32(u32::from_str_radix(&ent[2..], 16).ok()?)?
            }
            _ if ent.starts_with('#') => char::from_u32(ent[1..].parse().ok()?)?,
            _ => return None,
        };
        out.push(c);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Parse a document and return its root element.
pub fn parse(doc: &str) -> Result<Element, XmlError> {
    let mut p = P {
        s: doc.as_bytes(),
        i: 0,
    };
    p.skip_misc()?;
    if p.i >= p.s.len() {
        return err(p.i, "no root element");
    }
    let root = p.element(0)?;
    p.skip_misc()?;
    if p.i < p.s.len() {
        return err(p.i, "content after the root element");
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_attrs_comments_entities() {
        let doc = r#"<?xml version="1.0" ?>
<!-- c --><!DOCTYPE x>
<A a="1" b = 'two &amp; &lt;3&gt; &#65;&#x42;'>
  text <![CDATA[ <ignored/> ]]>
  <B x="y"/><!-- inner -->
  <C><D/></C>
  <B x="z"></B>
</A>
"#;
        let r = parse(doc).unwrap();
        assert_eq!(r.name, "A");
        assert_eq!(r.attr("a"), Some("1"));
        assert_eq!(r.attr("b"), Some("two & <3> AB"));
        assert_eq!(r.children.len(), 3);
        let bs: Vec<_> = r
            .children_named("B")
            .map(|b| b.attr("x").unwrap())
            .collect();
        assert_eq!(bs, ["y", "z"]);
        assert_eq!(r.child("C").unwrap().children[0].name, "D");
    }

    #[test]
    fn errors() {
        assert!(parse("").is_err());
        assert!(parse("<A>").is_err());
        assert!(parse("<A></B>").is_err());
        assert!(parse("<A x=1/>").is_err());
        assert!(parse("<A x='&bogus;'/>").is_err());
        assert!(parse("<A/><B/>").is_err());
        assert!(parse("<A x=\"unterminated/>").is_err());
    }
}
