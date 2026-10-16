//! The scoped planner (user design 2026-10-16, docs/VOICE.md "How the local voice AI decides"): instead of one
//! fixed prompt with every intent, Lara picks the category (and the runner-up when unsure), and the model gets
//! only that category's tools, the fuzzy matcher's top hints and the always-on ones (about 10-25 tools), a grammar of
//! the same subset, the exact state the category needs, and the question "which steps reach the state the user
//! wants?". One fixed system prompt per category set keeps llama-server's prompt cache useful.
//!
//! ```text
//!  heard ──► rules.rs (screen numbers, stop, sizes) ──► done
//!    │
//!    ├──► Lara (~30 ms) ──► categories ─┐
//!    └──► fuzzy hints ──────────────────┤
//!                                       ▼
//!         system prompt of those categories (cached) + grammar of those tools
//!         user: state blocks (screens by number with apps, focus, gaze, session ...) + hints + heard
//!                                       ▼
//!         model: [target: <end state>] step ; step ...  ──► dry run on the state (world::apply) ──► run
//! ```
//!
//! Every knob is a variant setting ([`crate::variant`]): `tools.scope` (all | category | category+hints | topk),
//! `tools.top_k`, `prompt.blocks`, `prompt.target_line`, so each experiment is a config change.

use crate::catalogue::{Catalogue, Intent, SlotType};
use crate::fuzzy::Cand;
use crate::remote::{self, Category};

/// How the tools are scoped.
#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    /// Every intent (the old single prompt).
    All,
    /// Only the router's categories (+ the always-on tools).
    Category,
    /// The router's categories plus the fuzzy matcher's top hints.
    CategoryHints,
    /// The router's categories cut to the `k` intents closest to the words, plus the hints.
    TopK(usize),
}

/// The planner's settings: production defaults, or the active variant's.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub scope: Scope,
    pub target_line: bool,
    /// State blocks ([`crate::world::describe`]); empty = the blocks the categories need.
    pub blocks: Vec<String>,
    /// Fuzzy hints in the user message.
    pub hints: bool,
    /// The opening of the system prompt from a file (variant `prompt.template`); None = the built-in one.
    pub template: Option<String>,
}

impl Settings {
    /// Production: the single prompt (`All`) until a scoped variant wins the state audit. Measured 2026-10-16 on the
    /// intent audit (dry run, Lara + Qwen3.5-4B): scoped category+hints 280/298 natural, p50 655 ms, against the single
    /// prompt's 293/298, p50 311 ms (a cross-category plan loses its second step; each category's prompt is a cache
    /// miss in the one llama-server slot). The scoped planner runs with a variant (`tools.scope`).
    pub fn production() -> Settings {
        Settings { scope: Scope::All, target_line: false, blocks: Vec::new(), hints: true, template: None }
    }

    /// The active variant's settings ([`crate::variant::active`]); production when none.
    pub fn current() -> Settings {
        let Some(v) = crate::variant::active() else { return Settings::production() };
        let mut s = Settings::production();
        s.scope = Scope::CategoryHints;
        crate::variant::honour("tools.scope");
        crate::variant::honour("tools.top_k");
        s.scope = match v.tools.scope.as_str() {
            "all" => Scope::All,
            "category" => Scope::Category,
            "topk" => Scope::TopK(v.tools.top_k.max(3)),
            _ => Scope::CategoryHints,
        };
        if v.prompt.template != "current" && !v.prompt.template.is_empty() {
            if let Ok(t) = std::fs::read_to_string(&v.prompt.template) {
                crate::variant::honour("prompt.template");
                s.template = Some(t);
            }
        }
        crate::variant::honour("prompt.target_line");
        s.target_line = v.prompt.target_line;
        if !v.prompt.blocks.is_empty() {
            crate::variant::honour("prompt.blocks");
            s.hints = v.prompt.blocks.iter().any(|b| b == "hints");
            s.blocks = v.prompt.blocks.iter().filter(|b| *b != "hints").cloned().collect();
        }
        s
    }
}

/// The state blocks a category set needs (`world::describe` names).
pub fn blocks_for(cats: &[Category]) -> Vec<&'static str> {
    let mut b: Vec<&'static str> = vec!["focus"];
    let mut add = |x: &'static str| {
        if !b.contains(&x) {
            b.push(x);
        }
    };
    for c in cats {
        match c {
            Category::Screens | Category::Apps => {
                add("screens");
                add("apps");
                add("gaze");
            }
            Category::Tabs | Category::Browser => {
                add("apps");
                add("tabs");
            }
            Category::Typing => add("apps"),
            Category::Session | Category::Passthrough | Category::Recording => {
                add("session");
                add("view");
            }
            Category::System => add("media"),
            Category::Captions => add("view"),
            Category::Other => {}
        }
    }
    b
}

