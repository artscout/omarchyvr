//! Optional LLM fallback for voice commands the offline matcher did not understand (only after the wake word).
//!
//! The model gets the list of enabled intent ids (with one example phrase and the slot types) and must answer
//! `{"intent": "<id>", "slots": {...}}` or `{"intent": "none"}`. The answer is validated against the catalogue:
//! an unknown or disabled id, missing or extra slots, or a slot value that fails its type check is rejected, so the
//! model can never produce a command that is not in the catalogue. Intents that need a confirmation still need it.
//!
//! Backends (`[llm]` in the catalogue / user override):
//! * `off`;
//! * `auto` (default): ollama on 127.0.0.1:11434 if it answers within 300 ms, else off;
//! * `ollama`: `url` (default http://127.0.0.1:11434), `model` (default: the first installed model);
//! * `openai`: any OpenAI-compatible chat endpoint, `url` (default https://api.openai.com/v1/chat/completions),
//!   `model`, the key from `api_key_file` or the environment variable named by `api_key_env` (never from the repo);
//!   sent with `curl -K -` so the key is not on the command line.

use crate::catalogue::{Catalogue, Llm, SlotType};
use crate::command::{self, Match};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The instruction + catalogue prompt.
pub fn prompt(c: &Catalogue) -> String {
    let mut s = String::from(
        "You map a short spoken command for a VR desktop to exactly one intent from the list below.\n\
         Reply with JSON only: {\"intent\": \"<id>\", \"slots\": {<name>: <value>}} using an id from the list, \
         or {\"intent\": \"none\"} if nothing fits. Never invent ids or slots. Slot types: number = 1..999, \
         word = one plain word, text/name = a few plain words.\n\nIntents:\n",
    );
    for i in c.enabled() {
        let slots: Vec<String> = i.slots.iter().map(|(k, t)| format!("{k}:{}", format!("{t:?}").to_lowercase())).collect();
        s += &format!(
            "- {} e.g. \"{}\"{}\n",
            i.id,
            i.phrases.first().map(String::as_str).unwrap_or(""),
            if slots.is_empty() { String::new() } else { format!(" slots {}", slots.join(", ")) }
        );
    }
    s
}

/// Check a model answer (the JSON text) against the catalogue.
pub fn validate(c: &Catalogue, answer: &str) -> Result<Option<Match>, String> {
    // models sometimes wrap the JSON: take the outermost {...}
    let a = answer.find('{').zip(answer.rfind('}')).map(|(x, y)| &answer[x..=y]).ok_or("no JSON object")?;
    let v: serde_json::Value = serde_json::from_str(a).map_err(|e| format!("bad JSON: {e}"))?;
    let id = v.get("intent").and_then(|x| x.as_str()).ok_or("no intent")?;
    if id == "none" {
        return Ok(None);
    }
    let i = c.get(id).filter(|i| i.enabled).ok_or_else(|| format!("unknown or disabled intent {id:?}"))?;
    let mut slots = BTreeMap::new();
    let given = v.get("slots").and_then(|s| s.as_object()).cloned().unwrap_or_default();
    for k in given.keys() {
        if !i.slots.contains_key(k) {
            return Err(format!("{id}: unexpected slot {k:?}"));
        }
    }
    for (k, t) in &i.slots {
        let raw = match given.get(k) {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => return Err(format!("{id}: slot {k:?} missing")),
        };
        // the same normalisation as spoken words (numbers as digits, lower case)
        let val = match t {
            SlotType::Number | SlotType::Word | SlotType::Side => command::words(&raw).join(" "),
            SlotType::Screen => command::norm_slot(SlotType::Screen, command::words(&raw).join(" ")),
            SlotType::Text | SlotType::Name => raw.trim().to_string(),
        };
        if !command::slot_ok(*t, &val) {
            return Err(format!("{id}: bad value for {k}: {raw:?}"));
        }
        slots.insert(k.clone(), val);
    }
    Ok(Some(Match { id: id.to_string(), slots, score: 0 }))
}

