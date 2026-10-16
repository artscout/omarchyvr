//! Stage 1 of the two-stage AI on this PC ("Lara"; docs/AI.md section 16): sort a voice command into a
//! [`Category`] with a router on port 8081 (`pimax-ai serve --slot router`), so that the action model on port 8080
//! gets only that category's tools. Same job as `typesafe/jev-router` online ([`crate::remote`]).
//!
//! Two kinds:
//! * **Laya** (the default, `laya`): the open-weights counterpart of Jev; it speaks Jev's `/v1/systemone` protocol and
//!   answers a `choice` question with a probability per category in one encoder pass (no text generated). The
//!   categories go in as short descriptions ([`laya_criteria`]): Laya reads them within a 192-token option budget.
//! * **LLM** (`qwen3.5-0.8b`): a small model on llama-server, asked like the online classifier (the same prompt and
//!   JSON schema).
//!
//! Both give a [`Class`]: the best category, its probability and the runner-up. When the top probability is low, the
//! act request gets the runner-up's tools too ([`Class::cats`]), and the third when the two together are still unsure.

use crate::agent::{Curl, Provider, Transport, Wire};
use crate::remote::{Category, Class};
use serde_json::{json, Value};
use std::time::Duration;

/// The question Laya answers.
pub const LAYA_INSTRUCTIONS: &str = "Which part of the VR desktop does this voice command control?";

/// The categories as Laya reads them: short, distinct descriptions (the long glosses of the online prompt overflow
/// Laya's option budget and cost accuracy; measured in docs/AI.md section 16).
pub fn laya_gloss(c: Category) -> &'static str {
    match c {
        Category::Screens => "VR screens, windows, workspaces",
        Category::Apps => "apps and programs: open, focus, fullscreen, close",
        Category::Tabs => "tabs inside an app",
        Category::Typing => "type text or press keys",
        Category::Browser => "web search and websites",
        Category::Session => "VR modes and presets, undo",
        Category::Passthrough => "passthrough, the real room, background",
        Category::Recording => "recording and screenshots",
        Category::Captions => "captions, debug line, voice help",
        Category::System => "volume, music, battery, power, reminders",
        Category::Other => "chatter or not a command",
    }
}

/// The `criteria` object of the choice question.
pub fn laya_criteria() -> Value {
    let mut m = serde_json::Map::new();
    for c in Category::ALL {
        m.insert(c.id().into(), laya_gloss(c).into());
    }
    Value::Object(m)
}

/// The `/v1/systemone` body for one command. `model` names the checkpoint ("lara" tuned, "english" published):
/// without it laya-serve picks one by the language it detects, and an English sentence went to the published
/// checkpoint even when the tuned one was the default (measured 2026-10-16).
pub fn laya_body(heard: &str, model: &str) -> Value {
    json!({
        "state": {"voice command": crate::windows::clean_title(heard)},
        "questions": {"category": {"type": "choice", "instructions": LAYA_INSTRUCTIONS, "criteria": laya_criteria()}},
        "model": model,
    })
}

/// Laya's answer: the categories by probability.
pub fn parse_laya(body: &str) -> Result<Class, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad reply from the router: {e}"))?;
    if let Some(e) = v.get("detail").or_else(|| v.get("error")) {
        return Err(format!("router error: {}", e.to_string().chars().take(160).collect::<String>()));
    }
    let a = v.pointer("/answers/category").ok_or("bad reply from the router: no answer")?;
    let mut probs: Vec<(Category, f64)> = a
        .get("probabilities")
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, p)| Some((Category::parse(k)?, p.as_f64()?))).collect())
        .unwrap_or_default();
    if probs.is_empty() {
        let c = a.get("choice").and_then(Value::as_str).and_then(Category::parse).ok_or("bad reply from the router: no choice")?;
        return Ok(Class { category: c, confidence: a.get("confidence").and_then(Value::as_f64).unwrap_or(0.5), second: None, third: None });
    }
    probs.sort_by(|x, y| y.1.total_cmp(&x.1));
    let (category, confidence) = probs[0];
    let second = probs.get(1).map(|p| p.0);
    // the third only when the first two together are still unsure
    let third = probs.get(2).filter(|_| probs[0].1 + probs.get(1).map(|p| p.1).unwrap_or(0.0) < 0.8).map(|p| p.0);
    Ok(Class { category, confidence, second, third })
}