/// The tools offered: the intents of `cats` (all intents for `All`, `Other` or none), plus the hints and the
/// always-on ones ([`remote::ALWAYS`], `voice.cancel`).
pub fn tool_ids(cat: &Catalogue, cats: &[Category], cands: &[Cand], scope: &Scope) -> Vec<String> {
    let offered = crate::nlu::offered(cat);
    let all = matches!(scope, Scope::All) || cats.is_empty() || cats.contains(&Category::Other);
    let mut ids: Vec<String> = offered.iter().filter(|i| all || remote::intent_in(&i.id, cats)).map(|i| i.id.clone()).collect();
    if let Scope::TopK(k) = scope {
        // the k closest to the words (the hints' order), the rest of the category after them
        let rank = |id: &str| cands.iter().position(|c| c.id == id).unwrap_or(usize::MAX);
        ids.sort_by_key(|id| rank(id));
        ids.truncate(*k);
    }
    if !matches!(scope, Scope::Category) {
        for c in cands.iter().filter(|c| c.score >= 0.45) {
            if offered.iter().any(|i| i.id == c.id) && !ids.contains(&c.id) {
                ids.push(c.id.clone());
            }
        }
    }
    for a in remote::ALWAYS.iter().chain(["voice.cancel"].iter()) {
        if offered.iter().any(|i| i.id == *a) && !ids.contains(&a.to_string()) {
            ids.push(a.to_string());
        }
    }
    ids
}

/// The key a system prompt is cached under (categories sorted).
pub fn key(cats: &[Category]) -> String {
    let mut v: Vec<&str> = cats.iter().map(|c| c.id()).collect();
    v.sort();
    v.dedup();
    v.join("+")
}

fn slot_hint(name: &str, t: SlotType) -> String {
    match t {
        SlotType::Number => format!("{name}=N"),
        SlotType::Word => format!("{name}=word"),
        SlotType::Side => format!("{name}=left|right|up|main|up-left|up-right"),
        SlotType::Screen => format!("{name}=1-6|left|right|up|main|up-left|up-right|here"),
        SlotType::Text | SlotType::Name => format!("{name}=\"words\""),
    }
}

/// One tool line: `win.screen.put_n screen=1-6|... app="words": apps of that kind or name to screen N`.
pub fn tool_line(i: &Intent) -> String {
    let mut line = i.id.clone();
    let mut slots: Vec<(&String, &SlotType)> = i.slots.iter().collect();
    slots.sort_by_key(|(k, t)| (matches!(t, SlotType::Text | SlotType::Name), (*k).clone()));
    for (k, t) in slots {
        line += &format!(" {}", slot_hint(k, *t));
    }
    let gloss = if !i.gloss.is_empty() { i.gloss.clone() } else { i.phrases.iter().take(3).cloned().collect::<Vec<_>>().join(" / ") };
    format!("{line}: {gloss}{}", if crate::agent::intent_safety(i) == "careful" { " (asks yes)" } else { "" })
}

/// Rules of one category (only the categories offered get theirs: shorter prompts, fewer wrong turns).
fn rules_of(c: Category) -> &'static str {
    match c {
        Category::Screens => "VR screens by number: 1 main, 2 left, 3 right, 4 up, 5 up-left, 6 up-right; \"here\" = the screen looked at. \
Close / remove / hide / shut down screen N = vr.screens.hide screens=\"N,M\" (it hides the screen; apps stay open); close everything / \
close the screens = vr.screens.hide_others. All screens half / double / N percent = win.screen.scale. \"N screens\" = vr.screens.count; \
\"N more screens\" = vr.screens.more. Apps by kind or name to screen N = win.screen.put_n; to a side = win.screen.put; THIS app = \
win.screen.move / win.screen.move_n. Move THIS SCREEN = win.screen.nudge / win.screen.adjust (the screen itself). \"in front of me\" = \
vr.screens.reset. win.ws.close closes every app of a workspace: only when the user asks to close the window (workspace) itself, never for a screen.",
        Category::Apps => "An \"app\" is one program (close this = win.close); \"window\" = a workspace / VR screen. Never close an app the user did not name.",
        Category::Tabs => "\"tab\" = a tab inside the focused app (browser or terminal).",
        Category::Typing => "Type only words the user dictated (type.text text=\"...\").",
        Category::Browser => "\"open X\" = browser.open site=X when X is a site; else a search.",
        Category::Session => "Movies: \"watch a movie\" = vr.preset.cinema; vr.preset.exit_cinema only to LEAVE cinema. Stop / shut down the VR / work / session = vr.session.stop (asks yes).",
        Category::Passthrough => "The room = passthrough: dim = vr.passthrough.dim, off / black = vr.passthrough.off.",
        Category::Recording => "Recording and screenshots of VR.",
        Category::Captions => "Captions, the debug line and voice help.",
        Category::System => "Volume, music, battery, reminders. \"play <a title>\" = media.play (a file).",
        Category::Other => "",
    }
}

