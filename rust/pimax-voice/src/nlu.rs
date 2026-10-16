//! The compact "what did the user mean" request for the voice AI (the default for the local model).
//!
//! The old agent request sent ~75 JSON tool schemas plus a JSON context: ~5,650 prompt tokens per command, and the
//! dynamic context broke llama.cpp's prefix cache, so every command re-read most of it (seconds on the CPU, worse on
//! a GPU busy with VR). This request is built for speed:
//!
//! * a **static** system prompt first (rules, known ASR confusions, one line per intent: id + a short gloss), byte
//!   for byte the same every time, so llama.cpp keeps it in the slot cache (`cache_prompt`) and only the short user
//!   message is evaluated per command; [`crate::agent::Agent::warm_up`] primes it;
//! * a **short dynamic** user message: windows (`w1..w8`, class, short title, workspace), what the eyes are on, the
//!   VR state, the fuzzy matcher's top 3 candidates with scores ([`crate::fuzzy`]) and the words
//!   heard;
//! * a **one-line answer** `<intent> <confidence 0-9> [slot=value ...]`, constrained by a GBNF grammar on the local
//!   server (only real intent ids, the right slots, valid values), so a 4B model answers in ~6-12 tokens.
//!
//! The decision goes through the agent's normal tool checks (safety, careful = spoken yes, window ids).

use crate::agent::{self, Context};
use crate::catalogue::{Catalogue, Intent, SlotType};
use crate::fuzzy::{self, Cand};
use crate::windows;
use std::collections::BTreeMap;

/// Windows listed in the request.
pub const WINDOWS: usize = 8;
/// Title characters per window.
pub const TITLE_CHARS: usize = 32;

/// Window pseudo-intents (by `wN` from the window list).
pub const WINDOW_INTENTS: &[(&str, &str, &[(&str, &str)])] = &[
    ("focus_window", "show/focus a listed app", &[("window", "wN")]),
    ("move_window", "move a listed app to window (workspace) N", &[("window", "wN"), ("workspace", "N")]),
    ("close_window", "close a listed app (asks yes)", &[("window", "wN")]),
];

/// The intents the model may pick (enabled, not "never", not meta).
pub fn offered(cat: &Catalogue) -> Vec<&Intent> {
    cat.enabled().filter(|i| agent::intent_safety(i) != "never" && !agent::NOT_TOOLS.contains(&i.id.as_str())).collect()
}

fn slot_hint(name: &str, t: SlotType) -> String {
    match t {
        SlotType::Number => format!("{name}=N"),
        SlotType::Word => format!("{name}=word"),
        SlotType::Side => format!("{name}=left|right|up|main"),
        SlotType::Text | SlotType::Name => format!("{name}=\"words\""),
    }
}

/// Slots in answer order: numbers and words first, quoted text last.
fn slot_order(i: &Intent) -> Vec<(&String, SlotType)> {
    let mut v: Vec<(&String, SlotType)> = i.slots.iter().map(|(k, t)| (k, *t)).collect();
    v.sort_by_key(|(k, t)| (matches!(t, SlotType::Text | SlotType::Name), (*k).clone()));
    v
}

