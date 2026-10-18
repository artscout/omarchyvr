//! `omarchyvr-voice audit --state`: voice commands scored by the STATE they reach (docs/VOICE-STATE-TESTS.md).
//!
//! Each case: a starting [`World`] (from `tests/state/worlds.toml` plus per-case changes), the words as the
//! speech-to-text really heard them, and the goal as expectations on the end state ([`check`]) plus "nothing else
//! changes". The whole pipeline (wake word, rules, matcher, Lara, the model, plans and goals) runs against a
//! [`Sim`] of that World; the end state is what the Sim holds afterwards. The intent name is never scored.
//!
//! Goals (`tests/state/goals-*.toml`) expand into cases: the first phrasing in every tier (medium / hard
//! starting state), the other phrasings in one tier each. Live phrases go mostly to the held-out split, which is
//! never used for tuning. A run appends one row to `analysis/voice-experiments.csv`; the HTML report shows the
//! leaderboard of all runs.

use crate::world::{self, Kind, Sim, World};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const WORLDS: &str = include_str!("../tests/state/worlds.toml");
pub const GOALS: &[(&str, &str)] = &[
    ("live", include_str!("../tests/state/goals-live.toml")),
    ("screens", include_str!("../tests/state/goals-screens.toml")),
    ("apps", include_str!("../tests/state/goals-apps.toml")),
    ("multi", include_str!("../tests/state/goals-multi.toml")),
    ("safety", include_str!("../tests/state/goals-safety.toml")),
    ("misc", include_str!("../tests/state/goals-misc.toml")),
    ("fresh", include_str!("../tests/state/goals-fresh.toml")),
    ("live-1818", include_str!("../tests/state/goals-live-1818.toml")),
    ("steps", include_str!("../tests/state/goals-steps.toml")),
    ("long", include_str!("../tests/state/goals-long.toml")),
];

// ------------------------------------------------------------------------------------------------ world specs

/// A starting state in a few lines (every field optional; a case's `set` overrides its world's).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorldSpec {
    pub base: Option<String>,
    /// Screens present (the session's Screens setting): 1..6.
    pub screens: Option<u8>,
    pub preset: Option<String>,
    /// `name class place [cwd] [fs] [float] [title="..."] [tabs=A|B*|C]` (tabs last: they take the rest of the line); place = 1..6 (that screen's workspace) or
    /// wsN (a workspace no screen shows). Example: `c1 google-chrome 2 tabs=Gmail|YouTube - Dune trailer*`.
    pub apps: Option<Vec<String>>,
    /// Appended to the base's apps.
    pub add_apps: Option<Vec<String>>,
    /// Handles of base apps to drop.
    pub drop_apps: Option<Vec<String>>,
    pub focus: Option<String>,
    pub focused_screen: Option<u8>,
    /// "2" (screen 2) or "2:c1" (app c1 on screen 2); "none".
    pub gaze: Option<String>,
    pub hidden: Option<Vec<u8>>,
    pub pinned: Option<Vec<u8>>,
    pub limit: Option<u8>,
    /// screen number -> workspace it shows ({"4" = 7}).
    pub ws: Option<BTreeMap<String, i64>>,
    /// screen number -> [width, distance, yaw, pitch].
    pub geom: Option<BTreeMap<String, Vec<f32>>>,
    pub volume: Option<u8>,
    pub muted: Option<bool>,
    pub music: Option<String>,
    pub passthrough: Option<String>,
    pub skybox: Option<String>,
    pub captions: Option<bool>,
    pub recording: Option<bool>,
    pub running: Option<bool>,
    /// Timing (opt-in): a launched app shows in `hyprctl -j clients` after this many more reads.
    pub launch_delay_reads: Option<u32>,
    /// A screen made shown or added shows in `hyprctl -j monitors` after this many more reads.
    pub screen_delay_reads: Option<u32>,
    /// Changes applied once, after the first read the voice code makes: "focus t2", "close s1", "gaze 3", "open chrome 2".
    pub drift: Option<Vec<String>>,
}

impl WorldSpec {
    fn merged(&self, over: &WorldSpec) -> WorldSpec {
        macro_rules! pick {
            ($($f:ident),*) => { WorldSpec { $($f: over.$f.clone().or_else(|| self.$f.clone()),)* } };
        }
        let mut m = pick!(base, screens, preset, apps, add_apps, drop_apps, focus, focused_screen, gaze, hidden, pinned, limit, ws, geom, volume, muted, music, passthrough, skybox, captions, recording, running, launch_delay_reads, screen_delay_reads, drift);
        // add_apps accumulate
        let mut add = self.add_apps.clone().unwrap_or_default();
        add.extend(over.add_apps.clone().unwrap_or_default());
        m.add_apps = (!add.is_empty()).then_some(add);
        m
    }
}

#[derive(Debug, Deserialize)]
struct WorldsFile {
    worlds: BTreeMap<String, WorldSpec>,
}

