//! `omarchyvr-voice audit`: every intent of the catalogue through the full route (exact, fuzzy, model), with its first
//! catalogue phrase, the same phrase as the speech-to-text might mangle it, and natural phrasings from
//! `tests/fixtures/audit.tsv`. Pure parts here (cases, mangling, scoring, the HTML page); the run is in main.rs.

use crate::catalogue::{Catalogue, SlotType};
use std::collections::BTreeMap;

/// The built-in natural phrasings.
pub const FIXTURE: &str = include_str!("../tests/fixtures/audit.tsv");

/// One phrase to test.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    /// The intent it should run ("none" = nothing).
    pub want: String,
    /// What was "heard" after "Max".
    pub heard: String,
    /// exact | mangled | natural
    pub kind: &'static str,
    /// Expected slot values (contained, case-insensitive).
    pub slots: Vec<(String, String)>,
}

/// A sample value for a slot (to fill a catalogue phrase).
fn sample(t: SlotType, name: &str) -> &'static str {
    match (t, name) {
        (SlotType::Number, "minutes") => "10",
        (SlotType::Number, _) => "3",
        (SlotType::Name, _) => "sky",
        (SlotType::Side, _) => "left",
        (SlotType::Screen, "from") => "2",
        (SlotType::Screen, "b") => "4",
        (SlotType::Screen, _) => "3",
        (_, "screens") => "3",
        (_, "size") => "50",
        (_, "app") => "terminal",
        (_, "site") => "github",
        (_, "key") => "tab",
        (_, "q") => "youtube",
        (_, "text") => "hello there",
        _ => "hello",
    }
}

/// Spoken -> heard, the way Parakeet errs: real words that sound alike ("next" -> "necks", "list" -> "liz",
/// "work" -> "war"), else a plural/singular slip of the longest word.
pub fn mangle(phrase: &str) -> String {
    const HOMO: &[(&str, &str)] = &[
        ("work", "war"), ("list", "liz"), ("three", "tree"), ("tab", "tap"), ("screens", "screams"), ("screen", "scream"),
        ("windows", "winters"), ("window", "winter"), ("cinema", "sinema"), ("one", "won"), ("four", "for"), ("next", "necks"),
        ("focus", "focused"), ("copy", "coffee"), ("enter", "inter"), ("lock", "look"), ("mute", "mood"), ("pause", "paws"),
        ("darker", "dark"), ("wider", "wide"), ("brighter", "bright"), ("recenter", "re center"), ("undo", "and do"),
        ("dictate", "dictator"), ("background", "back ground"), ("passthrough", "pass through"), ("workspace", "work space"),
        ("captions", "caption"), ("help", "health"), ("cancel", "cancelled"), ("escape", "a scape"), ("paste", "based"),
        ("reload", "reloaded"), ("scroll", "scrawl"), ("stats", "stat"), ("battery", "battle"), ("network", "net work"),
        ("screenshot", "screen shot"), ("terminal", "terminals"), ("browser", "browsers"), ("float", "flow"), ("pop", "pup"),
        ("fullscreen", "full scream"), ("previous", "previews"), ("select", "selected"), ("search", "church"), ("type", "tight"),
        ("wallpaper", "wall paper"), ("volume", "volumes"), ("spotify", "spot if i"), ("obsidian", "obsidians"), ("signal", "single"),
        ("assistant", "assistants"), ("dim", "them"), ("black", "block"), ("fps", "f p s"), ("hide", "high"), ("show", "so"),
    ];
    let words: Vec<&str> = phrase.split_whitespace().collect();
    let mut out: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    for w in out.iter_mut() {
        if let Some((_, h)) = HOMO.iter().find(|(a, _)| a == w) {
            *w = h.to_string();
            return out.join(" ");
        }
    }
    if let Some((i, w)) = words.iter().enumerate().filter(|(_, w)| w.len() >= 4 && w.chars().all(|c| c.is_ascii_alphabetic())).max_by_key(|(_, w)| w.len()) {
        out[i] = match w.strip_suffix('s') {
            Some(x) => x.to_string(),
            None => format!("{w}s"),
        };
    }
    out.join(" ")
}

