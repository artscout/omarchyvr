//! Offline intent matching for voice commands: utterance -> (wake word?) -> intent id + slot values.
//!
//! * Words are normalised ([`crate::intent::normalize`]: lower case, accents folded, punctuation dropped), a few
//!   compounds are joined ("pass through" -> passthrough, "full screen" -> fullscreen), number words become digits
//!   ("three" -> 3, "tres" -> 3).
//! * Filler words ("please", "the", "can you", "por favor", ...) are ignored on both sides, so "could you please
//!   switch to the cinema mode" matches the phrase "switch to cinema mode".
//! * A command must start with a wake word ("Max ..."; see `[wake]` in the catalogue) and the WHOLE rest must match
//!   one phrase: talk that merely contains a command, or captions, never trigger anything.
//! * The phrase with the most literal words wins (so "next background" beats "{name} background").

use crate::catalogue::{Catalogue, Intent, SlotType};
use std::collections::BTreeMap;

/// Ignored on both sides of a match.
pub const FILLERS: &[&str] = &[
    "please", "pls", "the", "a", "an", "my", "can", "could", "would", "will", "you", "kindly", "now", "just", "me",
    "for", "okay", "ok", "hey", "oye", "por", "favor", "el", "la", "los", "las", "un", "una", "thanks",
    "thank", "uh", "um", "eh", "so", "like",
];

/// Words the ASR produces from "Pi"/"Hey"/"Ok" in front of "Max" ("Pimax" -> "Five marks", "Time Max", "B Max").
/// "five" is "5" here because numbers are normalised first.
const LEAD: &[&str] = &[
    "hey", "uh", "um", "ah", "oh", "ok", "okay", "oye", "hi", "yo", "pi", "pie", "b", "be", "bee", "p", "5", "time", "fine", "by", "bye", "the",
];

/// The wake word must start within this many words, with only [`LEAD`] words before it (at most 2).
const WAKE_MAX_LEAD: usize = 2;

/// ASR mishearings of command words, mapped to the word the catalogue phrases use (applied while normalising,
/// before number words become digits, so "free" -> "three" -> 3). Parakeet hears "work three" as "war three".
const ALIASES: &[(&str, &str)] = &[
    ("war", "work"), ("walk", "work"), ("wok", "work"), ("worth", "work"),
    ("tree", "three"), ("free", "three"),
    ("won", "one"),
    ("cinemas", "cinema"), ("sinema", "cinema"),
    // live 2026-10-16: "clot screens" (close), "two screams" (screens)
    ("clot", "close"), ("screams", "screens"), ("scream", "screen"),
    // window control
    ("tabs", "tab"), ("tap", "tab"), ("tub", "tab"), ("taps", "tab"),
    ("prev", "previous"), ("preview", "previous"), ("previews", "previous"),
    ("workspaces", "workspace"), ("pestana", "tab"), ("pestanas", "tab"),
];

/// Number-looking words the ASR writes after "workspace"/"tab" ("workspace for" = 4, "tab to" = 2).
const AFTER_WORKSPACE: &[(&str, &str)] = &[("to", "2"), ("too", "2"), ("for", "4"), ("fore", "4"), ("tu", "2"), ("ate", "8")];

const NUMBERS: &[(&str, &str)] = &[
    ("zero", "0"), ("one", "1"), ("two", "2"), ("three", "3"), ("four", "4"), ("five", "5"), ("six", "6"),
    ("seven", "7"), ("eight", "8"), ("nine", "9"), ("ten", "10"), ("eleven", "11"), ("twelve", "12"),
    ("fifteen", "15"), ("twenty", "20"), ("thirty", "30"), ("forty", "40"), ("forty-five", "45"), ("fifty", "50"),
    ("sixty", "60"), ("ninety", "90"), ("uno", "1"), ("dos", "2"), ("tres", "3"), ("cuatro", "4"), ("cinco", "5"),
    ("diez", "10"), ("veinte", "20"), ("treinta", "30"),
    ("first", "1"), ("second", "2"), ("third", "3"), ("fourth", "4"), ("fifth", "5"), ("sixth", "6"), ("seventh", "7"), ("eighth", "8"), ("ninth", "9"),
];

pub(crate) const COMPOUNDS: &[(&[&str], &str)] = &[
    (&["pass", "through"], "passthrough"),
    (&["pass", "thru"], "passthrough"),
    (&["passthru"], "passthrough"),
    (&["full", "screen"], "fullscreen"),
    (&["full", "scream"], "fullscreen"),
    (&["full", "screams"], "fullscreen"),
    // "stop pass through" heard as "stop us through" / "past through" (live 2026-10-16)
    (&["us", "through"], "passthrough"),
    (&["past", "through"], "passthrough"),
    (&["pass", "true"], "passthrough"),
    // the screens above the left / right ones (6 screens, user decision 2026-10-16)
    (&["up", "left"], "upleft"),
    (&["upper", "left"], "upleft"),
    (&["top", "left"], "upleft"),
    (&["up", "right"], "upright"),
    (&["upper", "right"], "upright"),
    (&["top", "right"], "upright"),
    (&["screen", "shot"], "screenshot"),
    (&["wall", "paper"], "wallpaper"),
    (&["scratch", "pad"], "scratchpad"),
    (&["sky", "box"], "skybox"),
    (&["re", "center"], "recenter"),
    (&["re", "centre"], "recenter"),
    (&["recentre"], "recenter"),
    (&["frame", "rate"], "framerate"),
    (&["never", "mind"], "nevermind"),
    (&["what's"], "what"),
    (&["work", "space"], "workspace"),
    (&["war", "space"], "workspace"),
    (&["walk", "space"], "workspace"),
    (&["work", "spaces"], "workspace"),
    (&["espacio", "de", "trabajo"], "workspace"),
];