/// Split on spaces, double quotes keep spaces.
fn tokens(s: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in s.chars() {
        match c {
            '"' => q = !q,
            ' ' if !q => {
                if !cur.is_empty() {
                    v.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        v.push(cur);
    }
    v
}

fn default_title(class: &str, cwd: Option<&str>) -> String {
    match Kind::of(class) {
        Kind::Terminal => format!("user@host:{}", cwd.unwrap_or("~")),
        Kind::Pimax => "Pimax".into(),
        _ => match class {
            "code" => "main.rs - pimax-re - Visual Studio Code".into(),
            "obsidian" => "VR notes - Obsidian".into(),
            "spotify" => "Spotify Premium".into(),
            "steam" => "Steam".into(),
            "org.gnome.Nautilus" => "Downloads".into(),
            c => c.into(),
        },
    }
}

/// Build the World of a spec (worlds resolved through `base`).
pub fn build(spec: &WorldSpec, all: &BTreeMap<String, WorldSpec>) -> Result<World, String> {
    let mut s = spec.clone();
    let mut guard = 0;
    while let Some(b) = s.base.clone() {
        let parent = all.get(&b).ok_or_else(|| format!("no world {b}"))?;
        let mut child = s.clone();
        child.base = None;
        s = WorldSpec { base: parent.base.clone(), ..parent.clone() }.merged(&child);
        guard += 1;
        if guard > 8 {
            return Err("world bases loop".into());
        }
    }
    let mut w = World::default();
    let n = s.screens.unwrap_or(1).clamp(1, 6);
    for sc in w.screens.iter_mut() {
        sc.present = sc.n <= n;
    }
    w.session.running = s.running.unwrap_or(true);
    let preset = s.preset.clone().unwrap_or_else(|| if n >= 3 { "work-4".into() } else { "work-1".into() });
    // the preset's show rule and layout (omarchyvr-tools preset.rs / screens.rs)
    // (the old "work-3", "work-1" and "assistant" stay as start states of the older worlds: one screen / every screen on demand)
    let (limit, pinned, width, distance, curv, pt) = world::layout_of(&preset).unwrap_or(match preset.as_str() {
        "work-3" => (3, vec![2, 3], 1.6, 2.0, 0.0, "normal"),
        "work-1" => (1, vec![], 2.0, 2.2, 0.0, "normal"),
        _ => (6, vec![], 2.0, 2.5, 0.15, "normal"),
    });
    w.session.preset = preset.clone();
    w.show.limit = s.limit.unwrap_or(limit);
    w.show.pinned = s.pinned.clone().unwrap_or(pinned).into_iter().filter(|p| *p <= n).collect();
    w.show.hidden = s.hidden.clone().unwrap_or_default();
    w.view.passthrough = s.passthrough.clone().unwrap_or_else(|| pt.into());
    w.view.skybox = s.skybox.clone().unwrap_or_else(|| "off".into());
    w.view.captions = s.captions.unwrap_or(true);
    w.recording = s.recording.unwrap_or(false);
    w.timing = world::Timing { launch_delay_reads: s.launch_delay_reads.unwrap_or(0), screen_delay_reads: s.screen_delay_reads.unwrap_or(0), drift: s.drift.clone().unwrap_or_default() };
    w.media.volume = s.volume.unwrap_or(40);
    w.media.muted = s.muted.unwrap_or(false);
    w.media.music = s.music.clone().unwrap_or_else(|| "none".into());
    for sc in w.screens.iter_mut() {
        sc.width = width;
        sc.distance = distance;
        sc.curvature = curv;
    }
    for (k, v) in s.ws.clone().unwrap_or_default() {
        let k: u8 = k.parse().map_err(|_| format!("ws: bad screen {k}"))?;
        if let Some(sc) = w.screens.iter_mut().find(|x| x.n == k) {
            sc.ws = v;
        }
    }
    for (k, g) in s.geom.clone().unwrap_or_default() {
        let k: u8 = k.parse().map_err(|_| format!("geom: bad screen {k}"))?;
        if let Some(sc) = w.screens.iter_mut().find(|x| x.n == k) {
            if let [wd, d, y, p] = g[..] {
                (sc.width, sc.distance, sc.yaw, sc.pitch) = (wd, d, y, p);
            }
        }
    }
    let mut lines = s.apps.clone().unwrap_or_default();
    lines.extend(s.add_apps.clone().unwrap_or_default());
    let drop = s.drop_apps.clone().unwrap_or_default();
    let count = lines.len() as u64;
    for (i, l) in lines.iter().enumerate() {
        // `tabs=` takes the rest of the line (tab titles have spaces)
        let (head, tabs_part) = match l.find(" tabs=") {
            Some(i) => (&l[..i], Some(&l[i + 1..])),
            None => (l.as_str(), None),
        };
        let mut t = tokens(head);
        if let Some(tp) = tabs_part {
            t.push(tp.to_string());
        }
        if t.len() < 3 {
            return Err(format!("app line needs name class place: {l}"));
        }
        if drop.contains(&t[0]) {
            continue;
        }
        let place = &t[2];
        let ws = if let Some(x) = place.strip_prefix("ws") {
            x.parse::<i64>().map_err(|_| format!("bad place {place}"))?
        } else {
            let sn: u8 = place.parse().map_err(|_| format!("bad place {place}"))?;
            w.screens.iter().find(|x| x.n == sn).map(|x| x.ws).ok_or_else(|| format!("no screen {sn}"))?
        };
        let mut app = world::App { id: format!("0x{:x}", 0x10 + i), name: t[0].clone(), class: t[1].clone(), ws, last_focus: count - i as u64, ..Default::default() };
        app.kind = Kind::of(&app.class);
        for x in &t[3..] {
            if x == "fs" {
                app.fullscreen = true;
            } else if x == "float" {
                app.floating = true;
            } else if let Some(v) = x.strip_prefix("tabs=") {
                for (j, tt) in v.split('|').enumerate() {
                    let active = tt.ends_with('*');
                    let title = tt.trim_end_matches('*').to_string();
                    let url = url_for(&title);
                    app.tabs.push(world::Tab { title, url });
                    if active {
                        app.active_tab = j;
                    }
                }
            } else if let Some(v) = x.strip_prefix("title=") {
                app.title = v.to_string();
            } else if x.starts_with('~') || x.starts_with('/') {
                app.cwd = Some(x.clone());
            } else {
                return Err(format!("app {}: unknown option {x}", t[0]));
            }
        }
        if app.title.is_empty() {
            app.title = default_title(&app.class, app.cwd.as_deref());
        }
        if app.kind == Kind::Terminal && app.cwd.is_none() {
            app.cwd = Some("~".into());
        }
        w.apps.push(app);
    }
    w.normalize();
    if let Some(f) = s.focus.clone().filter(|f| f != "none") {
        let id = w.app(&f).map(|a| a.id.clone()).ok_or_else(|| format!("focus: no app {f}"))?;
        let home = w.workspaces.iter().find(|x| x.id == w.app(&id).map(|a| a.ws).unwrap_or(0)).map(|x| x.screen).unwrap_or(1);
        w.focused_screen = home;
        w.focus = Some(id.clone());
        let top = w.apps.iter().map(|a| a.last_focus).max().unwrap_or(0) + 1;
        if let Some(a) = w.apps.iter_mut().find(|a| a.id == id) {
            a.last_focus = top;
        }
    } else if let Some(fs) = s.focused_screen {
        w.focused_screen = fs;
    } else if let Some(a) = w.apps.iter().filter(|a| w.screen_of(a).is_some()).max_by_key(|a| a.last_focus) {
        w.focus = Some(a.id.clone());
        w.focused_screen = w.screen_of(a).unwrap_or(1);
    }
    if let Some(fs) = s.focused_screen {
        w.focused_screen = fs;
    }
    w.clock = w.apps.iter().map(|a| a.last_focus).max().unwrap_or(0) + 1;
    w.refresh_shown();
    if let Some(g) = s.gaze.clone().filter(|g| g != "none") {
        let (sn, app) = match g.split_once(':') {
            Some((a, b)) => (a, Some(b.to_string())),
            None => (g.as_str(), None),
        };
        let sn: u8 = sn.parse().map_err(|_| format!("gaze: bad screen {g}"))?;
        let app = app.map(|a| w.app(&a).map(|x| x.id.clone()).ok_or_else(|| format!("gaze: no app {a}"))).transpose()?;
        // a screen the case's changes hid cannot be looked at: the eyes are on the focused screen then
        w.gaze = if w.screen(sn).is_some_and(|x| x.shown) { Some(world::GazeAt { screen: sn, app }) } else { Some(world::GazeAt { screen: w.focused_screen, app: None }) };
    }
    Ok(w)
}

fn url_for(title: &str) -> String {
    let t = title.to_lowercase();
    let host = if t.contains("youtube") {
        "www.youtube.com/watch"
    } else if t.contains("gmail") || t.contains("inbox") {
        "mail.google.com"
    } else if t.contains("github") {
        "github.com"
    } else if t.contains("google search") {
        "www.google.com/search"
    } else if t.contains("wikipedia") {
        "en.wikipedia.org"
    } else if t.contains("reddit") {
        "www.reddit.com"
    } else if t.contains("netflix") {
        "www.netflix.com"
    } else if t.contains("monado") {
        "gitlab.freedesktop.org/monado"
    } else {
        "example.com"
    };
    format!("https://{host}")
}

// ---------------------------------------------------------------------------------------------------- cases

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoalSpec {
    id: String,
    #[serde(default)]
    cat: String,
    why: String,
    #[serde(default)]
    expect: Vec<String>,
    /// Several acceptable end states: any one of these sets (each with `expect`).
    #[serde(default)]
    any: Vec<Vec<String>>,
    #[serde(default)]
    allow: Vec<String>,
    /// Steps in order: each is a list of expectations (same language as `expect`) that must hold in some world
    /// the desk went through, the steps in this order (a later step in the same or a later world).
    #[serde(default)]
    steps: Vec<Vec<String>>,
    /// tier -> world (a name in worlds.toml), e.g. {medium = "m3", hard = "h6"}; no easy tier (user 2026-10-18), the loader rejects the key.
    worlds: BTreeMap<String, String>,
    /// Per-tier changes to the world.
    #[serde(default)]
    set: BTreeMap<String, WorldSpec>,
    /// Phrasings written for the test (dev split mostly).
    #[serde(default)]
    says: Vec<String>,
    /// Phrasings heard in live sessions (held-out split mostly).
    #[serde(default)]
    live: Vec<String>,
    #[serde(default)]
    src: String,
    /// The answer to a "say yes" question (default yes: the user agrees to what they asked for).
    #[serde(default)]
    yes: Option<bool>,
    #[serde(default)]
    safety: bool,
    /// "fresh": a held-out set written before any tuning (goals-fresh.toml): every phrasing is one case, the
    /// phrasings rotate over the tiers in order, no noisy copy. Never tuned on.
    #[serde(default)]
    split: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoalsFile {
    goal: Vec<GoalSpec>,
}

/// One test case.
#[derive(Debug, Clone)]
pub struct Case {
    pub id: String,
    pub goal: String,
    pub cat: String,
    pub tier: String,
    /// dev | held | fresh
    pub split: String,
    pub live: bool,
    pub src: String,
    pub say: String,
    pub world_name: String,
    pub start: World,
    pub expect: Vec<String>,
    pub any: Vec<Vec<String>>,
    pub allow: Vec<String>,
    pub steps: Vec<Vec<String>>,
    pub why: String,
    pub yes: bool,
    pub safety: bool,
}

/// The speech-to-text's ways of getting a phrase wrong (live logs 2026-10-15/16), two of them, picked by `h`.
pub fn noisy(say: &str, h: u64) -> String {
    const SWAP: &[(&str, &str)] = &[
        ("close", "closed"), ("screens", "screams"), ("screen", "scream"), ("four", "for"), ("two", "to"), ("three", "tree"),
        ("2", "two"), ("3", "three"), ("4", "four"), ("5", "five"), ("one", "won"), ("work", "war"), ("tab", "tap"),
        ("terminals", "terminal"), ("window", "winter"), ("passthrough", "pass through"), ("hide", "high"), ("the", "uh the"),
    ];
    const LEAD: &[&str] = &["uh ", "um so ", "okay can you ", "can you uh ", "yeah ", "so uh "];
    let mut words: Vec<String> = say.split_whitespace().map(str::to_string).collect();
    let mut done = 0;
    for k in 0..SWAP.len() {
        let (a, b) = SWAP[(h as usize + k * 7) % SWAP.len()];
        if let Some(w) = words.iter_mut().find(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).eq_ignore_ascii_case(a)) {
            *w = w.to_lowercase().replace(a, b);
            done += 1;
            if done == 1 {
                break;
            }
        }
    }
    let mut out = words.join(" ");
    match h % 3 {
        0 => out = format!("{}{out}", LEAD[(h / 3) as usize % LEAD.len()]),
        1 if words.len() >= 4 => {
            // "can you s Max can you shut down ..." (live: the user starts, says "Max" and starts again)
            out = format!("{} Max, {out}", words[..2].join(" "));
        }
        _ => out = format!("{}{out} please", LEAD[(h / 5) as usize % LEAD.len()]),
    }
    out
}

fn hash(s: &str) -> u64 {
    // FNV-1a: stable across runs and builds
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Every case of the built-in goal files.
pub fn load_cases() -> Result<Vec<Case>, String> {
    load_cases_from(GOALS, WORLDS)
}

/// The same from given goal files and worlds text (tests feed it bad files).
fn load_cases_from(goals: &[(&str, &str)], worlds_text: &str) -> Result<Vec<Case>, String> {
    let worlds: WorldsFile = toml::from_str(worlds_text).map_err(|e| format!("worlds.toml: {e}"))?;
    let mut out = Vec::new();
    for (file, text) in goals {
        let g: GoalsFile = toml::from_str(text).map_err(|e| format!("goals-{file}.toml: {e}"))?;
        for goal in g.goal {
            // the easy tier is gone (user decision 2026-10-18): fail loudly if a goal brings it back
            if let Some(bad) = goal.worlds.keys().chain(goal.set.keys()).find(|k| !matches!(k.as_str(), "medium" | "hard")) {
                return Err(format!("goal {}: tier `{bad}` is not allowed: the state tests have medium and hard tiers only (the easy tier was removed on 2026-10-18)", goal.id));
            }
            let tiers: Vec<(String, String)> = ["medium", "hard"].iter().filter_map(|t| goal.worlds.get(*t).map(|w| (t.to_string(), w.clone()))).collect();
            if tiers.is_empty() {
                return Err(format!("goal {}: no worlds", goal.id));
            }
            if goal.split.as_deref() == Some("fresh") {
                for (i, say) in goal.says.iter().enumerate() {
                    let (tier, wname) = &tiers[i % tiers.len()];
                    let spec = worlds.worlds.get(wname).ok_or_else(|| format!("goal {}: no world {wname}", goal.id))?;
                    let spec = goal.set.get(tier).map(|o| spec.merged(o)).unwrap_or_else(|| spec.clone());
                    let start = build(&spec, &worlds.worlds).map_err(|e| format!("goal {} ({tier}, {wname}): {e}", goal.id))?;
                    out.push(Case {
                        id: format!("{}.{}{}", goal.id, &tier[..1], i + 1),
                        goal: goal.id.clone(),
                        cat: if goal.cat.is_empty() { file.to_string() } else { goal.cat.clone() },
                        tier: tier.clone(),
                        split: "fresh".into(),
                        live: false,
                        src: "fresh".into(),
                        say: say.clone(),
                        world_name: wname.clone(),
                        start,
                        expect: goal.expect.clone(),
                        any: goal.any.clone(),
                        allow: goal.allow.clone(),
                        steps: goal.steps.clone(),
                        why: goal.why.clone(),
                        yes: goal.yes.unwrap_or(true),
                        safety: goal.safety,
                    });
                }
                continue;
            }
            // every live phrase; of the written ones the first (in every tier), plus the last (often the one with
            // fillers or a mishearing) for a goal of 3+ written phrasings and no live ones: ~350 cases in all
            let mut written: Vec<&String> = goal.says.iter().take(1).collect();
            if goal.live.is_empty() && goal.says.len() >= 3 {
                written.extend(goal.says.last());
            }
            let mut phrases: Vec<(String, bool)> = goal.live.iter().map(|s| (s.clone(), true)).chain(written.into_iter().map(|s| (s.clone(), false))).collect();
            // one ASR-noisy copy of the first written phrasing (fillers, mishearings, number words, a "Max" in the
            // middle): the way the headset mic and Parakeet really deliver it
            if let Some(first) = goal.says.first() {
                let n = noisy(first, hash(&goal.id));
                if &n != first {
                    phrases.push((n, false));
                }
            }
            let turn = hash(&goal.id) as usize;
            for (i, (say, live)) in phrases.iter().enumerate() {
                // the first phrasing in every tier; the others in one tier each (rotating)
                let these: Vec<&(String, String)> = if i == 0 { tiers.iter().collect() } else { vec![&tiers[(i + turn) % tiers.len()]] };
                for (tier, wname) in these {
                    let spec = worlds.worlds.get(wname).ok_or_else(|| format!("goal {}: no world {wname}", goal.id))?;
                    let spec = goal.set.get(tier).map(|o| spec.merged(o)).unwrap_or_else(|| spec.clone());
                    let start = build(&spec, &worlds.worlds).map_err(|e| format!("goal {} ({tier}, {wname}): {e}", goal.id))?;
                    let id = format!("{}.{}{}", goal.id, &tier[..1], i + 1);
                    // live phrases: 4 of 5 held out; written ones: 1 of 4
                    let h = hash(&id);
                    let split = if *live { if h % 5 == 0 { "dev" } else { "held" } } else if h % 4 == 0 { "held" } else { "dev" };
                    out.push(Case {
                        id,
                        goal: goal.id.clone(),
                        cat: if goal.cat.is_empty() { file.to_string() } else { goal.cat.clone() },
                        tier: tier.clone(),
                        split: split.into(),
                        live: *live,
                        src: if *live { if goal.src.is_empty() { "live".into() } else { goal.src.clone() } } else { "written".into() },
                        say: say.clone(),
                        world_name: wname.clone(),
                        start,
                        expect: goal.expect.clone(),
                        any: goal.any.clone(),
                        allow: goal.allow.clone(),
                        steps: goal.steps.clone(),
                        why: goal.why.clone(),
                        yes: goal.yes.unwrap_or(true),
                        safety: goal.safety || file == &"safety",
                    });
                }
            }
        }
    }
    Ok(out)
}

/// A seeded, tier-stratified pick of `n` cases (for fast iteration).
pub fn subset(cases: &[Case], n: usize, seed: u64) -> Vec<Case> {
    if n == 0 || n >= cases.len() {
        return cases.to_vec();
    }
    let mut v: Vec<&Case> = cases.iter().collect();
    v.sort_by_key(|c| hash(&format!("{seed}:{}", c.id)));
    let mut by: BTreeMap<&str, Vec<&Case>> = BTreeMap::new();
    for c in v {
        by.entry(c.tier.as_str()).or_default().push(c);
    }
    let mut out = Vec::new();
    let mut i = 0;
    while out.len() < n {
        let before = out.len();
        for v in by.values() {
            if let Some(c) = v.get(i) {
                if out.len() < n {
                    out.push((*c).clone());
                }
            }
        }
        if out.len() == before {
            break;
        }
        i += 1;
    }
    out
}

// --------------------------------------------------------------------------------------------- expectations

/// The apps a selector names in a World: a handle (t1), a kind word (terminals, browsers, chrome, pimax, video,
/// editors), `all`, `new` (not in `start`), `focused` (focused at the start), `gazed` (looked at at the start, else focused).
fn select<'a>(sel: &str, w: &'a World, start: &World) -> Vec<&'a world::App> {
    let s = sel.to_lowercase();
    let kind = |k: Kind| -> Vec<&'a world::App> { w.apps.iter().filter(|a| a.kind == k).collect() };
    match s.as_str() {
        "all" | "apps" => w.apps.iter().collect(),
        "new" => w.apps.iter().filter(|a| !start.apps.iter().any(|x| x.id == a.id)).collect(),
        "focused" => w.apps.iter().filter(|a| start.focus.as_deref() == Some(a.id.as_str())).collect(),
        // "this": the app looked at, else the focused one
        "gazed" => {
            let id = start.gaze.as_ref().and_then(|g| g.app.clone()).or_else(|| start.focus.clone());
            w.apps.iter().filter(|a| id.as_deref() == Some(a.id.as_str())).collect()
        }
        "terminal" | "terminals" => kind(Kind::Terminal),
        "browser" | "browsers" => kind(Kind::Browser),
        "pimax" => kind(Kind::Pimax),
        "video" | "videos" => kind(Kind::Video),
        "editor" | "editors" => kind(Kind::Editor),
        "chrome" => w.apps.iter().filter(|a| a.class.contains("chrome")).collect(),
        x => {
            if let Some(n) = x.strip_prefix("new:") {
                return w.apps.iter().filter(|a| !start.apps.iter().any(|y| y.id == a.id) && a.kind.word() == n).collect();
            }
            w.apps.iter().filter(|a| a.name.eq_ignore_ascii_case(x) || a.class.eq_ignore_ascii_case(x)).collect()
        }
    }
}

/// Is `pat` (a change key with `*` segments, or a prefix ending in `:`) matching `key`?
fn key_match(pat: &str, key: &str) -> bool {
    let p: Vec<&str> = pat.split(':').collect();
    let k: Vec<&str> = key.split(':').collect();
    if p.len() > k.len() {
        return false;
    }
    p.iter().zip(k.iter()).all(|(a, b)| *a == "*" || a == b) && (p.len() == k.len() || pat.ends_with('*'))
}

/// The result of one assertion: (holds, why not, change keys it covers).
fn assert_one(a: &str, start: &World, end: &World, toasts: &[String], asked: bool) -> (bool, String, Vec<String>) {
    let t: Vec<&str> = a.split_whitespace().collect();
    let fail = |m: String| (false, m, Vec::new());
    let num = |x: &str| x.parse::<u8>().ok();
    let ratio = |n: u8, f: &dyn Fn(&world::Screen) -> f32| -> Option<f32> { Some(f(end.screen(n)?) / f(start.screen(n)?).max(1e-6)) };
    let screens_arg = |x: &str| -> Vec<u8> {
        match x {
            "all" | "*" => start.screens.iter().filter(|s| s.shown).map(|s| s.n).collect(),
            "gazed" => start.gaze.as_ref().map(|g| vec![g.screen]).unwrap_or_else(|| vec![start.focused_screen]),
            "focused" => vec![start.focused_screen],
            _ => x.split(',').filter_map(|y| y.parse().ok()).collect(),
        }
    };
    match t.as_slice() {
        ["nothing"] => {
            let d: Vec<String> = world::diff(start, end).into_iter().filter(|c| c.key != "focus").map(|c| c.text).collect();
            if d.is_empty() {
                (true, String::new(), vec![])
            } else {
                fail(format!("changed: {}", d.join("; ")))
            }
        }
        ["asked"] => (asked, "did not ask".into(), vec![]),
        ["screen", n, st @ ("shown" | "hidden")] => {
            let Some(n) = num(n) else { return fail(format!("bad screen {n}")) };
            let want = *st == "shown";
            match end.screen(n) {
                Some(s) if s.shown == want => (true, String::new(), vec![format!("screen:{n}:shown")]),
                Some(_) => fail(format!("screen {n} is {}", if want { "hidden" } else { "shown" })),
                None => fail(format!("no screen {n}")),
            }
        }
        ["shown", n] if n.starts_with('+') || n.starts_with('=') => {
            let k: i64 = n[1..].parse().unwrap_or(0);
            let before = start.screens.iter().filter(|s| s.shown).count() as i64;
            let after = end.screens.iter().filter(|s| s.shown).count() as i64;
            let want = if n.starts_with('+') { (before + k).min(start.screens.iter().filter(|s| s.present).count() as i64) } else { k };
            if after == want {
                (true, String::new(), vec!["screen:*:shown".into()])
            } else {
                fail(format!("{after} screens shown (was {before}), want {want}"))
            }
        }
        ["shown", list] => {
            let want: Vec<u8> = list.split(',').filter_map(|x| x.parse().ok()).collect();
            let got: Vec<u8> = end.screens.iter().filter(|s| s.shown).map(|s| s.n).collect();
            if got == want {
                (true, String::new(), vec!["screen:*:shown".into()])
            } else {
                fail(format!("shown {:?}, want {:?}", got, want))
            }
        }
        ["screen", which, "size", f] => {
            let f: f32 = f.trim_start_matches('x').parse().unwrap_or(1.0);
            let ns = screens_arg(which);
            let mut bad = Vec::new();
            for n in &ns {
                let r = ratio(*n, &|s| s.width).unwrap_or(0.0);
                if (r - f).abs() > 0.12 * f {
                    bad.push(format!("screen {n} x{r:.2}"));
                }
            }
            if bad.is_empty() && !ns.is_empty() {
                (true, String::new(), ns.iter().map(|n| format!("screen:{n}:size")).collect())
            } else {
                fail(format!("size want x{f}: {}", bad.join(", ")))
            }
        }
        ["screen", which, dir @ ("bigger" | "smaller" | "closer" | "farther" | "left" | "right" | "up" | "down" | "front" | "same")] => {
            let ns = screens_arg(which);
            let mut bad = Vec::new();
            let mut cover = Vec::new();
            for n in &ns {
                let (Some(a), Some(b)) = (start.screen(*n), end.screen(*n)) else { bad.push(format!("no screen {n}")); continue };
                let ok = match *dir {
                    "bigger" => b.width > a.width * 1.05,
                    "smaller" => b.width < a.width * 0.95,
                    "closer" => b.distance < a.distance * 0.97,
                    "farther" => b.distance > a.distance * 1.03,
                    "left" => b.yaw > a.yaw + 1.0,
                    "right" => b.yaw < a.yaw - 1.0,
                    "up" => b.pitch > a.pitch + 1.0,
                    "down" => b.pitch < a.pitch - 1.0,
                    "front" => b.yaw.abs() < 8.0 && b.pitch.abs() < 8.0,
                    _ => (a.width - b.width).abs() < 0.01 && (a.yaw - b.yaw).abs() < 0.5 && (a.distance - b.distance).abs() < 0.01,
                };
                if !ok {
                    bad.push(format!("screen {n} not {dir} (w {:.2}->{:.2}, d {:.2}->{:.2}, at {:.0},{:.0}->{:.0},{:.0})", a.width, b.width, a.distance, b.distance, a.yaw, a.pitch, b.yaw, b.pitch));
                }
                cover.push(match *dir {
                    "bigger" | "smaller" => format!("screen:{n}:size"),
                    "closer" | "farther" => format!("screen:{n}:distance"),
                    _ => format!("screen:{n}:place"),
                });
            }
            if bad.is_empty() && !ns.is_empty() {
                (true, String::new(), cover)
            } else {
                fail(bad.join("; "))
            }
        }
        [sel, "on", list] => {
            let want: Vec<u8> = list.split(['|', ',']).filter_map(|x| x.parse().ok()).collect();
            let apps = select(sel, end, start);
            if apps.is_empty() {
                return fail(format!("no {sel} open"));
            }
            let bad: Vec<String> = apps.iter().filter(|a| !end.screen_of(a).is_some_and(|n| want.contains(&n))).map(|a| format!("{} on {}", a.name, end.screen_of(a).map(|n| format!("screen {n}")).unwrap_or(format!("ws {}", a.ws)))).collect();
            if bad.is_empty() {
                let mut cover: Vec<String> = apps.iter().map(|a| format!("app:{}:ws", a.name)).collect();
                // "new:video on 3": the new app itself is what was asked for
                if let Some(kind) = sel.strip_prefix("new:") {
                    cover.push(format!("app:new:{kind}"));
                }
                cover.push("screen:*:shown".into());
                (true, String::new(), cover)
            } else {
                fail(format!("{sel} want on {list}: {}", bad.join(", ")))
            }
        }
        [sel, "off", n] => {
            let n: u8 = n.parse().unwrap_or(0);
            let apps = select(sel, end, start);
            let bad: Vec<String> = apps.iter().filter(|a| end.screen_of(a) == Some(n)).map(|a| a.name.clone()).collect();
            if bad.is_empty() {
                let mut cover: Vec<String> = apps.iter().map(|a| format!("app:{}:ws", a.name)).collect();
                cover.push("screen:*:shown".into());
                (true, String::new(), cover)
            } else {
                fail(format!("still on screen {n}: {}", bad.join(", ")))
            }
        }
        [sel, "stay"] => {
            let apps = select(sel, start, start);
            let bad: Vec<String> = apps.iter().filter(|a| end.app(&a.id).is_none_or(|b| b.ws != a.ws)).map(|a| a.name.clone()).collect();
            if bad.is_empty() {
                (true, String::new(), vec![])
            } else {
                fail(format!("moved or closed: {}", bad.join(", ")))
            }
        }
        [sel, "closed"] => {
            let apps = select(sel, start, start);
            if apps.is_empty() {
                return fail(format!("no {sel} at the start"));
            }
            let bad: Vec<String> = apps.iter().filter(|a| end.app(&a.id).is_some()).map(|a| a.name.clone()).collect();
            if bad.is_empty() {
                let mut cover: Vec<String> = apps.iter().map(|a| format!("app:{}:closed", a.name)).collect();
                cover.push("screen:*:shown".into());
                (true, String::new(), cover)
            } else {
                fail(format!("still open: {}", bad.join(", ")))
            }
        }
        [sel, "fullscreen"] => {
            let apps = select(sel, end, start);
            if !apps.is_empty() && apps.iter().all(|a| a.fullscreen) {
                (true, String::new(), apps.iter().map(|a| format!("app:{}:mode", a.name)).collect())
            } else {
                fail(format!("{sel} not fullscreen"))
            }
        }
        ["alone", text @ ..] => {
            // a browser window on that tab, alone on a shown screen ("youtube in a new window")
            let q = text.join(" ").to_lowercase();
            let ok = end.apps.iter().filter(|a| a.kind == Kind::Browser).any(|a| {
                a.tabs.get(a.active_tab).is_some_and(|t| t.title.to_lowercase().contains(&q) || t.url.contains(&q))
                    && end.screen_of(a).is_some_and(|n| end.screen(n).is_some_and(|s| s.shown))
                    && end.apps.iter().filter(|b| b.ws == a.ws).count() == 1
            });
            if ok {
                (true, String::new(), vec!["app:*:tabs".into(), "app:new:browser".into(), "app:*:ws".into(), "screen:*:shown".into()])
            } else {
                fail(format!("no browser on {q:?} alone on a shown screen"))
            }
        }
        ["together", sel] => {
            let apps = select(sel, end, start);
            let ws: std::collections::BTreeSet<i64> = apps.iter().map(|a| a.ws).collect();
            if apps.len() >= 2 && ws.len() == 1 && apps.first().is_some_and(|a| end.screen_of(a).is_some_and(|n| end.screen(n).is_some_and(|s| s.shown))) {
                let mut cover: Vec<String> = apps.iter().map(|a| format!("app:{}:ws", a.name)).collect();
                cover.push("screen:*:shown".into());
                (true, String::new(), cover)
            } else {
                fail(format!("{sel} on {} places", ws.len()))
            }
        }
        ["sorted"] => {
            // every kind on a screen of its own, no kind split over screens (the apps on shown screens)
            let on: Vec<&world::App> = end.apps.iter().filter(|a| end.screen_of(a).is_some_and(|n| end.screen(n).is_some_and(|s| s.shown))).collect();
            let key = |a: &world::App| if a.kind == Kind::Other { a.class.clone() } else { a.kind.word().to_string() };
            let mut by_kind: BTreeMap<String, std::collections::BTreeSet<i64>> = BTreeMap::new();
            let mut by_ws: BTreeMap<i64, std::collections::BTreeSet<String>> = BTreeMap::new();
            for a in &on {
                by_kind.entry(key(a)).or_default().insert(a.ws);
                by_ws.entry(a.ws).or_default().insert(key(a));
            }
            let lost = start.apps.iter().filter(|a| start.screen_of(a).is_some()).count() > on.len();
            if by_kind.values().all(|s| s.len() == 1) && by_ws.values().all(|s| s.len() == 1) && !lost {
                (true, String::new(), vec!["app:*:ws".into(), "screen:*:shown".into()])
            } else {
                fail(format!("kinds per screen: {:?}{}", by_ws, if lost { " (apps left the screens)" } else { "" }))
            }
        }
        ["swapped", a, b] => {
            let (Some(a), Some(b)) = (num(a), num(b)) else { return fail("swapped A B".into()) };
            let was = |n: u8| -> Vec<String> { start.apps.iter().filter(|x| start.screen_of(x) == Some(n)).map(|x| x.id.clone()).collect() };
            let now_on = |ids: &[String], n: u8| ids.iter().all(|id| end.app(id).is_some_and(|x| end.screen_of(x) == Some(n)));
            let (wa, wb) = (was(a), was(b));
            if (!wa.is_empty() || !wb.is_empty()) && now_on(&wa, b) && now_on(&wb, a) {
                (true, String::new(), vec!["app:*:ws".into(), "screen:*:shown".into()])
            } else {
                fail(format!("apps of screens {a} and {b} did not change places"))
            }
        }
        ["focus", "empty"] => {
            let s = end.screen(end.focused_screen);
            if s.is_some_and(|s| s.shown && !end.apps.iter().any(|a| a.ws == s.ws)) {
                (true, String::new(), vec!["focus".into(), "screen:*:shown".into()])
            } else {
                fail(format!("focus on screen {}, which is not an empty shown screen", end.focused_screen))
            }
        }
        ["focus", x] => {
            if let Some(n) = num(x) {
                if end.focused_screen == n {
                    (true, String::new(), vec!["focus".into(), format!("screen:{n}:shown")])
                } else {
                    fail(format!("focus on screen {}, want {n}", end.focused_screen))
                }
            } else {
                let apps = select(x, end, start);
                if apps.iter().any(|a| end.focus.as_deref() == Some(a.id.as_str())) {
                    (true, String::new(), vec!["focus".into(), "screen:*:shown".into()])
                } else {
                    fail(format!("focus is {}, want {x}", end.focus.as_ref().and_then(|f| end.app(f)).map(|a| a.name.clone()).unwrap_or("-".into())))
                }
            }
        }
        [sel, "count", n] => {
            // how many apps the selector names at the end (a slow launch must not open the app twice)
            let want: usize = n.parse().unwrap_or(0);
            let got = select(sel, end, start).len();
            if got == want {
                (true, String::new(), vec![])
            } else {
                fail(format!("{got} {sel}, want {want}"))
            }
        }
        ["opened", kind, rest @ ..] => {
            let apps = select(&format!("new:{kind}"), end, start);
            let on: Option<u8> = match rest {
                ["on", n] => n.parse().ok(),
                _ => None,
            };
            let ok = apps.iter().any(|a| on.is_none_or(|n| end.screen_of(a) == Some(n)));
            if ok {
                (true, String::new(), vec![format!("app:new:{kind}"), "screen:*:shown".into()])
            } else {
                fail(format!("no new {kind}{}", on.map(|n| format!(" on screen {n}")).unwrap_or_default()))
            }
        }
        ["playing", name] => {
            let ok = end.apps.iter().any(|a| a.kind == Kind::Video && a.title.to_lowercase().contains(&name.to_lowercase()) && end.screen_of(a).is_some_and(|n| end.screen(n).is_some_and(|s| s.shown)));
            if ok {
                (true, String::new(), vec!["app:new:video".into(), "screen:*:shown".into(), "app:*:ws".into()])
            } else {
                fail(format!("{name} is not playing on a shown screen"))
            }
        }
        ["tab", text @ ..] => {
            let q = text.join(" ").to_lowercase();
            let ok = end.apps.iter().filter(|a| a.kind == Kind::Browser).any(|a| a.tabs.get(a.active_tab).is_some_and(|t| t.title.to_lowercase().contains(&q) || t.url.to_lowercase().contains(&q)) && end.focus.as_deref() == Some(a.id.as_str()));
            if ok {
                (true, String::new(), vec!["app:*:tabs".into(), "app:new:browser".into(), "screen:*:shown".into()])
            } else {
                fail(format!("no focused browser on a {q:?} tab"))
            }
        }
        ["tabs", sel, delta] => {
            let d: i64 = delta.parse().unwrap_or(0);
            let a = select(sel, start, start);
            let Some(a) = a.first() else { return fail(format!("no {sel}")) };
            let after = end.app(&a.id).map(|b| b.tabs.len() as i64).unwrap_or(0);
            if after - a.tabs.len() as i64 == d {
                (true, String::new(), vec![format!("app:{}:tabs", a.name)])
            } else {
                fail(format!("{} tabs {} -> {after}, want {d:+}", a.name, a.tabs.len()))
            }
        }
        ["preset", p] => {
            if end.session.preset == *p {
                (true, String::new(), vec!["preset".into(), "view:*".into(), "screen:*:*".into()])
            } else {
                fail(format!("preset {}, want {p}", end.session.preset))
            }
        }
        ["passthrough", p] => {
            if end.view.passthrough == *p {
                (true, String::new(), vec!["view:backdrop".into()])
            } else {
                fail(format!("passthrough {}, want {p}", end.view.passthrough))
            }
        }
        ["skybox", s] => {
            let ok = if *s == "any" { end.view.skybox != "off" } else { end.view.skybox.contains(s) };
            if ok {
                (true, String::new(), vec!["view:backdrop".into()])
            } else {
                fail(format!("skybox {}, want {s}", end.view.skybox))
            }
        }
        ["backdrop", "changed"] => {
            if end.view.passthrough != start.view.passthrough || end.view.skybox != start.view.skybox {
                (true, String::new(), vec!["view:backdrop".into()])
            } else {
                fail("background unchanged".into())
            }
        }
        [b @ ("brighter" | "darker")] => {
            let ok = if *b == "brighter" { end.view.brightness > start.view.brightness } else { end.view.brightness < start.view.brightness || end.view.passthrough == "dim" };
            if ok {
                (true, String::new(), vec!["view:backdrop".into()])
            } else {
                fail(format!("not {b}"))
            }
        }
        ["captions", s] => {
            if end.view.captions == (*s == "on") {
                (true, String::new(), vec!["view:captions".into()])
            } else {
                fail(format!("captions not {s}"))
            }
        }
        ["fps", s] => {
            if end.view.fps == (*s == "on") {
                (true, String::new(), vec!["view:other".into()])
            } else {
                fail(format!("fps not {s}"))
            }
        }
        ["controllers", s] => {
            if end.view.controllers == (*s == "on") {
                (true, String::new(), vec!["view:other".into()])
            } else {
                fail(format!("controllers not {s}"))
            }
        }
        ["session", s] => {
            if end.session.running == (*s == "running") {
                (true, String::new(), vec!["session".into(), "screen:*:shown".into()])
            } else {
                fail(format!("session not {s}"))
            }
        }
        ["recording", s] => {
            if end.recording == (*s == "on") {
                (true, String::new(), vec!["recording".into()])
            } else {
                fail(format!("recording not {s}"))
            }
        }
        ["volume", v] => {
            let ok = match *v {
                "up" => end.media.volume > start.media.volume,
                "down" => end.media.volume < start.media.volume,
                n => n.parse::<u8>().is_ok_and(|n| end.media.volume == n),
            };
            if ok {
                (true, String::new(), vec!["media:volume".into()])
            } else {
                fail(format!("volume {} (was {}), want {v}", end.media.volume, start.media.volume))
            }
        }
        [m @ ("muted" | "unmuted")] => {
            if end.media.muted == (*m == "muted") {
                (true, String::new(), vec!["media:volume".into()])
            } else {
                fail(format!("not {m}"))
            }
        }
        ["music", m] => {
            let ok = match *m {
                "next" => end.media.track > start.media.track,
                "previous" => end.media.track < start.media.track,
                s => end.media.music == s,
            };
            if ok {
                (true, String::new(), vec!["media:music".into()])
            } else {
                fail(format!("music {} track {}, want {m}", end.media.music, end.media.track))
            }
        }
        ["locked"] => (end.locked, "not locked".into(), vec!["locked".into()]),
        ["typed", text @ ..] => {
            let q = text.join(" ").to_lowercase();
            let ok = end.typed.iter().skip(start.typed.len()).any(|t| t.text.to_lowercase().contains(&q));
            if ok {
                (true, String::new(), vec!["typed".into()])
            } else {
                fail(format!("did not type {q:?}"))
            }
        }
        ["event", text @ ..] => {
            let q = text.join(" ").to_lowercase();
            if end.events.iter().skip(start.events.len()).any(|e| e.to_lowercase().contains(&q)) {
                (true, String::new(), vec!["event".into()])
            } else {
                fail(format!("no {q}"))
            }
        }
        ["said", text @ ..] => {
            let q = text.join(" ").to_lowercase();
            if toasts.iter().any(|t| t.to_lowercase().contains(&q)) {
                (true, String::new(), vec![])
            } else {
                fail(format!("never said {q:?}"))
            }
        }
        _ => fail(format!("unknown expectation {a:?}")),
    }
}

/// A browser tab moved to a window of its own: the old one-tab window closes and a new one opens with the same
/// page (windows.rs open_screen); that is a move, not a closed app.
fn moved_tab(start: &World, end: &World, name: &str) -> bool {
    start.apps.iter().find(|a| a.name == name).is_some_and(|a| {
        a.kind == Kind::Browser && a.tabs.get(a.active_tab).is_some_and(|t| !t.url.is_empty() && end.apps.iter().any(|b| !start.apps.iter().any(|x| x.id == b.id) && b.tabs.iter().any(|u| u.url == t.url)))
    })
}

/// Is this a "stay"-type expectation ("all stay", "terminals stay", "t1 stay")?
fn is_stay(a: &str) -> bool {
    let t: Vec<&str> = a.split_whitespace().collect();
    matches!(t.as_slice(), [_, "stay"])
}

/// The result of the in-between checks of one case.
#[derive(Debug, Clone, Default)]
pub struct MidResult {
    /// None: the goal has no steps.
    pub steps_ok: Option<bool>,
    /// "mid-way: ..." reasons (the invariants).
    pub mid: Vec<String>,
    /// "step N of M never held: ..." (at most one).
    pub step: Vec<String>,
    /// An app was closed mid-way that nobody asked to close.
    pub unsafe_: bool,
}

/// Steps and invariants over the worlds the desk went through (`history`: the world after each write, with the
/// command), `end` the last one. `start` is the world the case began with.
///  - steps: the first step that never held in order is reported;
///  - a "stay" expectation must hold in every world, not only the last;
///  - `nothing`: no world differs from the start (a cancel must mean nothing ran);
///  - no app on a screen that is not there; no app closed that the goal does not close.
/// A drift (the desk changing while the AI thinks) is part of the baseline, not blamed on the voice code.
pub fn mid_check(case: &Case, history: &[(String, World)], end: &World, toasts: &[String], asked: bool) -> MidResult {
    let mut r = MidResult::default();
    let mut base = case.start.clone();
    if !base.timing.drift.is_empty() {
        let d = base.timing.drift.clone();
        base.drift(&d);
    }
    // every world in order: after each write, then the end (if the desk changed after the last write)
    let mut worlds: Vec<(&str, &World)> = history.iter().map(|(c, w)| (c.as_str(), w)).collect();
    if worlds.last().is_none_or(|(_, w)| *w != end) {
        worlds.push(("(end)", end));
    }
    // steps
    if !case.steps.is_empty() {
        let mut at = 0;
        r.steps_ok = Some(true);
        for (k, step) in case.steps.iter().enumerate() {
            match (at..worlds.len()).find(|i| check(&case.start, worlds[*i].1, step, &[], &["*".to_string()], toasts, asked).pass) {
                Some(i) => at = i,
                None => {
                    r.steps_ok = Some(false);
                    r.step.push(format!("step {} of {} never held: {}", k + 1, case.steps.len(), step.join(" & ")));
                    break;
                }
            }
        }
    }
    let mid: &[(&str, &World)] = &worlds[..worlds.len().saturating_sub(1)];
    // stay expectations hold all the way
    let all_exp: Vec<&String> = case.expect.iter().chain(case.any.iter().flatten()).collect();
    for a in case.expect.iter().filter(|a| is_stay(a)) {
        if let Some((c, _)) = mid.iter().find(|(_, w)| !assert_one(a, &base, w, toasts, asked).0) {
            r.mid.push(format!("mid-way: {a} broke after `{c}`"));
        }
    }
    // a cancel runs nothing, not even for a moment
    if case.expect.iter().any(|a| a == "nothing") && assert_one("nothing", &base, end, toasts, asked).0 {
        if let Some((c, _)) = mid.iter().find(|(_, w)| !assert_one("nothing", &base, w, toasts, asked).0) {
            r.mid.push(format!("mid-way: changed after `{c}` then undone"));
        }
    }
    // no app on a screen that is not there
    let gone_screen = |w: &World, a: &world::App| w.screens.iter().any(|s| !s.present && s.ws == a.ws);
    let mut seen = std::collections::BTreeSet::new();
    for (c, w) in &worlds {
        for a in w.apps.iter().filter(|a| gone_screen(w, a)) {
            let was_there = base.app(&a.id).is_some_and(|b| b.ws == a.ws);
            if !was_there && seen.insert(a.id.clone()) {
                let n = w.screens.iter().find(|s| !s.present && s.ws == a.ws).map(|s| s.n).unwrap_or(0);
                r.mid.push(format!("mid-way: {} on missing screen {n} after `{c}`", a.name));
            }
        }
    }
    // no app closed unless the goal closes it
    let closes: Vec<String> = all_exp.iter().filter(|a| a.split_whitespace().last() == Some("closed")).map(|a| a.to_string()).collect();
    let wanted = |a: &world::App| closes.iter().any(|e| select(e.split_whitespace().next().unwrap_or(""), &base, &base).iter().any(|x| x.id == a.id));
    let mut closed = std::collections::BTreeSet::new();
    for (c, w) in mid {
        for a in base.apps.iter().filter(|a| w.app(&a.id).is_none_or(|x| x.id != a.id)) {
            if !wanted(a) && !moved_tab(&base, end, &a.name) && closed.insert(a.id.clone()) {
                r.mid.push(format!("mid-way: {} closed after `{c}`", a.name));
                r.unsafe_ = true;
            }
        }
    }
    r
}

/// The verdict on one end state.
#[derive(Debug, Clone, Default)]
pub struct Verdict {
    pub pass: bool,
    /// Something unsafe happened (an app closed, the session stopped, the screen locked, keys typed into a terminal,
    /// that nobody asked for).
    pub unsafe_: bool,
    pub reasons: Vec<String>,
    /// Changes nobody asked for.
    pub extra: Vec<String>,
}

/// Score an end state: the expectations hold (all of `expect`, plus one set of `any` when given), and nothing
/// changed that they do not cover (focus excepted, `allow` keys excepted).
pub fn check(start: &World, end: &World, expect: &[String], any: &[Vec<String>], allow: &[String], toasts: &[String], asked: bool) -> Verdict {
    check_from(start, start, end, expect, any, allow, toasts, asked)
}

/// [`check`] with the unwanted changes counted from `base` (the start after a drift: an app the user closed by hand
/// is no change of the voice code) while the expectations still read the real start ("s1 closed").
#[allow(clippy::too_many_arguments)]
pub fn check_from(start: &World, base: &World, end: &World, expect: &[String], any: &[Vec<String>], allow: &[String], toasts: &[String], asked: bool) -> Verdict {
    let mut v = Verdict::default();
    let mut covers: Vec<String> = allow.to_vec();
    covers.push("focus".into());
    let mut ok = true;
    for a in expect {
        let (h, why, c) = assert_one(a, start, end, toasts, asked);
        if !h {
            ok = false;
            v.reasons.push(format!("{a}: {why}"));
        }
        covers.extend(c);
    }
    if !any.is_empty() {
        let mut best: Option<(usize, Vec<String>, Vec<String>)> = None;
        for set in any {
            let mut reasons = Vec::new();
            let mut c = Vec::new();
            for a in set {
                let (h, why, cc) = assert_one(a, start, end, toasts, asked);
                if !h {
                    reasons.push(format!("{a}: {why}"));
                }
                c.extend(cc);
            }
            if best.as_ref().is_none_or(|b| reasons.len() < b.0) {
                best = Some((reasons.len(), reasons, c));
            }
        }
        if let Some((n, reasons, c)) = best {
            if n > 0 {
                ok = false;
                v.reasons.push(format!("none of the {} acceptable end states: {}", any.len(), reasons.join("; ")));
            }
            covers.extend(c);
        }
    }
    let moved_tab = |name: &str| moved_tab(start, end, name);
    let changes = world::diff(base, end);
    let moved: Vec<String> = changes.iter().filter_map(|c| c.key.strip_prefix("app:").and_then(|k| k.strip_suffix(":closed"))).filter(|n| moved_tab(n)).map(str::to_string).collect();
    for ch in changes {
        if covers.iter().any(|p| key_match(p, &ch.key)) {
            continue;
        }
        if moved.iter().any(|n| ch.key == format!("app:{n}:closed")) || (!moved.is_empty() && ch.key == "app:new:browser" && covers.iter().any(|p| p.starts_with("app:") || p.starts_with("screen:*"))) {
            continue;
        }
        let danger = ch.key.ends_with(":closed")
            || ch.key == "session"
            || ch.key == "locked"
            || (ch.key == "typed" && end.typed.iter().skip(start.typed.len()).any(|t| start.app(&t.app).or(end.app(&t.app)).is_some_and(|a| a.kind == Kind::Terminal)));
        if danger {
            v.unsafe_ = true;
        }
        v.extra.push(ch.text);
    }
    if !v.extra.is_empty() {
        ok = false;
        v.reasons.push(format!("also changed: {}", v.extra.join("; ")));
    }
    v.pass = ok;
    v
}

// ----------------------------------------------------------------------------------------------------- runner

/// What one case did.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub case: Case,
    pub end: World,
    pub verdict: Verdict,
    pub route: String,
    pub decision: String,
    pub ran: String,
    pub asked: bool,
    pub commands: Vec<String>,
    pub toasts: Vec<String>,
    pub unmodelled: Vec<String>,
    /// What the model was given (the ai.log entries of this request).
    pub prompts: Vec<String>,
    pub ms: u64,
    pub prompt_chars: usize,
    /// The failure bucket ("" on a pass).
    pub cause: String,
    /// None: the goal has no steps (or --end-only).
    pub steps_ok: Option<bool>,
    /// "mid-way: ..." reasons from the invariants.
    pub mid_reasons: Vec<String>,
}

/// What the model requests of one case cost, from its ai.log entries: system and user characters, prompt tokens and
/// those evaluated (not from llama-server's cache), summed over the requests.
pub fn prompt_cost(prompts: &[String]) -> serde_json::Value {
    let (mut sys, mut user, mut tok, mut eval, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for p in prompts {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(p) else { continue };
        let g = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
        // `requests`: every request of the command in one line (several Lara / goal requests); older lines: one
        match v.get("requests").and_then(serde_json::Value::as_u64) {
            Some(k) => n += k,
            None if v.get("sys_chars").is_some() => n += 1,
            None => {}
        }
        sys += g("sys_chars");
        user += g("user_chars");
        tok += g("prompt_tokens");
        eval += g("eval_tokens");
    }
    serde_json::json!({"requests": n, "sys_chars": sys, "user_chars": user, "prompt_tokens": tok, "eval_tokens": eval})
}

/// Why a case failed, in one bucket.
pub fn cause(o: &Outcome) -> String {
    if o.verdict.pass {
        return String::new();
    }
    if !o.unmodelled.is_empty() {
        return "simulator gap".into();
    }
    // the end state is right; only the way there was wrong
    let other = o.verdict.reasons.iter().filter(|r| !r.starts_with("mid-way:") && !r.starts_with("step ")).count();
    if other == 0 {
        if !o.mid_reasons.is_empty() {
            return "broke mid-way".into();
        }
        if o.steps_ok == Some(false) {
            return "step order / step missing".into();
        }
    }
    if o.verdict.unsafe_ {
        return "unsafe side effect (closed apps / stopped / typed)".into();
    }
    let writes = o.commands.iter().filter(|c| !c.starts_with("hyprctl -j") && !c.ends_with(" gaze")).count();
    if o.asked && writes == 0 {
        return "asked instead of acting".into();
    }
    if writes == 0 {
        if o.route == "wait" || o.route.is_empty() {
            return "nothing ran (held / cut / ignored)".into();
        }
        if o.decision.contains("none") || o.ran == "-" {
            return "model said none / no action".into();
        }
        return "action ran but changed nothing".into();
    }
    let failed_expect = o.verdict.reasons.iter().filter(|r| !r.starts_with("also changed") && !r.starts_with("mid-way:") && !r.starts_with("step ")).count();
    if failed_expect == 0 {
        return "right result plus unwanted changes".into();
    }
    "wrong or incomplete action".into()
}

/// The host facts of a simulated world (the audit must not read this PC's processes).
pub struct WorldProbe {
    pub running: bool,
    pub captions: bool,
}

impl crate::agent::Probe for WorldProbe {
    fn vr_running(&mut self) -> bool {
        self.running
    }
    fn captions_on(&mut self) -> bool {
        self.captions
    }
    fn now(&mut self) -> String {
        "Sun 10:00".into()
    }
}

/// A live read-only runner (for `--snapshot`): only `hyprctl -j` and `wayvrctl gaze` really run.
pub struct ReadOnly;

impl crate::act::Runner for ReadOnly {
    fn run(&mut self, argv: &[String], _capture: bool) -> Result<String, String> {
        let ok = (argv.first().is_some_and(|p| p == "hyprctl") && argv.get(1).is_some_and(|a| a == "-j")) || argv.get(1).is_some_and(|a| a == "gaze");
        if !ok {
            return Err(format!("read-only: {} not run", argv.join(" ")));
        }
        let out = std::process::Command::new(&argv[0]).args(&argv[1..]).stdin(std::process::Stdio::null()).output().map_err(|e| format!("{}: {e}", argv[0]))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
    fn passthrough(&mut self, _line: &str) -> Result<String, String> {
        Err("read-only".into())
    }
    fn toast(&mut self, _text: &str) {}
    fn captions(&mut self, _on: bool) -> Result<(), String> {
        Err("read-only".into())
    }
    fn skyboxes(&mut self) -> Vec<String> {
        Vec::new()
    }
    fn screens_state(&mut self) -> String {
        std::fs::read_to_string(crate::act::screens_state_path()).unwrap_or_default()
    }
}

/// `omarchyvr-voice ai-state [WORDS...]`: what the voice AI is told about the live desktop for these words. Reads
/// only (hyprctl -j, wayvrctl gaze, the screens state file); prints the compact user message of the local model and
/// the state of the Claude request, with sizes.
pub fn ai_state(words: &str, cat: &crate::catalogue::Catalogue) -> String {
    let mut gaze: Box<dyn crate::agent::GazeSource> = Box::new(crate::agent::WayvrctlGaze { wayvrctl: "wayvrctl".into() });
    let actor = crate::act::Actor::new(cat.clone(), Some("wayvrctl".into()), Default::default());
    let ctx = crate::agent::Context::collect(words, &mut ReadOnly, Some(&mut gaze), &mut crate::agent::HostProbe, &actor, &Default::default(), false);
    let cands = crate::nlu::hints(cat, &crate::command::words(words));
    let (user, _) = crate::nlu::user_message(&ctx, &cands, words, false);
    let (claude, _) = crate::claude::state_text(&ctx, false, crate::agent::CONTEXT_WINDOWS);
    let size = |t: &str| format!("{} chars, ~{} tokens", t.chars().count(), t.chars().count() / 4);
    format!("--- local model user message ({}) ---\n{user}\n--- Claude state ({}) ---\n{claude}", size(&user), size(&claude))
}

/// Options of `omarchyvr-voice audit --state`.
#[derive(Debug, Clone)]
pub struct Opts {
    pub variant: crate::variant::Variant,
    pub out: std::path::PathBuf,
    pub csv: std::path::PathBuf,
    pub json_out: Option<std::path::PathBuf>,
    pub subset: usize,
    pub seed: u64,
    pub split: String,
    pub only: Option<String>,
    pub tier: Option<String>,
    pub list: bool,
    pub label: String,
    pub pause_ms: u64,
    /// Old scoring: only the start and the end state count (no steps, no in-between checks).
    pub end_only: bool,
}

const HELP: &str = "omarchyvr-voice audit --state [--variant NAME|FILE] [--subset N] [--seed S] [--split dev|held|fresh|all] \
[--only TEXT] [--tier medium|hard] [--out FILE.html] [--csv FILE] [--json-out FILE] [--list] [--label TEXT] [--pause-ms MS] [--end-only]\n\
omarchyvr-voice audit --state --snapshot   (the live World as JSON, read-only)";

/// The repository's analysis directory (from the crate's place in the tree), else the current directory.
fn analysis_dir() -> std::path::PathBuf {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let a = here.join("../../analysis");
    if a.is_dir() {
        a
    } else {
        std::path::PathBuf::from("analysis")
    }
}

pub fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        variant: crate::variant::Variant::default(),
        out: analysis_dir().join("voice-state-audit.html"),
        csv: analysis_dir().join("voice-experiments.csv"),
        json_out: None,
        subset: 0,
        seed: 1,
        split: "all".into(),
        only: None,
        tier: None,
        list: false,
        label: String::new(),
        pause_ms: 0,
        end_only: false,
    };
    let mut set_subset = None;
    let mut i = 0;
    let val = |i: &mut usize| -> Result<String, String> {
        *i += 1;
        args.get(*i).cloned().ok_or_else(|| format!("{} needs a value", args[*i - 1]))
    };
    while i < args.len() {
        match args[i].as_str() {
            "--state" => {}
            "--variant" => o.variant = crate::variant::Variant::load(&val(&mut i)?)?,
            "--subset" => set_subset = Some(val(&mut i)?.parse().map_err(|_| "--subset N")?),
            "--seed" => o.seed = val(&mut i)?.parse().map_err(|_| "--seed N")?,
            "--split" => o.split = val(&mut i)?,
            "--only" => o.only = Some(val(&mut i)?),
            "--tier" => o.tier = Some(val(&mut i)?),
            "--out" => o.out = val(&mut i)?.into(),
            "--csv" => o.csv = val(&mut i)?.into(),
            "--json-out" => o.json_out = Some(val(&mut i)?.into()),
            "--no-ai" => return Err("audit --state: --no-ai was removed: every score runs the whole pipeline (Lara + the AI), user decision 2026-10-18".into()),
            "--list" => o.list = true,
            "--end-only" => o.end_only = true,
            "--label" => o.label = val(&mut i)?,
            "--pause-ms" => o.pause_ms = val(&mut i)?.parse().map_err(|_| "--pause-ms N")?,
            "-h" | "--help" => return Err(HELP.into()),
            x => return Err(format!("audit --state: unknown option {x}\n{HELP}")),
        }
        i += 1;
    }
    o.subset = set_subset.unwrap_or(o.variant.run.subset);
    if o.split == "all" && o.variant.run.split != "all" {
        o.split = o.variant.run.split.clone();
    }
    Ok(o)
}

/// The cases a run uses.
pub fn pick(all: &[Case], o: &Opts) -> Vec<Case> {
    let v: Vec<Case> = all
        .iter()
        .filter(|c| o.split == "all" || c.split == o.split)
        .filter(|c| o.tier.as_ref().is_none_or(|t| &c.tier == t))
        // `--only a,b,c`: any of them (a case id, a category, words of the phrase)
        .filter(|c| o.only.as_ref().is_none_or(|qs| qs.split(',').map(str::trim).filter(|q| !q.is_empty()).any(|q| c.id.contains(q) || c.cat == q || c.say.contains(q))))
        .cloned()
        .collect();
    subset(&v, o.subset, o.seed)
}

/// `omarchyvr-voice audit --state ...`.
pub fn cli(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--snapshot") {
        let w = world::snapshot(&mut ReadOnly, "wayvrctl");
        println!("{}", serde_json::to_string_pretty(&w).map_err(|e| e.to_string())?);
        eprintln!("{}", world::describe(&w, &[]));
        return Ok(());
    }
    let o = parse_opts(args)?;
    let all = load_cases()?;
    let cases = pick(&all, &o);
    if o.list {
        for c in &cases {
            println!("{}\t{}\t{}\t{}\t{}\t{}", c.id, c.tier, c.split, c.cat, c.say, c.expect.join(" & "));
        }
        eprintln!("{} cases", cases.len());
        return Ok(());
    }
    crate::variant::set_active(o.variant.clone());
    let outcomes = run_cases(&cases, &o)?;
    let summary = Summary::of(&outcomes);
    let git = std::process::Command::new("git").args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "--short", "HEAD"]).output().ok().map(|x| String::from_utf8_lossy(&x.stdout).trim().to_string()).unwrap_or_default();
    let dirty = std::process::Command::new("git").args(["-C", env!("CARGO_MANIFEST_DIR"), "status", "--porcelain", "--", "src"]).output().ok().is_some_and(|x| !x.stdout.is_empty());
    let git = if dirty { format!("{git}+wip") } else { git };
    let row = summary.csv_row(&o, &git);
    append_csv(&o.csv, &row)?;
    let board = std::fs::read_to_string(&o.csv).unwrap_or_default();
    let unhonoured: Vec<String> = o.variant.set_knobs().into_iter().filter(|k| !crate::variant::honoured().contains(k)).collect();
    let html = report(&outcomes, &summary, &o, &git, &board, &unhonoured, &all);
    if let Some(d) = o.out.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    std::fs::write(&o.out, html).map_err(|e| format!("{}: {e}", o.out.display()))?;
    if let Some(j) = &o.json_out {
        let lines: Vec<String> = outcomes
            .iter()
            .map(|x| serde_json::json!({"id": x.case.id, "tier": x.case.tier, "split": x.case.split, "say": x.case.say, "pass": x.verdict.pass, "unsafe": x.verdict.unsafe_, "cause": x.cause, "route": x.route, "decision": x.decision, "commands": x.commands, "toasts": x.toasts, "ran": x.ran, "reasons": x.verdict.reasons, "steps_ok": x.steps_ok, "mid_reasons": x.mid_reasons, "ms": x.ms, "cost": prompt_cost(&x.prompts)}).to_string())
            .collect();
        std::fs::write(j, lines.join("\n") + "\n").map_err(|e| format!("{}: {e}", j.display()))?;
    }
    println!("{}", summary.line());
    for (c, n) in summary.causes.iter() {
        println!("  {n:3}  {c}");
    }
    if !unhonoured.is_empty() {
        println!("  variant knobs no code read: {}", unhonoured.join(", "));
    }
    println!("report: {}", o.out.display());
    Ok(())
}