/// The fixed system prompt for these categories and tools (cached by [`key`] by the caller).
pub fn system_prompt(cat: &Catalogue, cats: &[Category], ids: &[String], target_line: bool) -> String {
    system_prompt_with(None, cat, cats, ids, target_line)
}

/// [`system_prompt`] with another opening (a variant's template file).
pub fn system_prompt_with(template: Option<&str>, cat: &Catalogue, cats: &[Category], ids: &[String], target_line: bool) -> String {
    let mut s = match template {
        Some(t) => format!("{}\n", t.trim_end()),
        None => String::from(
        "You are Max, an expert operator of this Linux desktop seen in a VR headset. Speech-to-text is imperfect, word for word (war=work, \
liz=list, tree/free=three, for=four, to=two, scream=screen, winter=window, write=right, lift=left, of=off). \
The user said something; you get the current state. Using ONLY the tools below, answer the steps, in order and with \
their inputs, that reach the state the user wants: `tool confidence input=value ...`, the confidence one digit (9 sure, \
6 likely, 3 a guess), steps separated by ` ; ` (at most 4), e.g. `win.screen.put_n 9 screen=3 app=\"terminals\"`. \
`none 9` for noise, a test or chatter. The hints are the tools whose phrases sound closest: often right, but the \
meaning decides; a short command with one odd word (a misheard verb or noun) is usually the best hint. Never invent \
an app or a screen the user did not name.\n",
        ),
    };
    if target_line {
        s += "First write one line `target: <the state the user wants, a few words>`, then the steps.\n";
    }
    for c in cats {
        let r = rules_of(*c);
        if !r.is_empty() {
            s += r;
            s.push('\n');
        }
    }
    s += "Tools:\n";
    for id in ids {
        if let Some(i) = cat.get(id) {
            s += &tool_line(i);
            s.push('\n');
        }
    }
    // the examples of the single prompt whose answer is one of these tools
    let ex = examples(cat, ids);
    if !ex.is_empty() {
        s += &format!("Examples: {}\n", ex.join(" | "));
    }
    if cats.iter().any(|c| matches!(c, Category::Screens | Category::Apps)) {
        for (id, gloss, slots) in crate::nlu::WINDOW_INTENTS {
            let sl: Vec<String> = slots.iter().map(|(k, v)| format!("{k}={v}")).collect();
            s += &format!("{id} {}: {gloss}\n", sl.join(" "));
        }
    }
    s
}

/// The examples of [`crate::nlu::system_prompt`] (`"words" -> answer`) whose every step is one of `ids`.
pub fn examples(cat: &Catalogue, ids: &[String]) -> Vec<String> {
    let full = crate::nlu::system_prompt(cat);
    let Some(i) = full.find("Examples: ") else { return Vec::new() };
    full[i + 10..]
        .lines()
        .next()
        .unwrap_or("")
        .split(" | ")
        .filter(|e| {
            e.split("-> ").nth(1).is_some_and(|ans| ans.split(" ; ").all(|step| step.split_whitespace().next().is_some_and(|id| id == "none" || ids.iter().any(|x| x == id))))
        })
        .map(|e| e.trim().to_string())
        .collect()
}

/// The grammar of these tools; with a target line first when asked.
pub fn grammar(cat: &Catalogue, ids: &[String], plan: bool, target_line: bool) -> String {
    let g = crate::nlu::grammar_subset(cat, plan, &|id| ids.iter().any(|x| x == id) || crate::nlu::WINDOW_INTENTS.iter().any(|w| w.0 == id));
    if !target_line {
        return g;
    }
    let g = g.replacen("root ::= ", "root ::= tgt ", 1);
    g + "tgt ::= \"target: \" [a-zA-Z0-9 ,.'-]{1,60} \"\\n\"\n"
}

/// The answer without its target line: (target, steps).
pub fn split_target(answer: &str) -> (Option<String>, String) {
    let mut target = None;
    let mut rest = Vec::new();
    for l in answer.lines() {
        match l.trim().strip_prefix("target:") {
            Some(t) if target.is_none() => target = Some(t.trim().to_string()),
            _ => rest.push(l),
        }
    }
    (target, rest.join("\n"))
}

