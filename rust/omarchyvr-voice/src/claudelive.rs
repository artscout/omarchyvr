//! `omarchyvr-voice claude-live [--model M] "request" ...`: the real `claude` CLI through the real Claude CLI code path
//! (`Agent::claude_route`) over a simulated desktop. Nothing touches the real desktop: hyprctl is a small simulation
//! (focus and move-to-workspace change its state), every other action is only recorded. Prints, per request, the calls,
//! done, expect, the wall time, the API time, the tokens and the cost. Used to measure the provider; each request is
//! one or more real `claude -p` calls.
use crate::act::{Actor, Runner};
use crate::agent::{Agent, Gaze, Mode, Provider, Settings};
use crate::catalogue::Catalogue;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

struct Win {
    address: &'static str,
    class: &'static str,
    title: &'static str,
    ws: i64,
    focus: i64,
}

/// The simulated desktop: three VR screens (workspaces 1, 2, 3), eight windows.
pub struct SimDesk {
    wins: Vec<Win>,
    pub dispatched: Vec<String>,
    pub ran: Vec<String>,
    pub toasts: Vec<String>,
}

impl SimDesk {
    pub fn new() -> SimDesk {
        let w = |address, class, title, ws, focus| Win { address, class, title, ws, focus };
        SimDesk {
            wins: vec![
                w("0x55d4c8a1b2c0", "firefox", "YouTube - Lofi hip hop radio - beats to relax/study to - Mozilla Firefox", 1, 0),
                w("0x55d4c8a1c3d0", "foot", "~/omarchyvr/rust", 2, 1),
                w("0x55d4c8a1d4e0", "spotify", "Spotify Premium", 3, 2),
                w("0x55d4c8a1e5f0", "foot", "nvim agent.rs", 2, 3),
                w("0x55d4c8a1f600", "Code", "agent.rs - omarchyvr - Visual Studio Code", 1, 4),
                w("0x55d4c8a20710", "discord", "#general | Friends - Discord", 3, 5),
                w("0x55d4c8a21820", "firefox", "Pull requests - laughtt/omarchyvr - Mozilla Firefox", 4, 6),
                w("0x55d4c8a22930", "steam", "Steam", 4, 7),
            ],
            dispatched: Vec::new(),
            ran: Vec::new(),
            toasts: Vec::new(),
        }
    }

    fn clients(&self) -> String {
        let v: Vec<Value> = self.wins.iter().map(|w| json!({"address": w.address, "class": w.class, "initialClass": w.class, "title": w.title, "workspace": {"id": w.ws}, "focusHistoryID": w.focus, "mapped": true, "pid": 1})).collect();
        Value::Array(v).to_string()
    }

    fn active(&self) -> Option<usize> {
        self.wins.iter().position(|w| w.focus == 0)
    }

    fn dispatch(&mut self, line: &str) {
        if let Some(a) = line.split("address:").nth(1).and_then(|x| x.split('"').next()) {
            if line.contains("focus(") {
                if let Some(i) = self.wins.iter().position(|w| w.address == a) {
                    let was = self.wins[i].focus;
                    for w in &mut self.wins {
                        if w.focus < was {
                            w.focus += 1;
                        }
                    }
                    self.wins[i].focus = 0;
                }
            }
        }
        if let (Some(ws), Some(i)) = (line.split("workspace = \"").nth(1).and_then(|x| x.split('"').next()).and_then(|x| x.parse::<i64>().ok()), self.active()) {
            if line.contains("window.move") {
                self.wins[i].ws = ws;
            }
        }
        self.dispatched.push(line.to_string());
    }
}

impl Default for SimDesk {
    fn default() -> Self {
        SimDesk::new()
    }
}