/// Words of a text: normalised, compounds joined, numbers as digits (fillers kept: see [`content`]).
pub fn words(text: &str) -> Vec<String> {
    let w = crate::intent::normalize(&text.replace('-', " "));
    let mut out: Vec<String> = Vec::with_capacity(w.len());
    let mut i = 0;
    'outer: while i < w.len() {
        for (pat, rep) in COMPOUNDS {
            if w.len() >= i + pat.len() && w[i..i + pat.len()].iter().zip(pat.iter()).all(|(a, b)| a == b) {
                out.push((*rep).to_string());
                i += pat.len();
                continue 'outer;
            }
        }
        let x = &w[i];
        // "workspace for" / "tab to" / "screen number for" / "screens to and four": the ASR spells the number as a
        // word that is a filler or a preposition (not before "the", "me", a side: "the screen to the left")
        if let Some((_, d)) = AFTER_WORKSPACE.iter().find(|(a, _)| a == x) {
            let next_ok = w.get(i + 1).is_none_or(|n| !matches!(n.as_str(), "the" | "a" | "an" | "me" | "my" | "you" | "it" | "this" | "that" | "left" | "right" | "up" | "down" | "main" | "top" | "bottom" | "upper" | "now"));
            let screenish = out.last().is_some_and(|p| matches!(p.as_str(), "screen" | "screens" | "number" | "numbers")) && next_ok;
            if out.last().is_some_and(|p| p == "workspace" || p == "tab") || screenish {
                out.push((*d).to_string());
                i += 1;
                continue;
            }
        }
        // "desktop two" / "escritorio 3": a numbered desktop is a workspace (not an app called "desktop")
        let num_next = w.get(i + 1).is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()) || NUMBERS.iter().any(|(k, _)| k == n) || ALIASES.iter().any(|(a, r)| a == n && NUMBERS.iter().any(|(k, _)| k == r)));
        if num_next && matches!(x.as_str(), "desktop" | "escritorio") {
            out.push("workspace".into());
            i += 1;
            continue;
        }
        let x = ALIASES.iter().find(|(a, _)| *a == x).map(|(_, r)| r.to_string()).unwrap_or_else(|| x.clone());
        let x = NUMBERS.iter().find(|(n, _)| *n == x).map(|(_, d)| d.to_string()).unwrap_or_else(|| x.clone());
        out.push(x);
        i += 1;
    }
    out
}

/// Indices of the non-filler words.
pub(crate) fn content(w: &[String]) -> Vec<usize> {
    (0..w.len()).filter(|&i| !FILLERS.contains(&w[i].as_str())).collect()
}

/// A compiled phrase token.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    Lit(String),
    Slot(String),
}

pub(crate) fn compile(phrase: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut lit = String::new();
    let flush = |lit: &mut String, out: &mut Vec<Tok>| {
        for w in words(lit) {
            if !FILLERS.contains(&w.as_str()) {
                out.push(Tok::Lit(w));
            }
        }
        lit.clear();
    };
    for part in phrase.split_whitespace() {
        if let Some(name) = part.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            flush(&mut lit, &mut out);
            out.push(Tok::Slot(name.to_string()));
        } else {
            lit.push(' ');
            lit.push_str(part);
        }
    }
    flush(&mut lit, &mut out);
    out
}

/// A match.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub id: String,
    pub slots: BTreeMap<String, String>,
    /// Literal words matched (the ranking).
    pub score: usize,
}

/// A side or a screen is stored in its one canonical word: top / above -> up, middle -> main, "three" -> 3.
pub fn norm_slot(ty: SlotType, val: String) -> String {
    match ty {
        SlotType::Side => crate::windows::slot_word(&val).map(str::to_string).unwrap_or(val),
        SlotType::Screen => crate::windows::screen_value(&val).unwrap_or(val),
        _ => val,
    }
}

/// Is `v` an acceptable value of a slot?
pub fn slot_ok(t: SlotType, v: &str) -> bool {
    match t {
        SlotType::Number => v.parse::<u32>().is_ok_and(|n| (1..=999).contains(&n)),
        SlotType::Word => !v.is_empty() && v.len() <= 32 && v.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'),
        SlotType::Side => crate::windows::slot_word(v).is_some(),
        SlotType::Screen => crate::windows::screen_value(v).is_ok(),
        SlotType::Text | SlotType::Name => {
            !v.trim().is_empty()
                && v.chars().count() <= 120
                && v.chars().all(|c| c.is_alphanumeric() || " ',.?!-_".contains(c))
        }
    }
}

/// Match `toks` against the content words `cw` (indices into `w`); slot values are taken from the original
/// words (fillers inside a slot are kept).
fn match_toks(
    toks: &[Tok],
    w: &[String],
    cw: &[usize],
    slots: &BTreeMap<String, SlotType>,
    out: &mut BTreeMap<String, String>,
) -> bool {
    let Some(first) = toks.first() else {
        return cw.is_empty();
    };
    match first {
        Tok::Lit(l) => !cw.is_empty() && &w[cw[0]] == l && match_toks(&toks[1..], w, &cw[1..], slots, out),
        Tok::Slot(name) => {
            let ty = slots.get(name).copied().unwrap_or(SlotType::Text);
            let max = match ty {
                SlotType::Number | SlotType::Word | SlotType::Side | SlotType::Screen => 1,
                _ => cw.len(),
            };
            // shortest first, so the literal words after a slot bind as early as possible
            for n in 1..=max.min(cw.len()) {
                let last = toks.len() == 1;
                let end = if last { w.len() } else { cw[n - 1] + 1 };
                let val = w[cw[0]..end].join(" ");
                if !slot_ok(ty, &val) {
                    continue;
                }
                if last && n != cw.len() {
                    continue;
                }
                let mut o2 = out.clone();
                // a side is stored in its one word: top / above -> up, middle -> main
                let val = norm_slot(ty, val);
                o2.insert(name.clone(), val);
                if match_toks(&toks[1..], w, &cw[n..], slots, &mut o2) {
                    *out = o2;
                    return true;
                }
            }
            false
        }
    }
}

