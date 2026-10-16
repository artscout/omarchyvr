//! Fuzzy, sound-aware matching of a misheard command against the catalogue: the evidence the voice AI gets
//! ("hints") and the fast path for near-exact hits.
//!
//! Parakeet is good but not perfect: "liz windows" / "leaks list windows" = list windows, "Time Max War Three" =
//! Max work three. The exact matcher ([`crate::command::best`]) misses those; this scorer aligns the heard words
//! with every catalogue phrase (word edit distance where a substitution costs `1 - similarity`), with a word
//! similarity that combines spelling, a small phonetic key (Metaphone-like, see [`phonetic`]) and a table of known
//! ASR confusions ([`CONFUSIONS`]). Slots absorb words (numbers want a number). The score is 0..1 (1 = exact).

use crate::catalogue::{Catalogue, SlotType};
use crate::command::{self, Tok};
use std::collections::BTreeMap;

/// Known ASR confusions (heard, meant), both directions count. Also listed in the model's prompt.
pub const CONFUSIONS: &[(&str, &str)] = &[
    ("war", "work"), ("walk", "work"), ("wok", "work"), ("worth", "work"), ("were", "work"),
    ("liz", "list"), ("lis", "list"), ("leaks", "list"), ("least", "list"), ("lists", "list"), ("missed", "list"),
    ("mars", "max"), ("marks", "max"), ("macs", "max"), ("mac", "max"), ("mags", "max"),
    ("tree", "3"), ("free", "3"), ("three", "3"), ("won", "1"), ("to", "2"), ("too", "2"), ("for", "4"),
    ("tap", "tab"), ("tub", "tab"), ("taps", "tab"),
    ("screams", "screens"), ("screen", "screens"), ("cream", "screen"),
    ("windows", "window"), ("widows", "windows"), ("winters", "windows"),
    ("sinema", "cinema"), ("cinemas", "cinema"),
    ("pass", "passthrough"), ("through", "passthrough"),
    ("bug", "debug"), ("de", "debug"),
    ("necks", "next"), ("nex", "next"), ("look", "lock"), ("coffee", "copy"), ("inter", "enter"), ("paws", "pause"),
    ("mood", "mute"), ("cancelled", "cancel"), ("tight", "type"), ("battle", "battery"), ("focused", "focus"), ("scream", "screen"), ("scrawl", "scroll"), ("church", "search"), ("single", "signal"), ("health", "help"),
];

/// Words the ASR makes of "Max" (and the wake word itself): only these after the wake word = noise.
const WAKEISH: &[&str] = &["max", "ma", "mars", "marks", "macs", "mac", "maks", "mags", "pimax", "mm", "hmm", "mhm", "matt", "mat"];

/// Chatter that is not a command ("can you hear me", "testing 1 2 3", "hello"): ignored without asking a model.
const CHATTER: &[&str] = &[
    "hear", "hearing", "testing", "test", "hello", "hi", "there", "listening", "working", "check", "mic", "microphone", "1", "2", "3",
    "are", "is", "it", "this", "on", "do", "ok", "okay", "yes", "no", "alright", "right", "well", "oh",
];

/// One fuzzy candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct Cand {
    pub id: String,
    /// The best phrase of the intent (as written in the catalogue).
    pub phrase: String,
    /// 0..1, 1 = exact.
    pub score: f64,
    /// Slot values taken from the heard words (when the alignment filled them all with valid values).
    pub slots: BTreeMap<String, String>,
    /// All slots of the intent are filled and valid.
    pub complete: bool,
    /// The intent has a free-text slot (never a fast path: the slot can swallow anything).
    pub free_text: bool,
}