/// The dry run of a plan on the state snapshot ([`crate::world::apply`] step by step). Refused: a tool the
/// catalogue does not have or an input missing; a plan that closes an app the words did not name (no kind, class or
/// title word heard, and no "this" / "it" / "window"). A step the simulator does not model is let through (the
/// catalogue's own checks still run before it acts).
pub fn check(w: &crate::world::World, cat: &Catalogue, steps: &[crate::nlu::Decision], heard: &str) -> Result<(), String> {
    let words = crate::command::words(heard);
    let mut cur = w.clone();
    for d in steps.iter().filter(|d| d.intent != "none" && d.intent != "done") {
        let window_tool = crate::nlu::WINDOW_INTENTS.iter().any(|x| x.0 == d.intent);
        if !window_tool && cat.get(&d.intent).is_none() {
            return Err(format!("no tool {}", d.intent));
        }
        match crate::world::apply(&cur, cat, &d.intent, &d.slots) {
            Ok(a) => cur = a.world,
            Err(e) if e.contains("missing") || e.starts_with("no tool") => return Err(e),
            Err(_) => {}
        }
    }
    let deictic = words.iter().any(|x| matches!(x.as_str(), "this" | "it" | "that" | "window" | "windows"));
    for c in crate::world::diff(w, &cur) {
        if c.key.ends_with(":closed") && !deictic {
            let name = c.key.split(':').nth(1).unwrap_or("");
            let named = w.app(name).is_some_and(|a| {
                let hay = format!("{} {} {}", a.class, a.title, a.kind.word()).to_lowercase();
                words.iter().filter(|x| x.len() >= 3 && !crate::command::FILLERS.contains(&x.as_str())).any(|x| hay.contains(x.trim_end_matches('s')))
            });
            if !named {
                return Err(format!("it would close an app nobody named ({})", c.text));
            }
        }
    }
    Ok(())
}

/// The user message: the state, the hints, the words heard.
pub fn user_message(state: &str, cands: &[Cand], heard: &str, hints: bool) -> String {
    let mut s = String::new();
    if !state.trim().is_empty() {
        s += &format!("state:\n{}\n", state.trim_end());
    }
    if hints && !cands.is_empty() {
        let h: Vec<String> = cands.iter().map(|c| format!("{} {:.2}", c.id, c.score)).collect();
        s += &format!("hints: {}\n", h.join(", "));
    }
    s += &format!("heard: \"{}\"", crate::windows::clean_title(heard).replace('"', "'"));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> Catalogue {
        Catalogue::from_texts(crate::catalogue::BUILTIN, None).unwrap()
    }

    #[test]
    fn a_screen_request_sees_only_screen_tools() {
        let c = cat();
        let ids = tool_ids(&c, &[Category::Screens], &[], &Scope::Category);
        assert!(ids.contains(&"vr.screens.hide".to_string()) && ids.contains(&"win.screen.put_n".to_string()));
        assert!(!ids.iter().any(|i| i.starts_with("browser.") || i == "win.close" || i.starts_with("type.")), "{ids:?}");
        assert!(ids.len() < 60, "{}", ids.len());
        let p = system_prompt(&c, &[Category::Screens], &ids, false);
        assert!(p.contains("vr.screens.hide") && !p.contains("browser.search"));
        assert!(p.len() < crate::nlu::system_prompt(&c).len() / 2, "{} vs {}", p.len(), crate::nlu::system_prompt(&c).len());
        let g = grammar(&c, &ids, true, true);
        assert!(g.starts_with("root ::= tgt one") && g.contains("tgt ::= \"target: \""), "{}", &g[..80]);
        assert!(!g.contains("\"browser.search\""));
    }

    #[test]
    fn hints_widen_the_scope_and_the_target_line_is_split_off() {
        let c = cat();
        let cands = crate::fuzzy::candidates(&c, &crate::command::words("louder please"), 3);
        let ids = tool_ids(&c, &[Category::Screens], &cands, &Scope::CategoryHints);
        assert!(cands.iter().filter(|x| x.score >= 0.45).all(|x| ids.contains(&x.id)));
        let (t, rest) = split_target("target: screens 3 and 4 hidden\nvr.screens.hide 9 screens=\"4,3\"");
        assert_eq!(t.as_deref(), Some("screens 3 and 4 hidden"));
        assert_eq!(crate::nlu::parse_plan(&rest).unwrap()[0].intent, "vr.screens.hide");
        assert_eq!(key(&[Category::Screens, Category::Apps]), key(&[Category::Apps, Category::Screens]));
    }
}