/// Match a command (without the wake word) against one intent.
pub fn match_intent(i: &Intent, w: &[String]) -> Option<Match> {
    let cw = content(w);
    if cw.is_empty() {
        return None;
    }
    let mut best: Option<Match> = None;
    for p in &i.phrases {
        let toks = compile(p);
        if toks.is_empty() {
            continue;
        }
        let mut slots = BTreeMap::new();
        if match_toks(&toks, w, &cw, &i.slots, &mut slots) {
            // an intent that destroys something never takes a slot of "all" / "every" ("close all windows")
            if i.confirm && slots.values().any(|v| words(v).iter().any(|x| matches!(x.as_str(), "all" | "every" | "everything" | "windows" | "apps"))) {
                continue;
            }
            let score = toks.iter().filter(|t| matches!(t, Tok::Lit(_))).count();
            if best.as_ref().is_none_or(|b| score > b.score) {
                best = Some(Match { id: i.id.clone(), slots, score });
            }
        }
    }
    best
}

/// The best enabled intent for a command (without the wake word).
pub fn best(c: &Catalogue, w: &[String]) -> Option<Match> {
    let mut best: Option<Match> = None;
    for i in c.enabled() {
        if let Some(m) = match_intent(i, w) {
            if best.as_ref().is_none_or(|b| m.score > b.score) {
                best = Some(m);
            }
        }
    }
    best
}

/// What an utterance is, as far as the wake word goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    /// No wake word at the start: ignore (normal talk).
    Talk,
    /// Only the wake word: listen for the command in the next utterance.
    WakeOnly,
    /// Wake word + these words.
    Command(Vec<String>),
}

/// Split off the wake word (`wake` = the catalogue's spellings, normalised here).
pub fn wake(text: &str, wake_words: &[String]) -> Heard {
    let w = words(text);
    let pats: Vec<Vec<String>> = wake_words.iter().map(|ww| words(ww)).filter(|p| !p.is_empty()).collect();
    // The wake word starts after at most WAKE_MAX_LEAD lead words, and only lead words may precede it.
    let mut found = None;
    for i in 0..=WAKE_MAX_LEAD {
        if i > w.len() || (i > 0 && !LEAD.contains(&w[i - 1].as_str())) {
            break;
        }
        let best = pats.iter().filter(|p| w.len() >= i + p.len() && w[i..i + p.len()] == p[..]).map(|p| p.len()).max();
        if let Some(b) = best {
            found = Some(i + b);
            break;
        }
    }
    let Some(end) = found else { return Heard::Talk };
    let rest = w[end..].to_vec();
    if content(&rest).is_empty() {
        Heard::WakeOnly
    } else {
        Heard::Command(rest)
    }
}

/// A wake word in the middle of a sentence, after a pause the speech-to-text marks with punctuation ("Windows, can
/// we ha Max, can we have two more windows?"): the text from that wake word on, when a punctuation mark of the
/// speech-to-text comes right before or after it and at least 2 words follow it (user decision 2026-10-16). "set it
/// to max" and "I told Max to switch" (no pause marked) never wake.
pub fn wake_mid(text: &str, wake_words: &[String]) -> Option<String> {
    let toks: Vec<&str> = text.split_whitespace().collect();
    let singles: Vec<String> = wake_words.iter().map(|w| words(w)).filter(|p| p.len() == 1).map(|p| p[0].clone()).collect();
    // the last one: "max can we... max, can we have two more" means the second try
    for i in (1..toks.len()).rev() {
        // a pause before it ("..., Max can you") or the name called out ("we ha Max, can we")
        let prev = toks[i - 1];
        if !prev.ends_with([',', '.', '?', '!', ';', ':']) && !toks[i].ends_with(',') {
            continue;
        }
        let w = words(toks[i]);
        if w.len() == 1 && singles.contains(&w[0]) && toks.len() >= i + 3 {
            return Some(mid_text(&toks, i));
        }
    }
    // no pause marked, but only fillers before it ("Can you Max can you shut down ...", "uh Max can you ..."), or the
    // name called out before a polite request ("so Max can you close screen 2"; live 2026-10-16): "I told Max to
    // switch" (no request after it) and "set it to max" (nothing after it) still never wake
    for i in (1..toks.len()).rev() {
        let w = words(toks[i]);
        if !(w.len() == 1 && singles.contains(&w[0]) && toks.len() >= i + 3) {
            continue;
        }
        let before = words(&toks[..i].join(" "));
        let after = words(&toks[i + 1..].join(" "));
        let only_fillers = before.iter().all(|x| FILLERS.contains(&x.as_str()) || LEAD.contains(&x.as_str()) || matches!(x.as_str(), "yes" | "yeah" | "and" | "s" | "sh" | "st" | "all" | "of" | "them" | "this" | "make" | "do"));
        let polite = after.first().is_some_and(|x| matches!(x.as_str(), "can" | "could" | "would" | "will" | "please" | "lets" | "let's"));
        if only_fillers || polite {
            return Some(mid_text(&toks, i));
        }
    }
    None
}