/// The fixed system prompt (cached by the server). Built once per catalogue.
pub fn system_prompt(cat: &Catalogue) -> String {
    let mut s = String::from(
        "You are Max, the voice control of a Linux desktop seen in a VR headset. Speech-to-text is imperfect: words \
         are often misheard. Decide what the user MEANS and answer with one line: an intent id from the list, then \
         your confidence as one digit (9 sure, 6 likely, 3 a guess), then the intent's slots, e.g. \
         `win.ws.goto 9 n=4`. Answer `none 9` if the words are noise, a test (\"can you hear me\"), chatter or no \
         command. A request with several actions gets up to 3 steps separated by ` ; ` (in order).\n\
         Known mishearings: war/walk/were=work, liz/leaks/least=list, tree/free=three, mars/marks/macs=max, \
         tap=tab, won=one, for=four, to=two, sinema=cinema, winter=window, tight=type, full scream=fullscreen, \
         and do=undo, high=hide.\n\
         hints = the intents whose phrases sound closest (score 0-1): often right, but the meaning decides; a short \
         command with an odd word (\"necks song\", \"mood\", \"coffee that\") is usually the best hint, not none. \
         Words: \"window\" = a workspace (one VR screen, all its apps; close window = win.ws.close); one program = \
         an \"app\" (close this = win.close); \"tab\" = inside an app; \"N screens\" = VR presets (work three = 3); \
         VR screens: main (centre), left, right, up (move this left = win.screen.move side=left; close the other \
         windows = vr.preset.work1, nothing closes).\n\
         Intents (id: what it does / example phrases):\n",
    );
    for i in offered(cat) {
        let mut line = format!("{}", i.id);
        for (k, t) in slot_order(i) {
            line += &format!(" {}", slot_hint(k, t));
        }
        let gloss = if !i.gloss.is_empty() {
            i.gloss.clone()
        } else {
            i.phrases.iter().take(3).cloned().collect::<Vec<_>>().join(" / ")
        };
        s += &format!("{line}: {gloss}{}\n", if agent::intent_safety(i) == "careful" { " (asks yes)" } else { "" });
    }
    for (id, gloss, slots) in WINDOW_INTENTS {
        let sl: Vec<String> = slots.iter().map(|(k, v)| format!("{k}={v}")).collect();
        s += &format!("{id} {}: {gloss}\n", sl.join(" "));
    }
    s += "Examples: \"liz windows\" -> win.list 8 | \"time max war three\" -> vr.preset.work3 8 | \
          \"go to work space for\" -> win.ws.goto 9 n=4 | \"remind me in 5 minutes to stretch\" -> omarchy.reminder 9 minutes=5 text=\"stretch\" | \
          \"can you hear me\" -> none 9 | \"dim the lights in here\" -> vr.passthrough.dim 7 | \"my screens are gone\" -> vr.screens.reset 8 | \"close the other two screens\" -> vr.preset.work1 8 | \"in the browser type how to cook rice\" -> browser.search 9 q=\"how to cook rice\" | \"type hello world\" -> type.text 9 text=\"hello world\" | \"check the internet browser\" -> omarchy.launch.browser 8 | \"go to the youtube tab\" -> browser.tab.focus 9 q=\"youtube\" | \"open two more screens\" -> vr.preset.work3 8 | \"close the window i'm looking at\" -> win.ws.close 8 | \"go to workspace 2 and make it fullscreen\" -> win.ws.goto 9 n=2 ; omarchy.window.fullscreen 8\n";
    s
}

fn gbnf_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The GBNF grammar of an answer: only offered ids, each with its own slots and value shapes.
pub fn grammar(cat: &Catalogue) -> String {
    grammar_steps(cat, true)
}

/// Does the request ask for several things ("... and ...", "then")? Only then may the answer be a plan: a small
/// model otherwise invents a first step.
pub fn wants_plan(heard: &str) -> bool {
    let w = crate::command::words(heard);
    w.iter().any(|x| matches!(x.as_str(), "and" | "then" | "also" | "y" | "luego"))
}