/// The router of the two-stage mode on this PC.
pub enum Router {
    /// `tuned`: Lara (Laya tuned to the commands on this PC), else Laya as published.
    Laya { url: String, transport: Box<dyn Transport>, tuned: bool },
    Llm(Provider),
}

impl Router {
    /// From ai.toml's `[local_two_stage] router` (an id of `pimax_config::ai::ROUTER_MODELS`) at `base`
    /// (`pimax_config::ai::ROUTER_BASE`).
    pub fn new(id: &str, base: &str, transport: Box<dyn Transport>) -> Router {
        let base = base.trim_end_matches('/');
        match pimax_config::ai::local_model(id).map(|m| m.engine) {
            Some(pimax_config::ai::LocalEngine::Llama) => Router::Llm(Provider::new("router", Wire::OpenAi, &format!("{base}/v1/chat/completions"), crate::agent::LOCAL_MODEL, None, transport)),
            _ => Router::Laya { url: format!("{base}/v1/systemone"), transport, tuned: id != "laya" },
        }
    }

    /// The router's `/health`.
    pub fn health_url(&self) -> String {
        let u = match self {
            Router::Laya { url, .. } => url.as_str(),
            Router::Llm(p) => p.endpoint.as_str(),
        };
        let base = u.split("/v1/").next().unwrap_or(u);
        format!("{base}/health")
    }

    /// For the log and the debug line.
    pub fn describe(&self) -> &'static str {
        match self {
            Router::Laya { tuned: true, .. } => "Lara (tuned Laya)",
            Router::Laya { .. } => "Lara (Laya)",
            Router::Llm(_) => "Lara (Qwen3.5-0.8B)",
        }
    }

    /// The request (url, body).
    pub fn request(&self, heard: &str, summary: &str, class_system: &str) -> (String, Value) {
        match self {
            Router::Laya { url, tuned, .. } => (url.clone(), laya_body(heard, if *tuned { "lara" } else { "english" })),
            Router::Llm(p) => {
                let mut b = p.classify_body(class_system, &crate::remote::classify_user(heard, summary));
                b["chat_template_kwargs"] = json!({"enable_thinking": false});
                b["max_tokens"] = 60.into();
                (p.endpoint.clone(), b)
            }
        }
    }

    /// Parse an answer.
    pub fn parse(&self, out: &str) -> Result<Class, String> {
        match self {
            Router::Laya { .. } => parse_laya(out),
            Router::Llm(p) => crate::remote::parse_class(&p.parse(out)?.text),
        }
    }

    fn transport(&self) -> &dyn Transport {
        match self {
            Router::Laya { transport, .. } => transport.as_ref(),
            Router::Llm(p) => p.transport.as_ref(),
        }
    }

    /// Classify on another thread (the act request runs meanwhile); None when the transport cannot be copied.
    pub fn classify_async(&self, heard: &str, summary: &str, class_system: &str, timeout: Duration) -> Option<std::thread::JoinHandle<Result<String, String>>> {
        let mut t = self.transport().fork()?;
        let (url, body) = self.request(heard, summary, class_system);
        let body = body.to_string();
        Some(std::thread::spawn(move || t.post(&url, &[], &body, timeout)))
    }

    /// The raw answer, waiting.
    pub fn post(&mut self, heard: &str, summary: &str, class_system: &str, timeout: Duration) -> Result<String, String> {
        let (url, body) = self.request(heard, summary, class_system);
        match self {
            Router::Laya { transport, .. } => transport.post(&url, &[], &body.to_string(), timeout),
            Router::Llm(p) => p.transport.post(&url, &[], &body.to_string(), timeout),
        }
    }

    /// Classify, waiting.
    pub fn classify(&mut self, heard: &str, summary: &str, class_system: &str, timeout: Duration) -> Result<Class, String> {
        let out = self.post(heard, summary, class_system, timeout)?;
        self.parse(&out)
    }
}