/// The text from the wake word at `i` on; when the words after it are only a fragment ("..., Max, the screen.") the
/// words before it stay too: "Can you move it in front of me, Max, the screen." = "Max can you move it in front of
/// me the screen" (live 2026-10-16).
fn mid_text(toks: &[&str], i: usize) -> String {
    let after = words(&toks[i + 1..].join(" "));
    if fragment(&after) {
        let before = toks[..i].join(" ");
        return format!("{} {} {}", toks[i].trim_end_matches([',', '.', '?', '!']), before.trim_end_matches([',', '.']), toks[i + 1..].join(" "));
    }
    toks[i..].join(" ")
}

/// Words that are no command of their own: at most 2 content words, all of them pointing at something said before
/// ("the screen", "this one", "please").
pub fn fragment(after: &[String]) -> bool {
    let c: Vec<&str> = content(after).iter().map(|&i| after[i].as_str()).collect();
    c.len() <= 2 && c.iter().all(|x| matches!(*x, "screen" | "screens" | "window" | "windows" | "it" | "this" | "that" | "one" | "please" | "there" | "here" | "them" | "thing"))
}

/// A wake word again inside the command words ("can you s max can you shut down uh work vr", "make max can you make
/// all of these screens half the size"): the speaker started over, so the words after the LAST one count. When
/// those are only a fragment ("move it in front of me max the screen"), the wake word is just dropped. Only for a
/// restart: the words after it start a request ("can", "could", "please", ...) or only fillers come before it.
pub fn after_last_wake(w: &[String], wake_words: &[String]) -> Vec<String> {
    let singles: Vec<String> = wake_words.iter().map(|x| words(x)).filter(|p| p.len() == 1).map(|p| p[0].clone()).collect();
    let Some(j) = w.iter().rposition(|x| singles.contains(x)) else { return w.to_vec() };
    let (before, after) = (&w[..j], &w[j + 1..]);
    if fragment(after) {
        let mut v = before.to_vec();
        v.extend_from_slice(after);
        return v;
    }
    let polite = after.first().is_some_and(|x| matches!(x.as_str(), "can" | "could" | "would" | "will" | "please" | "lets" | "let's"));
    let stutter = content(before).len() <= 2;
    if polite || stutter {
        return after.to_vec();
    }
    w.to_vec()
}

/// Words a sentence does not end on when it is complete: articles, prepositions, conjunctions, hesitations.
const DANGLE: &[&str] = &[
    "the", "a", "an", "of", "to", "for", "with", "and", "or", "but", "uh", "um", "uhm", "er", "erm", "ah", "eh", "hmm", "mm",
    "my", "your", "our", "their", "some", "can", "could", "would", "will", "you", "we", "into",
    "onto", "from", "about", "than", "then", "is", "are", "las", "los", "el", "la", "de", "y",
];
/// Verbs that need an object: "can you move" alone is not a command yet.
const NEEDS_OBJECT: &[&str] = &["move", "open", "put", "set", "make", "send", "type", "write", "bring", "add", "create", "close", "go", "switch", "search", "press", "have", "give", "want", "need"];

/// Was the speaker cut off ("can you move uh one of the", "can you move", "can you write uh")? Then the next
/// sentence continues this one (docs/VOICE.md, "Pauses").
pub fn dangling(w: &[String]) -> bool {
    let Some(last) = w.last() else { return false };
    if DANGLE.contains(&last.as_str()) {
        return true;
    }
    let c: Vec<&str> = content(w).iter().map(|&i| w[i].as_str()).filter(|x| !matches!(*x, "we" | "i" | "uh" | "um")).collect();
    !c.is_empty() && c.iter().all(|x| NEEDS_OBJECT.contains(x))
}

/// "Max, cancel", "cancel that", "stop" (with or without the wake word): the words that stop a task of several
/// steps while it runs (user decision 2026-10-16). Never a new goal.
pub fn is_cancel(text: &str, wake_words: &[String]) -> bool {
    let w = match wake(text, wake_words) {
        Heard::Command(w) => w,
        Heard::WakeOnly => return false,
        Heard::Talk => words(text),
    };
    let c: Vec<&str> = content(&w).iter().map(|&i| w[i].as_str()).filter(|x| !matches!(*x, "that" | "it" | "this" | "everything" | "all" | "task" | "please")).collect();
    matches!(c.as_slice(), ["cancel"] | ["stop"] | ["abort"] | ["nevermind"] | ["forget"] | ["cancela"] | ["para"] | ["stop", "stop"] | ["cancel", "cancel"])
}

/// A request taken back in the same breath (general grammar, not phrases): "hide screen two, no, forget it",
/// "put the terminals on 3, no wait, cancel that", "make them bigger. Max, cancel." -> [`TakeBack::Cancel`];
/// "two screens, no, three screens", "give me two more, actually three screens in all" -> the correction alone.
#[derive(Debug, Clone, PartialEq)]
pub enum TakeBack {
    None,
    Cancel,
    Correct(Vec<String>),
}

/// Words that take back what was just said.
const CANCEL_WORDS: &[&str] = &["cancel", "nevermind", "forget", "abort", "scratch", "ignore", "cancela", "olvidalo"];
/// Words a take-back tail may have besides a cancel word ("no wait, forget about it, never mind that").
const CANCEL_OK: &[&str] = &["no", "wait", "it", "that", "this", "about", "dont", "do", "not", "never", "mind", "max", "actually", "sorry", "oh", "please", "all", "the", "last", "one", "thing", "request", "command"];
/// Words that start a correction ("no", "actually", "i mean", "sorry", "wait", "or rather").
const CORRECT_WORDS: &[&str] = &["no", "actually", "sorry", "wait", "rather", "mean", "instead"];