/// A minimal HTTP/1.1 request over plain TCP (ollama is local): returns the body.
pub fn http(method: &str, url: &str, body: Option<&str>, timeout: Duration) -> Result<String, String> {
    let rest = url.strip_prefix("http://").ok_or("only http:// for the local backend")?;
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let addr = host.to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    let mut s = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_millis(500))).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    parse_response(&String::from_utf8_lossy(&buf))
}

/// Status line, headers and (possibly chunked) body -> body; an error for a non-2xx status.
pub fn parse_response(raw: &str) -> Result<String, String> {
    let (head, body) = raw.split_once("\r\n\r\n").ok_or("no HTTP header")?;
    let status = head.split_whitespace().nth(1).unwrap_or("");
    if !status.starts_with('2') {
        return Err(format!("HTTP {status}"));
    }
    let chunked = head.lines().any(|l| l.to_ascii_lowercase().replace(' ', "") == "transfer-encoding:chunked");
    if !chunked {
        return Ok(body.to_string());
    }
    let mut out = String::new();
    let mut rest = body;
    loop {
        let (size, after) = rest.split_once("\r\n").ok_or("bad chunk")?;
        let n = usize::from_str_radix(size.trim(), 16).map_err(|_| "bad chunk size")?;
        if n == 0 {
            break;
        }
        out += after.get(..n).ok_or("short chunk")?;
        rest = after.get(n + 2..).unwrap_or("");
    }
    Ok(out)
}

/// The text of a chat answer (ollama `message.content`, OpenAI `choices[0].message.content`, Anthropic `content[0].text`).
pub fn chat_content(body: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("bad reply: {e}"))?;
    v.pointer("/message/content")
        .or_else(|| v.pointer("/choices/0/message/content"))
        .or_else(|| v.pointer("/content/0/text"))
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .ok_or_else(|| "no content in the reply".into())
}

/// A configured fallback.
pub struct Backend {
    kind: String,
    url: String,
    model: String,
    key: Option<String>,
    timeout: Duration,
    prompt: String,
}

impl Backend {
    /// Resolve the `[llm]` settings; None = off (also when `auto` finds no local ollama).
    pub fn from_config(c: &Catalogue) -> Result<Option<Backend>, String> {
        let l: &Llm = &c.llm;
        let timeout = Duration::from_secs_f64(if l.timeout_s > 0.0 { l.timeout_s } else { 6.0 });
        let kind = if l.backend.is_empty() { "auto" } else { l.backend.as_str() };
        let mut b = Backend { kind: kind.into(), url: l.url.clone(), model: l.model.clone(), key: None, timeout, prompt: prompt(c) };
        match kind {
            "off" => Ok(None),
            "auto" | "ollama" => {
                if b.url.is_empty() {
                    b.url = "http://127.0.0.1:11434".into();
                }
                let tags = match http("GET", &format!("{}/api/tags", b.url), None, Duration::from_millis(300)) {
                    Ok(t) => t,
                    Err(e) if kind == "auto" => {
                        let _ = e;
                        return Ok(None);
                    }
                    Err(e) => return Err(format!("ollama at {}: {e}", b.url)),
                };
                if b.model.is_empty() {
                    let v: serde_json::Value = serde_json::from_str(&tags).unwrap_or_default();
                    match v.pointer("/models/0/name").and_then(|x| x.as_str()) {
                        Some(m) => b.model = m.to_string(),
                        None if kind == "auto" => return Ok(None),
                        None => return Err("ollama has no models (ollama pull ...)".into()),
                    }
                }
                b.kind = "ollama".into();
                Ok(Some(b))
            }
            "openai" => {
                if b.url.is_empty() {
                    b.url = "https://api.openai.com/v1/chat/completions".into();
                }
                if b.model.is_empty() {
                    return Err("[llm] backend = \"openai\" needs `model`".into());
                }
                let key = if !l.api_key_file.is_empty() {
                    std::fs::read_to_string(crate::catalogue::expand(&l.api_key_file)).map_err(|e| format!("{}: {e}", l.api_key_file))?
                } else if !l.api_key_env.is_empty() {
                    std::env::var(&l.api_key_env).map_err(|_| format!("${} is not set", l.api_key_env))?
                } else {
                    String::new()
                };
                b.key = Some(key.trim().to_string()).filter(|k| !k.is_empty());
                Ok(Some(b))
            }
            other => Err(format!("[llm] backend {other:?}: off, auto, ollama or openai")),
        }
    }