/// The config id of a router (for a copy on another thread).
pub fn router_id(r: Option<&Router>) -> String {
    match r {
        Some(Router::Llm(_)) => "qwen3.5-0.8b".into(),
        Some(Router::Laya { tuned: false, .. }) => "laya".into(),
        _ => "lara".into(),
    }
}

/// The router from ai.toml (two stages on this PC), with curl. `PIMAX_AI_ROUTER_BASE` (loopback only) points it
/// elsewhere.
pub fn from_config(cfg: &pimax_config::ai::AiConfig) -> Option<Router> {
    // PIMAX_AI_ROUTER_BASE: another router address (benchmarks next to a running session)
    let base = std::env::var("PIMAX_AI_ROUTER_BASE").ok().filter(|b| pimax_config::ai::is_loopback(b)).unwrap_or_else(|| pimax_config::ai::ROUTER_BASE.to_string());
    (cfg.voice_local == pimax_config::ai::VoiceLocal::TwoStage).then(|| Router::new(&cfg.two_stage().router, &base, Box::new(Curl)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn laya_request_and_answer() {
        let b = laya_body("Max make the music louder", "lara");
        assert_eq!(b["model"], "lara", "the checkpoint is named, never picked by language");
        assert_eq!(b["questions"]["category"]["type"], "choice");
        let crit = b["questions"]["category"]["criteria"].as_object().unwrap();
        assert_eq!(crit.len(), Category::ALL.len());
        let budget: usize = crit.iter().map(|(k, v)| k.len() + v.as_str().unwrap().len()).sum();
        assert!(budget < 600, "short enough for Laya's option budget ({budget} chars)");
        let ans = r#"{"model":"laya","answers":{"category":{"type":"choice","choice":"system","confidence":0.6,"probabilities":{"system":0.62,"screens":0.2,"apps":0.1,"other":0.08}}},"usage":{"input_tokens":141,"output_tokens":0}}"#;
        let c = parse_laya(ans).unwrap();
        assert_eq!((c.category, c.second, c.third), (Category::System, Some(Category::Screens), None));
        assert!((c.confidence - 0.62).abs() < 1e-9);
        assert_eq!(c.cats(), vec![Category::System, Category::Screens], "unsure: the runner-up's tools too");
        let unsure = r#"{"answers":{"category":{"type":"choice","choice":"apps","probabilities":{"apps":0.35,"screens":0.3,"system":0.2,"other":0.15}}}}"#;
        let c = parse_laya(unsure).unwrap();
        assert_eq!(c.cats(), vec![Category::Apps, Category::Screens, Category::System], "very unsure: three categories");
        let sure = r#"{"answers":{"category":{"type":"choice","choice":"tabs","probabilities":{"tabs":0.95,"apps":0.05}}}}"#;
        assert_eq!(parse_laya(sure).unwrap().cats(), vec![Category::Tabs]);
        assert!(parse_laya(r#"{"detail":"bad request"}"#).unwrap_err().contains("router error"));
        assert!(parse_laya("<html>").is_err());
    }

    #[test]
    fn routers_by_id() {
        let r = Router::new("laya", "http://127.0.0.1:8081/", Box::new(Curl));
        assert_eq!(r.health_url(), "http://127.0.0.1:8081/health");
        assert_eq!(r.request("x", "", "").0, "http://127.0.0.1:8081/v1/systemone");
        let l = Router::new("qwen3.5-0.8b", "http://127.0.0.1:8081", Box::new(Curl));
        let (u, b) = l.request("louder", "3 VR screens", &crate::remote::classify_system());
        assert_eq!(u, "http://127.0.0.1:8081/v1/chat/completions");
        assert!(b["response_format"].is_object() && b["chat_template_kwargs"]["enable_thinking"] == false);
        assert_eq!(l.health_url(), "http://127.0.0.1:8081/health");
        let reply = json!({"choices": [{"message": {"content": "{\"category\":\"system\",\"confidence\":0.9,\"second\":\"none\"}"}, "finish_reason": "stop"}]}).to_string();
        assert_eq!(l.parse(&reply).unwrap().category, Category::System);
    }
}
