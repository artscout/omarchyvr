//! Autofix reply contract: what an LLM may answer, and how the app reads it.
//!
//! The model gets the redacted diagnostic bundle plus the [`crate::check::CATALOGUE`] (ids, why, args) and must
//! answer in plain lines:
//!
//! ```text
//! EXPLAIN <one line of text for the user>
//! FIX <catalogue id> [arg=value ...]
//! ```
//!
//! Anything else is dropped and reported as rejected. A `FIX` line becomes a [`Planned`] fix only if the id is in the
//! catalogue and every argument passes [`crate::check::safe_arg`]; the app then shows it for approval, one at a time,
//! with its undo. Nothing here runs a command, and there is no way to express free-form shell.

use crate::check::{FixRef, Planned, CATALOGUE};
use std::fmt::Write as _;

/// One line of the model's answer, after checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proposal {
    Explain(String),
    Fix(Planned),
    Rejected { line: String, why: String },
}

/// Most proposals accepted from one answer (the rest are rejected).
pub const MAX_FIXES: usize = 5;

/// Parse a model answer.
pub fn parse_reply(text: &str) -> Vec<Proposal> {
    let mut out = Vec::new();
    let mut fixes = 0;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let reject = |why: &str| Proposal::Rejected { line: line.chars().take(200).collect(), why: why.into() };
        if let Some(t) = line.strip_prefix("EXPLAIN ") {
            out.push(Proposal::Explain(t.trim().chars().filter(|c| !c.is_control()).take(500).collect()));
        } else if let Some(rest) = line.strip_prefix("FIX ") {
            let mut parts = rest.split_whitespace();
            let Some(id) = parts.next() else {
                out.push(reject("FIX without an id"));
                continue;
            };
            let Some(f) = CATALOGUE.iter().find(|f| f.id == id) else {
                out.push(reject("not a catalogue fix"));
                continue;
            };
            let mut args = Vec::new();
            let mut bad = false;
            for p in parts {
                match p.split_once('=') {
                    Some((k, v)) => args.push((k.to_string(), v.to_string())),
                    None => bad = true,
                }
            }
            if bad {
                out.push(reject("arguments must be key=value"));
                continue;
            }
            if fixes >= MAX_FIXES {
                out.push(reject("too many fixes in one answer"));
                continue;
            }
            match (FixRef { id: f.id, args }).plan() {
                Ok(p) => {
                    fixes += 1;
                    out.push(Proposal::Fix(p));
                }
                Err(e) => out.push(reject(&e)),
            }
        } else {
            out.push(reject("not an EXPLAIN or FIX line"));
        }
    }
    out
}

/// The catalogue as the model sees it (part of the prompt; shown to the user with the bundle).
pub fn catalogue_prompt() -> String {
    let mut s = String::from("Fixes you may propose (FIX <id> arg=value ...). Nothing else can be run.\n");
    for f in CATALOGUE {
        let _ = writeln!(s, "- {} (args: {}; root: {}; reboot: {}): {}", f.id, f.args.join(", "), f.root, f.reboot, f.why);
    }
    s
}

/// One line of `~/.local/state/omarchyvr/fixes.log` (tab separated: time, id, outcome, commands, undo).
pub fn log_line(unix_s: u64, p: &Planned, outcome: &str) -> String {
    let clean = |s: &str| s.replace(['\t', '\n'], " ");
    format!("{unix_s}\t{}\t{}\t{}\t{}\n", p.fix.id, clean(outcome), clean(&p.commands.join(" && ")), clean(&p.undo.join(" && ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_catalogue_fixes_with_safe_args_survive() {
        let a = "EXPLAIN Your monado build lost cap_sys_nice after the last relink.\n\
                 FIX cpu.setcap_monado path=/home/u/src/monado/build/src/xrt/targets/service/monado-service\n\
                 FIX cpu.setcap_monado path=/x;rm\n\
                 FIX shell.run cmd=rm\n\
                 sudo rm -rf /\n\
                 ```bash\n\
                 FIX user.keepalive_enable\n";
        let p = parse_reply(a);
        assert!(matches!(&p[0], Proposal::Explain(t) if t.contains("cap_sys_nice")));
        assert!(matches!(&p[1], Proposal::Fix(pl) if pl.fix.id == "cpu.setcap_monado" && pl.fix.root));
        assert!(matches!(&p[2], Proposal::Rejected { why, .. } if why.contains("unsafe")));
        assert!(matches!(&p[3], Proposal::Rejected { why, .. } if why.contains("catalogue")));
        assert!(matches!(&p[4], Proposal::Rejected { .. }));
        assert!(matches!(&p[5], Proposal::Rejected { .. }));
        assert!(matches!(&p[6], Proposal::Fix(pl) if pl.fix.id == "user.keepalive_enable"));
        assert_eq!(p.iter().filter(|x| matches!(x, Proposal::Fix(_))).count(), 2);
    }

    #[test]
    fn log_line_is_one_line() {
        let Proposal::Fix(p) = &parse_reply("FIX runtime.stop_stale_monado")[0] else { panic!() };
        let l = log_line(1, p, "ok\nreally");
        assert_eq!(l.matches('\n').count(), 1);
        assert!(l.starts_with("1\truntime.stop_stale_monado\tok really\tomarchyvr vr stop --grace 30\t"));
        assert!(catalogue_prompt().contains("cpu.setcap_monado"));
    }
}