/// Every case: per enabled intent its first phrase (exact) and a mangled one, then the fixture's phrasings.
pub fn cases(cat: &Catalogue, fixture: &str) -> Vec<Case> {
    let mut v = Vec::new();
    for i in cat.enabled() {
        let Some(p) = i.phrases.first() else { continue };
        let mut slots = Vec::new();
        let mut filled = p.clone();
        for (k, t) in i.slots.iter().filter(|(k, _)| p.contains(&format!("{{{k}}}"))) {
            // "3 screens" is also the work-3 preset's own phrase: the screen count is tested with 4
            let s = if i.id.starts_with("vr.screens.") && *t == SlotType::Number { "4" } else { sample(*t, k) };
            filled = filled.replace(&format!("{{{k}}}"), s);
            slots.push((k.clone(), s.to_string()));
        }
        v.push(Case { want: i.id.clone(), heard: filled.clone(), kind: "exact", slots: slots.clone() });
        let m = mangle(&filled);
        if m != filled {
            // a mangled slot value cannot be expected verbatim
            v.push(Case { want: i.id.clone(), heard: m, kind: "mangled", slots: slots.iter().filter(|(_, s)| s.parse::<u32>().is_ok()).cloned().collect() });
        }
    }
    for l in fixture.lines() {
        let l = l.split("\t#").next().unwrap_or("").trim_end();
        if l.trim().is_empty() || l.trim_start().starts_with('#') {
            continue;
        }
        let mut f = l.split('\t');
        let (Some(want), Some(heard)) = (f.next(), f.next()) else { continue };
        let slots = f.next().unwrap_or("").split(',').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
        v.push(Case { want: want.trim().into(), heard: heard.trim().into(), kind: "natural", slots });
    }
    v
}

/// Training phrases for the router "Lara" (docs/AI.md section 16): EVERY catalogue phrase of every enabled intent
/// (slots filled with a few sample values), an ASR-mangled copy of each, and chatter for "none". The audit's natural
/// phrasings (`tests/fixtures/audit.tsv`) are NOT in it: they stay the held-out test.
pub fn train_cases(cat: &Catalogue) -> Vec<Case> {
    const FILL: [(&str, &str, &str); 9] = [("app", "terminal", "spotify"), ("site", "github", "youtube"), ("q", "the weather", "rust tutorials"), ("text", "hello there", "see you tomorrow"), ("key", "tab", "enter"), ("minutes", "10", "25"), ("n", "3", "2"), ("side", "left", "right"), ("name", "sky", "beach")];
    let mut v = Vec::new();
    for i in cat.enabled() {
        for p in &i.phrases {
            let mut first = String::new();
            for variant in 0..2 {
                let mut filled = p.clone();
                for (k, t) in &i.slots {
                    let val = FILL.iter().find(|f| f.0 == k.as_str()).map(|f| if variant == 0 { f.1 } else { f.2 }).unwrap_or_else(|| sample(*t, k));
                    filled = filled.replace(&format!("{{{k}}}"), val);
                }
                // a phrase without slots gives the same row twice: once is enough
                if variant == 1 && filled == first {
                    continue;
                }
                if variant == 0 {
                    first = filled.clone();
                }
                v.push(Case { want: i.id.clone(), heard: filled.clone(), kind: "train", slots: vec![] });
                let m = mangle(&filled);
                if m != filled && variant == 0 {
                    v.push(Case { want: i.id.clone(), heard: m, kind: "train", slots: vec![] });
                }
            }
        }
    }
    for c in ["hello", "testing testing", "is this thing on", "never mind", "sorry not you", "what was i saying", "ok thanks", "hmm", "i'm talking to someone else", "nothing", "that was a joke", "good morning everyone", "wait a second let me think", "oops"] {
        v.push(Case { want: "none".into(), heard: c.into(), kind: "train", slots: vec![] });
    }
    v
}

/// At most `max` cases spread over the intents (round robin over the wanted ids, natural phrasings first), for a
/// paid remote run.
pub fn spread(cases: &[Case], max: usize) -> Vec<Case> {
    let mut by: BTreeMap<&str, Vec<&Case>> = BTreeMap::new();
    for c in cases {
        by.entry(c.want.as_str()).or_default().push(c);
    }
    for v in by.values_mut() {
        v.sort_by_key(|c| match c.kind {
            "natural" => 0,
            "mangled" => 1,
            _ => 2,
        });
    }
    let mut out = Vec::new();
    let mut round = 0;
    while out.len() < max {
        let before = out.len();
        for v in by.values() {
            if let Some(c) = v.get(round) {
                if out.len() < max {
                    out.push((*c).clone());
                }
            }
        }
        if out.len() == before {
            break;
        }
        round += 1;
    }
    out
}