/// The grammar; `plan`: up to 3 steps.
pub fn grammar_steps(cat: &Catalogue, plan: bool) -> String {
    let mut plain = vec![gbnf_str("none")];
    let mut rules = Vec::new();
    let mut alts = Vec::new();
    for i in offered(cat) {
        if i.slots.is_empty() {
            plain.push(gbnf_str(&i.id));
            continue;
        }
        let mut r = format!("{} \" \" c", gbnf_str(&i.id));
        for (k, t) in slot_order(i) {
            let v = match t {
                SlotType::Number => "num",
                SlotType::Word => "word",
                SlotType::Side => "side",
                SlotType::Text | SlotType::Name => "txt",
            };
            r += &format!(" \" {k}=\" {v}");
        }
        let name = format!("r{}", rules.len());
        rules.push(format!("{name} ::= {r}"));
        alts.push(name);
    }
    for (id, _, slots) in WINDOW_INTENTS {
        let mut r = format!("{} \" \" c", gbnf_str(id));
        for (k, v) in *slots {
            r += &format!(" \" {k}=\" {}", if *v == "wN" { "win" } else { "num" });
        }
        let name = format!("r{}", rules.len());
        rules.push(format!("{name} ::= {r}"));
        alts.push(name);
    }
    // a plan: up to 3 steps separated by " ; "
    let mut g = String::from(if plan { "root ::= one (\" ; \" one){0,2}\n" } else { "root ::= one\n" });
    g += &format!("one ::= plain{}\n", alts.iter().map(|a| format!(" | {a}")).collect::<String>());
    g += &format!("plain ::= ({}) \" \" c\n", plain.join(" | "));
    for r in rules {
        g += &r;
        g.push('\n');
    }
    g += "c ::= [0-9]\nnum ::= [1-9] [0-9]? [0-9]?\nword ::= [a-zA-Z0-9_-]{1,24}\n\
          txt ::= \"\\\"\" [a-zA-Z0-9 ',.?!-]{1,80} \"\\\"\"\nwin ::= \"w\" [1-9]\nside ::= \"left\" | \"right\" | \"up\" | \"main\"\n";
    g
}

/// Does the request talk about windows (so the window list is worth its tokens)? "window", "this"/"that", or a
/// word of a window's class or title ("go to chrome", "the one with the google search").
pub fn needs_windows(ctx: &Context, heard: &str) -> bool {
    let words = crate::command::words(heard);
    const W: &[&str] = &["window", "windows", "this", "that", "it", "app", "move", "close", "focus", "switch", "show", "go", "where", "send", "put", "ventana"];
    if words.iter().any(|w| W.contains(&w.as_str())) {
        return true;
    }
    let hay: Vec<String> = ctx.clients.iter().take(WINDOWS).flat_map(|c| crate::command::words(&format!("{} {}", c.class, c.title))).filter(|w| w.len() >= 3).collect();
    words.iter().filter(|w| w.len() >= 3 && !crate::command::FILLERS.contains(&w.as_str())).any(|w| hay.contains(w))
}

/// The per-command user message and the `wN` -> window address map. Only the lines that matter: the window list
/// only when the request is about windows ([`needs_windows`]), the hints as ids + scores.
pub fn user_message(ctx: &Context, cands: &[Cand], heard: &str, strip_titles: bool) -> (String, Vec<String>) {
    if needs_windows(ctx, heard) {
        return user_message_inner(ctx, cands, heard, strip_titles);
    }
    let mut c2 = ctx.clone();
    c2.clients.clear();
    c2.focused = None;
    c2.this = None;
    user_message_inner(&c2, cands, heard, strip_titles)
}