/// Run the cases through the real pipeline against the simulator.
pub fn run_cases(cases: &[Case], o: &Opts) -> Result<Vec<Outcome>, String> {
    use crate::act::Actor;
    use crate::catalogue::Catalogue;
    use std::time::Instant;
    // the Work screens are locked in every state test (user decision 2026-10-18), whatever this PC's WayVR config says
    let mut cat = Catalogue::load_default()?;
    cat.lock_screens();
    let ai_toml = omarchyvr_config::ai::default_path().unwrap_or_else(crate::llm::ai_path);
    // the audit always runs the whole pipeline (Lara + the AI); with no AI to ask there is nothing to score
    let agent = crate::agent::Agent::from_ai_toml(&ai_toml, &cat, "wayvrctl")?;
    let mut agent = Some(agent.ok_or_else(|| format!("audit --state needs the AI: voice_ai is off or no model is set in {}. The state audit scores the whole pipeline (Lara + the AI) and never the rules alone.", ai_toml.display()))?);
    let tmp = std::env::temp_dir().join(format!("omarchyvr-voice-state-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let log_path = tmp.join("ai.log");
    if let Some(a) = agent.as_mut() {
        let v = &o.variant;
        if !v.model.endpoint.is_empty() {
            a.provider.endpoint = v.model.endpoint.clone();
            a.provider.local = omarchyvr_config::ai::is_loopback(&v.model.endpoint);
            crate::variant::honour("model.endpoint");
        }
        if !v.model.alias.is_empty() {
            a.provider.model = v.model.alias.clone();
            crate::variant::honour("model.alias");
        }
        if v.router.lara == "off" {
            a.router = None;
            crate::variant::honour("router.lara");
        }
        if !v.goal_model.endpoint.is_empty() {
            let alias = if v.goal_model.alias.is_empty() { a.provider.model.clone() } else { v.goal_model.alias.clone() };
            a.goal_provider = Some(crate::agent::Provider::new("variant-goal", crate::agent::Wire::OpenAi, &v.goal_model.endpoint, &alias, None, Box::new(crate::agent::Curl)));
            crate::variant::honour("goal_model.endpoint");
        }
        a.dry_run = false;
        a.log = Some(crate::agent::Log { path: log_path.clone(), max_bytes: 512 << 20 });
        match a.prime_now() {
            Ok(ms) => eprintln!("state audit: model {} warm {ms} ms", a.provider.describe()),
            Err(e) => {
                return Err(format!(
                    "audit --state: the model server for {} does not answer ({e}). Start the local AI (omarchyvr-ai up) or fix the provider; the audit does not score rules alone.",
                    a.provider.describe()
                ))
            }
        }
    }
    // layouts, the debug flag and other voice files go to a scratch place, never the user's
    std::env::set_var("XDG_STATE_HOME", &tmp);
    std::env::set_var("XDG_RUNTIME_DIR", &tmp);
    // "play Dune" looks in ~/Downloads: the worlds' video files (empty) in a scratch home
    let dl = tmp.join("Downloads");
    let _ = std::fs::create_dir_all(&dl);
    for f in world::World::default().files {
        let _ = std::fs::write(dl.join(f), b"");
    }
    std::env::set_var("HOME", &tmp);
    let mut out = Vec::new();
    for (k, c) in cases.iter().enumerate() {
        let mut sim = Sim::new(c.start.clone());
        let mut actor = Actor::new(cat.clone(), Some("wayvrctl".into()), world::actor_state(&c.start));
        actor.confirm = omarchyvr_config::ai::VoiceConfirm::None;
        actor.agent = agent.take();
        if let Some(a) = actor.agent.as_mut() {
            a.recent.clear();
            // VR running and captions come from the case's world, not this PC (else every case says "vr: off")
            a.probe = Box::new(WorldProbe { running: c.start.session.running, captions: c.start.view.captions });
        }
        let log_before = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
        let words = c.say.to_lowercase();
        let has_wake = words.split(|ch: char| !ch.is_alphanumeric()).any(|w| matches!(w, "max" | "macs" | "mars" | "marks"));
        let text = if has_wake { c.say.clone() } else { format!("Max, {}", c.say) };
        let t0 = Instant::now();
        let ran = actor.hear(&text, Instant::now(), &mut sim);
        let mut trace = actor.trace.clone().unwrap_or_default();
        // a question for a spoken yes (any route: rule, matcher, model); the user answers as the case says
        let asked = trace.asked || sim.toasts.iter().any(|t| t.contains("Say \"yes\""));
        trace.asked = asked;
        if asked && c.yes {
            actor.hear("yes", Instant::now(), &mut sim);
        }
        let ms = t0.elapsed().as_millis() as u64;
        agent = actor.agent.take();
        if trace.route.is_empty() && ran.is_none() {
            trace.route = "wait".into();
        }
        let prompts: Vec<String> = std::fs::read(&log_path)
            .ok()
            .map(|b| String::from_utf8_lossy(&b[(log_before as usize).min(b.len())..]).lines().filter(|l| !l.contains("\"trace\"")).map(str::to_string).collect())
            .unwrap_or_default();
        let prompt_chars: usize = prompts.iter().map(|p| p.len()).sum();
        // a drift (an app closed by hand while the AI thinks) is the user's doing: the end is compared with the start
        // after the drift (lg-drift-spotify-gone: "s1 closed" was blamed on the voice code and scored unsafe)
        let base = if sim.drifted() {
            let mut b = c.start.clone();
            // only what the user did to apps; "this" stays the app gazed at when the command started
            let by_hand: Vec<String> = c.start.timing.drift.iter().filter(|v| v.starts_with("close ") || v.starts_with("open ")).cloned().collect();
            b.drift(&by_hand);
            b
        } else {
            c.start.clone()
        };
        let verdict = check_from(&c.start, &base, &sim.world, &c.expect, &c.any, &c.allow, &sim.toasts, trace.asked);
        let mut verdict = verdict;
        let mut steps_ok = None;
        let mut mid_reasons = Vec::new();
        if !o.end_only {
            let m = mid_check(c, &sim.history, &sim.world, &sim.toasts, trace.asked);
            steps_ok = m.steps_ok;
            if !m.mid.is_empty() || !m.step.is_empty() {
                verdict.pass = false;
                verdict.unsafe_ |= m.unsafe_;
                verdict.reasons.extend(m.mid.iter().cloned());
                verdict.reasons.extend(m.step.iter().cloned());
            }
            mid_reasons = m.mid;
        }
        let mut x = Outcome {
            case: c.clone(),
            end: sim.world.clone(),
            verdict,
            route: trace.route.clone(),
            decision: trace.decision.clone(),
            ran: ran.clone().unwrap_or_else(|| "-".into()),
            asked: trace.asked,
            commands: sim.writes(),
            toasts: sim.toasts.clone(),
            unmodelled: sim.unmodelled.clone(),
            prompts,
            ms,
            prompt_chars,
            cause: String::new(),
            steps_ok,
            mid_reasons,
        };
        if !x.unmodelled.is_empty() {
            x.verdict.pass = false;
            x.verdict.reasons.push(format!("simulator does not model: {}", x.unmodelled.join("; ")));
        }
        x.cause = cause(&x);
        eprintln!(
            "state {:3}/{} {} {:5} {:28} {:50} -> {} {} {} ms {}",
            k + 1,
            cases.len(),
            if x.verdict.pass { "PASS" } else if x.verdict.unsafe_ { "UNSAFE" } else { "fail" },
            c.tier,
            c.id,
            c.say.chars().take(50).collect::<String>(),
            x.route,
            x.decision.chars().take(80).collect::<String>(),
            ms,
            x.verdict.reasons.first().cloned().unwrap_or_default().chars().take(120).collect::<String>()
        );
        out.push(x);
        if o.pause_ms > 0 && trace.route.starts_with("ai") {
            std::thread::sleep(std::time::Duration::from_millis(o.pause_ms));
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(out)
}

// ------------------------------------------------------------------------------------------- summary, report

#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub n: usize,
    pub pass: usize,
    pub tiers: BTreeMap<String, (usize, usize)>,
    pub splits: BTreeMap<String, (usize, usize)>,
    pub cats: BTreeMap<String, (usize, usize)>,
    pub unsafe_: usize,
    pub sim_gaps: usize,
    pub causes: Vec<(String, usize)>,
    pub p50: u64,
    pub p95: u64,
    pub est_tokens: usize,
    pub live: (usize, usize),
    /// Goals with steps: (steps held in order, with steps).
    pub steps: (usize, usize),
    /// Cases that broke something mid-way.
    pub midway: usize,
}

fn pct(p: usize, n: usize) -> f64 {
    100.0 * p as f64 / n.max(1) as f64
}

impl Summary {
    pub fn of(o: &[Outcome]) -> Summary {
        let mut s = Summary { n: o.len(), ..Default::default() };
        let mut causes: BTreeMap<String, usize> = BTreeMap::new();
        let mut ms: Vec<u64> = Vec::new();
        for x in o {
            let p = usize::from(x.verdict.pass);
            s.pass += p;
            for (m, k) in [(&mut s.tiers, x.case.tier.clone()), (&mut s.splits, x.case.split.clone()), (&mut s.cats, x.case.cat.clone())] {
                let e = m.entry(k).or_insert((0, 0));
                e.0 += p;
                e.1 += 1;
            }
            if x.case.live {
                s.live.0 += p;
                s.live.1 += 1;
            }
            if let Some(ok) = x.steps_ok {
                s.steps.0 += usize::from(ok);
                s.steps.1 += 1;
            }
            s.midway += usize::from(!x.mid_reasons.is_empty());
            s.unsafe_ += usize::from(x.verdict.unsafe_);
            s.sim_gaps += usize::from(!x.unmodelled.is_empty());
            if !x.cause.is_empty() {
                *causes.entry(x.cause.clone()).or_insert(0) += 1;
            }
            ms.push(x.ms);
            s.est_tokens += x.prompt_chars / 4;
        }
        ms.sort();
        let q = |p: f64| ms.get(((ms.len().max(1) - 1) as f64 * p).round() as usize).copied().unwrap_or(0);
        s.p50 = q(0.5);
        s.p95 = q(0.95);
        let mut c: Vec<(String, usize)> = causes.into_iter().collect();
        c.sort_by(|a, b| b.1.cmp(&a.1));
        s.causes = c;
        s
    }

    fn tier(&self, t: &str) -> String {
        self.tiers.get(t).map(|(p, n)| format!("{:.0}", pct(*p, *n))).unwrap_or_default()
    }
    fn split(&self, t: &str) -> String {
        self.splits.get(t).map(|(p, n)| format!("{:.0}", pct(*p, *n))).unwrap_or_default()
    }

    pub fn line(&self) -> String {
        format!(
            "state audit: {}/{} reach the state ({:.0}%); medium {}% hard {}%; dev {}% held {}% fresh {}%; live phrases {}/{}; unsafe {}; simulator gaps {}; steps {}/{}; broke mid-way {}; p50 {} ms p95 {} ms",
            self.pass,
            self.n,
            pct(self.pass, self.n),
            self.tier("medium"),
            self.tier("hard"),
            self.split("dev"),
            self.split("held"),
            self.split("fresh"),
            self.live.0,
            self.live.1,
            self.unsafe_,
            self.sim_gaps,
            self.steps.0,
            self.steps.1,
            self.midway,
            self.p50,
            self.p95
        )
    }

    pub fn csv_row(&self, o: &Opts, git: &str) -> String {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let label = if o.label.is_empty() { o.variant.notes.clone() } else { o.label.clone() };
        let ai = format!("{}{}", "ai", if o.end_only { " end-only" } else { "" });
        format!(
            "{now},{},{git},{},{},{},{:.1},{},{},{},{},{},{},{},{},{},{},{},{},\"{}\",{},{},{},{}",
            o.variant.name,
            o.split,
            if o.subset > 0 { format!("subset{}s{}", o.subset, o.seed) } else { "full".into() },
            self.n,
            pct(self.pass, self.n),
            "", // the retired easy column (old rows keep it)
            self.tier("medium"),
            self.tier("hard"),
            self.split("dev"),
            self.split("held"),
            self.unsafe_,
            self.sim_gaps,
            self.p50,
            self.p95,
            self.est_tokens,
            gpu_used_mib().map(|m| m.to_string()).unwrap_or_default(),
            ai,
            label.replace('"', "'"),
            self.split("fresh"),
            self.steps.0,
            self.steps.1,
            self.midway
        )
    }
}

/// GPU memory in use now (MiB, all processes; nvidia-smi), for the leaderboard's VRAM column.
pub fn gpu_used_mib() -> Option<u64> {
    let o = std::process::Command::new("nvidia-smi").args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"]).output().ok()?;
    String::from_utf8_lossy(&o.stdout).lines().next()?.trim().parse().ok()
}

pub const CSV_HEAD: &str = "unix_time,variant,git,split,cases_set,n,score,easy_retired,medium,hard,dev,held,unsafe,sim_gaps,p50_ms,p95_ms,est_tokens,vram_mib,mode,label,fresh,steps_pass,steps_n,midway";

fn append_csv(p: &std::path::Path, row: &str) -> Result<(), String> {
    use std::io::Write;
    let new = !p.exists();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if new {
        writeln!(f, "{CSV_HEAD}").map_err(|e| e.to_string())?;
    }
    writeln!(f, "{row}").map_err(|e| e.to_string())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Why the old intent audit reported ~99% (docs/VOICE-STATE-TESTS.md section 1), measured on its own cases.
pub fn old_audit_note() -> String {
    let cat = crate::catalogue::Catalogue::from_texts(crate::catalogue::BUILTIN, None).ok();
    let Some(cat) = cat else { return String::new() };
    let cases = crate::audit::cases(&cat, crate::audit::FIXTURE);
    let exact = cases.iter().filter(|c| c.kind == "exact").count();
    let mangled = cases.iter().filter(|c| c.kind == "mangled").count();
    let natural: Vec<&crate::audit::Case> = cases.iter().filter(|c| c.kind == "natural").collect();
    let sys = crate::nlu::system_prompt(&cat).to_lowercase();
    let in_prompt = natural.iter().filter(|c| sys.contains(&format!("\"{}\"", c.heard.to_lowercase()))).count();
    let phrases: Vec<String> = cat.enabled().flat_map(|i| i.phrases.iter().map(|p| p.to_lowercase())).collect();
    let in_cat = natural.iter().filter(|c| phrases.iter().any(|p| !p.contains('{') && *p == c.heard.to_lowercase())).count();
    format!(
        "{} cases: {exact} are each intent's own first catalogue phrase (the exact matcher hits them by construction), {mangled} the same phrase with one word swapped by a table the alias list undoes, {} natural phrasings ({in_prompt} of them word for word an example in the model's prompt, {in_cat} word for word a catalogue phrase). The verdict compares the intent NAME (with a table of equivalent ids) and that slot values are contained; a right name whose action failed still passes (\"decided right\"); the desktop is whatever is live or none, never a state with screens; no case checks that nothing else changed, so a command that also closes apps can pass. None of today's failures (close screen 2 asking to close window 1, terminals gathered on the wrong screen, half size applied to one screen) is visible to it.",
        cases.len(),
        natural.len()
    )
}

/// The HTML page.
#[allow(clippy::too_many_arguments)]
pub fn report(o: &[Outcome], s: &Summary, opts: &Opts, git: &str, csv: &str, unhonoured: &[String], all: &[Case]) -> String {
    let mut h = String::new();
    h += "<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>Voice State Audit</title><style>";
    h += ":root{--bg:#f6f6f3;--fg:#1c1c1a;--dim:#66665f;--card:#fff;--line:#e2e2dc;--pass:#1f7a3a;--fail:#b3261e;--warn:#9a6b00;--chip:#eeefe9;--code:#f1f1ec}\
@media (prefers-color-scheme:dark){:root:not([data-theme=light]){--bg:#121312;--fg:#e8e8e2;--dim:#9b9b93;--card:#1b1c1b;--line:#2d2e2b;--pass:#5cc97c;--fail:#ff7b72;--warn:#e0b04a;--chip:#262825;--code:#202220}}\
:root[data-theme=dark]{--bg:#121312;--fg:#e8e8e2;--dim:#9b9b93;--card:#1b1c1b;--line:#2d2e2b;--pass:#5cc97c;--fail:#ff7b72;--warn:#e0b04a;--chip:#262825;--code:#202220}\
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.45 system-ui,-apple-system,Segoe UI,sans-serif}\
main{max-width:1050px;margin:0 auto;padding:16px}h1{font-size:22px;margin:4px 0}h2{font-size:17px;margin:26px 0 8px}h3{font-size:15px;margin:0 0 4px}\
.sub{color:var(--dim);font-size:13px}.kpis{display:grid;grid-template-columns:repeat(auto-fill,minmax(130px,1fr));gap:8px;margin:12px 0}\
.kpi{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:8px 12px}.kpi b{font-size:21px;display:block}\
.wrap{overflow-x:auto;background:var(--card);border:1px solid var(--line);border-radius:10px}\
table{border-collapse:collapse;width:100%;font-size:13px}th,td{padding:6px 8px;border-bottom:1px solid var(--line);text-align:left;vertical-align:top}th{color:var(--dim);font-weight:600}\
.pass{color:var(--pass)}.fail{color:var(--fail)}.warn{color:var(--warn)}.chip{display:inline-block;background:var(--chip);border-radius:6px;padding:1px 7px;font-size:12px;margin:1px 2px 1px 0}\
.case{background:var(--card);border:1px solid var(--line);border-left:4px solid var(--fail);border-radius:10px;padding:10px 12px;margin:8px 0}.case.ok{border-left-color:var(--pass)}.case.bad{border-left-color:var(--fail)}\
.say{font-size:16px;font-weight:600}pre{background:var(--code);border-radius:8px;padding:8px;white-space:pre-wrap;word-break:break-word;font:12px/1.4 ui-monospace,monospace;margin:6px 0;max-height:340px;overflow:auto}\
details{margin:6px 0}summary{cursor:pointer;color:var(--dim)}ul{margin:4px 0;padding-left:20px}.bar{height:8px;background:var(--chip);border-radius:4px;overflow:hidden}.bar i{display:block;height:100%;background:var(--pass)}";
    h += "</style></head><body><main>";
    h += &format!("<h1>Voice state audit</h1><div class=sub>Variant <b>{}</b> ({}), git {}, split {}, {} of {} cases. Each case starts from a desk state, runs the words through the whole voice pipeline against a simulator of that desk, and is scored on the end state only. The intent name is not scored.</div>",
        esc(&opts.variant.name), esc(&opts.variant.notes), esc(git), esc(&opts.split), o.len(), all.len());
    h += "<div class=kpis>";
    h += &format!("<div class=kpi><span class=sub>reach the state</span><b class={}>{:.0}%</b>{} of {}</div>", if pct(s.pass, s.n) >= 80.0 { "pass" } else { "fail" }, pct(s.pass, s.n), s.pass, s.n);
    for t in ["medium", "hard"] {
        if let Some((p, n)) = s.tiers.get(t) {
            h += &format!("<div class=kpi><span class=sub>{t}</span><b>{:.0}%</b>{p} of {n}</div>", pct(*p, *n));
        }
    }
    for t in ["dev", "held", "fresh"] {
        if let Some((p, n)) = s.splits.get(t) {
            h += &format!("<div class=kpi><span class=sub>{} split</span><b>{:.0}%</b>{p} of {n}</div>", match t { "held" => "held-out", "fresh" => "fresh held-out", _ => "dev" }, pct(*p, *n));
        }
    }
    h += &format!("<div class=kpi><span class=sub>live phrases</span><b>{:.0}%</b>{} of {}</div>", pct(s.live.0, s.live.1), s.live.0, s.live.1);
    if s.steps.1 > 0 {
        h += &format!("<div class=kpi><span class=sub>steps in order</span><b>{:.0}%</b>{} of {}</div>", pct(s.steps.0, s.steps.1), s.steps.0, s.steps.1);
    }
    h += &format!("<div class=kpi><span class=sub>broke mid-way</span><b class={}>{}</b>cases</div>", if s.midway > 0 { "fail" } else { "pass" }, s.midway);
    h += &format!("<div class=kpi><span class=sub>unsafe</span><b class={}>{}</b>closed apps / stopped / typed</div>", if s.unsafe_ > 0 { "fail" } else { "pass" }, s.unsafe_);
    h += &format!("<div class=kpi><span class=sub>latency p50 / p95</span><b>{} / {}</b>ms per request</div></div>", s.p50, s.p95);
    if !unhonoured.is_empty() {
        h += &format!("<p class=warn>This variant sets knobs no code read in this run: {}. Its score is the baseline's for those.</p>", esc(&unhonoured.join(", ")));
    }
    h += "<h2>Why it fails</h2><div class=wrap><table><tr><th>cause</th><th>cases</th></tr>";
    for (c, n) in &s.causes {
        h += &format!("<tr><td>{}</td><td>{n}</td></tr>", esc(c));
    }
    h += "</table></div>";
    h += "<h2>By category</h2><div class=wrap><table><tr><th>category</th><th>pass</th><th></th></tr>";
    for (c, (p, n)) in &s.cats {
        h += &format!("<tr><td>{}</td><td>{p} / {n}</td><td style='width:40%'><div class=bar><i style='width:{:.0}%'></i></div></td></tr>", esc(c), pct(*p, *n));
    }
    h += "</table></div>";
    // leaderboard
    h += "<h2>Leaderboard (analysis/voice-experiments.csv)</h2><div class=wrap><table><tr><th>when</th><th>variant</th><th>git</th><th>set</th><th>n</th><th>score</th><th>med</th><th>hard</th><th>dev</th><th>held</th><th>fresh</th><th>unsafe</th><th>p50 / p95 ms</th><th>VRAM MiB</th><th>label</th></tr>";
    let mut rows: Vec<Vec<String>> = csv.lines().skip(1).map(|l| split_csv(l)).filter(|r| r.len() >= 20).collect();
    rows.sort_by(|a, b| b[6].parse::<f64>().unwrap_or(0.0).partial_cmp(&a[6].parse::<f64>().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
    for r in rows.iter().take(30) {
        let when = r[0].parse::<u64>().map(fmt_time).unwrap_or_default();
        h += &format!(
            "<tr><td>{when}</td><td>{}</td><td>{}</td><td>{} {}</td><td>{}</td><td><b>{}%</b></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{} / {}</td><td>{}</td><td>{} {}</td></tr>",
            esc(&r[1]), esc(&r[2]), esc(&r[3]), esc(&r[4]), esc(&r[5]), esc(&r[6]), esc(&r[8]), esc(&r[9]), esc(&r[10]), esc(&r[11]), esc(r.get(20).map(String::as_str).unwrap_or("")), esc(&r[12]), esc(&r[14]), esc(&r[15]), esc(&r[17]), esc(&r[18]), esc(&r[19])
        );
    }
    h += "</table></div>";
    h += &format!("<h2>Why the old audit said 99%</h2><p>{}</p><p class=sub>The intent audit (<code>omarchyvr-voice audit</code>, tests/fixtures/*.tsv) stays as a fast smoke test of the matcher and the model's intent choice. It is not the quality measure: this page is.</p>", esc(&old_audit_note()));
    // the fixed system prompt once
    if let Ok(mut cat) = crate::catalogue::Catalogue::load_default() {
        cat.lock_screens();
        h += &format!("<details><summary>The fixed system prompt the action model gets (every request)</summary><pre>{}</pre></details>", esc(&crate::nlu::system_prompt(&cat)));
    }
    h += "<h2>Cases (failures first)</h2>";
    let mut order: Vec<&Outcome> = o.iter().collect();
    order.sort_by_key(|x| (x.verdict.pass, !x.verdict.unsafe_, x.case.cat.clone(), x.case.id.clone()));
    for x in order {
        let c = &x.case;
        h += &format!("<div class='case {}'>", if x.verdict.pass { "ok" } else { "bad" });
        h += &format!(
            "<div><b class={}>{}</b> <span class=chip>{}</span><span class=chip>{}</span><span class=chip>{}</span><span class=chip>{}</span><span class=chip>{}</span> <span class=sub>{}</span></div>",
            if x.verdict.pass { "pass" } else { "fail" },
            if x.verdict.pass { "PASS" } else if x.verdict.unsafe_ { "UNSAFE" } else { "FAIL" },
            esc(&c.tier),
            esc(&c.cat),
            esc(&c.split),
            esc(&c.src),
            esc(&c.world_name),
            esc(&c.id)
        );
        if let Some(ok) = x.steps_ok {
            h += &format!("<div class=sub>Steps: <b class={}>{}</b> {}</div>", if ok { "pass" } else { "fail" }, if ok { "held in order" } else { "broken" }, esc(&c.steps.iter().map(|s| s.join(" & ")).collect::<Vec<_>>().join(" > ")));
        }
        if !x.mid_reasons.is_empty() {
            h += &format!("<div class=fail>Mid-way: {}</div>", esc(&x.mid_reasons.join("; ")));
        }
        h += &format!("<div class=say>&ldquo;{}&rdquo;</div><div class=sub>{}</div>", esc(&c.say), esc(&c.why));
        let exp = if c.any.is_empty() { c.expect.join(" &amp; ") } else { format!("{} + any of: {}", c.expect.join(" &amp; "), c.any.iter().map(|a| format!("[{}]", a.join(" &amp; "))).collect::<Vec<_>>().join(" | ")) };
        h += &format!("<div><b>Want:</b> {}</div>", if exp.is_empty() { "nothing changes".into() } else { exp.replace("&amp;amp;", "&amp;") });
        h += &format!("<div><b>AI chose:</b> <span class=chip>{}</span> {} {}</div>", esc(&x.route), esc(&x.decision), if x.asked { "<span class=chip>asked</span>" } else { "" });
        if !x.commands.is_empty() {
            h += &format!("<details><summary>{} command(s)</summary><pre>{}</pre></details>", x.commands.len(), esc(&x.commands.join("\n")));
        }
        let d: Vec<String> = world::diff(&c.start, &x.end).into_iter().filter(|ch| ch.key != "focus").map(|ch| ch.text).collect();
        h += &format!("<div><b>End state:</b> {}</div>", if d.is_empty() { "unchanged".into() } else { esc(&d.join("; ")) });
        if !x.toasts.is_empty() {
            h += &format!("<div class=sub>Said: {}</div>", esc(&x.toasts.join(" | ")));
        }
        if !x.verdict.pass {
            h += &format!("<div class=fail><b>{}</b><ul>{}</ul></div>", esc(&x.cause), x.verdict.reasons.iter().map(|r| format!("<li>{}</li>", esc(r))).collect::<String>());
        }
        h += &format!("<details><summary>Start state</summary><pre>{}</pre></details>", esc(&world::describe(&c.start, &[])));
        if !x.verdict.pass && !x.prompts.is_empty() {
            let p: Vec<String> = x.prompts.iter().map(|l| pretty_log(l)).collect();
            h += &format!("<details><summary>What the AI was given ({} request(s))</summary><pre>{}</pre></details>", x.prompts.len(), esc(&p.join("\n\n")));
        }
        h += "</div>";
    }
    h += "</main></body></html>\n";
    h
}

fn pretty_log(l: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { return l.chars().take(3000).collect() };
    let mut out = Vec::new();
    for k in ["style", "router", "prompt", "answer", "answers", "decision", "steps", "error", "context", "calls"] {
        if let Some(x) = v.get(k) {
            let s = match x {
                serde_json::Value::String(s) => s.clone(),
                o => serde_json::to_string_pretty(o).unwrap_or_default(),
            };
            out.push(format!("{k}: {}", s.chars().take(2500).collect::<String>()));
        }
    }
    if out.is_empty() {
        l.chars().take(2000).collect()
    } else {
        out.join("\n")
    }
}

fn split_csv(l: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in l.chars() {
        match c {
            '"' => q = !q,
            ',' if !q => v.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    v.push(cur);
    v
}

fn fmt_time(t: u64) -> String {
    // UTC date + time without a date crate
    let days = t / 86400;
    let secs = t % 86400;
    let (mut y, mut d) = (1970i64, days as i64);
    loop {
        let len = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 366 } else { 365 };
        if d < len {
            break;
        }
        d -= len;
        y += 1;
    }
    let ml = [31, if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    while m < 12 && d >= ml[m] {
        d -= ml[m];
        m += 1;
    }
    format!("{y}-{:02}-{:02} {:02}:{:02}Z", m + 1, d + 1, secs / 3600, (secs % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_builds_and_has_a_goal_state() {
        let v = load_cases().unwrap();
        assert!(v.len() >= 150, "{} cases", v.len());
        for c in &v {
            assert!(!c.expect.is_empty() || !c.any.is_empty(), "{} has no expectation", c.id);
            // every expectation parses (an unknown one would fail every run)
            for a in c.expect.iter().chain(c.any.iter().flatten()).chain(c.steps.iter().flatten()) {
                let (_, why, _) = assert_one(a, &c.start, &c.start, &[], false);
                assert!(!why.starts_with("unknown expectation"), "{}: {a}", c.id);
            }
            assert!(matches!(c.tier.as_str(), "medium" | "hard"), "{}: tier {}", c.id, c.tier);
        }
        // the long goals are registered (27 goals, ids lg-*)
        let lg: std::collections::BTreeSet<&str> = v.iter().filter(|c| c.goal.starts_with("lg-")).map(|c| c.goal.as_str()).collect();
        assert_eq!(lg.len(), 27, "{lg:?}");
        let held = v.iter().filter(|c| c.split == "held").count();
        let old = v.iter().filter(|c| c.split != "fresh").count();
        assert!(held * 5 >= old, "a held-out split of at least a fifth: {held}");
        let fresh = v.iter().filter(|c| c.split == "fresh").count();
        assert!(fresh >= 100, "a fresh held-out set of about 120 cases: {fresh}");
        let mut ids: Vec<&str> = v.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), v.len(), "case ids are unique");
        let live_held = v.iter().filter(|c| c.live && c.split == "held").count();
        assert!(live_held * 2 > v.iter().filter(|c| c.live).count(), "live phrases mostly held out");
    }

    #[test]
    fn the_easy_tier_is_rejected() {
        let goal = |extra: &str| format!("[[goal]]\nid = \"x\"\nwhy = \"w\"\nexpect = [\"nothing\"]\n{extra}\nsays = [\"hello\"]\n");
        let ok = load_cases_from(&[("t", &goal("worlds = { medium = \"m3\" }"))], WORLDS).unwrap();
        assert!(ok.iter().all(|c| c.tier == "medium"));
        let e = load_cases_from(&[("t", &goal("worlds = { easy = \"m3\", medium = \"m3\" }"))], WORLDS).unwrap_err();
        assert!(e.contains("easy tier was removed") && e.contains("goal x"), "{e}");
        let e = load_cases_from(&[("t", &goal("worlds = { medium = \"m3\" }\nset = { easy = { screens = 4 } }"))], WORLDS).unwrap_err();
        assert!(e.contains("`easy`"), "{e}");
    }

    #[test]
    fn no_case_passes_by_doing_nothing_unless_it_wants_nothing() {
        let mut bad = Vec::new();
        for c in load_cases().unwrap() {
            let v = check(&c.start, &c.start, &c.expect, &c.any, &c.allow, &[], false);
            let nothing = |e: &String| e == "nothing" || e.starts_with("said");
            let wants_nothing = c.expect.iter().any(nothing) || c.any.iter().any(|set| set.iter().all(|e| nothing(e) || e.ends_with(" stay")));
            if v.pass && !wants_nothing {
                bad.push(format!("{} {:?}", c.id, c.expect));
            }
        }
        assert!(bad.is_empty(), "pass with no action:\n{}", bad.join("\n"));
    }

    #[test]
    fn closing_an_app_nobody_named_is_unsafe() {
        let c = load_cases().unwrap().into_iter().find(|c| c.goal == "hide-2").unwrap();
        let mut end = c.start.clone();
        let gone = end.apps.remove(0);
        end.closed.push(gone);
        let v = check(&c.start, &end, &c.expect, &c.any, &c.allow, &[], false);
        assert!(!v.pass && v.unsafe_, "{v:?}");
    }

    fn case_of(goal: &str) -> Case {
        load_cases().unwrap().into_iter().find(|c| c.goal == goal && c.tier == "medium").unwrap_or_else(|| panic!("no medium case of {goal}"))
    }

    /// The world after a change, as a history entry.
    fn after(w: &World, cmd: &str, f: impl FnOnce(&mut World)) -> (String, World) {
        let mut w = w.clone();
        f(&mut w);
        w.normalize();
        (cmd.to_string(), w)
    }

    fn vol(w: &World, v: u8) -> (String, World) {
        after(w, &format!("volume {v}"), |x| x.media.volume = v)
    }

    #[test]
    fn steps_must_hold_in_order() {
        let mut c = case_of("volume-up");
        c.expect = vec![];
        c.steps = vec![vec!["volume up".into()], vec!["volume down".into()]];
        let up = vol(&c.start, 60);
        let down = vol(&c.start, 20);
        let ok = mid_check(&c, &[up.clone(), down.clone()], &down.1, &[], false);
        assert_eq!(ok.steps_ok, Some(true), "{ok:?}");
        let bad = mid_check(&c, &[down.clone(), up.clone()], &up.1, &[], false);
        assert_eq!(bad.steps_ok, Some(false));
        assert_eq!(bad.step, ["step 2 of 2 never held: volume down"]);
        // one world can hold several steps; no steps = None
        c.steps = vec![vec!["volume up".into()], vec!["volume up".into()]];
        assert_eq!(mid_check(&c, &[up.clone()], &up.1, &[], false).steps_ok, Some(true));
        c.steps.clear();
        assert_eq!(mid_check(&c, &[up.clone()], &up.1, &[], false).steps_ok, None);
    }

    #[test]
    fn a_stay_expectation_must_hold_in_every_world() {
        let mut c = case_of("hide-2");
        c.expect = vec!["terminals stay".into()];
        let t = c.start.apps.iter().find(|a| a.kind == Kind::Terminal).unwrap().id.clone();
        let away = after(&c.start, "move t1", |w| w.apps.iter_mut().find(|a| a.id == t).unwrap().ws = 3);
        let back = after(&away.1, "move t1 back", |w| w.apps.iter_mut().find(|a| a.id == t).unwrap().ws = 1);
        let m = mid_check(&c, &[away, back.clone()], &back.1, &[], false);
        assert_eq!(m.mid, ["mid-way: terminals stay broke after `move t1`"], "{m:?}");
        let quiet = mid_check(&c, &[back.clone()], &back.1, &[], false);
        assert!(quiet.mid.is_empty());
    }

    #[test]
    fn a_cancel_must_not_run_anything_even_if_it_is_undone() {
        let mut c = case_of("cancel-midway");
        assert_eq!(c.expect, ["nothing"]);
        let loud = vol(&c.start, 80);
        let undone = vol(&c.start, c.start.media.volume);
        let m = mid_check(&c, &[loud, undone.clone()], &undone.1, &[], false);
        assert_eq!(m.mid, ["mid-way: changed after `volume 80` then undone"], "{m:?}");
        assert!(mid_check(&c, &[], &c.start, &[], false).mid.is_empty());
        // not a cancel: no such reason
        c.expect = vec!["volume up".into()];
        assert!(mid_check(&c, &[vol(&c.start, 80), undone.clone()], &undone.1, &[], false).mid.is_empty());
    }

    #[test]
    fn no_app_on_a_screen_that_is_not_there() {
        let mut c = case_of("hide-2"); // e3c: three screens
        c.expect.clear();
        let t = c.start.apps[0].id.clone();
        let gone = after(&c.start, "move t1 to 5", |w| w.apps.iter_mut().find(|a| a.id == t).unwrap().ws = 5);
        let back = after(&gone.1, "move t1 back", |w| w.apps.iter_mut().find(|a| a.id == t).unwrap().ws = 1);
        let m = mid_check(&c, &[gone, back.clone()], &back.1, &[], false);
        assert_eq!(m.mid.len(), 1, "{m:?}");
        assert!(m.mid[0].contains("on missing screen 5") && m.mid[0].contains("after `move t1 to 5`"), "{m:?}");
        assert!(mid_check(&c, &[back.clone()], &back.1, &[], false).mid.is_empty());
    }

    #[test]
    fn no_app_closed_midway_unless_the_goal_closes_it() {
        let mut c = case_of("hide-2");
        c.expect.clear();
        let id = c.start.apps[0].id.clone();
        let shut = after(&c.start, "close t1", |w| w.apps.retain(|a| a.id != id));
        let m = mid_check(&c, &[shut.clone(), (String::from("later"), c.start.clone())], &c.start, &[], false);
        assert!(m.unsafe_ && m.mid == ["mid-way: t1 closed after `close t1`"], "{m:?}");
        // the goal closes it: fine
        let name = c.start.apps[0].name.clone();
        c.expect = vec![format!("{name} closed")];
        let m = mid_check(&c, &[shut.clone(), (String::from("later"), c.start.clone())], &c.start, &[], false);
        assert!(m.mid.is_empty() && !m.unsafe_, "{m:?}");
    }

    #[test]
    fn a_drift_is_not_blamed_on_the_voice_code() {
        let mut c = case_of("hide-2");
        c.expect = vec!["all stay".into()];
        c.start.timing.drift = vec![format!("close {}", c.start.apps[0].name)];
        let mut after_drift = c.start.clone();
        after_drift.drift(&c.start.timing.drift.clone());
        let w = vol(&after_drift, 10);
        let m = mid_check(&c, &[w.clone(), vol(&after_drift, 11)], &vol(&after_drift, 11).1, &[], false);
        assert!(m.mid.is_empty() && !m.unsafe_, "{m:?}");
    }

    #[test]
    fn the_goal_files_with_steps_build_and_every_step_parses() {
        for c in load_cases().unwrap().iter().filter(|c| !c.steps.is_empty()) {
            for a in c.steps.iter().flatten() {
                let (_, why, _) = assert_one(a, &c.start, &c.start, &[], false);
                assert!(!why.starts_with("unknown expectation"), "{}: {a}", c.id);
            }
        }
        assert!(load_cases().unwrap().iter().filter(|c| c.goal.starts_with("st-")).count() >= 60);
    }

    #[test]
    fn the_simulated_tools_reach_the_goals_they_are_for() {
        // the planner's dry run: the right tool, applied by the simulator, reaches the case's goal
        let mut cat = crate::catalogue::Catalogue::from_texts(crate::catalogue::BUILTIN, None).unwrap();
        cat.lock_screens();
        let all = load_cases().unwrap();
        let get = |goal: &str, tier: &str| all.iter().find(|c| c.goal == goal && c.tier == tier).unwrap_or_else(|| panic!("{goal} {tier}")).clone();
        let s = |kv: &[(&str, &str)]| kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<String, String>>();
        let runs: Vec<(&str, &str, &str, BTreeMap<String, String>)> = vec![
            ("terminals-to-3", "hard", "win.screen.put_n", s(&[("screen", "3"), ("app", "terminals")])),
            ("more-3", "medium", "vr.screens.more", s(&[("n", "3")])),
            ("hide-2", "hard", "vr.screens.hide", s(&[("screens", "2")])),
            ("bigger", "medium", "win.screen.bigger", s(&[])),
        ];
        for (goal, tier, tool, inputs) in runs {
            let c = get(goal, tier);
            let Ok(out) = world::apply(&c.start, &cat, tool, &inputs) else { continue }; // a tool this catalogue lacks
            let v = check(&c.start, &out.world, &c.expect, &c.any, &c.allow, &out.toasts, false);
            assert!(v.pass, "{goal} {tier} with {tool}: {:?} {:?}", v.reasons, out.commands);
        }
    }

    #[test]
    fn the_btop_windows_close_and_nothing_else() {
        let cat = crate::catalogue::Catalogue::from_texts(crate::catalogue::BUILTIN, None).unwrap();
        let all = load_cases().unwrap();
        let c = all.iter().find(|c| c.id.starts_with("lg-close-named-only.h")).unwrap().clone();
        let s: BTreeMap<String, String> = [("app", "btop windows")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let out = world::apply(&c.start, &cat, "win.close.named", &s).unwrap();
        let v = check(&c.start, &out.world, &["t3 closed".to_string(), "t4 closed".to_string(), "t5 stay".to_string(), "t1 stay".to_string()], &[], &["*".to_string()], &out.toasts, false);
        assert!(v.pass, "{:?} {:?}", v.reasons, out.toasts);
        // one name, two windows, no plural: nothing closes (it asks which)
        let s: BTreeMap<String, String> = [("app", "btop")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let out = world::apply(&c.start, &cat, "win.close.named", &s).unwrap();
        assert!(!world::diff(&c.start, &out.world).iter().any(|d| d.key.ends_with(":closed")), "{:?}", out.toasts);
        // a kind stays refused
        let s: BTreeMap<String, String> = [("app", "all the terminals")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let out = world::apply(&c.start, &cat, "win.close.named", &s).unwrap();
        assert!(!world::diff(&c.start, &out.world).iter().any(|d| d.key.ends_with(":closed")), "{:?}", out.toasts);
    }

    #[test]
    fn moving_an_off_screen_app_keeps_the_target_screen_on_its_workspace() {
        // "the terminals on 1" with a terminal on workspace 8 (no screen shows it): focusing it brought workspace 8
        // onto a screen, the fresh read then named 8 as screen 1's workspace and every terminal went there
        let cat = crate::catalogue::Catalogue::from_texts(crate::catalogue::BUILTIN, None).unwrap();
        let all = load_cases().unwrap();
        let c = all.iter().find(|c| c.id == "lg-setup-work.h2").unwrap().clone();
        let s: BTreeMap<String, String> = [("screen", "1"), ("app", "terminals")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let out = world::apply(&c.start, &cat, "win.screen.put_n", &s).unwrap();
        let v = check(&c.start, &out.world, &["terminals on 1".to_string()], &[], &["*".to_string()], &out.toasts, false);
        assert!(v.pass, "{:?} {:?}", v.reasons, out.toasts);
        assert!(!world::diff(&c.start, &out.world).iter().any(|d| d.text.contains("ws 8") && !d.text.contains("ws 8 ->")), "{:?}", out.commands);
    }

}
