//! Voice commands: utterance -> intent ([`crate::command`], then the optional LLM fallback [`crate::llm`]) ->
//! action, with feedback toasts, a spoken "yes" for `confirm` intents and "undo" for presets and backgrounds.
//!
//! The [`Actor`] is pure logic over a [`Runner`] (the side effects). [`Real`] runs commands, talks to the
//! passthrough socket and sends WayVR toasts (UDP 127.0.0.1:42069, the XSOverlay format WayVR listens to);
//! [`DryRun`] only prints what would happen. `pimax-voice act` feeds it the listener's final transcripts,
//! `pimax-voice do "..."` feeds it text.

use crate::catalogue::{expand, Catalogue, Intent};
use crate::command::{self, Heard, Match};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The built-in backdrops, in "next background" order; skybox images (`*.dds`) follow.
pub const BUILTIN_BACKDROPS: &[&str] = &["passthrough", "dimmed", "black", "sky"];

/// Side effects.
pub trait Runner {
    /// Run a program (argv, no shell). `capture`: wait (5 s at most) and return stdout; else start it and return.
    fn run(&mut self, argv: &[String], capture: bool) -> Result<String, String>;
    /// One line to the passthrough control socket; returns the reply.
    fn passthrough(&mut self, line: &str) -> Result<String, String>;
    /// Feedback for the user (a toast in the headset).
    fn toast(&mut self, text: &str);
    /// Captions on/off (a flag file `pimax-voice captions` watches).
    fn captions(&mut self, on: bool) -> Result<(), String>;
    /// Skybox images available (file stems in the skybox folder).
    fn skyboxes(&mut self) -> Vec<String>;
    /// Persist the undo state.
    fn save_state(&mut self, _s: &State) {}
    /// The voice-AI debug line (shown under the captions; see [`Trace`]).
    fn debug_line(&mut self, _line: &str) {}
    /// Programs really run (false: a dry run, a bench, a test). Typing is logged to typed.log only when true.
    fn live(&self) -> bool {
        false
    }
    /// A voice screen command ("new window", "move this left", "go to the left screen") lifts a preset's screen
    /// limit, so the screen it uses shows in VR (`$XDG_RUNTIME_DIR/pimax-screens.state`, read by `pimax desk screens
    /// watch`; pinned screens are kept). Nothing in dry runs and tests.
    fn screens_on_demand(&mut self) {}
    /// "Put everything in its own window" (true) / "put everything back together" (false): whether the screens
    /// watcher keeps apps of different kinds on separate screens (`sort = on|off` in the same state file).
    fn screens_sort(&mut self, _on: bool) {}
}

/// A runner wrapper that remembers the toasts of one request (they are the result shown in the debug line and
/// given to the model as a tool result).
pub struct Recorder<'a> {
    pub inner: &'a mut dyn Runner,
    pub toasts: Vec<String>,
}

impl<'a> Recorder<'a> {
    pub fn new(inner: &'a mut dyn Runner) -> Recorder<'a> {
        Recorder { inner, toasts: Vec::new() }
    }
}

impl Runner for Recorder<'_> {
    fn run(&mut self, argv: &[String], capture: bool) -> Result<String, String> {
        self.inner.run(argv, capture)
    }
    fn live(&self) -> bool {
        self.inner.live()
    }
    fn passthrough(&mut self, line: &str) -> Result<String, String> {
        self.inner.passthrough(line)
    }
    fn toast(&mut self, text: &str) {
        self.toasts.push(text.to_string());
        self.inner.toast(text);
    }
    fn captions(&mut self, on: bool) -> Result<(), String> {
        self.inner.captions(on)
    }
    fn skyboxes(&mut self) -> Vec<String> {
        self.inner.skyboxes()
    }
    fn save_state(&mut self, s: &State) {
        self.inner.save_state(s)
    }
    fn debug_line(&mut self, line: &str) {
        self.inner.debug_line(line)
    }
    fn screens_on_demand(&mut self) {
        self.inner.screens_on_demand()
    }
    fn screens_sort(&mut self, on: bool) {
        self.inner.screens_sort(on)
    }
}

/// What happened to one wake-word request, for the debug line under the captions and ai.log.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trace {
    /// The command words (after the wake word).
    pub heard: String,
    /// match (exact) | fuzzy (near-exact, no model) | noise | ai (compact model request) | ai-tools | yes | none
    pub route: String,
    /// The fuzzy matcher's top candidates (id, score).
    pub cands: Vec<(String, f64)>,
    /// The model's decision ("win.list 8/9"), or its error.
    pub decision: String,
    /// The id that ran or was asked about.
    pub ran: Option<String>,
    /// A "Did you mean ...?" / "say yes" question was asked instead of acting.
    pub asked: bool,
    /// The feedback (last toast), or "ignored".
    pub result: String,
    /// End of the transcript to the action, ms.
    pub ms: u64,
    /// The model round trip, ms.
    pub llm_ms: Option<u64>,
    /// The slots of what ran (or was asked about).
    pub slots: BTreeMap<String, String>,
    /// What the remote model requests cost (USD, when the provider says: OpenRouter's usage.cost).
    pub cost: Option<f64>,
}

/// A fuzzy candidate this good is asked about ("Did you mean ...?") when nothing else understood the request.
pub const ASK_BACK: f64 = 0.7;

/// The default caption width (characters) the debug line is fitted to.
pub const DEBUG_WIDTH: usize = 56;

fn short_id(id: &str) -> &str {
    id.strip_prefix("omarchy.").unwrap_or(id)
}

/// 0.95 -> ".95", 1.0 -> "1".
fn score2(s: f64) -> String {
    if s >= 0.995 {
        "1".into()
    } else {
        format!("{:.2}", s).trim_start_matches('0').to_string()
    }
}

impl Trace {
    /// The debug line, at most `width` characters; the latency always shows, the result is cut first:
    /// `= vr.preset.cinema > Cinema mode 3ms` (exact phrase), `~win.list .95 > 3 windows 4ms` (near-exact, no model),
    /// `noise > ignored 1ms`, `~media.next .80 > AI omarchy.media.next 9/9 > Next song 1290ms` (the model; the
    /// fuzzy matcher's best candidate first), `yes > Closed 40ms` (a confirmation).
    pub fn line(&self, width: usize) -> String {
        let cand = self.cands.first().map(|(id, s)| format!("~{} {}", short_id(id), score2(*s)));
        let mut parts: Vec<String> = match self.route.as_str() {
            "match" => vec![format!("= {}", self.ran.as_deref().map(short_id).unwrap_or("?"))],
            "yes" => vec!["yes".into()],
            "noise" => vec!["noise".into()],
            "fuzzy" => vec![cand.unwrap_or_else(|| "~?".into())],
            _ => {
                let mut v: Vec<String> = cand.into_iter().collect();
                v.push(if self.decision.is_empty() { "AI".into() } else { format!("AI {}", self.decision) });
                v
            }
        };
        let result = if !self.result.is_empty() {
            self.result.clone()
        } else if self.ran.is_some() {
            "ok".into()
        } else {
            "ignored".into()
        };
        parts.push(result);
        let tail = format!(" {}ms", self.ms);
        let fit = |parts: &[String]| parts.join(" > ").chars().count() + tail.chars().count() <= width;
        // cut the result, then the middle parts, to make room
        while !fit(&parts) {
            let i = parts.iter().rposition(|p| p.chars().count() > 6).unwrap_or(parts.len() - 1);
            let n = parts[i].chars().count();
            if n <= 2 {
                break;
            }
            let over = parts.join(" > ").chars().count() + tail.chars().count() - width;
            let keep = n.saturating_sub(over + 1).max(1);
            parts[i] = parts[i].chars().take(keep).collect::<String>() + "~";
        }
        let l = parts.join(" > ") + &tail;
        l.chars().take(width.max(tail.len())).collect()
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "trace": {
                "heard": self.heard, "route": self.route,
                "cands": self.cands.iter().map(|(i, s)| serde_json::json!([i, (s * 100.0).round() / 100.0])).collect::<Vec<_>>(),
                "decision": self.decision, "ran": self.ran, "asked": self.asked, "result": self.result,
                "ms": self.ms, "llm_ms": self.llm_ms, "slots": self.slots, "cost": self.cost,
            }
        })
    }
}

/// What can be undone, plus the current preset and backdrop as far as voice knows them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub preset: Option<String>,
    pub backdrop: Option<String>,
    /// (kind, previous value): kind = "preset" | "backdrop".
    pub history: Vec<(String, String)>,
}

