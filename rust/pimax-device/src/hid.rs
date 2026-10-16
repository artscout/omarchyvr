//! The HID command allowlist and the send gate.
//!
//! Every headset module declares the exact byte patterns it may send (`[[hid]]` in its `device.toml`). A pattern is
//! a list of literal bytes plus named parameter fields with an allowed range or set; the rest of the report must be
//! zero padding. [`Gate::send`] checks every report against the list **before** it writes anything: a report that
//! matches no pattern, has a parameter out of range, or matches an explicitly denied prefix is refused and nothing
//! reaches the device. A module without `[[hid]]` entries (every mock) can send nothing at all.
//!
//! Pattern syntax (`bytes`): space separated tokens, each a hex byte (`58`) or a field `{name:u8}` / `{name:u16le}`.
//! Constraints (`params`): `"name=LO..HI"` (inclusive, decimal or 0x hex) or `"name=A|B|C"`.

use crate::toml_lite::Section;
use std::collections::HashMap;
use std::fmt;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// One token of a pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Lit(u8),
    /// A parameter field, `width` bytes, little endian.
    Field { name: String, width: usize },
}

/// Allowed values of one parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint {
    Range(u32, u32),
    Set(Vec<u32>),
}

impl Constraint {
    pub fn allows(&self, v: u32) -> bool {
        match self {
            Constraint::Range(lo, hi) => (*lo..=*hi).contains(&v),
            Constraint::Set(s) => s.contains(&v),
        }
    }
}

/// How often a command may go out (documentation for callers; the gate enforces `min_interval`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Part of a periodic or session sequence (keepalive, wake, video prepare, reads).
    Session,
    /// Exactly once per explicit user request (IPD apply, StartPair). Never retried automatically.
    OncePerRequest,
    /// Once per USB enumeration (eye-module power-on).
    OncePerEnumeration,
}

/// One approved command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: String,
    /// USB interface the report goes to.
    pub interface: u8,
    pub toks: Vec<Tok>,
    pub params: Vec<(String, Constraint)>,
    /// Report length without the hidraw report-id prefix.
    pub report_len: usize,
    pub policy: Policy,
    /// Minimum time between two sends of this command through one gate.
    pub min_interval: Option<Duration>,
    /// Where it was seen (capture) and where the stack builds it (file:line). Required.
    pub source: String,
    /// Approval note (who/when), required for anything that changes device state.
    pub approved: String,
}

/// An explicitly refused prefix (with the reason shown to the user).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied {
    /// `None` = any byte (`??` in the descriptor, e.g. for the length byte).
    pub prefix: Vec<Option<u8>>,
    pub why: String,
}

impl Denied {
    pub fn parse(prefix: &str, why: &str) -> Result<Denied, SpecError> {
        let prefix = prefix
            .split_whitespace()
            .map(|t| match t {
                "??" => Ok(None),
                _ => u8::from_str_radix(t, 16).map(Some).map_err(|_| SpecError(format!("bad denied byte {t}"))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if prefix.is_empty() {
            return Err(SpecError("empty denied prefix".into()));
        }
        Ok(Denied { prefix, why: why.into() })
    }

    pub fn hits(&self, r: &[u8]) -> bool {
        r.len() >= self.prefix.len() && self.prefix.iter().zip(r).all(|(p, b)| !matches!(p, Some(x) if x != b))
    }
}

/// Why a report was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Matches a denied prefix.
    Denied { why: String },
    /// Matches no approved pattern.
    NotListed,
    /// The pattern matched but a field value is outside its constraint.
    OutOfRange { cmd: String, param: String, value: u32 },
    /// Bytes after the pattern are not zero.
    Padding { cmd: String },
    /// Wrong report length.
    Length { got: usize },
    /// `build` was asked for an id the module does not have.
    UnknownCommand(String),
    /// `build` lacks a parameter value.
    MissingParam { cmd: String, param: String },
    /// Sent again before `min_interval` passed.
    TooSoon { cmd: String },
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Denied { why } => write!(f, "refused (denied command): {why}"),
            Refused::NotListed => write!(f, "refused: not an approved command for this headset"),
            Refused::OutOfRange { cmd, param, value } => write!(f, "refused: {cmd}.{param} = {value} is out of range"),
            Refused::Padding { cmd } => write!(f, "refused: {cmd} has non-zero bytes after the approved pattern"),
            Refused::Length { got } => write!(f, "refused: report length {got}"),
            Refused::UnknownCommand(c) => write!(f, "refused: unknown command {c}"),
            Refused::MissingParam { cmd, param } => write!(f, "refused: {cmd} needs {param}"),
            Refused::TooSoon { cmd } => write!(f, "refused: {cmd} sent again too soon"),
        }
    }
}

impl std::error::Error for Refused {}

/// Descriptor errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecError(pub String);

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn num(s: &str) -> Result<u32, SpecError> {
    let s = s.trim();
    let r = match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16),
        None => s.parse(),
    };
    r.map_err(|_| SpecError(format!("bad number {s:?}")))
}