fn user_message_inner(ctx: &Context, cands: &[Cand], heard: &str, strip_titles: bool) -> (String, Vec<String>) {
    let mut ids = Vec::new();
    let mut s = String::new();
    // the most recent windows, listed in a stable order (workspace, address) so the text stays the same between
    // commands and llama.cpp reuses it from the cache; what changes (focus, eyes, hints, words) comes last
    let mut recent: Vec<&windows::Client> = ctx.clients.iter().take(WINDOWS).collect();
    recent.sort_by(|a, b| (a.workspace, &a.address).cmp(&(b.workspace, &b.address)));
    let wins: Vec<String> = recent
        .into_iter()
        .enumerate()
        .map(|(n, c)| {
            ids.push(c.address.clone());
            let mut w = format!("w{} {}", n + 1, windows::clean_title(&c.class));
            if !strip_titles && !c.title.trim().is_empty() {
                // leading spinners / status glyphs ("◑ build") change every second: drop them, so the line stays cacheable
                let t: String = windows::clean_title(&c.title).trim_start_matches(|ch: char| !ch.is_alphanumeric()).chars().take(TITLE_CHARS).collect();
                w += &format!(" \"{}\"", t.replace('"', "'"));
            }
            w += &format!(" ws{}", c.workspace);
            w
        })
        .collect();
    let wname = |a: &str| ids.iter().position(|x| x == a).map(|p| format!("w{}", p + 1));
    if !wins.is_empty() {
        s += &format!("apps: {}\n", wins.join("; "));
    }
    // no VR state line: the small model copied it ("vr now: assistant preset" turned "dark" and "necks
    // background" into vr.preset.assistant, audit 2026-10-15); no intent needs it to decide
    if let Some(n) = ctx.focused.as_ref().and_then(|f| wname(&f.address)) {
        s += &format!("focused: {n}\n");
    }
    if let Some((w, src)) = &ctx.this {
        if *src == "gaze" {
            if let Some(n) = wname(w) {
                s += &format!("eyes on: {n}\n");
            }
        }
    }
    // (no "last command" line: a small model copies it instead of reading the request)
    if !cands.is_empty() {
        let h: Vec<String> = cands.iter().map(|c| format!("{} {:.2}", c.id, c.score)).collect();
        s += &format!("hints: {}\n", h.join(", "));
    }
    s += &format!("heard: \"{}\"", windows::clean_title(heard).replace('"', "'"));
    (s, ids)
}

/// The model's decision.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// An intent id, a window pseudo-intent, or "none".
    pub intent: String,
    /// 0..1.
    pub confidence: f64,
    pub slots: BTreeMap<String, String>,
}

/// Parse an answer line leniently (remote models have no grammar: code fences, "intent: x", 0.8 or 8).
pub fn parse(answer: &str) -> Result<Decision, String> {
    let line = answer
        .lines()
        .map(|l| l.trim().trim_matches('`').trim())
        .find(|l| !l.is_empty() && !l.starts_with("```"))
        .ok_or("empty answer")?;
    let line = line.strip_prefix("intent:").unwrap_or(line).trim();
    let mut rest = line;
    let tok = |r: &mut &str| -> Option<String> {
        let t = r.trim_start();
        if t.is_empty() {
            return None;
        }
        let end = t.find(char::is_whitespace).unwrap_or(t.len());
        let (a, b) = t.split_at(end);
        *r = b;
        Some(a.to_string())
    };
    let intent = tok(&mut rest).ok_or("no intent")?.trim_matches(|c: char| c == ',' || c == '"').to_string();
    if !intent.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
        return Err(format!("bad intent {intent:?}"));
    }
    let mut confidence = 0.5;
    let save = rest;
    match tok(&mut rest).map(|c| c.trim_matches(',').to_string()) {
        Some(c) if c.parse::<f64>().is_ok() => {
            let x: f64 = c.parse().unwrap_or(5.0);
            confidence = if x > 1.0 { (x / 9.0).min(1.0) } else { x.max(0.0) };
        }
        _ => rest = save,
    }
    let mut slots = BTreeMap::new();
    let mut r = rest.trim();
    while !r.is_empty() {
        let Some(eq) = r.find('=') else { break };
        let key = r[..eq].trim().to_string();
        let after = &r[eq + 1..];
        let (val, next) = if let Some(q) = after.strip_prefix('"') {
            match q.find('"') {
                Some(e) => (q[..e].to_string(), &q[e + 1..]),
                None => (q.to_string(), ""),
            }
        } else {
            let e = after.find(char::is_whitespace).unwrap_or(after.len());
            (after[..e].to_string(), &after[e..])
        };
        if !key.is_empty() {
            slots.insert(key, val.trim().to_string());
        }
        r = next.trim();
    }
    Ok(Decision { intent, confidence, slots })
}