/// The result of one case.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub case: Case,
    /// match | fuzzy | noise | ai | ...
    pub route: String,
    /// What ran (an intent id, or "-").
    pub ran: String,
    pub asked: bool,
    pub decision: String,
    pub slots: BTreeMap<String, String>,
    pub result: String,
    pub ms: u64,
    /// pass | ask | fail
    pub verdict: &'static str,
    pub why: String,
    /// The exact user message the model got (the state, the hints, the words); empty when no model was asked.
    pub prompt: String,
}

/// The heard cell: the words, and the model's exact input under them (folded).
fn heard_cell(x: &Outcome) -> String {
    if x.prompt.is_empty() {
        return esc(&x.case.heard);
    }
    format!("{}<details><summary>what the model got</summary><pre>{}</pre></details>", esc(&x.case.heard), esc(&x.prompt))
}

/// `ai:win_list?` / `win.list` -> the catalogue id form for comparing.
pub fn norm_id(s: &str) -> String {
    s.trim_start_matches("ai:").trim_end_matches('?').replace(['.', '-'], "_")
}

/// Judge a run.
pub fn judge(case: &Case, ran: Option<&str>, asked: bool, slots: &BTreeMap<String, String>) -> (&'static str, String) {
    judge_full(case, ran, asked, slots, "", "")
}