/// A Metaphone-like key: consonant skeleton with sound-alike letters merged (ph=f, ck=k, c before e/i/y = s,
/// th=t, z=s, v=f, d=t, b=p, g=k, q=k, x=ks), vowels dropped after the first letter, repeats collapsed.
/// Digits are kept as they are.
pub fn phonetic(w: &str) -> String {
    let w: Vec<char> = w.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if w.iter().all(|c| c.is_ascii_digit()) {
        return w.into_iter().collect();
    }
    let mut out = String::new();
    let mut i = 0;
    let at = |k: usize| w.get(k).copied().unwrap_or(' ');
    while i < w.len() {
        let c = w[i];
        let n = at(i + 1);
        let (code, step): (Option<char>, usize) = match (c, n) {
            ('p', 'h') => (Some('f'), 2),
            ('c', 'k') => (Some('k'), 2),
            ('s', 'h') | ('c', 'h') => (Some('x'), 2),
            ('t', 'h') => (Some('t'), 2),
            ('w', 'h') => (Some('w'), 2),
            ('k', 'n') | ('g', 'n') if i == 0 => (Some('n'), 2),
            ('w', 'r') if i == 0 => (Some('r'), 2),
            ('g', 'h') => (None, 2),
            ('c', 'e' | 'i' | 'y') => (Some('s'), 1),
            ('c', _) | ('q', _) | ('g', _) => (Some('k'), 1),
            ('x', _) => {
                if out.is_empty() {
                    (Some('s'), 1)
                } else {
                    out.push('k');
                    (Some('s'), 1)
                }
            }
            ('z', _) => (Some('s'), 1),
            ('v', _) => (Some('f'), 1),
            ('d', _) => (Some('t'), 1),
            ('b', _) => (Some('p'), 1),
            ('j', _) => (Some('x'), 1),
            ('h', _) if i > 0 => (None, 1),
            (v, _) if "aeiouy".contains(v) => (if i == 0 { Some('a') } else { None }, 1),
            (other, _) => (Some(other), 1),
        };
        if let Some(k) = code {
            if !out.ends_with(k) {
                out.push(k);
            }
        }
        i += step;
    }
    out
}

/// Levenshtein distance over chars.
pub fn lev(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn confused(a: &str, b: &str) -> bool {
    CONFUSIONS.iter().any(|(x, y)| (*x == a && *y == b) || (*x == b && *y == a))
}

/// Similarity of a heard word and a phrase word, 0..1 (below 0.5 counts as 0).
pub fn word_sim(heard: &str, want: &str) -> f64 {
    if heard == want {
        return 1.0;
    }
    if confused(heard, want) {
        return 0.9;
    }
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if digits(heard) || digits(want) {
        return 0.0;
    }
    let n = heard.chars().count().max(want.chars().count()) as f64;
    let spell = 1.0 - lev(heard, want) as f64 / n;
    let (ka, kb) = (phonetic(heard), phonetic(want));
    let sound = if ka.is_empty() || kb.is_empty() {
        0.0
    } else if ka == kb {
        0.85
    } else {
        0.8 * (1.0 - lev(&ka, &kb) as f64 / ka.len().max(kb.len()) as f64)
    };
    let s = spell.max(sound);
    if s < 0.5 {
        0.0
    } else {
        s
    }
}

/// Cost of an extra heard word (not in the phrase); a stutter of a neighbour ("leaks list") is cheap.
const INSERT: f64 = 0.6;
const INSERT_STUTTER: f64 = 0.15;
/// Cost of a phrase word that was not heard.
const DELETE: f64 = 1.0;

/// Align the heard content words with a compiled phrase: (cost, slot values, all slots valid).
fn align(toks: &[Tok], w: &[String], slots: &BTreeMap<String, SlotType>) -> (f64, BTreeMap<String, String>, bool) {
    let (n, m) = (toks.len(), w.len());
    const INF: f64 = 1e9;
    // dp[i][j]: tokens[..i] against words[..j]; back[i][j] = (pi, pj) of the previous cell
    let mut dp = vec![vec![INF; m + 1]; n + 1];
    let mut back = vec![vec![(usize::MAX, usize::MAX); m + 1]; n + 1];
    dp[0][0] = 0.0;
    let ins_cost = |j: usize| -> f64 {
        let x = &w[j];
        let near = |k: Option<usize>| k.and_then(|k| w.get(k)).is_some_and(|y| y != x && word_sim(x, y) >= 0.8);
        if near(j.checked_sub(1)) || near(Some(j + 1)) {
            INSERT_STUTTER
        } else {
            INSERT
        }
    };
    for i in 0..=n {
        for j in 0..=m {
            let base = dp[i][j];
            if base >= INF {
                continue;
            }
            let relax = |ni: usize, nj: usize, c: f64, dp: &mut Vec<Vec<f64>>, back: &mut Vec<Vec<(usize, usize)>>| {
                if base + c < dp[ni][nj] - 1e-12 {
                    dp[ni][nj] = base + c;
                    back[ni][nj] = (i, j);
                }
            };
            if j < m {
                relax(i, j + 1, ins_cost(j), &mut dp, &mut back);
            }
            if i < n {
                relax(i + 1, j, DELETE, &mut dp, &mut back);
                match &toks[i] {
                    Tok::Lit(l) if j < m => {
                        let s = word_sim(&w[j], l);
                        relax(i + 1, j + 1, 1.0 - s + if s == 0.0 { 0.2 } else { 0.0 }, &mut dp, &mut back);
                    }
                    Tok::Lit(_) => {}
                    Tok::Slot(name) => {
                        let ty = slots.get(name).copied().unwrap_or(SlotType::Text);
                        let max = match ty {
                            SlotType::Number | SlotType::Word | SlotType::Side => 1,
                            SlotType::Text | SlotType::Name => m - j,
                        };
                        for k in 1..=max.min(m - j) {
                            let val = w[j..j + k].join(" ");
                            let c = match ty {
                                SlotType::Number | SlotType::Side => {
                                    if command::slot_ok(ty, &val) {
                                        0.0
                                    } else {
                                        0.8
                                    }
                                }
                                SlotType::Word => 0.1,
                                // free text: the first word is cheap, more words are weak evidence
                                SlotType::Text | SlotType::Name => 0.1 + 0.35 * (k - 1) as f64,
                            };
                            relax(i + 1, j + k, c, &mut dp, &mut back);
                        }
                    }
                }
            }
        }
    }
    // backtrack for the slot values
    let mut vals = BTreeMap::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        let (pi, pj) = back[i][j];
        if pi == usize::MAX {
            break;
        }
        if pi + 1 == i && pj < j {
            if let Tok::Slot(name) = &toks[pi] {
                let v = w[pj..j].join(" ");
                // a side is stored in its one word: top / above -> up
                let v = if slots.get(name) == Some(&SlotType::Side) { crate::windows::slot_word(&v).map(str::to_string).unwrap_or(v) } else { v };
                vals.insert(name.clone(), v);
            }
        }
        i = pi;
        j = pj;
    }
    let ok = slots.iter().all(|(k, t)| vals.get(k).is_some_and(|v| command::slot_ok(*t, v)));
    (dp[n][m], vals, ok)
}