/// `raw`: the text as heard; a "no" corrects only when the speech-to-text marked a pause at it ("two screens, no,
/// three") or "wait" / "sorry" / "actually" follows it ("cinema mode, no room" is passthrough off, no correction).
pub fn take_back(w: &[String], raw: &str) -> TakeBack {
    let rl = raw.to_lowercase();
    let no_marked = rl.split_whitespace().collect::<Vec<_>>().windows(2).any(|p| (p[0] == "no," || p[0] == "no." || p[0] == "no!") || (p[0].trim_end_matches(',') == "no" && matches!(p[1].trim_matches(|c: char| !c.is_alphanumeric()), "wait" | "sorry" | "actually" | "i"))) || rl.trim_end().ends_with(" no");
    let c = content(w);
    if c.len() < 2 {
        return TakeBack::None;
    }
    // a cancel word in the tail, with only take-back words around it, after at least one other content word
    if let Some(k) = c.iter().rposition(|&i| CANCEL_WORDS.contains(&w[i].as_str())) {
        let mut start = k;
        while start > 0 && (CANCEL_OK.contains(&w[c[start - 1]].as_str()) || CORRECT_WORDS.contains(&w[c[start - 1]].as_str())) {
            start -= 1;
        }
        let tail_ok = c[k + 1..].iter().all(|&i| CANCEL_OK.contains(&w[i].as_str()));
        // "cancel the cinema mode" / "ignore the terminal" name a thing to act on: no take-back
        if tail_ok && start > 0 {
            return TakeBack::Cancel;
        }
    }
    // a correction: "<request> no <request>", "<request> actually <request>"; the last one counts. Not "no" in front
    // of a number ("number no 3"), and the first part must be a request of its own (2+ content words)
    for (j, &i) in c.iter().enumerate().rev() {
        let x = w[i].as_str();
        let marker = matches!(x, "no" | "actually" | "sorry" | "instead") || (x == "mean" && i > 0 && w[i - 1] == "i") || (x == "rather" && i > 0 && w[i - 1] == "or");
        if !marker || j < 2 {
            continue;
        }
        if x == "no" && (!no_marked || w.get(i + 1).is_some_and(|n| matches!(n.as_str(), "more" | "longer" | "one" | "need"))) {
            continue;
        }
        let mut k = j + 1;
        while k < c.len() && (CORRECT_WORDS.contains(&w[c[k]].as_str()) || matches!(w[c[k]].as_str(), "i" | "or" | "max")) {
            k += 1;
        }
        if k >= c.len() || c.len() - k < 1 {
            continue;
        }
        let tail: Vec<String> = w[c[k]..].to_vec();
        if content(&tail).is_empty() {
            continue;
        }
        return TakeBack::Correct(tail);
    }
    TakeBack::None
}

/// Verbs that start a command of their own after "and" / "then" ("close screen 2 and make the main one bigger").
const CLAUSE_VERBS: &[&str] = &[
    "make", "put", "move", "open", "close", "hide", "show", "turn", "switch", "go", "play", "start", "stop", "set", "add",
    "give", "bring", "take", "mute", "unmute", "pause", "resume", "search", "google", "type", "write", "dim", "record",
    "remind", "send", "focus", "launch", "change", "exit", "leave", "enter", "lower", "raise", "swap", "reset", "skip",
    "shrink", "enlarge", "push", "pull", "press", "scroll", "reload", "maximize", "minimize", "quit", "remove", "get",
];
const CLAUSE_JOIN: &[&str] = &["and", "then", "also", "plus", "afterwards", "y", "luego"];

/// The clauses of a request ("close screen 2 and make the main one bigger" -> 2): a split at "and" / "then" right
/// before a command verb. "hide screens 2, 3 and 4" or "terminals left and chrome right" stay one clause.
pub fn clauses(w: &[String]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut i = 0;
    while i < w.len() {
        if CLAUSE_JOIN.contains(&w[i].as_str()) && !content(&cur).is_empty() {
            // "and then make", "and please make", "then can you make"
            let mut j = i + 1;
            while j < w.len() && (CLAUSE_JOIN.contains(&w[j].as_str()) || FILLERS.contains(&w[j].as_str())) {
                j += 1;
            }
            if j < w.len() && CLAUSE_VERBS.contains(&w[j].as_str()) {
                out.push(std::mem::take(&mut cur));
                i = j;
                continue;
            }
        }
        cur.push(w[i].clone());
        i += 1;
    }
    if !content(&cur).is_empty() {
        out.push(cur);
    } else if let Some(last) = out.last_mut() {
        last.extend(cur);
    }
    out
}

/// Words that open a sentence without being part of the command ("yeah put youtube ...", "so uh open ...", "well,
/// okay, hide ..."): dropped from the front.
const OPENERS: &[&str] = &["yeah", "yes", "yep", "well", "so", "okay", "ok", "alright", "and", "uh", "um", "uhm", "oh", "hmm", "eh", "ah", "now", "hey", "right", "also"];

pub fn drop_openers(w: &[String]) -> Vec<String> {
    let mut i = 0;
    // "right" is a direction too: only as an opener when more words follow and another opener or a verb comes next
    while i < w.len() && OPENERS.contains(&w[i].as_str()) && w.len() - i > 2 {
        if w[i] == "right" && !w.get(i + 1).is_some_and(|n| OPENERS.contains(&n.as_str()) || CLAUSE_VERBS.contains(&n.as_str()) || matches!(n.as_str(), "can" | "could" | "please")) {
            break;
        }
        i += 1;
    }
    w[i..].to_vec()
}