impl State {
    /// `key=value` lines: `preset=`, `backdrop=`, `undo=kind:value` (oldest first).
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        if let Some(p) = &self.preset {
            s += &format!("preset={p}\n");
        }
        if let Some(b) = &self.backdrop {
            s += &format!("backdrop={b}\n");
        }
        for (k, v) in &self.history {
            s += &format!("undo={k}:{v}\n");
        }
        s
    }

    pub fn from_text(t: &str) -> State {
        let mut s = State::default();
        for l in t.lines() {
            match l.split_once('=') {
                Some(("preset", v)) => s.preset = Some(v.to_string()),
                Some(("backdrop", v)) => s.backdrop = Some(v.to_string()),
                Some(("undo", kv)) => {
                    if let Some((k, v)) = kv.split_once(':') {
                        s.history.push((k.to_string(), v.to_string()));
                    }
                }
                _ => {}
            }
        }
        s
    }

    fn push(&mut self, kind: &str, prev: Option<String>) {
        if let Some(p) = prev {
            self.history.push((kind.to_string(), p));
            if self.history.len() > 10 {
                self.history.remove(0);
            }
        }
    }
}

/// The default preset of a session (`pimax desk start`), when voice has not switched yet.
pub const DEFAULT_PRESET: &str = "cinema";

/// What the passthrough/skybox look like for a backdrop: (passthrough line, skybox arg for wayvrctl).
pub fn backdrop_plan(name: &str, skybox_dir: &str) -> (&'static str, String) {
    match name {
        "passthrough" => ("normal", "off".into()),
        "dimmed" => ("dim", "off".into()),
        "black" => ("off", "off".into()),
        "sky" => ("off", "default".into()),
        file => ("off", format!("{skybox_dir}/{file}.dds")),
    }
}

/// Pick a backdrop by spoken name: exact, then the one whose name contains / is contained in the words.
pub fn find_backdrop(list: &[String], spoken: &str) -> Option<String> {
    let norm = |s: &str| command::words(&s.replace(['_', '-'], " ")).join(" ");
    let want = norm(spoken);
    let alias = match want.as_str() {
        "room" | "camera" | "normal" | "passthrough on" => "passthrough",
        "dim" | "dark room" | "dimmed passthrough" => "dimmed",
        "none" | "nothing" | "dark" | "off" | "negro" => "black",
        "skybox" | "space" | "default" | "cielo" => "sky",
        _ => "",
    };
    if !alias.is_empty() {
        return list.iter().find(|n| *n == alias).cloned();
    }
    list.iter()
        .find(|n| norm(n) == want)
        .or_else(|| list.iter().find(|n| norm(n).contains(&want) || want.contains(&norm(n))))
        .cloned()
}

/// What a spoken "yes" runs.
#[derive(Debug, Clone)]
pub enum PendingAction {
    /// A catalogue intent.
    Intent(Match),
    /// A careful tool call of the AI agent ([`crate::agent`]).
    Agent(crate::agent::Confirmable),
}

#[derive(Debug, Clone)]
struct Pending {
    action: PendingAction,
    until: Instant,
}

/// The voice command state machine.
pub struct Actor {
    /// The AI switch's model (window picking); None = off.
    pub ai: Option<Box<dyn crate::llm::LlmClient>>,
    /// The command words of the last wake utterance (the AI switch gets the whole request).
    pub last_words: String,
    /// The same, raw (case, punctuation and number words kept): typed text and searches come from it.
    pub last_raw: String,
    pub cat: Catalogue,
    pub wayvrctl: String,
    pub state: State,
    pending: Option<Pending>,
    followup_until: Option<Instant>,
    /// LLM fallback (None = off).
    pub llm: Option<Box<dyn FnMut(&str) -> Result<Option<Match>, String>>>,
    /// The AI agent with tools (`voice_ai` in ai.toml; None = off). See [`crate::agent`].
    pub agent: Option<crate::agent::Agent>,
    /// Show the debug line under the captions (`voice_ai_debug`, "Max, debug on/off").
    pub debug: bool,
    /// The last wake-word request (see [`Trace`]).
    pub trace: Option<Trace>,
    /// When a command waits for a spoken "yes" (`voice_confirm` in ai.toml; the library default is
    /// `Dangerous`, `pimax-voice act` uses the file's value, default `None`).
    pub confirm: pimax_config::ai::VoiceConfirm,
    /// The command running now was confirmed with "yes" (a terminal may be typed into).
    yes_given: bool,
    /// Dictation: final transcripts are typed (no wake word) until "stop dictation" or this deadline.
    dictating: Option<(Instant, bool)>,
}

/// Dictation stops after this long without speech.
pub const DICTATION_IDLE_S: u64 = 60;