/// Parse `bytes`.
pub fn parse_pattern(s: &str) -> Result<Vec<Tok>, SpecError> {
    s.split_whitespace()
        .map(|t| {
            if let Some(f) = t.strip_prefix('{').and_then(|f| f.strip_suffix('}')) {
                let (name, ty) = f.split_once(':').ok_or_else(|| SpecError(format!("field {t} needs a type")))?;
                let width = match ty {
                    "u8" => 1,
                    "u16le" => 2,
                    _ => return Err(SpecError(format!("field type {ty} (u8 or u16le)"))),
                };
                Ok(Tok::Field { name: name.into(), width })
            } else if t.len() == 2 {
                u8::from_str_radix(t, 16).map(Tok::Lit).map_err(|_| SpecError(format!("bad byte {t}")))
            } else {
                Err(SpecError(format!("bad token {t}")))
            }
        })
        .collect()
}

/// Parse one `params` entry.
pub fn parse_constraint(s: &str) -> Result<(String, Constraint), SpecError> {
    let (name, c) = s.split_once('=').ok_or_else(|| SpecError(format!("constraint {s:?} needs name=")))?;
    let c = if let Some((lo, hi)) = c.split_once("..") {
        let (lo, hi) = (num(lo)?, num(hi)?);
        if lo > hi {
            return Err(SpecError(format!("empty range in {s:?}")));
        }
        Constraint::Range(lo, hi)
    } else {
        Constraint::Set(c.split('|').map(num).collect::<Result<_, _>>()?)
    };
    Ok((name.trim().into(), c))
}

impl CommandSpec {
    /// From a `[[hid]]` section.
    pub fn from_section(s: &Section) -> Result<CommandSpec, SpecError> {
        let need = |k: &str| s.str(k).ok_or_else(|| SpecError(format!("[[hid]] at line {} lacks {k}", s.line)));
        let id = need("id")?.to_string();
        let toks = parse_pattern(need("bytes")?)?;
        let params: Vec<(String, Constraint)> =
            s.strs("params").iter().map(|p| parse_constraint(p)).collect::<Result<_, _>>()?;
        // every field needs exactly one constraint, and every constraint a field
        for t in &toks {
            if let Tok::Field { name, .. } = t {
                if !params.iter().any(|(n, _)| n == name) {
                    return Err(SpecError(format!("{id}: field {name} has no constraint")));
                }
            }
        }
        for (n, _) in &params {
            if !toks.iter().any(|t| matches!(t, Tok::Field { name, .. } if name == n)) {
                return Err(SpecError(format!("{id}: constraint {n} has no field")));
            }
        }
        let policy = match s.str("policy").unwrap_or("session") {
            "session" => Policy::Session,
            "once_per_request" => Policy::OncePerRequest,
            "once_per_enumeration" => Policy::OncePerEnumeration,
            p => return Err(SpecError(format!("{id}: policy {p}"))),
        };
        let report_len = s.int("report_len").unwrap_or(64) as usize;
        let spec = CommandSpec {
            interface: s.int("interface").unwrap_or(0) as u8,
            report_len,
            policy,
            min_interval: s.int("min_interval_ms").map(|m| Duration::from_millis(m as u64)),
            source: need("source")?.to_string(),
            approved: s.str("approved").unwrap_or("").to_string(),
            toks,
            params,
            id,
        };
        if spec.pattern_len() > report_len {
            return Err(SpecError(format!("{}: pattern longer than the report", spec.id)));
        }
        if spec.policy != Policy::Session && spec.approved.is_empty() {
            return Err(SpecError(format!("{}: a state-changing command needs `approved`", spec.id)));
        }
        Ok(spec)
    }

    fn pattern_len(&self) -> usize {
        self.toks.iter().map(|t| if let Tok::Field { width, .. } = t { *width } else { 1 }).sum()
    }

    /// Does `r` (exact report, no report id) match? `Err` carries the most specific reason.
    fn matches(&self, r: &[u8]) -> Result<(), Refused> {
        if r.len() != self.report_len {
            return Err(Refused::NotListed);
        }
        let mut i = 0;
        let mut range_err = None;
        for t in &self.toks {
            match t {
                Tok::Lit(b) => {
                    if r[i] != *b {
                        return Err(Refused::NotListed);
                    }
                    i += 1;
                }
                Tok::Field { name, width } => {
                    let v = if *width == 1 { u32::from(r[i]) } else { u32::from(u16::from_le_bytes([r[i], r[i + 1]])) };
                    let c = &self.params.iter().find(|(n, _)| n == name).expect("checked at load").1;
                    if !c.allows(v) && range_err.is_none() {
                        range_err = Some(Refused::OutOfRange { cmd: self.id.clone(), param: name.clone(), value: v });
                    }
                    i += width;
                }
            }
        }
        if let Some(e) = range_err {
            return Err(e);
        }
        if r[i..].iter().any(|&b| b != 0) {
            return Err(Refused::Padding { cmd: self.id.clone() });
        }
        Ok(())
    }