/// Score one phrase against the heard command words (without the wake word).
pub fn score_phrase(phrase: &str, words: &[String], slots: &BTreeMap<String, SlotType>) -> (f64, BTreeMap<String, String>, bool) {
    let toks = command::compile(phrase);
    let cw: Vec<String> = command::content(words).into_iter().map(|i| words[i].clone()).collect();
    if toks.is_empty() || cw.is_empty() {
        return (0.0, BTreeMap::new(), false);
    }
    let (cost, vals, ok) = align(&toks, &cw, slots);
    let norm = toks.len().max(cw.len()) as f64;
    ((1.0 - cost / norm).clamp(0.0, 1.0), vals, ok)
}

/// The best `k` enabled intents for the heard command words (best first; each intent once).
pub fn candidates(cat: &Catalogue, words: &[String], k: usize) -> Vec<Cand> {
    let mut v: Vec<Cand> = Vec::new();
    for i in cat.enabled() {
        let mut best: Option<Cand> = None;
        for p in &i.phrases {
            let (score, slots, complete) = score_phrase(p, words, &i.slots);
            if best.as_ref().is_none_or(|b| score > b.score + 1e-9) {
                best = Some(Cand { id: i.id.clone(), phrase: p.clone(), score, slots, complete, free_text: i.slots.values().any(|t| matches!(t, SlotType::Text | SlotType::Name)) });
            }
        }
        if let Some(b) = best.filter(|b| b.score > 0.0) {
            v.push(b);
        }
    }
    v.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(k);
    v
}