/// The same; `decision` = the model's answer, `result` = the feedback. A right decision whose action failed only
/// because the dry run's desktop lacks the target (no browser focused, no such tab) passes, with a note.
pub fn judge_full(case: &Case, ran: Option<&str>, asked: bool, slots: &BTreeMap<String, String>, decision: &str, result: &str) -> (&'static str, String) {
    if case.want == "none" {
        return match ran {
            None => ("pass", String::new()),
            Some(r) => ("fail", format!("ran {r}")),
        };
    }
    if case.want.contains(" ; ") {
        // a plan: every step, in this order (what ran, else what the model decided when an action failed on the
        // dry run's desktop)
        let want: Vec<String> = case.want.split(" ; ").map(norm_id).collect();
        let ran_steps: Vec<String> = ran.map(|r| r.split(',').map(|x| norm_id(x.trim_start_matches("ai:").trim_end_matches('?'))).collect()).unwrap_or_default();
        let decided: Vec<String> = decision.split(" > ").last().unwrap_or("").split(" ; ").filter_map(|d| d.split_whitespace().next()).map(norm_id).collect();
        return if ran_steps == want {
            ("pass", String::new())
        } else if decided == want {
            ("pass", format!("decided right; here the actions gave {result:?}"))
        } else {
            ("fail", format!("ran {}, decided {}", ran.unwrap_or("-"), decided.join(" ; ")))
        };
    }
    let decided = decision.split(" > ").last().unwrap_or("").split(" ; ").next().and_then(|d| d.split_whitespace().next()).unwrap_or("");
    // two stages: the right tool was called, its action failed on the dry run's desktop and the model explained
    if ran == Some("ai:answer") && decision.split([' ', ',']).any(|t| norm_id(t) == norm_id(&case.want)) {
        return ("pass", format!("called the right tool; here the action said {result:?}"));
    }
    let Some(r) = ran.or_else(|| (!decided.is_empty() && norm_id(decided) == norm_id(&case.want)).then_some(decided)) else { return ("fail", "nothing ran".into()) };
    if ran.is_none() {
        return ("pass", format!("decided right; here the action said {result:?}"));
    }
    // a plan's first step counts
    let first = r.split(',').next().unwrap_or(r);
    // intents that do the same thing for these words
    const SAME: &[(&str, &str)] = &[("vr.passthrough.off", "vr.backdrop.set"), ("vr.passthrough.dim", "vr.passthrough.darker"), ("vr.passthrough.on", "vr.backdrop.set"), ("omarchy.focus.app", "omarchy.launch.browser"), ("omarchy.focus.app", "focus_window"), ("omarchy.launch.browser", "focus_window")];
    let same = SAME.iter().any(|(a, b)| norm_id(&case.want) == norm_id(a) && norm_id(first) == norm_id(b));
    if norm_id(first) != norm_id(&case.want) && !same {
        return ("fail", format!("ran {r}"));
    }
    for (k, v) in &case.slots {
        let got = slots.get(k).map(|x| x.to_lowercase()).unwrap_or_default();
        let want = v.to_lowercase();
        if !(got.contains(&want) || (!got.is_empty() && want.contains(&got))) {
            return ("fail", format!("{k}={got:?}, want {want:?}"));
        }
    }
    if asked {
        ("ask", "asked \"Did you mean\"".into())
    } else {
        ("pass", String::new())
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn pct(v: &[u64], p: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    let mut s = v.to_vec();
    s.sort();
    s[((s.len() - 1) as f64 * p).round() as usize]
}

/// A summary line: (pass, ask, fail, total).
pub fn tally(o: &[Outcome]) -> (usize, usize, usize, usize) {
    let p = o.iter().filter(|x| x.verdict == "pass").count();
    let a = o.iter().filter(|x| x.verdict == "ask").count();
    (p, a, o.len() - p - a, o.len())
}

/// The self-contained report page (dark mode aware, readable on a phone).
pub fn html(title: &str, sub: &str, runs: &[(&str, Vec<Outcome>)], live: &[(String, bool, String)]) -> String {
    let mut h = String::new();
    h += "<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>";
    h += &format!("<title>Voice Audit</title><style>");
    h += ":root{--bg:#f7f7f5;--fg:#1d1d1b;--dim:#6b6b66;--card:#fff;--line:#e3e3de;--pass:#1f7a3a;--ask:#9a6b00;--fail:#b3261e;--chip:#eef0ea}\
@media (prefers-color-scheme:dark){:root:not([data-theme=light]){--bg:#121312;--fg:#e8e8e3;--dim:#9a9a93;--card:#1b1c1b;--line:#2c2d2b;--pass:#5cc97c;--ask:#e0b04a;--fail:#ff7b72;--chip:#252725}}\
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.45 system-ui,-apple-system,Segoe UI,sans-serif}\
main{max-width:1100px;margin:0 auto;padding:16px}h1{font-size:22px;margin:4px 0}h2{font-size:17px;margin:24px 0 8px}\
.sub{color:var(--dim);font-size:13px}.kpis{display:flex;flex-wrap:wrap;gap:8px;margin:12px 0}\
.kpi{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:8px 12px;min-width:120px}.kpi b{font-size:20px;display:block}\
.wrap{overflow-x:auto;background:var(--card);border:1px solid var(--line);border-radius:10px}\
table{border-collapse:collapse;width:100%;font-size:13px}th,td{padding:6px 8px;border-bottom:1px solid var(--line);text-align:left;vertical-align:top}\
th{position:sticky;top:0;background:var(--card);color:var(--dim);font-weight:600}td.m{font-family:ui-monospace,monospace;font-size:12px;word-break:break-word}\
.v{font-weight:700}.pass{color:var(--pass)}.ask{color:var(--ask)}.fail{color:var(--fail)}.chip{background:var(--chip);border-radius:6px;padding:1px 6px;font-size:12px}\
details{margin:8px 0}summary{cursor:pointer;color:var(--dim)}pre{white-space:pre-wrap;font-size:12px;margin:4px 0}";
    h += "</style></head><body><main>";
    h += &format!("<h1>{}</h1><div class=sub>{}</div>", esc(title), esc(sub));
    for (name, o) in runs {
        let (p, a, f, n) = tally(o);
        let ms: Vec<u64> = o.iter().map(|x| x.ms).collect();
        let ai: Vec<u64> = o.iter().filter(|x| x.route.starts_with("ai")).map(|x| x.ms).collect();
        h += &format!("<h2>{}</h2><div class=kpis>", esc(name));
        h += &format!("<div class=kpi><span class=sub>pass</span><b class=pass>{:.0}%</b>{p} of {n}</div>", 100.0 * p as f64 / n.max(1) as f64);
        h += &format!("<div class=kpi><span class=sub>asked</span><b class=ask>{a}</b>right intent, asked</div>");
        h += &format!("<div class=kpi><span class=sub>fail</span><b class=fail>{f}</b>wrong or nothing</div>");
        h += &format!("<div class=kpi><span class=sub>latency p50 / p95</span><b>{} / {} ms</b>all phrases</div>", pct(&ms, 0.5), pct(&ms, 0.95));
        h += &format!("<div class=kpi><span class=sub>model p50 / p95</span><b>{} / {} ms</b>{} phrases</div></div>", pct(&ai, 0.5), pct(&ai, 0.95), ai.len());
        let fails: Vec<&Outcome> = o.iter().filter(|x| x.verdict != "pass").collect();
        if !fails.is_empty() {
            h += "<div class=wrap><table><tr><th>verdict</th><th>want</th><th>heard</th><th>route</th><th>ran / decision</th><th>why</th></tr>";
            for x in fails {
                h += &format!(
                    "<tr><td class='v {v}'>{v}</td><td class=m>{}</td><td>{}</td><td><span class=chip>{}</span></td><td class=m>{} {}</td><td>{}</td></tr>",
                    esc(&x.case.want), heard_cell(x), esc(&x.route), esc(&x.ran), esc(&x.decision), esc(&x.why), v = x.verdict
                );
            }
            h += "</table></div>";
        }
        h += &format!("<details><summary>all {n} phrases</summary><div class=wrap><table><tr><th></th><th>want</th><th>kind</th><th>heard</th><th>route</th><th>ran</th><th>slots</th><th>ms</th></tr>");
        for x in o {
            let sl: Vec<String> = x.slots.iter().map(|(k, v)| format!("{k}={v}")).collect();
            h += &format!(
                "<tr><td class='v {v}'>{v}</td><td class=m>{}</td><td>{}</td><td>{}</td><td><span class=chip>{}</span></td><td class=m>{}{}</td><td class=m>{}</td><td>{}</td></tr>",
                esc(&x.case.want), x.case.kind, heard_cell(x), esc(&x.route), esc(&x.ran), if x.asked { " (asked)" } else { "" }, esc(&sl.join(" ")), x.ms, v = x.verdict
            );
        }
        h += "</table></div></details>";
    }
    if !live.is_empty() {
        let ok = live.iter().filter(|x| x.1).count();
        h += &format!("<h2>Live on the desktop: {ok} of {} passed</h2><div class=wrap><table><tr><th></th><th>check</th><th>detail</th></tr>", live.len());
        for (name, pass, detail) in live {
            let v = if *pass { "pass" } else { "fail" };
            h += &format!("<tr><td class='v {v}'>{v}</td><td>{}</td><td class=m>{}</td></tr>", esc(name), esc(detail));
        }
        h += "</table></div>";
    }
    h += "</main></body></html>\n";
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::BUILTIN;

    #[test]
    fn cases_cover_every_intent() {
        let c = Catalogue::from_texts(BUILTIN, None).unwrap();
        let v = cases(&c, FIXTURE);
        for i in c.enabled() {
            assert!(v.iter().any(|x| x.want == i.id && x.kind == "exact"), "{}", i.id);
            assert!(v.iter().filter(|x| x.want == i.id && x.kind == "natural").count() >= 1, "no natural phrasing for {}", i.id);
        }
        assert_eq!(mangle("list windows"), "liz windows");
        assert_eq!(mangle("cinema"), "sinema");
        assert_eq!(mangle("take a screenshot"), "take a screen shot");
        assert_eq!(mangle("maximize"), "maximizes");
    }

    #[test]
    fn a_remote_run_is_capped_and_spread_over_the_intents() {
        let c = Catalogue::from_texts(BUILTIN, None).unwrap();
        let v = cases(&c, FIXTURE);
        let s = spread(&v, 40);
        assert_eq!(s.len(), 40);
        let ids: std::collections::BTreeSet<&str> = s.iter().map(|x| x.want.as_str()).collect();
        assert_eq!(ids.len(), 40, "one phrase per intent before a second one");
        assert!(s.iter().all(|x| x.kind == "natural"), "natural phrasings first");
        assert_eq!(spread(&v[..3], 40).len(), 3);
    }

    #[test]
    fn judging() {
        let c = Case { want: "browser.search".into(), heard: "x".into(), kind: "natural", slots: vec![("q".into(), "how to cook rice".into())] };
        let s: BTreeMap<String, String> = [("q".to_string(), "How to cook rice".to_string())].into();
        assert_eq!(judge(&c, Some("ai:browser_search"), false, &s).0, "pass");
        assert_eq!(judge(&c, Some("type.text"), false, &s).0, "fail");
        assert_eq!(judge(&c, Some("browser.search"), true, &s).0, "ask");
        let n = Case { want: "none".into(), ..c };
        assert_eq!(judge(&n, None, false, &BTreeMap::new()).0, "pass");
    }
}