/// Fill `{slot}`, `{wayvrctl}`, `{pimax}` in one argument.
pub fn fill(arg: &str, slots: &BTreeMap<String, String>, wayvrctl: &str, pimax: &str) -> String {
    let mut s = arg.to_string();
    for (k, v) in slots {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s.replace("{wayvrctl}", wayvrctl).replace("{pimax}", pimax)
}

impl Actor {
    pub fn new(cat: Catalogue, wayvrctl: Option<String>, state: State) -> Actor {
        let wayvrctl = wayvrctl.unwrap_or_else(|| expand(&cat.paths.wayvrctl).to_string_lossy().into_owned());
        Actor { cat, wayvrctl, state, pending: None, followup_until: None, llm: None, ai: None, last_words: String::new(), last_raw: String::new(), agent: None, debug: false, trace: None, confirm: pimax_config::ai::VoiceConfirm::Dangerous, yes_given: false, dictating: None }
    }

    /// Forget a pending question and the "Max" follow-up window (the AI bench between phrases).
    pub fn reset(&mut self) {
        self.pending = None;
        self.followup_until = None;
        self.dictating = None;
    }

    /// One final transcript. Returns the intent id that ran (or was asked to confirm), if any. A wake-word request
    /// leaves its [`Trace`] in `self.trace`; with `debug` on it is also shown (Runner::debug_line), and the agent
    /// logs it to ai.log.
    pub fn hear(&mut self, text: &str, now: Instant, r: &mut dyn Runner) -> Option<String> {
        let t0 = Instant::now();
        self.trace = None;
        let mut rec = Recorder::new(r);
        let out = self.hear_inner(text, now, &mut rec);
        let toasts = std::mem::take(&mut rec.toasts);
        if let Some(mut t) = self.trace.take() {
            t.ms = t0.elapsed().as_millis() as u64;
            if t.ran.is_none() {
                t.ran = out.clone();
            }
            if t.result.is_empty() {
                t.result = toasts.last().cloned().unwrap_or_default();
            }
            if self.debug {
                r.debug_line(&t.line(DEBUG_WIDTH));
            }
            if let Some(a) = self.agent.as_mut() {
                a.log(t.json());
                if t.route != "noise" {
                    a.remember(&t.heard, t.ran.as_deref(), &t.result);
                }
            }
            self.trace = Some(t);
        }
        out
    }

    /// Ask "Did you mean ...?" for a fuzzy candidate (a spoken "yes" runs it).
    fn ask_back(&mut self, c: &crate::fuzzy::Cand, now: Instant, r: &mut dyn Runner) -> String {
        let m = Match { id: c.id.clone(), slots: c.slots.clone(), score: 0 };
        if let Some(t) = self.trace.as_mut() {
            t.slots = c.slots.clone();
        }
        let what = format!("Did you mean \"{}\"", command::words(&c.phrase).join(" "));
        self.ask(&what, PendingAction::Intent(m), now, r);
        if let Some(t) = self.trace.as_mut() {
            t.asked = true;
        }
        c.id.clone()
    }

    /// Forget an unanswered confirmation after its time; call about once a second.
    pub fn tick(&mut self, now: Instant, r: &mut dyn Runner) {
        if self.pending.as_ref().is_some_and(|p| now >= p.until) {
            self.pending = None;
            r.toast("Cancelled (no answer)");
        }
        if self.followup_until.is_some_and(|t| now >= t) {
            self.followup_until = None;
        }
    }

    fn hear_inner(&mut self, text: &str, now: Instant, r: &mut dyn Runner) -> Option<String> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        // dictation: everything is typed until "stop dictation" (with or without "Max")
        if let Some((until, allow)) = self.dictating {
            let w = command::words(text);
            let stop = w.windows(2).any(|p| matches!(p[0].as_str(), "stop" | "end" | "finish" | "cancel") && p[1].starts_with("dictat"));
            if stop || now >= until {
                self.dictating = None;
                r.toast("Dictation stopped");
                if stop {
                    return Some("type.dictate.stop".into());
                }
            } else {
                self.dictating = Some((now + Duration::from_secs(DICTATION_IDLE_S), allow));
                match crate::desk::type_text(&format!("{} ", text), allow, r) {
                    Ok(_) => {}
                    Err(e) => r.toast(&e),
                }
                return Some("type.dictate.text".into());
            }
        }
        // a pending confirmation: "yes" runs it, anything else cancels it (a new wake command still runs below)
        if let Some(p) = self.pending.take() {
            if now < p.until && command::is_yes(text) {
                self.trace = Some(Trace { heard: text.to_string(), route: "yes".into(), ..Default::default() });
                self.yes_given = true;
                let id = match p.action {
                    PendingAction::Intent(m) => {
                        self.execute(&m, r);
                        m.id
                    }
                    PendingAction::Agent(c) => crate::agent::run_confirmed(self, &c, r),
                };
                self.yes_given = false;
                return Some(id);
            }
            r.toast("Cancelled");
        }
        let words = match command::wake(text, &self.cat.wake.words) {
            Heard::Talk => match self.followup_until.take() {
                Some(t) if now < t => command::words(text),
                _ => return None,
            },
            Heard::WakeOnly => {
                self.followup_until = Some(now + Duration::from_secs_f64(self.cat.wake.followup_s.max(1.0)));
                r.toast("Listening...");
                return None;
            }
            Heard::Command(w) => w,
        };
        self.followup_until = None;
        self.last_words = words.join(" ");
        self.last_raw = command::raw_after_wake(text, &self.cat.wake.words);
        let cands = crate::fuzzy::candidates(&self.cat, &words, 3);
        self.trace = Some(Trace {
            heard: self.last_words.clone(),
            route: "match".into(),
            cands: cands.iter().map(|c| (c.id.clone(), c.score)).collect(),
            ..Default::default()
        });
        let always = self.agent.as_ref().is_some_and(|a| a.settings.mode == crate::agent::Mode::Always);
        // exact phrase, else (not in "always" mode) a clear near-exact fuzzy hit: the fast path, no model
        let mut matched = command::best(&self.cat, &words);
        // a free-text slot swallows anything ("show {app}" for "show me the real room"): such a match only counts
        // when its value names something real (a window, a site, a backdrop); otherwise the request goes on to
        // the fuzzy hints and the model
        if matched.as_ref().is_some_and(|m| !self.plausible(m, r)) {
            matched = None;
        }
        // an exact match only through a free-text slot ("focus {app}" for "focus rights") loses to a close phrase
        // without one ("focus right")
        if let Some(m) = &matched {
            let free = self.cat.get(&m.id).is_some_and(|i| i.slots.values().any(|t| matches!(t, crate::catalogue::SlotType::Text | crate::catalogue::SlotType::Name)));
            if free {
                if let Some(c) = cands.iter().find(|c| !c.free_text && c.complete && c.score >= 0.85 && c.id != m.id) {
                    matched = Some(Match { id: c.id.clone(), slots: c.slots.clone(), score: 0 });
                    self.set_route("fuzzy");
                }
            }
        }
        if matched.is_none() {
            if crate::fuzzy::is_noise(&words) {
                // "max ma max max", "can you hear me": nothing, no toast
                self.set_route("noise");
                return None;
            }
            if !always {
                if let Some(c) = crate::fuzzy::confident(&cands) {
                    matched = Some(Match { id: c.id.clone(), slots: c.slots.clone(), score: 0 });
                    self.set_route("fuzzy");
                }
            }
        }
        // the AI agent: unmatched or deictic requests (mode fallback), every request (mode always)
        if let Some(mut ag) = self.agent.take() {
            if matched.is_none() || always {
                self.set_route(match ag.style() {
                    crate::agent::Style::Compact => "ai",
                    crate::agent::Style::Tools => "ai-tools",
                    crate::agent::Style::TwoStage => "ai2",
                });
            }
            let routed = ag.route(self, &words, matched.as_ref(), &cands, now, r);
            self.agent = Some(ag);
            match routed {
                crate::agent::Routed::Handled(id) => return Some(id),
                crate::agent::Routed::Ignored => return None,
                // a gaze-resolved deictic command: the target window was focused, run the match as usual
                crate::agent::Routed::Matcher => {}
                crate::agent::Routed::Failed(e) => {
                    if matched.is_none() {
                        // the model failed: a plausible candidate is asked about, else "Didn't get"
                        if let Some(c) = cands.first().filter(|c| c.score >= ASK_BACK && c.complete && !c.free_text) {
                            let c = c.clone();
                            r.toast(&format!("AI: {e}"));
                            return Some(self.ask_back(&c, now, r));
                        }
                        r.toast(&format!("Didn't get \"{}\" (AI: {e}). Say: Max, help", words.join(" ")));
                        return None;
                    }
                    r.toast(&format!("AI: {e}"));
                }
            }
            let m = matched?;
            if let Some(a) = self.agent.as_mut() {
                a.note(&self.last_words, &m.id, true);
            }
            return Some(self.run_or_confirm(m, now, r));
        }
        let m = match matched {
            Some(m) => Some(m),
            None => match self.llm.as_mut() {
                Some(f) => match f(&words.join(" ")) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("voice: llm: {e}");
                        None
                    }
                },
                None => None,
            },
        };
        let Some(m) = m else {
            // the AI switch: a descriptive window request nothing else understood
            if self.ai.is_some() {
                match crate::windows::ai_switch(&self.last_words, &mut self.ai, r) {
                    Ok(true) => return Some("win.ai".into()),
                    Ok(false) => {}
                    Err(e) => eprintln!("voice: ai switch: {e}"),
                }
            }
            // no model: a plausible fuzzy candidate is asked about
            if let Some(c) = cands.first().filter(|c| c.score >= ASK_BACK && c.complete && !c.free_text) {
                let c = c.clone();
                return Some(self.ask_back(&c, now, r));
            }
            r.toast(&format!("Didn't get \"{}\". Say: Max, help", words.join(" ")));
            return None;
        };
        Some(self.run_or_confirm(m, now, r))
    }

    /// Does a match's free-text slot name something that exists?
    pub fn plausible(&mut self, m: &Match, r: &mut dyn Runner) -> bool {
        let v = |k: &str| m.slots.get(k).cloned().unwrap_or_default();
        let titles = |r: &mut dyn Runner| -> Vec<crate::windows::Client> {
            crate::windows::hyprctl_json(r, "clients").and_then(|j| crate::windows::parse_clients(&j)).unwrap_or_default()
        };
        let mentions = |cs: &[crate::windows::Client], q: &str| {
            let q = q.to_lowercase();
            let w: Vec<String> = command::words(&q).into_iter().filter(|w| w.len() >= 3 && !command::FILLERS.contains(&w.as_str())).collect();
            !w.is_empty() && cs.iter().any(|c| w.iter().all(|x| c.title.to_lowercase().contains(x.as_str()) || c.class.to_lowercase().contains(x.as_str())))
        };
        match m.id.as_str() {
            "omarchy.focus.app" => {
                let app = v("app");
                let cs = titles(r);
                crate::windows::find_app(&cs, &app, None).is_some() || mentions(&cs, &app) || crate::desk::site_url(&app).is_some() || crate::windows::app_alias(&app)
            }
            "browser.tab.focus" => {
                let q = v("q");
                let cs = titles(r);
                let w = command::words(&q);
                !w.iter().any(|x| x.chars().all(|c| c.is_ascii_digit()) || matches!(x.as_str(), "next" | "previous" | "last" | "first" | "third" | "second" | "new"))
                    && (mentions(&cs, &q) || crate::desk::site_url(&q).is_some())
            }
            "vr.backdrop.set" => find_backdrop(&self.backdrops(r), &v("name")).is_some(),
            _ => true,
        }
    }

    fn set_route(&mut self, route: &str) {
        if let Some(t) = self.trace.as_mut() {
            t.route = route.to_string();
        }
    }

    /// Run a match, or ask for a spoken "yes" first when its intent needs one. Returns the intent id.
    pub fn run_or_confirm(&mut self, mut m: Match, now: Instant, r: &mut dyn Runner) -> String {
        let id = m.id.clone();
        self.raw_slots(&mut m);
        if let Some(t) = self.trace.as_mut() {
            t.slots = m.slots.clone();
        }
        if self.must_ask(&m, r) {
            let what = self.dangerous(&m, r).or_else(|| self.cat.get(&m.id).and_then(|i| i.phrases.first().cloned())).unwrap_or_else(|| m.id.clone());
            self.ask(&what, PendingAction::Intent(m), now, r);
        } else {
            self.execute(&m, r);
        }
        id
    }

    /// Typing and search take their free text from the raw words ("type line two" types "line two", not "line 2").
    pub fn raw_slots(&self, m: &mut Match) {
        let Some(i) = self.cat.get(&m.id) else { return };
        if !(i.run.first().is_some_and(|p| p == "@desk") && matches!(i.run.get(1).map(String::as_str), Some("type" | "search"))) {
            return;
        }
        for (k, t) in &i.slots {
            if !matches!(t, crate::catalogue::SlotType::Text) {
                continue;
            }
            // the phrase that matched: the first whose trailing literal occurs in the raw text
            if let Some(v) = i.phrases.iter().filter(|p| p.ends_with(&format!("{{{k}}}"))).find_map(|p| command::raw_slot(&self.last_raw, p)) {
                if command::slot_ok(*t, &v) || v.chars().count() <= 200 {
                    m.slots.insert(k.clone(), v);
                }
            }
        }
    }

    /// Is this match dangerous: `confirm = true` in the catalogue, or typing / Enter into a terminal (@desk)?
    pub fn dangerous(&self, m: &Match, r: &mut dyn Runner) -> Option<String> {
        let i = self.cat.get(&m.id)?;
        if i.run.first().is_some_and(|p| p == "@desk") {
            let argv: Vec<String> = i.run[1..].iter().map(|a| fill(a, &m.slots, &self.wayvrctl, &self.cat.paths.pimax)).collect();
            if let Some(q) = crate::desk::needs_confirm(&argv, r) {
                return Some(q);
            }
        }
        if !i.confirm {
            return None;
        }
        // closing a window (= workspace) says how many apps go with it
        if i.run.first().is_some_and(|p| p == "@win") && i.run.get(1).is_some_and(|a| a == "ws_close") {
            if let Some(q) = crate::windows::close_prompt(r, &self.wayvrctl) {
                return Some(q);
            }
        }
        Some(i.phrases.first().cloned().unwrap_or_else(|| m.id.clone()))
    }

    /// Does this match wait for "yes" under the `voice_confirm` policy?
    pub fn must_ask(&self, m: &Match, r: &mut dyn Runner) -> bool {
        // user decision 2026-10-15: "close window" (a whole VR screen) asks only when it has 2+ apps, whatever the
        // policy says (`always` still asks); one app, or "close this" (one app), closes right away
        if self.confirm != pimax_config::ai::VoiceConfirm::Always {
            if let Some(i) = self.cat.get(&m.id).filter(|i| i.run.first().is_some_and(|p| p == "@win")) {
                match i.run.get(1).map(String::as_str) {
                    Some("ws_close") => return crate::windows::close_count(r, &self.wayvrctl).is_some_and(|n| n >= 2),
                    Some("close") => return false,
                    _ => {}
                }
            }
        }
        match self.confirm {
            pimax_config::ai::VoiceConfirm::None => false,
            pimax_config::ai::VoiceConfirm::Always => true,
            pimax_config::ai::VoiceConfirm::Dangerous => self.dangerous(m, r).is_some(),
        }
    }

    /// Ask for a spoken "yes" (within `confirm_s`) before running `action`.
    pub fn ask(&mut self, what: &str, action: PendingAction, now: Instant, r: &mut dyn Runner) {
        r.toast(&format!("{what}? Say \"yes\" to confirm"));
        self.pending = Some(Pending { action, until: now + Duration::from_secs_f64(self.cat.wake.confirm_s.max(2.0)) });
    }

    /// Run a matched intent.
    pub fn execute(&mut self, m: &Match, r: &mut dyn Runner) {
        let Some(i) = self.cat.get(&m.id).cloned() else { return };
        let argv: Vec<String> = i.run.iter().map(|a| fill(a, &m.slots, &self.wayvrctl, &self.cat.paths.pimax)).collect();
        let say = fill(&i.say, &m.slots, "", "");
        match argv.first().map(String::as_str) {
            Some("@preset") => self.preset(argv.get(1).cloned().unwrap_or_default(), true, &say, r),
            Some("@backdrop") => self.backdrop(argv.get(1).map(String::as_str).unwrap_or("next"), true, r),
            Some("@passthrough") => {
                let cmd = argv.get(1).map(String::as_str).unwrap_or("status");
                match r.passthrough(cmd) {
                    Ok(reply) => {
                        r.toast(&format!("Passthrough {}", reply.trim_start_matches("ok ")));
                        if cmd != "status" {
                            self.state.backdrop = Some("passthrough".into());
                        }
                    }
                    Err(e) => r.toast(&format!("Passthrough: {e}")),
                }
            }
            Some("@captions") => {
                let on = argv.get(1).is_some_and(|a| a == "on");
                match r.captions(on) {
                    Ok(()) => r.toast(if say.is_empty() { if on { "Captions on" } else { "Captions off" } } else { &say }),
                    Err(e) => r.toast(&format!("Captions: {e}")),
                }
            }
            Some("@help") => r.toast(&self.help()),
            Some("@undo") => self.undo(r),
            Some("@debug") => {
                self.debug = argv.get(1).is_none_or(|a| a == "on");
                let _ = std::fs::write(debug_state_path(), if self.debug { "on\n" } else { "off\n" });
                if !self.debug {
                    r.debug_line("");
                }
                r.toast(if self.debug { "Debug on" } else { "Debug off" });
            }
            Some("@cancel") => r.toast("Nothing to cancel"),
            Some("@say") => r.toast(&argv[1..].join(" ")),
            Some("@desk") => {
                if argv.get(1).is_some_and(|a| a == "dictate") {
                    let allow = self.yes_given || !self.confirm.asks(true);
                    self.dictating = Some((Instant::now() + Duration::from_secs(DICTATION_IDLE_S), allow));
                    r.toast("Dictating: say \"stop dictation\" to end");
                } else {
                    // a terminal is fine when there is no confirmation policy for it, or after the yes
                    let allow = self.yes_given || !self.confirm.asks(true);
                    crate::desk::execute(&argv[1..], allow, r);
                }
            }
            Some("@win") => {
                let transcript = self.last_words.clone();
                crate::windows::execute(&argv[1..], &transcript, &mut self.ai, &self.wayvrctl, r)
            }
            Some(_) => self.external(&i, &argv, &say, r),
            None => {}
        }
        r.save_state(&self.state);
    }

    fn external(&mut self, i: &Intent, argv: &[String], say: &str, r: &mut dyn Runner) {
        match r.run(argv, i.output) {
            Ok(out) if i.output => {
                let lines: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).collect();
                r.toast(&if lines.is_empty() { "(no output)".to_string() } else { lines.join(" | ") });
            }
            Ok(_) => {
                if !say.is_empty() {
                    r.toast(say);
                }
            }
            Err(e) => r.toast(&format!("{} failed: {e}", i.id)),
        }
    }

    fn preset(&mut self, name: String, record: bool, say: &str, r: &mut dyn Runner) {
        let prev = self.state.preset.clone().unwrap_or_else(|| DEFAULT_PRESET.into());
        let argv = vec![self.cat.paths.pimax.clone(), "desk".into(), "preset".into(), name.clone()];
        match r.run(&argv, true) {
            Ok(out) => {
                if record && prev != name {
                    self.state.push("preset", Some(prev));
                }
                self.state.preset = Some(name.clone());
                // the preset sets the passthrough too
                self.state.backdrop = Some(if name == "cinema" { "dimmed" } else { "passthrough" }.into());
                let base = if say.is_empty() { name.as_str() } else { say };
                r.toast(&preset_toast(base, &out));
            }
            Err(e) => r.toast(&format!("Preset {name} failed: {e}")),
        }
    }

    /// All backdrops: the built-in ones, then the skybox images.
    pub fn backdrops(&self, r: &mut dyn Runner) -> Vec<String> {
        let mut v: Vec<String> = BUILTIN_BACKDROPS.iter().map(|s| s.to_string()).collect();
        for s in r.skyboxes() {
            if !v.contains(&s) {
                v.push(s);
            }
        }
        v
    }

    fn backdrop(&mut self, what: &str, record: bool, r: &mut dyn Runner) {
        let list = self.backdrops(r);
        let cur = self.state.backdrop.clone().unwrap_or_else(|| {
            if self.state.preset.as_deref().unwrap_or(DEFAULT_PRESET) == "cinema" { "dimmed" } else { "passthrough" }.into()
        });
        let idx = list.iter().position(|n| *n == cur).unwrap_or(0);
        let target = match what {
            "next" => list[(idx + 1) % list.len()].clone(),
            "previous" => list[(idx + list.len() - 1) % list.len()].clone(),
            name => match find_backdrop(&list, name) {
                Some(n) => n,
                None => {
                    r.toast(&format!("No background \"{name}\". Have: {}", list.join(", ")));
                    return;
                }
            },
        };
        let (pt, sky) = backdrop_plan(&target, &self.cat.backdrop.skybox_dir);
        let sky_argv = vec![self.wayvrctl.clone(), "skybox".into(), sky.clone()];
        // to an image: draw the skybox first, then turn the camera off (no flash); back: the camera first
        let (a, b) = if sky != "off" { (true, false) } else { (false, true) };
        let mut errors = Vec::new();
        for step_sky in [a, b] {
            if step_sky {
                if let Err(e) = r.run(&sky_argv, true) {
                    errors.push(format!("skybox: {e}"));
                }
            } else if let Err(e) = r.passthrough(pt) {
                errors.push(format!("passthrough: {e}"));
            }
        }
        if errors.len() == 2 {
            r.toast(&format!("Background {target} not applied: {}", errors.join("; ")));
            return;
        }
        if record && cur != target {
            self.state.push("backdrop", Some(cur));
        }
        self.state.backdrop = Some(target.clone());
        let mut msg = format!("Background: {target}");
        if !errors.is_empty() {
            msg += &format!(" ({})", errors.join("; "));
        }
        r.toast(&msg);
    }

    fn undo(&mut self, r: &mut dyn Runner) {
        match self.state.history.pop() {
            Some((k, v)) if k == "preset" => {
                let say = format!("Back to {v}");
                self.preset(v, false, &say, r);
            }
            Some((k, v)) if k == "backdrop" => self.backdrop(&v, false, r),
            _ => r.toast("Nothing to undo"),
        }
    }

    /// The short help toast.
    pub fn help(&self) -> String {
        "Say \"Max\" then: cinema / work 3 / work 1 / assistant, passthrough on/off, dim, brighter, next background, \
         reset screens, recenter, show fps, captions off, undo, volume up, next song, open terminal, fullscreen, \
         take a screenshot, lock the screen (full list: docs/VOICE.md)"
            .into()
    }

    /// One line per enabled intent: id and phrases (for `pimax-voice intents` and the logs).
    pub fn list(&self, all: bool) -> Vec<String> {
        self.cat
            .intents
            .iter()
            .filter(|i| all || i.enabled)
            .map(|i| {
                format!(
                    "{}{:34} [{}{}{}] {}",
                    if i.enabled { "" } else { "(off) " },
                    i.id,
                    i.safety,
                    if i.source.is_empty() { String::new() } else { format!(", {}", i.source) },
                    if i.confirm { ", confirm" } else { "" },
                    i.phrases.join(" | ")
                )
            })
            .collect()
    }
}