    pub fn describe(&self) -> String {
        format!("{} {} at {}", self.kind, self.model, self.url)
    }

    fn body(&self, text: &str) -> String {
        let mut v = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": self.prompt},
                {"role": "user", "content": text},
            ],
            "stream": false,
            "temperature": 0,
        });
        if self.kind == "ollama" {
            v["format"] = "json".into();
            v["options"] = serde_json::json!({"temperature": 0});
        }
        v.to_string()
    }

    /// Ask the model and validate its answer.
    pub fn classify(&self, c: &Catalogue, text: &str) -> Result<Option<Match>, String> {
        let body = self.body(text);
        let reply = if self.kind == "ollama" {
            http("POST", &format!("{}/api/chat", self.url), Some(&body), self.timeout)?
        } else {
            curl_post(&self.url, self.key.as_deref(), &body, self.timeout)?
        };
        let content = chat_content(&reply)?;
        validate(c, &content)
    }
}

/// curl config syntax: a double-quoted value.
fn curl_quote(s: &str) -> String {
    let mut o = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => o += "\\\\",
            '"' => o += "\\\"",
            '\n' => o += "\\n",
            '\r' => o += "\\r",
            '\t' => o += "\\t",
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The `curl -K -` config (on stdin, so the key never shows in `ps`): Bearer key.
pub fn curl_config(url: &str, key: Option<&str>, body: &str, timeout: Duration) -> String {
    let headers: Vec<String> = key.map(|k| format!("Authorization: Bearer {k}")).into_iter().collect();
    curl_config_headers(url, &headers, body, timeout)
}

/// curl's `max-time`: fractional seconds ("1.50"; whole seconds as "6"), at least 0.2 s (a 1.5 s router deadline
/// must not become 1 s).
pub fn curl_seconds(t: Duration) -> String {
    let s = t.as_secs_f64().max(0.2);
    if s.fract() == 0.0 {
        format!("{}", s as u64)
    } else {
        format!("{s:.2}")
    }
}

/// [`curl_config`] with arbitrary extra headers ("Name: value").
pub fn curl_config_headers(url: &str, headers: &[String], body: &str, timeout: Duration) -> String {
    let mut c = format!(
        "url = {}\nrequest = \"POST\"\nsilent\nshow-error\nfail-with-body\nmax-time = {}\nheader = \"Content-Type: application/json\"\n",
        curl_quote(url),
        curl_seconds(timeout)
    );
    for h in headers {
        c += &format!("header = {}\n", curl_quote(h));
    }
    c += &format!("data-binary = {}\n", curl_quote(body));
    c
}

fn curl_post(url: &str, key: Option<&str>, body: &str, timeout: Duration) -> Result<String, String> {
    let headers: Vec<String> = key.map(|k| format!("Authorization: Bearer {k}")).into_iter().collect();
    curl_post_headers(url, &headers, body, timeout)
}

pub(crate) fn curl_post_headers(url: &str, headers: &[String], body: &str, timeout: Duration) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("curl")
        .args(["-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("curl stdin")?
        .write_all(curl_config_headers(url, headers, body, timeout).as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(http_error(&String::from_utf8_lossy(&out.stderr), &String::from_utf8_lossy(&out.stdout)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A failed request in plain words: "HTTP 400: failed to parse grammar" (the server's own error message when the
/// body has one), else curl's message ("curl: (28) Operation timed out ...").
pub fn http_error(stderr: &str, body: &str) -> String {
    let stderr = stderr.trim();
    let code = stderr.split("error: ").nth(1).and_then(|c| c.split_whitespace().next()).filter(|c| c.chars().all(|x| x.is_ascii_digit()));
    let msg = serde_json::from_str::<serde_json::Value>(body.trim()).ok().and_then(|v| {
        v.pointer("/error/message").or_else(|| v.get("error")).or_else(|| v.get("message")).and_then(|m| m.as_str()).map(|m| m.chars().take(160).collect::<String>())
    });
    match (code, msg) {
        (Some(c), Some(m)) => format!("HTTP {c}: {m}"),
        (Some(c), None) => format!("HTTP {c}"),
        _ if stderr.starts_with("curl") => stderr.to_string(),
        _ => format!("curl: {stderr}"),
    }
}

// ------------------------------------------------------------------------------------------ chat client (AI switch)

/// A chat model for free-form tasks (the window "AI switch"). Behind a trait so tests use a mock and the provider
/// config can move to `pimax_config::ai` without touching the callers.
pub trait LlmClient {
    /// One system + user message in, the model's text out.
    fn complete(&mut self, system: &str, user: &str) -> Result<String, String>;
    /// A short description for logs ("anthropic claude-haiku-5-5").
    fn describe(&self) -> String;
}

/// Provider kinds of `~/.config/pimax/ai.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Messages API (`x-api-key`).
    Anthropic,
    /// OpenAI chat completions (also any OpenAI-compatible server: ollama, llama-server, ...).
    OpenAi,
}

/// The default Anthropic model: the fastest.
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-5-5";

/// `~/.config/pimax/ai.toml`.
pub fn ai_path() -> std::path::PathBuf {
    crate::catalogue::expand("~/.config/pimax/ai.toml")
}

/// A provider from ai.toml. Interim reader (keys at the top level or in `[switch]`; `provider = "name"` picks
/// `[providers.name]`): the GUI's `pimax_config::ai` will replace [`ChatClient::from_ai_text`].
#[derive(Debug, Clone)]
pub struct ChatClient {
    pub kind: Kind,
    pub url: String,
    pub model: String,
    key: Option<String>,
    timeout: Duration,
}

fn toml_str(t: &toml::Table, k: &str) -> String {
    t.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string()
}

impl ChatClient {
    /// Read ai.toml: `Ok(None)` when it does not exist.
    pub fn from_ai_toml(path: &std::path::Path) -> Result<Option<ChatClient>, String> {
        match std::fs::read_to_string(path) {
            Ok(t) => Self::from_ai_text(&t, &|n| std::env::var(n).ok()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Parse ai.toml text (`env` looks up environment variables: injected for tests). `Ok(None)` = no provider set.
    pub fn from_ai_text(text: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Option<ChatClient>, String> {
        let root: toml::Table = text.parse().map_err(|e| format!("ai.toml: {e}"))?;
        let mut t = root.clone();
        if let Some(sw) = root.get("switch").and_then(|v| v.as_table()) {
            for (k, v) in sw {
                t.insert(k.clone(), v.clone());
            }
        }
        let name = toml_str(&t, "provider");
        if !name.is_empty() {
            let p = root.get("providers").and_then(|p| p.get(&name)).and_then(|v| v.as_table()).ok_or_else(|| format!("ai.toml: no [providers.{name}]"))?;
            for (k, v) in p {
                t.insert(k.clone(), v.clone());
            }
        }
        let kind_s = toml_str(&t, "kind").to_lowercase();
        let kind = match kind_s.as_str() {
            "" => return Ok(None),
            "anthropic" => Kind::Anthropic,
            "openai" | "openai-compatible" | "openai_compatible" => Kind::OpenAi,
            other => return Err(format!("ai.toml: kind {other:?}: anthropic, openai or openai-compatible")),
        };
        let mut url = toml_str(&t, "url");
        let mut model = toml_str(&t, "model");
        match kind {
            Kind::Anthropic => {
                if url.is_empty() {
                    url = "https://api.anthropic.com/v1/messages".into();
                } else if !url.contains("/messages") {
                    url = format!("{}/v1/messages", url.trim_end_matches('/'));
                }
                if model.is_empty() {
                    model = DEFAULT_ANTHROPIC_MODEL.into();
                }
            }
            Kind::OpenAi => {
                if url.is_empty() {
                    if kind_s == "openai" {
                        url = "https://api.openai.com/v1/chat/completions".into();
                    } else {
                        return Err("ai.toml: openai-compatible needs `url`".into());
                    }
                } else if !url.contains("/chat/completions") {
                    url = format!("{}/chat/completions", url.trim_end_matches('/'));
                }
                if model.is_empty() {
                    return Err("ai.toml: openai needs `model`".into());
                }
            }
        }
        let key_file = toml_str(&t, "api_key_file");
        let key_env = toml_str(&t, "api_key_env");
        let default_env = if kind == Kind::Anthropic { "ANTHROPIC_API_KEY" } else { "OPENAI_API_KEY" };
        let key = if !toml_str(&t, "api_key").is_empty() {
            Some(toml_str(&t, "api_key"))
        } else if !key_file.is_empty() {
            Some(std::fs::read_to_string(crate::catalogue::expand(&key_file)).map_err(|e| format!("{key_file}: {e}"))?.trim().to_string())
        } else {
            env(if key_env.is_empty() { default_env } else { &key_env }).map(|k| k.trim().to_string())
        }
        .filter(|k| !k.is_empty());
        // Anthropic and the OpenAI cloud need a key; a local OpenAI-compatible server does not.
        if key.is_none() && (kind == Kind::Anthropic || kind_s == "openai") {
            return Err(format!("ai.toml: no API key (api_key_file, or ${})", if key_env.is_empty() { default_env } else { &key_env }));
        }
        let secs = t.get("timeout_s").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(8.0);
        Ok(Some(ChatClient { kind, url, model, key, timeout: Duration::from_secs_f64(secs.clamp(1.0, 60.0)) }))
    }

    /// The request body.
    pub fn body(&self, system: &str, user: &str) -> String {
        match self.kind {
            Kind::Anthropic => serde_json::json!({
                "model": self.model,
                "max_tokens": 200,
                "temperature": 0,
                "system": system,
                "messages": [{"role": "user", "content": user}],
            }),
            Kind::OpenAi => serde_json::json!({
                "model": self.model,
                "temperature": 0,
                "max_tokens": 200,
                "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            }),
        }
        .to_string()
    }

    /// The request headers beyond Content-Type.
    pub fn headers(&self) -> Vec<String> {
        match (self.kind, &self.key) {
            (Kind::Anthropic, k) => {
                let mut h = vec!["anthropic-version: 2023-06-01".to_string()];
                h.extend(k.iter().map(|k| format!("x-api-key: {k}")));
                h
            }
            (Kind::OpenAi, Some(k)) => vec![format!("Authorization: Bearer {k}")],
            (Kind::OpenAi, None) => vec![],
        }
    }
}

impl LlmClient for ChatClient {
    fn complete(&mut self, system: &str, user: &str) -> Result<String, String> {
        let reply = curl_post_headers(&self.url, &self.headers(), &self.body(system, user), self.timeout)?;
        chat_content(&reply)
    }

    fn describe(&self) -> String {
        format!("{} {} at {}", if self.kind == Kind::Anthropic { "anthropic" } else { "openai" }, self.model, self.url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::BUILTIN;

    fn cat() -> Catalogue {
        Catalogue::from_texts(BUILTIN, None).unwrap()
    }

    #[test]
    fn http_errors_say_what_the_server_said() {
        let llama = r#"{"error":{"code":400,"message":"Failed to initialize samplers: failed to parse grammar","type":"invalid_request_error"}}"#;
        assert_eq!(http_error("curl: (22) The requested URL returned error: 400\n", llama), "HTTP 400: Failed to initialize samplers: failed to parse grammar");
        assert_eq!(http_error("curl: (22) The requested URL returned error: 503", ""), "HTTP 503");
        assert_eq!(http_error("curl: (28) Operation timed out after 4001 milliseconds", ""), "curl: (28) Operation timed out after 4001 milliseconds");
        assert!(curl_config_headers("http://x/v1", &[], "{}", Duration::from_secs(2)).contains("\nfail-with-body\n"));
    }

    #[test]
    fn answers_are_validated_against_the_catalogue() {
        let c = cat();
        assert_eq!(validate(&c, r#"{"intent":"vr.preset.cinema"}"#).unwrap().unwrap().id, "vr.preset.cinema");
        assert_eq!(validate(&c, r#"Sure! {"intent": "none"}"#).unwrap(), None);
        let m = validate(&c, r#"{"intent":"omarchy.reminder","slots":{"minutes":"ten","text":"call mum"}}"#).unwrap().unwrap();
        assert_eq!((m.slots["minutes"].as_str(), m.slots["text"].as_str()), ("10", "call mum"));
        let m = validate(&c, r#"{"intent":"omarchy.window.to_workspace","slots":{"n":3}}"#).unwrap().unwrap();
        assert_eq!(m.slots["n"], "3");
        for bad in [
            r#"{"intent":"omarchy.shutdown"}"#,
            r#"{"intent":"run","slots":{"cmd":"rm -rf ~"}}"#,
            r#"{"intent":"vr.preset.cinema","slots":{"x":"1"}}"#,
            r#"{"intent":"omarchy.focus.app","slots":{"app":"slack; reboot"}}"#,
            r#"{"intent":"omarchy.reminder","slots":{"minutes":"5000","text":"x"}}"#,
            r#"{"intent":"omarchy.reminder","slots":{"minutes":"5"}}"#,
            r#"{"intent":"omarchy.reminder","slots":{"minutes":"5","text":"$(reboot)"}}"#,
            "no json here",
        ] {
            assert!(validate(&c, bad).is_err(), "{bad}");
        }
        // a disabled intent is refused too
        let c2 = Catalogue::from_texts(BUILTIN, Some("[[intent]]\nid = \"omarchy.screenshot\"\nenabled = false\n")).unwrap();
        assert!(validate(&c2, r#"{"intent":"omarchy.screenshot"}"#).is_err());
    }

    #[test]
    fn prompt_lists_only_enabled_ids() {
        let c2 = Catalogue::from_texts(BUILTIN, Some("[[intent]]\nid = \"omarchy.screenshot\"\nenabled = false\n")).unwrap();
        let p = prompt(&c2);
        assert!(p.contains("- vr.preset.cinema e.g. \"cinema\""));
        assert!(p.contains("omarchy.reminder") && p.contains("minutes:number"));
        assert!(!p.contains("omarchy.screenshot"));
    }

    #[test]
    fn http_bodies_and_curl_config() {
        assert_eq!(parse_response("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap(), "{}");
        assert_eq!(
            parse_response("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n").unwrap(),
            "abcde"
        );
        assert!(parse_response("HTTP/1.1 404 Not Found\r\n\r\n").is_err());
        assert_eq!(chat_content(r#"{"message":{"content":"x"}}"#).unwrap(), "x");
        assert_eq!(chat_content(r#"{"choices":[{"message":{"content":"y"}}]}"#).unwrap(), "y");
        let cfg = curl_config("https://h/v1", Some("k\"1"), r#"{"a":"b\n"}"#, Duration::from_secs(6));
        assert!(cfg.contains(r#"header = "Authorization: Bearer k\"1""#));
        assert!(cfg.contains(r#"data-binary = "{\"a\":\"b\\n\"}""#));
        assert!(cfg.contains("max-time = 6"));
        assert_eq!((curl_seconds(Duration::from_millis(1500)), curl_seconds(Duration::from_millis(50))), ("1.50".to_string(), "0.20".to_string()));
    }

    #[test]
    fn off_backend_is_off() {
        let c = Catalogue::from_texts(BUILTIN, Some("[llm]\nbackend = \"off\"\n")).unwrap();
        assert!(Backend::from_config(&c).unwrap().is_none());
        let c = Catalogue::from_texts(BUILTIN, Some("[llm]\nbackend = \"openai\"\n")).unwrap();
        assert!(Backend::from_config(&c).is_err(), "openai needs a model");
        let c = Catalogue::from_texts(BUILTIN, Some("[llm]\nbackend = \"magic\"\n")).unwrap();
        assert!(Backend::from_config(&c).is_err());
    }

    #[test]
    fn ai_toml_providers() {
        let no_env = |_: &str| None;
        let env = |n: &str| (n == "ANTHROPIC_API_KEY").then(|| "sk-ant-x\n".to_string());
        // nothing configured
        assert!(ChatClient::from_ai_text("", &no_env).unwrap().is_none());
        // anthropic: default model and url, key from the environment
        let c = ChatClient::from_ai_text("kind = \"anthropic\"\n", &env).unwrap().unwrap();
        assert_eq!((c.model.as_str(), c.url.as_str()), ("claude-haiku-5-5", "https://api.anthropic.com/v1/messages"));
        assert_eq!(c.headers(), ["anthropic-version: 2023-06-01", "x-api-key: sk-ant-x"]);
        let b: serde_json::Value = serde_json::from_str(&c.body("sys", "usr")).unwrap();
        assert_eq!((b["system"].as_str(), b["messages"][0]["content"].as_str()), (Some("sys"), Some("usr")));
        assert!(ChatClient::from_ai_text("kind = \"anthropic\"\n", &no_env).is_err(), "needs a key");
        // a local OpenAI-compatible server: no key, /chat/completions appended
        let c = ChatClient::from_ai_text("[switch]\nkind = \"openai-compatible\"\nurl = \"http://127.0.0.1:11434/v1\"\nmodel = \"qwen3\"\n", &no_env).unwrap().unwrap();
        assert_eq!(c.url, "http://127.0.0.1:11434/v1/chat/completions");
        assert!(c.headers().is_empty());
        let b: serde_json::Value = serde_json::from_str(&c.body("sys", "usr")).unwrap();
        assert_eq!(b["messages"][0]["role"], "system");
        // a named provider; unknown kinds and a missing model are errors
        let t = "[switch]\nprovider = \"mine\"\n[providers.mine]\nkind = \"openai\"\nmodel = \"gpt-x\"\napi_key = \"k\"\n";
        let c = ChatClient::from_ai_text(t, &no_env).unwrap().unwrap();
        assert_eq!((c.model.as_str(), c.headers()[0].as_str()), ("gpt-x", "Authorization: Bearer k"));
        assert!(ChatClient::from_ai_text("kind = \"magic\"\n", &no_env).is_err());
        assert!(ChatClient::from_ai_text("kind = \"openai\"\napi_key = \"k\"\n", &no_env).is_err());
        assert_eq!(chat_content(r#"{"content":[{"type":"text","text":"z"}]}"#).unwrap(), "z");
        let cfg = curl_config_headers("https://h", &["x-api-key: k".into()], "{}", Duration::from_secs(8));
        assert!(cfg.contains("header = \"x-api-key: k\""));
    }
}