/// A clear fuzzy winner, safe to run without asking a model: score >= 0.9, at least 0.1 ahead of the next one,
/// slots complete, no free-text slot.
pub fn confident(c: &[Cand]) -> Option<&Cand> {
    let top = c.first()?;
    let second = c.get(1).map(|x| x.score).unwrap_or(0.0);
    ((top.score >= 0.9 && top.score - second >= 0.1 || top.score >= 0.85 && top.score - second >= 0.2) && top.complete && !top.free_text).then_some(top)
}

/// Noise after the wake word: nothing but "max"-like sounds and fillers ("max ma max max"), or chatter such as
/// "can you hear me" / "testing". Never sent to a model; no toast.
pub fn is_noise(words: &[String]) -> bool {
    let cw: Vec<&str> = command::content(words).into_iter().map(|i| words[i].as_str()).filter(|w| !WAKEISH.contains(w)).collect();
    cw.is_empty() || cw.iter().all(|w| CHATTER.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::BUILTIN;
    use crate::command::words;

    fn cat() -> Catalogue {
        Catalogue::from_texts(BUILTIN, None).unwrap()
    }

    #[test]
    fn phonetic_keys() {
        assert_eq!(phonetic("list"), "lst");
        assert_eq!(phonetic("least"), "lst");
        assert_eq!(phonetic("three"), phonetic("tree"));
        assert_eq!(phonetic("cinema"), phonetic("sinema"));
        assert_eq!(phonetic("phone"), "fn");
        assert_eq!(phonetic("max"), "mks");
        assert_eq!(phonetic("42"), "42");
        assert_eq!(lev("kitten", "sitting"), 3);
    }

    #[test]
    fn word_similarity() {
        assert_eq!(word_sim("list", "list"), 1.0);
        assert!(word_sim("liz", "list") >= 0.9);
        assert!(word_sim("windws", "windows") > 0.8);
        assert_eq!(word_sim("cinema", "volume"), 0.0);
        assert_eq!(word_sim("3", "4"), 0.0);
    }

    fn top(text: &str) -> Cand {
        candidates(&cat(), &words(text), 3).remove(0)
    }

    #[test]
    fn real_mishearings_rank_the_meant_intent_first() {
        for (heard, want) in [
            ("liz windows", "win.list"),
            ("leaks list windows", "win.list"),
            ("war three", "vr.preset.work3"),
            ("cinema mod", "vr.preset.cinema"),
            ("next tap", "win.tab.next"),
            ("volume app", "omarchy.volume.up"),
            ("take a screen shot please", "omarchy.screenshot"),
        ] {
            let c = top(heard);
            assert_eq!(c.id, want, "{heard}: {c:?}");
        }
        // close enough to run without a model
        let c = candidates(&cat(), &words("liz windows"), 3);
        assert_eq!(confident(&c).map(|c| c.id.as_str()), Some("win.list"), "{c:?}");
        let c = candidates(&cat(), &words("leaks list windows"), 3);
        assert_eq!(confident(&c).map(|c| c.id.as_str()), Some("win.list"), "{c:?}");
        // not close: no fast path
        for t in ["open 2 more screens", "how fast is it running", "that thing with the issue tracker"] {
            let c = candidates(&cat(), &words(t), 3);
            assert!(confident(&c).is_none(), "{t}: {c:?}");
        }
        // a free-text slot swallows anything: weak evidence
        let c = candidates(&cat(), &words("that thing with the issue tracker"), 3);
        assert!(c[0].score < 0.7, "{c:?}");
    }

    #[test]
    fn slots_are_filled_from_the_words() {
        let c = top("go to work space for");
        assert_eq!((c.id.as_str(), c.slots.get("n").map(String::as_str), c.complete), ("win.ws.goto", Some("4"), true));
        let c = top("remind me in 10 minute to stretch my legs");
        assert!(c.score > 0.8, "{c:?}");
        assert_eq!(c.id, "omarchy.reminder");
        assert_eq!(c.slots.get("minutes").map(String::as_str), Some("10"));
        assert!(c.slots["text"].contains("stretch"));
    }

    #[test]
    fn noise_and_chatter() {
        for t in ["ma max max", "can you hear me", "testing 1 2 3", "hello", "uh"] {
            assert!(is_noise(&words(t)), "{t}");
        }
        for t in ["list windows", "cinema", "open 2 more screens"] {
            assert!(!is_noise(&words(t)), "{t}");
        }
    }
}