// ------------------------------------------------------------------------------------------------- runners

/// The toast of a mode switch: a mode only hides screens, so it says which apps stay open on them (from the
/// launcher's "still open on hidden screens: left (google-chrome), right (pimax)" line) and how to see them.
pub fn preset_toast(say: &str, launcher_out: &str) -> String {
    let hidden = launcher_out.lines().find_map(|l| l.split_once("still open on hidden screens: ").map(|(_, r)| r.trim()));
    match hidden.filter(|h| !h.is_empty()) {
        Some(h) => {
            let first = h.split(' ').next().unwrap_or("left");
            format!("{say}. Still open on hidden screens: {h}. Say \"go to the {first} screen\"")
        }
        None => say.to_string(),
    }
}

/// The VR screens' show state (same file and format as the launcher's `vr/screens.rs`).
pub fn screens_state_path() -> PathBuf {
    runtime_dir().join("pimax-screens.state")
}

/// The state text with `limit = 4` (every screen on demand), the other lines kept.
pub fn screens_unlimited(old: &str) -> String {
    screens_state_set(old, "limit", "4")
}

/// The state text with `key = value`, the other lines kept.
pub fn screens_state_set(old: &str, key: &str, value: &str) -> String {
    let mut out: Vec<String> =
        old.lines().filter(|l| l.split_once('=').is_none_or(|(k, _)| k.trim() != key)).map(str::to_string).collect();
    out.push(format!("{key} = {value}"));
    out.join("\n") + "\n"
}

