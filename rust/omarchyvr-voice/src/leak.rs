//! Leak check (user decision 2026-10-16): no test answers in what the model or the matcher sees. A phrase of a
//! test case (tests/state/goals-*.toml, every split; tests/fixtures/*.tsv) may not appear, verbatim or nearly, in:
//! * the prompts (nlu::system_prompt with its examples, the planner's prompt, the router's prompt, the tools
//!   prompt), every quoted piece of them;
//! * the catalogue's literal phrases (voice-intents.toml) of 5+ content words, against the state cases and the live
//!   fixtures (a tool's own plain name or a template with a slot is its grammar);
//! * the matcher's compounds (command::COMPOUNDS) and the string literals of the rules' code (rules.rs).
//!
//! Single-word ASR aliases (tree=three, for=four) are word mappings, not phrases, and are not checked.
//! A prompt piece counts from 3 content words (a 2-word vocabulary definition like "go back" is no example).
//! Near = the content words (fillers out, number words as digits) are equal, or their longest common subsequence
//! covers [`NEAR`] of both together.

/// Similarity at or above which two phrases count as the same.
pub const NEAR: f64 = 0.8;

/// The content words of a phrase (fillers and the wake word out, numbers as digits).
pub fn content_words(s: &str) -> Vec<String> {
    crate::command::words(s).into_iter().filter(|w| !crate::command::FILLERS.contains(&w.as_str()) && w != "max").collect()
}

/// 2 * LCS / (|a| + |b|) over words; a `{slot}` word matches nothing.
pub fn similarity(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            dp[i][j] = if a[i - 1] == b[j - 1] && !a[i - 1].starts_with('{') { dp[i - 1][j - 1] + 1 } else { dp[i - 1][j].max(dp[i][j - 1]) };
        }
    }
    2.0 * dp[a.len()][b.len()] as f64 / (a.len() + b.len()) as f64
}

/// The quoted pieces of a prompt ("..." inside it).
pub fn quoted(text: &str) -> Vec<String> {
    text.split('"').skip(1).step_by(2).filter(|q| q.contains(' ')).map(str::to_string).collect()
}

/// The string literals with a space in Rust code, comments and the test module left out.
pub fn code_literals(src: &str) -> Vec<String> {
    let code = src.split("#[cfg(test)]").next().unwrap_or(src);
    let mut out = Vec::new();
    for line in code.lines() {
        let l = line.trim_start();
        if l.starts_with("//") {
            continue;
        }
        let l = match l.find(" // ") {
            Some(i) => &l[..i],
            None => l,
        };
        out.extend(quoted(l));
    }
    out
}

/// Every test utterance: (words, where from).
pub fn test_utterances() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = crate::statetest::load_cases().unwrap_or_default().into_iter().map(|c| (c.say, format!("state {}", c.id))).collect();
    for (name, text) in FIXTURES {
        for l in text.lines().filter(|l| !l.starts_with('#')) {
            if let Some(p) = l.split('\t').nth(1) {
                v.push((p.trim().to_string(), name.to_string()));
            }
        }
    }
    v
}

const FIXTURES: &[(&str, &str)] = &[
    ("audit.tsv", include_str!("../tests/fixtures/audit.tsv")),
    ("aibench.tsv", include_str!("../tests/fixtures/aibench.tsv")),
    ("live-2026-10-16.tsv", include_str!("../tests/fixtures/live-2026-10-16.tsv")),
    ("live-2026-10-16-b.tsv", include_str!("../tests/fixtures/live-2026-10-16-b.tsv")),
    ("agent-2026-10-16.tsv", include_str!("../tests/fixtures/agent-2026-10-16.tsv")),
    ("agent27b-2026-10-16.tsv", include_str!("../tests/fixtures/agent27b-2026-10-16.tsv")),
];

/// The material the model and the matcher see: (phrase, where, fewest content words that count).
pub fn material() -> Vec<(String, String, usize)> {
    let cat = crate::catalogue::Catalogue::load_default().or_else(|_| crate::catalogue::Catalogue::from_texts(crate::catalogue::BUILTIN, None)).expect("catalogue");
    let mut m: Vec<(String, String, usize)> = Vec::new();
    for q in quoted(&crate::nlu::system_prompt(&cat)) {
        m.push((q, "nlu::system_prompt".into(), 3));
    }
    let ids: Vec<String> = cat.intents.iter().map(|i| i.id.clone()).collect();
    for q in quoted(&crate::planner::system_prompt(&cat, &crate::remote::Category::ALL, &ids, true)) {
        m.push((q, "planner::system_prompt".into(), 3));
    }
    for q in quoted(&crate::remote::classify_system()) {
        m.push((q, "remote::classify_system".into(), 3));
    }
    for q in quoted(&crate::remote::act_system()) {
        m.push((q, "remote::act_system".into(), 3));
    }
    for q in quoted(&crate::agent::system_prompt()) {
        m.push((q, "agent::system_prompt".into(), 3));
    }
    // a template with a slot is the tool's grammar; a literal phrase of 5+ content words must not copy a test
    for i in &cat.intents {
        for p in i.phrases.iter().filter(|p| !p.contains('{')) {
            m.push((p.clone(), format!("catalogue {}", i.id), 5));
        }
    }
    for (pat, _) in crate::command::COMPOUNDS {
        m.push((pat.join(" "), "command::COMPOUNDS".into(), 2));
    }
    for q in code_literals(include_str!("rules.rs")) {
        m.push((q, "rules.rs".into(), 3));
    }
    m
}

/// Every leak: (phrase, where, the test phrase, its source, similarity).
pub fn leaks() -> Vec<(String, String, String, String, f64)> {
    let tests: Vec<(Vec<String>, String, String)> = test_utterances().into_iter().map(|(s, w)| (content_words(&s), s, w)).filter(|t| t.0.len() >= 2).collect();
    let mut out = Vec::new();
    for (p, src, min) in material() {
        let w = content_words(&p);
        if w.len() < min {
            continue;
        }
        // the catalogue is the matcher's grammar: the old intent smoke test (audit.tsv, aibench.tsv) is written from
        // it by design (docs/VOICE-STATE-TESTS.md section 5); the state cases and the live fixtures may not be in it
        let cat_src = src.starts_with("catalogue");
        if let Some((t, s)) = tests.iter().filter(|t| !(cat_src && (t.2 == "audit.tsv" || t.2 == "aibench.tsv"))).map(|t| (t, similarity(&w, &t.0))).filter(|(_, s)| *s >= NEAR).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()) {
            out.push((p.clone(), src.clone(), t.1.clone(), t.2.clone(), s));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_is_word_based() {
        let a = content_words("move chrome to screen 3");
        assert_eq!(similarity(&a, &content_words("can you move chrome to screen three")), 1.0);
        assert!(similarity(&a, &content_words("put the editor on the left")) < NEAR);
        assert!(similarity(&content_words("put {app} on screen 2"), &content_words("put chrome on screen 2")) < 1.0);
    }

    #[test]
    fn no_test_phrase_in_what_the_model_or_the_matcher_sees() {
        let l = leaks();
        let lines: Vec<String> = l.iter().map(|(p, src, t, ts, s)| format!("{s:.2} {src}: {p:?} ~ {t:?} ({ts})")).collect();
        assert!(l.is_empty(), "{} leaks:\n{}", l.len(), lines.join("\n"));
    }
}