impl Decision {
    /// The agent tool call for this decision (window `wN` resolved to addresses); None for "none".
    pub fn to_call(&self, ids: &[String]) -> Result<Option<agent::Call>, String> {
        if self.intent == "none" {
            return Ok(None);
        }
        let mut args = serde_json::Map::new();
        let is_window_tool = WINDOW_INTENTS.iter().any(|(id, _, _)| *id == self.intent);
        for (k, v) in &self.slots {
            let val = if k == "window" {
                let n: usize = v.trim_start_matches('w').parse().map_err(|_| format!("bad window {v:?}"))?;
                serde_json::Value::String(ids.get(n.wrapping_sub(1)).cloned().ok_or_else(|| format!("no window {v}"))?)
            } else if is_window_tool && k == "workspace" {
                serde_json::Value::from(v.parse::<u64>().map_err(|_| format!("bad workspace {v:?}"))?)
            } else {
                serde_json::Value::String(v.clone())
            };
            args.insert(k.clone(), val);
        }
        let name = if is_window_tool { self.intent.clone() } else { agent::tool_name(&self.intent) };
        Ok(Some(agent::Call { id: "nlu".into(), name, args: serde_json::Value::Object(args) }))
    }

    /// "win.list 8/9 n=3" for the debug line and log.
    pub fn short(&self) -> String {
        let mut s = format!("{} {:.0}/9", self.intent, (self.confidence * 9.0).round());
        for (k, v) in &self.slots {
            s += &format!(" {k}={v}");
        }
        s
    }
}

/// A plan: one to three decisions separated by " ; " (or new lines).
pub fn parse_plan(answer: &str) -> Result<Vec<Decision>, String> {
    let mut v = Vec::new();
    for part in answer.split(|c| c == ';' || c == '\n').map(str::trim).filter(|p| !p.is_empty() && !p.starts_with("```")) {
        v.push(parse(part)?);
        if v.len() == 3 {
            break;
        }
    }
    if v.is_empty() {
        return Err("empty answer".into());
    }
    Ok(v)
}