fn write_screens_state(f: impl Fn(&str) -> String) {
    let p = screens_state_path();
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    let new = f(&old);
    if new != old {
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, new).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// Where the flag file of "captions off" lives.
pub fn captions_off_path() -> PathBuf {
    runtime_dir().join("pimax-voice-captions.off")
}

/// "Max, debug on/off" (on/off; it wins over `voice_ai_debug` until the next login).
pub fn debug_state_path() -> PathBuf {
    runtime_dir().join("pimax-voice-debug.state")
}

/// The debug line `pimax-voice act` writes and `pimax-voice captions` shows under the transcript.
pub fn debug_line_path() -> PathBuf {
    runtime_dir().join("pimax-voice-debug.line")
}

/// Debug on or off: the spoken toggle if any, else the config.
pub fn debug_enabled(config: bool) -> bool {
    match std::fs::read_to_string(debug_state_path()).map(|s| s.trim().to_string()).as_deref() {
        Ok("on") => true,
        Ok("off") => false,
        _ => config,
    }
}

/// The persisted undo state.
pub fn state_path() -> PathBuf {
    runtime_dir().join("pimax-voice-act.state")
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// The passthrough control socket (same rule as `pimax-passthrough`).
pub fn passthrough_socket() -> PathBuf {
    if let Some(p) = std::env::var_os("PIMAX_PASSTHROUGH_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|p| !p.is_empty()) {
        Some(d) => PathBuf::from(d).join("pimax-passthrough.sock"),
        // SAFETY: getuid cannot fail.
        None => PathBuf::from(format!("/tmp/pimax-passthrough-{}.sock", unsafe { libc::getuid() })),
    }
}

/// Skybox image stems in `<wayvr home>/config/wayvr/<dir>`.
pub fn list_skyboxes(wayvr_home: &Path, dir: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(wayvr_home.join("config/wayvr").join(dir))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    (p.extension().and_then(|x| x.to_str()) == Some("dds"))
                        .then(|| p.file_stem()?.to_str().map(str::to_string))
                        .flatten()
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The XSOverlay-format toast WayVR shows (UDP 127.0.0.1:42069).
pub fn toast_json(title: &str, text: &str) -> String {
    serde_json::json!({ "messageType": 1, "title": title, "content": text, "timeout": 3.5, "volume": -1.0 }).to_string()
}

/// Real side effects.
pub struct Real {
    pub wayvr_home: PathBuf,
    pub skybox_dir: String,
    pub toasts: bool,
    pub persist: bool,
    /// The persistent virtual keyboard (lazily connected; `wtype` argv goes through it).
    pub vkbd: VkbdSlot,
}

/// The lazily connected virtual keyboard, with a back-off after a failed connect (no reconnect storms).
#[derive(Default)]
pub struct VkbdSlot {
    kb: Option<crate::vkbd::Keyboard>,
    failed_at: Option<Instant>,
}

impl Real {
    pub fn new(wayvr_home: PathBuf, skybox_dir: String, toasts: bool, persist: bool) -> Self {
        Real { wayvr_home, skybox_dir, toasts, persist, vkbd: VkbdSlot::default() }
    }

    /// Run a `wtype` argv (without "wtype") through the persistent virtual keyboard. `Err((why, sent))`: `sent` =
    /// keys may already have reached the app (then no fallback: `wtype` would type the text a second time).
    fn vkbd_run(&mut self, args: &[String]) -> Result<(), (String, bool)> {
        let ops = crate::vkbd::parse_wtype(args).map_err(|e| (e, false))?;
        let slot = &mut self.vkbd;
        if slot.kb.is_none() {
            if slot.failed_at.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
                return Err(("virtual keyboard unavailable (recent failure)".into(), false));
            }
            match crate::vkbd::Keyboard::connect() {
                Ok(k) => {
                    slot.failed_at = None;
                    slot.kb = Some(k);
                }
                Err(e) => {
                    slot.failed_at = Some(Instant::now());
                    return Err((e, false));
                }
            }
        }
        let r = slot.kb.as_mut().map(|k| k.run(&ops)).unwrap_or(Ok(()));
        if r.is_err() {
            slot.kb = None; // dead connection: reconnect next time
        }
        r.map_err(|e| (e, true))
    }
}

impl Runner for Real {
    fn live(&self) -> bool {
        true
    }
    fn run(&mut self, argv: &[String], capture: bool) -> Result<String, String> {
        use std::process::{Command, Stdio};
        let (prog, args) = argv.split_first().ok_or("empty command")?;
        if prog == "wtype" {
            match self.vkbd_run(args) {
                Ok(()) => {
                    eprintln!("voice: vkbd {}", argv.join(" "));
                    return Ok(String::new());
                }
                Err((e, true)) => return Err(format!("virtual keyboard: {e}")),
                Err((e, false)) => eprintln!("voice: vkbd failed ({e}); falling back to wtype"),
            }
        }
        let prog = expand(prog);
        eprintln!("voice: run {}", argv.join(" "));
        let mut cmd = Command::new(&prog);
        cmd.args(args).stdin(Stdio::null());
        if capture {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", prog.display()))?;
            let t0 = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if t0.elapsed() > Duration::from_secs(8) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err("timed out".into());
                    }
                    // short commands (hyprctl, wayvrctl) end within a few ms: poll finely at first
                    Ok(None) => std::thread::sleep(Duration::from_millis(if t0.elapsed() < Duration::from_millis(100) { 1 } else { 20 })),
                    Err(e) => return Err(e.to_string()),
                }
            }
            let out = child.wait_with_output().map_err(|e| e.to_string())?;
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(format!("{} ({})", out.status, err.lines().last().unwrap_or("").trim()))
            }
        } else {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
            let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", prog.display()))?;
            // reap it in the background; launchers fork and return quickly
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(String::new())
        }
    }

    fn passthrough(&mut self, line: &str) -> Result<String, String> {
        use std::io::{BufRead, BufReader, Write};
        let p = passthrough_socket();
        let mut s = std::os::unix::net::UnixStream::connect(&p).map_err(|_| "passthrough is not running".to_string())?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        writeln!(s, "{line}").map_err(|e| e.to_string())?;
        let mut r = String::new();
        BufReader::new(s).read_line(&mut r).map_err(|e| e.to_string())?;
        let r = r.trim().to_string();
        if r.starts_with("ok") {
            Ok(r)
        } else {
            Err(r)
        }
    }

    fn toast(&mut self, text: &str) {
        eprintln!("voice: {text}");
        if self.toasts {
            if let Ok(s) = std::net::UdpSocket::bind("127.0.0.1:0") {
                let _ = s.send_to(toast_json("Pimax", text).as_bytes(), "127.0.0.1:42069");
            }
        }
    }

    fn captions(&mut self, on: bool) -> Result<(), String> {
        let f = captions_off_path();
        if on {
            match std::fs::remove_file(&f) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        } else {
            std::fs::write(&f, "off\n").map_err(|e| e.to_string())
        }
    }

    fn skyboxes(&mut self) -> Vec<String> {
        list_skyboxes(&self.wayvr_home, &self.skybox_dir)
    }

    fn save_state(&mut self, s: &State) {
        if self.persist {
            let _ = std::fs::write(state_path(), s.to_text());
        }
    }

    fn screens_on_demand(&mut self) {
        write_screens_state(screens_unlimited);
    }

    fn screens_sort(&mut self, on: bool) {
        write_screens_state(|old| screens_state_set(old, "sort", if on { "on" } else { "off" }));
    }

    fn debug_line(&mut self, line: &str) {
        if !line.is_empty() {
            eprintln!("voice: debug: {line}");
        }
        // write + rename: the captions process never reads half a line
        let p = debug_line_path();
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, format!("{line}\n")).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// Prints what would happen; changes nothing. `lines` collects the output (also printed).
#[derive(Default)]
pub struct DryRun {
    pub lines: Vec<String>,
    /// Skybox names to pretend exist.
    pub skyboxes: Vec<String>,
    /// Fail `run` for argv[0] equal to this (tests).
    pub fail: Option<String>,
    pub quiet: bool,
}