impl Runner for SimDesk {
    fn run(&mut self, argv: &[String], _capture: bool) -> Result<String, String> {
        let a = |i: usize| argv.get(i).map(String::as_str);
        match (a(0), a(1), a(2)) {
            (Some("hyprctl"), Some("-j"), Some("clients")) => Ok(self.clients()),
            (Some("hyprctl"), Some("-j"), Some("activewindow")) => {
                let v: Vec<Value> = serde_json::from_str(&self.clients()).unwrap_or_default();
                Ok(self.active().and_then(|i| v.get(i).cloned()).unwrap_or(json!({})).to_string())
            }
            (Some("hyprctl"), Some("-j"), Some("monitors")) => Ok(json!([
                {"name": "HDMI-A-2", "activeWorkspace": {"id": 1}, "focused": true, "x": 0, "y": 0, "width": 1920},
                {"name": "PIMAX-LEFT", "activeWorkspace": {"id": 2}, "focused": false, "x": -1920, "y": 0, "width": 1920},
                {"name": "PIMAX-RIGHT", "activeWorkspace": {"id": 3}, "focused": false, "x": 1920, "y": 0, "width": 1920}
            ])
            .to_string()),
            (Some("hyprctl"), Some("dispatch"), Some(l)) => {
                self.dispatch(l);
                Ok("ok".into())
            }
            _ => {
                self.ran.push(argv.join(" "));
                Ok(String::new())
            }
        }
    }
    fn passthrough(&mut self, _l: &str) -> Result<String, String> {
        Ok("ok normal".into())
    }
    fn toast(&mut self, t: &str) {
        self.toasts.push(t.to_string());
    }
    fn captions(&mut self, _on: bool) -> Result<(), String> {
        Ok(())
    }
    fn skyboxes(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// Run the requests; the report is a text table plus the calls of each request. `look_at` = the window index the eyes
/// rest on for a request that says "this".
pub fn run(requests: &[String], model: &str, prewarm: bool) -> Result<String, String> {
    let cat = Catalogue::load_default()?;
    let mut out = String::from("request | calls | done | expect | wall ms | api ms | in tok | out tok | cost USD\n");
    let mut total = 0.0;
    for req in requests {
        let mut desk = SimDesk::new();
        let runner = crate::claude::ProcRunner::new(prewarm);
        let p = Provider::claude_cli("claude-cli", model, "", Box::new(runner));
        let mut ag = Agent::new(Settings { mode: Mode::Always, log: false, ..Default::default() }, p, &cat);
        ag.dry_run = false;
        ag.gaze = Some(Box::new(crate::agent::FixedGaze(Some(Gaze { window: Some("0x55d4c8a21820".into()), screen: Some("PIMAX-UP".into()), x: Some(0.4), y: Some(0.6), fixation_ms: Some(900), age_ms: Some(100), ..Default::default() }))));
        if prewarm {
            ag.warm_up();
            std::thread::sleep(Duration::from_millis(2500)); // the process is up and waiting before the command
        }
        let mut a = Actor::new(cat.clone(), Some("wayvrctl".into()), Default::default());
        a.agent = Some(ag);
        let t0 = Instant::now();
        let id = a.hear(&format!("Max {req}"), Instant::now(), &mut desk);
        let wall = t0.elapsed().as_millis();
        let ag = a.agent.as_ref().ok_or("agent lost")?;
        let e = ag.last_claude.clone().unwrap_or(Value::Null);
        let rounds = e["rounds"].as_array().cloned().unwrap_or_default();
        let sum = |k: &str| rounds.iter().map(|r| r[k].as_u64().unwrap_or(0)).sum::<u64>();
        let calls: Vec<String> = rounds.iter().flat_map(|r| r["calls"].as_array().cloned().unwrap_or_default()).map(|c| format!("{}({})", c["name"].as_str().unwrap_or("?"), c["args"].as_object().map(|o| o.values().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect::<Vec<_>>().join(",")).unwrap_or_default())).collect();
        let done = rounds.last().map(|r| r["done"].to_string()).unwrap_or_default();
        let cost = e["cost"].as_f64().unwrap_or(0.0);
        total += cost;
        out += &format!(
            "{req} | {} | {done} ({} round{}) | {} | {wall} | {} | {} | {} | {cost:.5}\n",
            if calls.is_empty() { "-".to_string() } else { calls.join("; ") },
            rounds.len(),
            if rounds.len() == 1 { "" } else { "s" },
            e["expect"].as_str().unwrap_or(""),
            sum("api_ms"),
            sum("tokens_in"),
            sum("tokens_out"),
        );
        out += &format!("    outcome: {:?}; desk: {} ; ran: {} ; toasts: {}\n", id, desk.dispatched.join(" ; "), desk.ran.join(" ; "), desk.toasts.join(" | "));
        if let Some(ve) = e.get("verify").filter(|v| !v.is_null()) {
            out += &format!("    verify: expect words found {}\n", ve["found"]);
        }
        if let Some(end) = e["end"].as_str().filter(|x| !x.is_empty()) {
            out += &format!("    end: {end}\n");
        }
        if let Some(s) = e["say"].as_str().filter(|x| !x.is_empty()) {
            out += &format!("    say: {s}\n");
        }
    }
    out += &format!("total cost: {total:.5} USD\n");
    Ok(out)
}