    /// Encode with the given values (each checked by `matches` afterwards).
    fn encode(&self, vals: &[(&str, u32)]) -> Result<Vec<u8>, Refused> {
        let mut r = vec![0u8; self.report_len];
        let mut i = 0;
        for t in &self.toks {
            match t {
                Tok::Lit(b) => {
                    r[i] = *b;
                    i += 1;
                }
                Tok::Field { name, width } => {
                    let v = vals.iter().find(|(n, _)| n == name).map(|(_, v)| *v).ok_or_else(|| {
                        Refused::MissingParam { cmd: self.id.clone(), param: name.clone() }
                    })?;
                    let max = if *width == 1 { 0xff } else { 0xffff };
                    if v > max {
                        return Err(Refused::OutOfRange { cmd: self.id.clone(), param: name.clone(), value: v });
                    }
                    r[i..i + width].copy_from_slice(&v.to_le_bytes()[..*width]);
                    i += width;
                }
            }
        }
        Ok(r)
    }
}

/// A module's approved commands plus its denied prefixes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Allowlist {
    pub commands: Vec<CommandSpec>,
    pub denied: Vec<Denied>,
}

impl Allowlist {
    /// Check one report for `interface`. Accepts the bare report or the hidraw write buffer (leading report id 0;
    /// every real report starts with a non-zero magic, so the prefix is unambiguous).
    pub fn check(&self, interface: u8, report: &[u8]) -> Result<&CommandSpec, Refused> {
        let bare = match report.first() {
            Some(0) => &report[1..],
            _ => report,
        };
        if let Some(d) = self.denied.iter().find(|d| d.hits(bare)) {
            return Err(Refused::Denied { why: d.why.clone() });
        }
        let mut best = None;
        let (mut len_ok, mut any) = (false, false);
        for c in self.commands.iter().filter(|c| c.interface == interface) {
            any = true;
            len_ok |= bare.len() == c.report_len;
            match c.matches(bare) {
                Ok(()) => return Ok(c),
                Err(Refused::NotListed) => {}
                Err(e) => best = best.or(Some(e)),
            }
        }
        match best {
            Some(e) => Err(e),
            None if any && !len_ok => Err(Refused::Length { got: bare.len() }),
            None => Err(Refused::NotListed),
        }
    }

    /// Build the report for command `id` with parameter values; refused exactly like a send would be.
    pub fn build(&self, id: &str, vals: &[(&str, u32)]) -> Result<Vec<u8>, Refused> {
        let c = self.commands.iter().find(|c| c.id == id).ok_or_else(|| Refused::UnknownCommand(id.into()))?;
        let r = c.encode(vals)?;
        c.matches(&r)?;
        if let Some(d) = self.denied.iter().find(|d| d.hits(&r)) {
            return Err(Refused::Denied { why: d.why.clone() });
        }
        Ok(r)
    }

    pub fn get(&self, id: &str) -> Option<&CommandSpec> {
        self.commands.iter().find(|c| c.id == id)
    }
}

/// Send error: refused before writing, or the write failed.
#[derive(Debug)]
pub enum SendError {
    Refused(Refused),
    Io(io::Error),
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendError::Refused(r) => r.fmt(f),
            SendError::Io(e) => write!(f, "hidraw write failed: {e}"),
        }
    }
}

impl std::error::Error for SendError {}

/// The send layer: the only way a module-aware tool should write to a headset interface. Owns the writer (an open
/// hidraw node in real use, a `Vec<u8>` in tests). Opening the node is the caller's job, so this crate never touches
/// a device by itself.
pub struct Gate<'a, W: Write> {
    allow: &'a Allowlist,
    interface: u8,
    out: W,
    last: HashMap<String, Instant>,
}

impl<'a, W: Write> Gate<'a, W> {
    pub fn new(allow: &'a Allowlist, interface: u8, out: W) -> Self {
        Gate { allow, interface, out, last: HashMap::new() }
    }

    /// Check, then write `00 + report` in one `write`. Returns the matched command id.
    pub fn send(&mut self, report: &[u8]) -> Result<String, SendError> {
        let spec = self.allow.check(self.interface, report).map_err(SendError::Refused)?;
        if let Some(min) = spec.min_interval {
            if self.last.get(&spec.id).is_some_and(|t| t.elapsed() < min) {
                return Err(SendError::Refused(Refused::TooSoon { cmd: spec.id.clone() }));
            }
        }
        let bare = if report.first() == Some(&0) { &report[1..] } else { report };
        let mut buf = Vec::with_capacity(bare.len() + 1);
        buf.push(0);
        buf.extend_from_slice(bare);
        let n = self.out.write(&buf).map_err(SendError::Io)?;
        if n != buf.len() {
            return Err(SendError::Io(io::Error::new(io::ErrorKind::WriteZero, format!("short write {n}/{}", buf.len()))));
        }
        self.last.insert(spec.id.clone(), Instant::now());
        Ok(spec.id.clone())
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}