impl DryRun {
    fn say(&mut self, l: String) {
        if !self.quiet {
            println!("{l}");
        }
        self.lines.push(l);
    }
}

impl Runner for DryRun {
    fn run(&mut self, argv: &[String], _capture: bool) -> Result<String, String> {
        let shown: Vec<String> = argv.iter().map(|a| if a.contains(' ') || a.is_empty() { format!("'{a}'") } else { a.clone() }).collect();
        self.say(format!("would run: {}", shown.join(" ")));
        if self.fail.as_deref() == argv.first().map(String::as_str) {
            return Err("failed (test)".into());
        }
        Ok("(dry run)".into())
    }
    fn passthrough(&mut self, line: &str) -> Result<String, String> {
        self.say(format!("would send to the passthrough socket: {line}"));
        Ok(format!("ok ({line})"))
    }
    fn toast(&mut self, text: &str) {
        self.say(format!("toast: {text}"));
    }
    fn captions(&mut self, on: bool) -> Result<(), String> {
        self.say(format!("would turn captions {}", if on { "on" } else { "off" }));
        Ok(())
    }
    fn skyboxes(&mut self) -> Vec<String> {
        self.skyboxes.clone()
    }
    fn debug_line(&mut self, line: &str) {
        if !line.is_empty() {
            self.say(format!("debug: {line}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::BUILTIN;

    fn actor() -> Actor {
        Actor::new(Catalogue::from_texts(BUILTIN, None).unwrap(), Some("wayvrctl".into()), State::default())
    }
    fn dry() -> DryRun {
        DryRun { quiet: true, skyboxes: vec!["mountain_lake".into()], ..Default::default() }
    }

    #[test]
    fn preset_switch_and_undo() {
        let mut a = actor();
        let mut r = dry();
        let t = Instant::now();
        assert_eq!(a.hear("Pimax, work three", t, &mut r).as_deref(), Some("vr.preset.work3"));
        assert_eq!(r.lines[0], "would run: pimax desk preset work-3");
        assert_eq!(r.lines[1], "toast: Work mode, 3 screens");
        assert_eq!(a.state.preset.as_deref(), Some("work-3"));
        a.hear("Pimax one screen", t, &mut r);
        r.lines.clear();
        a.hear("Pimax undo", t, &mut r);
        assert_eq!(r.lines[0], "would run: pimax desk preset work-3");
        a.hear("Pimax go back", t, &mut r);
        assert_eq!(a.state.preset.as_deref(), Some("cinema"), "back to the session default");
        r.lines.clear();
        a.hear("Pimax undo", t, &mut r);
        assert_eq!(r.lines, ["toast: Nothing to undo"]);
    }

    #[test]
    fn a_mode_says_which_apps_stay_open_on_hidden_screens() {
        let out = "pimax desk: still open on hidden screens: left (google-chrome), right (pimax)\npimax desk: preset cinema\n";
        assert_eq!(
            preset_toast("Cinema mode", out),
            "Cinema mode. Still open on hidden screens: left (google-chrome), right (pimax). Say \"go to the left screen\""
        );
        assert_eq!(preset_toast("Work mode, 3 screens", "pimax desk: preset work-3\n"), "Work mode, 3 screens");
        // the sort switch and the limit share the state file with the launcher; other lines are kept
        let st = "# VR screens shown\nlimit = 1\npinned = \nsort = on\n";
        let off = screens_state_set(st, "sort", "off");
        assert!(off.contains("limit = 1") && off.contains("sort = off") && !off.contains("sort = on"), "{off}");
        assert!(screens_unlimited(&off).contains("limit = 4") && screens_unlimited(&off).contains("sort = off"));
    }

    #[test]
    fn backgrounds_cycle_name_and_undo() {
        let mut a = actor();
        let mut r = dry();
        let t = Instant::now();
        // the session starts in cinema: dimmed
        a.hear("Pimax next background", t, &mut r);
        assert_eq!(a.state.backdrop.as_deref(), Some("black"));
        assert_eq!(r.lines[0], "would send to the passthrough socket: off");
        assert_eq!(r.lines[1], "would run: wayvrctl skybox off");
        r.lines.clear();
        a.hear("Pimax next background", t, &mut r);
        assert_eq!(a.state.backdrop.as_deref(), Some("sky"));
        // to an image: skybox first, then the camera off
        assert_eq!(r.lines[0], "would run: wayvrctl skybox default");
        assert_eq!(r.lines[1], "would send to the passthrough socket: off");
        r.lines.clear();
        a.hear("Pimax mountain lake background", t, &mut r);
        assert_eq!(r.lines[0], "would run: wayvrctl skybox skybox/mountain_lake.dds");
        a.hear("Pimax next background", t, &mut r);
        assert_eq!(a.state.backdrop.as_deref(), Some("passthrough"), "wraps around");
        a.hear("Pimax undo", t, &mut r);
        assert_eq!(a.state.backdrop.as_deref(), Some("mountain_lake"));
        r.lines.clear();
        a.hear("Pimax background disco", t, &mut r);
        // an unknown name is no match (the model, or "Didn't get", decides), not a failed backdrop
        assert!(r.lines[0].starts_with("toast: Didn't get \"background disco\""), "{:?}", r.lines);
        a.hear("Pimax turn on passthrough", t, &mut r);
        assert_eq!(a.state.backdrop.as_deref(), Some("passthrough"));
        a.hear("Pimax dim the room", t, &mut r);
        assert_eq!(a.state.backdrop.as_deref(), Some("dimmed"));
    }

    #[test]
    fn lock_needs_a_spoken_yes_in_time() {
        let mut a = actor();
        let mut r = dry();
        let t = Instant::now();
        assert_eq!(a.hear("Pimax lock the screen", t, &mut r).as_deref(), Some("omarchy.lock"));
        assert_eq!(r.lines, ["toast: lock the screen? Say \"yes\" to confirm"]);
        r.lines.clear();
        a.hear("yes", t + Duration::from_secs(2), &mut r);
        assert_eq!(r.lines[0], "would run: omarchy-system-lock");
        // no: cancelled, nothing runs
        r.lines.clear();
        a.hear("Pimax lock", t, &mut r);
        a.hear("no wait", t + Duration::from_secs(1), &mut r);
        assert!(!r.lines.iter().any(|l| l.contains("would run")));
        assert!(r.lines.contains(&"toast: Cancelled".to_string()));
        // too late
        r.lines.clear();
        a.hear("Pimax lock", t, &mut r);
        a.tick(t + Duration::from_secs(9), &mut r);
        a.hear("yes", t + Duration::from_secs(10), &mut r);
        assert!(!r.lines.iter().any(|l| l.contains("would run")));
        // "yes" without anything pending does nothing
        r.lines.clear();
        assert_eq!(a.hear("yes", t, &mut r), None);
        assert!(r.lines.is_empty());
    }

    #[test]
    fn wake_only_then_the_command() {
        let mut a = actor();
        let mut r = dry();
        let t = Instant::now();
        assert_eq!(a.hear("Pimax.", t, &mut r), None);
        assert_eq!(r.lines, ["toast: Listening..."]);
        assert_eq!(a.hear("volume up", t + Duration::from_secs(2), &mut r).as_deref(), Some("omarchy.volume.up"));
        // the window is used once
        assert_eq!(a.hear("volume up", t + Duration::from_secs(3), &mut r), None);
        // and expires
        a.hear("Pimax", t, &mut r);
        assert_eq!(a.hear("volume up", t + Duration::from_secs(7), &mut r), None);
        // plain talk never acts
        assert_eq!(a.hear("I think the cinema mode is nice", t, &mut r), None);
    }

    #[test]
    fn slots_output_and_failures() {
        let mut a = actor();
        let mut r = dry();
        let t = Instant::now();
        a.hear("Pimax remind me in 5 minutes to stretch", t, &mut r);
        assert_eq!(r.lines[0], "would run: omarchy-reminder 5 stretch");
        assert_eq!(r.lines[1], "toast: Reminder in 5 min: stretch");
        r.lines.clear();
        a.hear("Pimax send this to workspace 2", t, &mut r);
        assert_eq!(r.lines[0], "would run: hyprctl dispatch 'hl.dsp.window.move({ workspace = \"2\", follow = false })'");
        r.lines.clear();
        a.hear("Pimax battery", t, &mut r);
        assert_eq!(r.lines[1], "toast: (dry run)");
        r.lines.clear();
        r.fail = Some("omarchy-capture-screenshot".into());
        a.hear("Pimax take a screenshot", t, &mut r);
        assert!(r.lines[1].contains("failed"));
        r.lines.clear();
        a.hear("Pimax reset screens", t, &mut r);
        assert_eq!(r.lines[0], "would run: wayvrctl reset-screens");
        r.lines.clear();
        a.hear("Pimax captions off", t, &mut r);
        assert_eq!(r.lines, ["would turn captions off", "toast: Captions off"]);
    }

    #[test]
    fn llm_fallback_is_only_asked_for_unmatched_wake_commands() {
        let mut a = actor();
        let mut r = dry();
        let t = Instant::now();
        let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let c2 = calls.clone();
        a.llm = Some(Box::new(move |text: &str| {
            c2.borrow_mut().push(text.to_string());
            Ok(Some(Match { id: "vr.fps.show".into(), slots: BTreeMap::new(), score: 0 }))
        }));
        a.hear("Pimax volume up", t, &mut r);
        a.hear("how is the weather", t, &mut r);
        assert!(calls.borrow().is_empty());
        assert_eq!(a.hear("Pimax how fast is it running", t, &mut r).as_deref(), Some("vr.fps.show"));
        assert_eq!(calls.borrow().as_slice(), ["how fast is it running"]);
    }

    /// A DryRun-like runner that fakes the compositor for window commands.
    struct Desk {
        dispatched: Vec<String>,
        toasts: Vec<String>,
        active: String,
    }
    impl Runner for Desk {
        fn run(&mut self, argv: &[String], _c: bool) -> Result<String, String> {
            match argv.get(2).map(String::as_str) {
                Some("clients") => Ok(serde_json::json!([
                    {"address":"0x1","mapped":true,"class":"foot","title":"zsh","workspace":{"id":2},"grouped":[],"focusHistoryID":0},
                    {"address":"0x2","mapped":true,"class":"chromium","title":"Monado - Chromium","workspace":{"id":3},"grouped":[],"focusHistoryID":1},
                    {"address":"0x3","mapped":true,"class":"steam","title":"Steam","workspace":{"id":4},"grouped":[],"focusHistoryID":2},
                    {"address":"0x4","mapped":true,"class":"foot","title":"logs","workspace":{"id":3},"grouped":[],"focusHistoryID":3}
                ]).to_string()),
                Some("activewindow") => Ok(self.active.clone()),
                Some(lua) if argv[1] == "dispatch" => {
                    let _ = lua;
                    self.dispatched.push(argv[2].clone());
                    Ok("ok".into())
                }
                _ => Err("?".into()),
            }
        }
        fn passthrough(&mut self, _l: &str) -> Result<String, String> {
            Err("x".into())
        }
        fn toast(&mut self, t: &str) {
            self.toasts.push(t.into());
        }
        fn captions(&mut self, _on: bool) -> Result<(), String> {
            Ok(())
        }
        fn skyboxes(&mut self) -> Vec<String> {
            vec![]
        }
    }
    fn desk() -> Desk {
        Desk { dispatched: vec![], toasts: vec![], active: r#"{"address":"0x2","mapped":true,"class":"chromium","title":"t","workspace":{"id":3},"grouped":[]}"#.into() }
    }

    #[test]
    fn window_phrases_run_the_right_dispatch() {
        let t = Instant::now();
        let cases: &[(&str, &str, &str)] = &[
            ("Max, go to workspace 3", "win.ws.goto", "hl.dsp.focus({ workspace = \"3\" })"),
            ("Max, go to workspace three", "win.ws.goto", "hl.dsp.focus({ workspace = \"3\" })"),
            ("Max, work space two", "win.ws.goto", "hl.dsp.focus({ workspace = \"2\" })"),
            ("Max, war space for", "win.ws.goto", "hl.dsp.focus({ workspace = \"4\" })"),
            ("Max, next workspace", "win.ws.next", "hl.dsp.focus({ workspace = \"e+1\" })"),
            ("Five marks, previous work space", "win.ws.previous", "hl.dsp.focus({ workspace = \"e-1\" })"),
            ("Max, switch to steam", "omarchy.focus.app", "hl.dsp.focus({ window = \"address:0x3\" })"),
            ("Max, please go to the terminal", "omarchy.focus.app", "hl.dsp.focus({ window = \"address:0x1\" })"),
            ("Max, focus browser", "omarchy.focus.app", "hl.dsp.focus({ window = \"address:0x2\" })"),
            ("Max, next tab", "win.tab.next", "key = \"Page_Down\", state = \"down\""),
            ("Max, next tabs", "win.tab.next", "key = \"Page_Down\", state = \"down\""),
            ("Max, previous tap", "win.tab.previous", "key = \"Page_Up\", state = \"down\""),
            ("Max, tab 3", "win.tab.goto", "key = \"3\", state = \"down\""),
            ("Max, tap tree", "win.tab.goto", "key = \"3\", state = \"down\""),
            ("Max, go to tab to", "win.tab.goto", "key = \"2\", state = \"down\""),
            ("Max, new tab", "win.tab.new", "key = \"T\", state = \"down\""),
            ("Max, move this to workspace 2", "omarchy.window.to_workspace", "workspace = \"2\""),
            ("Max, move it to workspace 2", "omarchy.window.to_workspace", "workspace = \"2\""),
            ("Max, fullscreen", "omarchy.window.fullscreen", "fullscreen"),
            ("Max, focus left", "omarchy.focus.left", "direction = \"l\""),
        ];
        for (said, id, dispatched) in cases {
            let mut a = actor();
            let mut r = desk();
            assert_eq!(a.hear(said, t, &mut r).as_deref(), Some(*id), "{said}");
            assert!(r.dispatched.iter().any(|d| d.contains(dispatched)), "{said}: {:?} {:?}", r.dispatched, r.toasts);
        }
        // list windows: a toast, no dispatch
        let (mut a, mut r) = (actor(), desk());
        assert_eq!(a.hear("Max, list windows", t, &mut r).as_deref(), Some("win.list"));
        assert!(r.dispatched.is_empty() && r.toasts[0].starts_with("4 windows"), "{:?}", r.toasts);
    }

    #[test]
    fn closing_needs_a_spoken_yes() {
        let t = Instant::now();
        // "close this" (one app) closes at once, whatever the policy (docs/VOICE-DECISIONS.md 2)
        let (mut a, mut r) = (actor(), desk());
        assert_eq!(a.hear("Max, close this", t, &mut r).as_deref(), Some("win.close"));
        assert!(r.dispatched.iter().any(|d| d.contains("window.close")), "{:?}", r.dispatched);
        // "close window" with one app on the screen (workspace 2: foot) closes at once; with 2+ (workspace 3) it asks
        let (mut a, mut r) = (actor(), desk());
        r.active = r#"{"address":"0x1","mapped":true,"class":"foot","title":"zsh","workspace":{"id":2},"grouped":[]}"#.into();
        assert_eq!(a.hear("Max, close window", t, &mut r).as_deref(), Some("win.ws.close"));
        assert!(r.dispatched.iter().any(|d| d.contains("window.close")), "{:?} {:?}", r.dispatched, r.toasts);
        for (said, id, what) in [("Max, close this window", "win.ws.close", "window.close"), ("Max, close tab", "win.tab.close", "key = \"W\"")] {
            let (mut a, mut r) = (actor(), desk());
            assert_eq!(a.hear(said, t, &mut r).as_deref(), Some(id));
            assert!(r.dispatched.is_empty(), "nothing before the yes");
            assert!(r.toasts[0].contains("Say \"yes\""), "{:?}", r.toasts);
            // anything but yes cancels
            a.hear("Max, next workspace", t, &mut r);
            assert_eq!(r.toasts[1], "Cancelled");
            assert!(!r.dispatched.iter().any(|d| d.contains(what)));
            // yes runs it
            let (mut a, mut r) = (actor(), desk());
            a.hear(said, t, &mut r);
            a.hear("yes", t, &mut r);
            assert!(r.dispatched.iter().any(|d| d.contains(what)), "{said}: {:?}", r.dispatched);
            // too late: nothing
            let (mut a, mut r) = (actor(), desk());
            a.hear(said, t, &mut r);
            a.hear("yes", t + Duration::from_secs(30), &mut r);
            assert!(r.dispatched.is_empty());
        }
    }

    #[test]
    fn ai_switch_runs_only_for_unmatched_commands_and_validated_picks() {
        struct Fixed(&'static str);
        impl crate::llm::LlmClient for Fixed {
            fn complete(&mut self, _s: &str, _u: &str) -> Result<String, String> {
                Ok(self.0.into())
            }
            fn describe(&self) -> String {
                "fixed".into()
            }
        }
        let t = Instant::now();
        // matched by the catalogue: the AI is not asked (it would pick 0x3, steam)
        let (mut a, mut r) = (actor(), desk());
        a.ai = Some(Box::new(Fixed(r#"{"intent":"focus_window","slots":{"address":"0x3"}}"#)));
        a.hear("Max, next workspace", t, &mut r);
        assert!(!r.dispatched.iter().any(|d| d.contains("0x3")));
        // descriptive request: the explicit intent
        a.hear("Max, go to the window where i was reading about monado", t, &mut r);
        assert!(r.dispatched.iter().any(|d| d.contains("address:0x3")), "{:?}", r.dispatched);
        // nothing matches at all: the fallback
        let (mut a, mut r) = (actor(), desk());
        a.ai = Some(Box::new(Fixed(r#"{"intent":"focus_window","slots":{"address":"0x2"}}"#)));
        assert_eq!(a.hear("Max, that thing with the issue tracker", t, &mut r).as_deref(), Some("win.ai"));
        assert!(r.dispatched.iter().any(|d| d.contains("address:0x2")));
        // a made-up address is rejected, and the user hears "Didn't get"
        let (mut a, mut r) = (actor(), desk());
        a.ai = Some(Box::new(Fixed(r#"{"intent":"focus_window","slots":{"address":"0x999"}}"#)));
        assert_eq!(a.hear("Max, that thing with the issue tracker", t, &mut r), None);
        assert!(r.dispatched.is_empty() && r.toasts[0].starts_with("Didn't get"));
        // no AI configured: the same
        let (mut a, mut r) = (actor(), desk());
        assert_eq!(a.hear("Max, that thing with the issue tracker", t, &mut r), None);
    }

    #[test]
    fn debug_lines_fit_and_keep_the_latency() {
        let t = Trace { route: "ai".into(), cands: vec![("omarchy.media.next".into(), 0.8)], decision: "omarchy.media.next 9/9".into(), result: "Didn't get \"open 2 more screens\" (AI: timed out). Say: Max, help".into(), ms: 1290, ..Default::default() };
        let l = t.line(DEBUG_WIDTH);
        assert!(l.chars().count() <= DEBUG_WIDTH && l.ends_with(" 1290ms") && l.starts_with("~media.next .80 > AI "), "{l}");
        let t = Trace { route: "noise".into(), ms: 1, ..Default::default() };
        assert_eq!(t.line(DEBUG_WIDTH), "noise > ignored 1ms");
        let t = Trace { route: "match".into(), ran: Some("vr.preset.cinema".into()), result: "Cinema mode".into(), ms: 3, ..Default::default() };
        assert_eq!(t.line(DEBUG_WIDTH), "= vr.preset.cinema > Cinema mode 3ms");
        let t = Trace { route: "fuzzy".into(), cands: vec![("win.list".into(), 0.95)], ran: Some("win.list".into()), result: "3 windows".into(), ms: 4, ..Default::default() };
        assert_eq!(t.line(DEBUG_WIDTH), "~win.list .95 > 3 windows 4ms");
    }

    #[test]
    fn state_file_round_trip_and_toast_format() {
        let s = State { preset: Some("work-3".into()), backdrop: Some("sky".into()), history: vec![("preset".into(), "cinema".into())] };
        assert_eq!(State::from_text(&s.to_text()), s);
        let j: serde_json::Value = serde_json::from_str(&toast_json("Pimax", "hi \"x\"")).unwrap();
        assert_eq!(j["messageType"], 1);
        assert_eq!(j["content"], "hi \"x\"");
        assert_eq!(find_backdrop(&["passthrough".into(), "sky".into(), "mountain_lake".into()], "space"), Some("sky".into()));
        assert_eq!(find_backdrop(&["passthrough".into(), "mountain_lake".into()], "lake"), Some("mountain_lake".into()));
    }
}