/// The fuzzy candidates for a request (re-exported for the agent and the bench).
pub fn hints(cat: &Catalogue, words: &[String]) -> Vec<Cand> {
    fuzzy::candidates(cat, words, 3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::BUILTIN;
    use crate::windows::Client;

    fn cat() -> Catalogue {
        Catalogue::from_texts(BUILTIN, None).unwrap()
    }

    #[test]
    fn system_prompt_is_compact_and_static() {
        let c = cat();
        let p = system_prompt(&c);
        assert_eq!(p, system_prompt(&c));
        assert!(p.contains("win.list: "));
        assert!(p.contains("omarchy.reminder minutes=N text=\"words\""));
        assert!(p.contains("focus_window window=wN"));
        assert!(!p.contains("win.ai.switch"));
        // about 4 characters per token: about 2,300 tokens, cached by the server (was ~5,650 with the tool schemas;
        // +9 VR screen intents on 2026-10-15)
        assert!(p.len() < 9800, "{} chars", p.len());
    }

    /// Every rule a GBNF grammar uses is defined (an undefined one makes llama-server answer HTTP 400 to every
    /// request: the `side` slot type of 2026-10-15).
    fn undefined_rules(g: &str) -> Vec<String> {
        let mut defined = std::collections::BTreeSet::new();
        let mut used = std::collections::BTreeSet::new();
        for line in g.lines() {
            let Some((lhs, rhs)) = line.split_once("::=") else { continue };
            defined.insert(lhs.trim().to_string());
            // drop quoted strings and character classes, keep identifiers
            let mut out = String::new();
            let (mut q, mut br, mut esc) = (false, false, false);
            for ch in rhs.chars() {
                if esc {
                    esc = false;
                    continue;
                }
                match ch {
                    '\\' if q || br => esc = true,
                    '"' if !br => q = !q,
                    '[' if !q => br = true,
                    ']' if br => br = false,
                    c if !q && !br => out.push(c),
                    _ => {}
                }
            }
            for w in out.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
                if !w.is_empty() && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                    used.insert(w.to_string());
                }
            }
        }
        used.into_iter().filter(|u| !defined.contains(u)).collect()
    }

    #[test]
    fn every_grammar_rule_is_defined() {
        let c = cat();
        for g in [grammar(&c), grammar_steps(&c, false)] {
            assert_eq!(undefined_rules(&g), Vec::<String>::new());
        }
        assert_eq!(undefined_rules("root ::= a | \"x\" side\na ::= [a-z]\n"), vec!["side".to_string()]);
    }

    #[test]
    fn grammar_lists_ids_and_slot_rules() {
        let g = grammar(&cat());
        assert!(g.starts_with("root ::= one (\" ; \" one){0,2}\none ::= plain | r0"), "{}", &g[..80]);
        assert!(g.contains("\"vr.preset.cinema\""));
        assert!(g.contains("\"omarchy.reminder\" \" \" c \" minutes=\" num \" text=\" txt"));
        assert!(g.contains("\"move_window\" \" \" c \" window=\" win \" workspace=\" num"));
    }

    #[test]
    fn answers_parse() {
        let d = parse("win.list 8").unwrap();
        assert_eq!((d.intent.as_str(), (d.confidence * 9.0).round() as i32), ("win.list", 8));
        let d = parse("omarchy.reminder 9 minutes=5 text=\"stretch my legs\"").unwrap();
        assert_eq!(d.slots["minutes"], "5");
        assert_eq!(d.slots["text"], "stretch my legs");
        let d = parse("```\nvr.preset.work3 0.7\n```").unwrap();
        assert_eq!((d.intent.as_str(), d.confidence), ("vr.preset.work3", 0.7));
        let d = parse("focus_window 7 window=w2").unwrap();
        let ids = vec!["0xa1".to_string(), "0xb2".to_string()];
        let call = d.to_call(&ids).unwrap().unwrap();
        assert_eq!((call.name.as_str(), call.args["window"].as_str()), ("focus_window", Some("0xb2")));
        assert!(parse("focus_window 7 window=w9").unwrap().to_call(&ids).is_err());
        let call = parse("win.ws.goto 9 n=4").unwrap().to_call(&ids).unwrap().unwrap();
        assert_eq!((call.name.as_str(), call.args["n"].as_str()), ("win_ws_goto", Some("4")));
        assert!(parse("none 9").unwrap().to_call(&ids).unwrap().is_none());
        assert!(parse("").is_err());
        let p = parse_plan("win.ws.goto 9 n=2 ; omarchy.window.fullscreen 8").unwrap();
        assert_eq!((p.len(), p[1].intent.as_str()), (2, "omarchy.window.fullscreen"));
        assert!(parse("$(reboot) 9").is_err());
    }

    #[test]
    fn user_message_is_short() {
        let mut ctx = Context::default();
        for i in 0..12 {
            ctx.clients.push(Client { address: format!("0x{i}a"), class: "foot".into(), title: "x".repeat(100), workspace: 2, ..Default::default() });
        }
        ctx.focused = ctx.clients.first().cloned();
        ctx.this = Some(("0x1a".into(), "gaze"));
        let cands = fuzzy::candidates(&cat(), &crate::command::words("liz windows"), 3);
        let (m, ids) = user_message(&ctx, &cands, "liz windows", false);
        // not about windows: no list
        let (m2, ids2) = user_message(&ctx, &cands, "cinema", false);
        assert!(ids2.is_empty() && !m2.contains("windows:"), "{m2}");
        // a word of a window class: the list
        assert!(needs_windows(&ctx, "the foot one") && !needs_windows(&ctx, "volume up"));
        assert_eq!(ids.len(), WINDOWS);
        assert!(m.contains("w1 foot \"xxxx") && m.contains("focused: w1") && m.contains("eyes on: w2"), "{m}");
        assert!(m.contains("hints: win.list 0.9"), "{m}");
        assert!(m.ends_with("heard: \"liz windows\""));
        assert!(m.len() < 900, "{}", m.len());
        let (m, _) = user_message(&ctx, &[], "x", true);
        assert!(!m.contains("xxxx"));
    }
}