/// Hesitations dropped from typed text and dictation ("uh something in the" types "something in the").
pub fn strip_fillers(text: &str) -> String {
    text.split_whitespace()
        .filter(|t| {
            let w = t.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
            !matches!(w.as_str(), "uh" | "um" | "uhm" | "umm" | "uhh" | "er" | "erm" | "hmm" | "mm" | "ehm")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The raw words after the wake word (case and punctuation kept, numbers and aliases NOT normalised): what typing
/// and search use, so "type free shipping for two" types exactly that.
pub fn raw_after_wake(text: &str, wake_words: &[String]) -> String {
    let toks: Vec<&str> = text.split_whitespace().collect();
    let pats: Vec<Vec<String>> = wake_words.iter().map(|ww| words(ww)).filter(|p| !p.is_empty()).collect();
    for i in 0..toks.len().min(WAKE_MAX_LEAD + 1) {
        for p in &pats {
            if i + p.len() <= toks.len() && words(&toks[i..i + p.len()].join(" ")) == *p {
                return toks[i + p.len()..].join(" ").trim_start_matches([',', '.', ' ']).trim().to_string();
            }
        }
    }
    text.trim().to_string()
}

/// The raw text of a trailing free-text slot: what follows the phrase's last literal word in the raw text
/// ("can you type how to get 2 friends" with "type {text}" -> "how to get 2 friends").
pub fn raw_slot(raw: &str, phrase: &str) -> Option<String> {
    let parts: Vec<&str> = phrase.split_whitespace().collect();
    if !parts.last().is_some_and(|p| p.starts_with('{')) || parts.len() < 2 {
        return None;
    }
    let lit = words(parts[parts.len() - 2]);
    let lit = lit.last()?;
    let toks: Vec<&str> = raw.split_whitespace().collect();
    let pos = toks.iter().position(|t| words(t).first() == Some(lit))?;
    let rest = toks[pos + 1..].join(" ");
    let rest = rest.trim().trim_end_matches('.').trim().to_string();
    (!rest.is_empty()).then_some(rest)
}

/// A spoken confirmation: the whole utterance is a yes-word (no wake word needed).
pub fn is_yes(text: &str) -> bool {
    let w = words(text);
    let c: Vec<&str> = content(&w).iter().map(|&i| w[i].as_str()).collect();
    matches!(
        c.as_slice(),
        ["yes"] | ["yeah"] | ["yep"] | ["yup"] | ["sure"] | ["confirm"] | ["do", "it"] | ["yes", "do", "it"] | ["yes", "lock"]
            | ["yes", "lock", "it"] | ["si"] | ["claro"] | ["confirmo"] | ["yes", "please"] | ["pimax", "yes"] | ["max", "yes"]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::{Catalogue, BUILTIN};

    fn cat() -> Catalogue {
        Catalogue::from_texts(BUILTIN, None).unwrap()
    }

    #[test]
    fn take_backs_clauses_openers() {
        let w = |t: &str| words(t);
        assert_eq!(take_back(&w("mute the sound, no wait, forget about it"), "mute the sound, no wait, forget about it"), TakeBack::Cancel);
        assert_eq!(take_back(&w("open spotify actually never mind"), "open spotify actually never mind"), TakeBack::Cancel);
        assert_eq!(take_back(&w("five screens, no, six screens"), "five screens, no, six screens"), TakeBack::Correct(w("6 screens")));
        // "no" without a pause is a word of the request ("no room" = passthrough off)
        assert_eq!(take_back(&w("music off and no captions"), "music off and no captions"), TakeBack::None);
        assert_eq!(take_back(&w("cancel the reminder"), "cancel the reminder"), TakeBack::None);
        assert_eq!(clauses(&w("mute the sound and then open spotify")).len(), 2);
        assert_eq!(clauses(&w("hide screens 4, 5 and 6")).len(), 1);
        assert_eq!(clauses(&w("editor left and spotify right")).len(), 1);
        assert_eq!(drop_openers(&w("yeah so open the files")), w("open the files"));
        assert_eq!(drop_openers(&w("right screen bigger")), w("right screen bigger"));
        // "screen for" / "screens to and five" are numbers; "the screen to the left" is not
        assert_eq!(w("hide screen for"), w("hide screen 4"));
        assert_eq!(w("hide screens to and five"), w("hide screens 2 and 5"));
        assert!(w("move it to the screen to the left").contains(&"to".to_string()));
    }

    #[test]
    fn wake_late_cut_off_and_fillers() {
        let ww: Vec<String> = ["max", "pimax"].map(String::from).to_vec();
        assert_eq!(wake_mid("Windows, can we ha Max, can we have two more windows?", &ww).as_deref(), Some("Max, can we have two more windows?"));
        assert_eq!(wake_mid("I told Max to switch to cinema", &ww), None);
        assert_eq!(wake_mid("okay. max", &ww), None);
        for t in ["can you move uh 1 of the", "can you move all the", "can you move", "can you write uh", "max can you", "do the"] {
            assert!(dangling(&words(t)), "{t}");
        }
        for t in ["cinema", "zoom in", "passthrough on", "close it all", "focus this", "type hello", "turn it up", "go back", "volume up please"] {
            assert!(!dangling(&words(t)), "{t}");
        }
        assert_eq!(strip_fillers("uh something in the, um, line"), "something in the, line");
        for t in ["Max, cancel", "max cancel that", "cancel", "Max, stop.", "stop it", "Max abort"] {
            assert!(is_cancel(t, &ww), "{t}");
        }
        for t in ["Max, cancel mode", "stop recording", "Max cinema", "cancel the cinema and go"] {
            assert!(!is_cancel(t, &ww), "{t}");
        }
    }

    #[test]
    fn short_utterances_do_not_panic() {
        let ww: Vec<String> = vec!["max".into()];
        for t in ["", "I", "I'm", "Yeah.", "uh"] {
            assert_eq!(wake(t, &ww), Heard::Talk, "{t:?}");
        }
        assert_eq!(wake("Uh, Max.", &ww), Heard::WakeOnly);
    }

    /// The intent id (and slots) for a full utterance including the wake word.
    fn id(c: &Catalogue, text: &str) -> Option<(String, BTreeMap<String, String>)> {
        match wake(text, &c.wake.words) {
            Heard::Command(w) => best(c, &w).map(|m| (m.id, m.slots)),
            _ => None,
        }
    }

    fn is(c: &Catalogue, text: &str, want: &str) {
        assert_eq!(id(c, text).map(|x| x.0).as_deref(), Some(want), "{text:?}");
    }

    #[test]
    fn raw_text_for_typing() {
        let ww: Vec<String> = vec!["max".into()];
        assert_eq!(raw_after_wake("Max, in the browser can you type how to get friends in Bali?", &ww), "in the browser can you type how to get friends in Bali?");
        assert_eq!(raw_slot("can you write line two, please.", "write {text}").as_deref(), Some("line two, please"));
        assert_eq!(raw_slot("search for free shipping", "search for {q}").as_deref(), Some("free shipping"));
        assert_eq!(raw_slot("cinema", "cinema"), None);
    }

    #[test]
    fn words_compounds_and_numbers() {
        assert_eq!(words("Pass-through ON!"), ["passthrough", "on"]);
        assert_eq!(words("work three"), ["work", "3"]);
        assert_eq!(words("Full screen"), ["fullscreen"]);
        assert_eq!(words("¡Modo cine!"), ["modo", "cine"]);
        assert_eq!(words("desktop two"), ["workspace", "2"]);
        assert_eq!(words("desktop background"), ["desktop", "background"]);
    }

    #[test]
    fn max_wake_word_and_asr_forms() {
        let c = cat();
        let w = &c.wake.words;
        for t in ["Max.", "Hi, Max.", "B Max.", "Five Mars.", "Five marks.", "max's", "Macs", "Hey max", "ok pi max", "Time max"] {
            assert_eq!(wake(t, w), Heard::WakeOnly, "{t:?}");
        }
        assert_eq!(wake("Time Max War Three", w), Heard::Command(vec!["work".into(), "3".into()]));
        is(&c, "Time Max War Three", "vr.preset.work3");
        is(&c, "Max, cinema", "vr.preset.cinema");
        is(&c, "max sinema", "vr.preset.cinema");
        is(&c, "Max work tree", "vr.preset.work3");
        is(&c, "Max walk free", "vr.preset.work3");
        is(&c, "Max work won", "vr.preset.work1");
        is(&c, "Pimax cinemas", "vr.preset.cinema");
    }

    #[test]
    fn max_in_normal_talk_is_ignored() {
        let c = cat();
        let w = &c.wake.words;
        for t in [
            "set it to max", "turn the volume to max please", "the volume is at max", "I like max", "what is the max speed",
            "he said max", "give it the max", "it has a max of ten", "switch to cinema", "five screens max", 
            "this is the marks", "one two max", "hey you max", "time to go max",
        ] {
            assert_eq!(wake(t, w), Heard::Talk, "{t:?}");
        }
    }

    #[test]
    fn mars_sentence_matches_no_intent() {
        let c = cat();
        assert!(id(&c, "Mars is red").is_none());
        assert!(id(&c, "Marks the spot").is_none());
    }

    #[test]
    fn wake_word_is_required_at_the_start() {
        let c = cat();
        let w = &c.wake.words;
        assert_eq!(wake("switch to cinema", w), Heard::Talk);
        assert_eq!(wake("I told pimax to switch to cinema", w), Heard::Talk);
        assert_eq!(wake("Pimax.", w), Heard::WakeOnly);
        assert_eq!(wake("Hey, Pi Max!", w), Heard::WakeOnly);
        assert_eq!(wake("pimax please", w), Heard::WakeOnly);
        assert_eq!(wake("Pymax, cinema mode", w), Heard::Command(vec!["cinema".into(), "mode".into()]));
        assert_eq!(wake("Pie max cinema", w), Heard::Command(vec!["cinema".into()]));
    }

    #[test]
    fn presets_many_phrasings() {
        let c = cat();
        for t in [
            "Pimax, cinema mode.",
            "Pimax cinema",
            "pimax switch to cinema",
            "Pimax, could you please switch to cinema mode?",
            "pimax go to the cinema mode",
            "Pimax movie time",
            "Pimax, modo cine",
        ] {
            is(&c, t, "vr.preset.cinema");
        }
        for t in [
            "Pimax, huge cinema",
            "Max, imax",
            "Max imax mode",
            "Max, big cinema",
            "Max, cinema mode huge",
            "Max, make it a huge screen",
            "pimax huge cinema mode",
            "Max, theater mode",
        ] {
            is(&c, t, "vr.preset.cinema_huge");
        }
        for t in ["Max, exit cinema", "Max, exit imax", "Max, normal screens", "Max, leave huge cinema"] {
            is(&c, t, "vr.preset.exit_cinema");
        }
        is(&c, "Max, cinema mode", "vr.preset.cinema");
        for t in ["Pimax work mode", "pimax work", "Pimax, work three", "pimax work 3", "Pimax three screens", "pimax switch to work mode", "Pimax tres pantallas"] {
            is(&c, t, "vr.preset.work3");
        }
        for t in ["Pimax work one", "pimax one screen", "Pimax single screen", "pimax work 1"] {
            is(&c, t, "vr.preset.work1");
        }
        is(&c, "Pimax assistant mode", "vr.preset.assistant");
    }

    #[test]
    fn passthrough_and_backgrounds() {
        let c = cat();
        is(&c, "Pimax pass through on", "vr.passthrough.on");
        is(&c, "pimax, show me the room", "vr.passthrough.on");
        is(&c, "Pimax turn off passthrough", "vr.passthrough.off");
        is(&c, "pimax black background", "vr.passthrough.off");
        is(&c, "Pimax dim the room", "vr.passthrough.dim");
        is(&c, "Pimax brighter", "vr.passthrough.brighter");
        is(&c, "Pimax make the room darker", "vr.passthrough.darker");
        is(&c, "pimax next background", "vr.backdrop.next");
        is(&c, "Pimax change the background", "vr.backdrop.next");
        is(&c, "Pimax previous background", "vr.backdrop.previous");
        let (i, s) = id(&c, "Pimax set background to sky").unwrap();
        assert_eq!((i.as_str(), s["name"].as_str()), ("vr.backdrop.set", "sky"));
        let (i, s) = id(&c, "Pimax mountain lake background").unwrap();
        assert_eq!((i.as_str(), s["name"].as_str()), ("vr.backdrop.set", "mountain lake"));
        for (said, name) in [("Pimax set background flowers", "flowers"), ("Pimax set the background to hawaii", "hawaii"), ("Pimax set background universe", "universe"), ("Pimax set background ocean", "ocean")] {
            let (i, s) = id(&c, said).unwrap();
            assert_eq!((i.as_str(), s["name"].as_str()), ("vr.backdrop.set", name), "{said}");
        }
        is(&c, "Pimax background off", "vr.passthrough.on");
        is(&c, "Pimax turn the background off", "vr.passthrough.on");
        is(&c, "Pimax next background", "vr.backdrop.next");
        // the desktop wallpaper is another thing
        is(&c, "Pimax next wallpaper", "omarchy.wallpaper.next");
        is(&c, "Pimax desktop background", "omarchy.wallpaper.next");
    }

    #[test]
    fn vr_and_voice_controls() {
        let c = cat();
        is(&c, "Pimax reset the screens", "vr.screens.reset");
        is(&c, "Pimax, where are my screens?", "vr.screens.reset");
        is(&c, "Pimax recenter", "vr.recenter");
        is(&c, "Pimax re-center the view", "vr.recenter");
        is(&c, "Pimax show fps", "vr.fps.show");
        is(&c, "Pimax hide the frame rate", "vr.fps.hide");
        is(&c, "Pimax captions off", "voice.captions.off");
        is(&c, "Pimax show subtitles", "voice.captions.on");
        is(&c, "Pimax, what can I say?", "voice.help");
        is(&c, "Pimax undo", "voice.undo");
        is(&c, "Pimax go back", "voice.undo");
        is(&c, "Pimax never mind", "voice.cancel");
    }

    #[test]
    fn omarchy_rows() {
        let c = cat();
        is(&c, "Pimax make it fullscreen", "omarchy.window.fullscreen");
        is(&c, "Pimax full screen", "omarchy.window.fullscreen");
        is(&c, "Pimax make it full width", "omarchy.window.maximize");
        is(&c, "Pimax float this", "omarchy.window.float");
        is(&c, "Pimax make it wider", "omarchy.window.wider");
        is(&c, "Pimax make it a lot wider", "omarchy.window.much_wider");
        is(&c, "Pimax focus the app on the left", "omarchy.focus.left");
        is(&c, "Pimax cycle apps", "omarchy.window.next");
        is(&c, "Pimax next window", "win.ws.next");
        is(&c, "Pimax flip the split", "omarchy.layout.flip_split");
        is(&c, "Pimax open a terminal", "omarchy.launch.terminal");
        is(&c, "Pimax open the browser", "omarchy.launch.browser");
        is(&c, "Pimax open spotify", "omarchy.launch_or_focus.spotify");
        is(&c, "Pimax volume up", "omarchy.volume.up");
        is(&c, "Pimax, turn it down", "omarchy.volume.down");
        is(&c, "Pimax mute", "omarchy.volume.mute_toggle");
        is(&c, "Pimax next song", "omarchy.media.next");
        is(&c, "Pimax pause", "omarchy.media.play_pause");
        is(&c, "Pimax battery level?", "omarchy.info.battery");
        is(&c, "Pimax take a screenshot", "omarchy.screenshot");
        is(&c, "Pimax dismiss all notifications", "omarchy.notifications.dismiss");
        is(&c, "Pimax lock the screen", "omarchy.lock");
        is(&c, "Pimax show the scratchpad", "omarchy.scratchpad.toggle");
        let (i, s) = id(&c, "Pimax focus Slack").unwrap();
        assert_eq!((i.as_str(), s["app"].as_str()), ("omarchy.focus.app", "slack"));
        let (i, s) = id(&c, "Pimax send this to workspace two").unwrap();
        assert_eq!((i.as_str(), s["n"].as_str()), ("omarchy.window.to_workspace", "2"));
        let (i, s) = id(&c, "Pimax, remind me in ten minutes to check the oven.").unwrap();
        assert_eq!(i, "omarchy.reminder");
        assert_eq!((s["minutes"].as_str(), s["text"].as_str()), ("10", "check the oven"));
    }

    #[test]
    fn not_approved_and_nonsense_do_not_match() {
        let c = cat();
        for t in [
            "Pimax shut down",
            "Pimax reboot",
            "Pimax close all windows",
            "Pimax reload the desktop",
            "Pimax update omarchy",
            "Pimax mute the mic",
            "Pimax open the omarchy menu",
            "Pimax switch theme to nord",
            "Pimax cinema mode and then delete everything",
            "Pimax remind me in 5000 minutes to x",
            "Pimax",
        ] {
            assert_eq!(id(&c, t), None, "{t:?}");
        }
        // talk without the wake word never matches, even an exact phrase
        assert_eq!(id(&c, "cinema mode"), None);
    }

    #[test]
    fn yes_words() {
        assert!(is_yes("Yes."));
        assert!(is_yes("yes, do it"));
        assert!(is_yes("Sí"));
        assert!(!is_yes("yes and no"));
        assert!(!is_yes("no"));
        assert!(!is_yes("lock the screen"));
    }
}
