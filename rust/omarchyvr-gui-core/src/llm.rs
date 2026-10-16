//! Talking to a language model: the providers of `~/.config/omarchyvr/ai.toml` (`omarchyvr_config::ai`), for Autofix and the
//! AI tab's connection test.
//!
//! Lightweight on purpose: a server on this machine (ollama, llama-server, LM Studio) is spoken to with a few lines of
//! HTTP/1.1 over `std::net` (loopback only), anything else through `curl` with the key in curl's config on its stdin
//! (never in its arguments or in a file), so the app carries no TLS stack. Requests leave the app only from the
//! callers' explicit actions (Autofix's Send, the AI tab's Test connection); the local-server probe is HTTP GETs to
//! 127.0.0.1 only.
use omarchyvr_config::ai::{self, Kind, Resolved, Secret};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `anthropic-version` header.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Server-side fallback on a refusal (`"fallbacks": "default"`; not on Haiku, which has none).
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// ollama's port.
pub const OLLAMA_PORT: u16 = 11434;

/// llama.cpp `llama-server`'s port.
pub const LLAMA_PORT: u16 = 8080;

/// How long one answer may take.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(300);

/// Largest reply read from a local server.
const MAX_REPLY: u64 = 4 << 20;

/// The fixed prompt of a connection test (no data from this machine).
pub const PING: &str = "Reply with the single word OK.";

/// A provider and its model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backend {
    /// Provider name in ai.toml (or the server for a detected one).
    pub provider: String,
    /// How to speak to it.
    pub kind: Kind,
    /// Model id.
    pub model: String,
    /// Base URL.
    pub base_url: String,
}

impl Backend {
    /// From a resolved provider.
    pub fn from_resolved(r: &Resolved) -> Backend {
        Backend { provider: r.provider.clone(), kind: r.kind, model: r.model.clone(), base_url: r.base_url.clone() }
    }

    /// For people: "claude (Claude Haiku 5.5)", "local (qwen3:8b on 127.0.0.1:11434)".
    pub fn label(&self) -> String {
        let model = ai::CLAUDE_MODELS.iter().find(|(id, _)| *id == self.model).map_or(self.model.as_str(), |(_, n)| n);
        if self.local() {
            let host = self.base_url.split("//").nth(1).and_then(|r| r.split('/').next()).unwrap_or("");
            format!("{} ({model} on {host})", self.provider)
        } else {
            format!("{} ({model})", self.provider)
        }
    }

    /// Requests stay on this machine.
    pub fn local(&self) -> bool {
        ai::is_loopback(&self.base_url)
    }

    /// The URL a request goes to.
    pub fn endpoint(&self) -> String {
        let b = self.base_url.trim_end_matches('/');
        match self.kind {
            Kind::Anthropic => format!("{b}/v1/messages"),
            _ => format!("{b}/chat/completions"),
        }
    }
}

/// One request.
#[derive(Debug, Clone)]
pub struct Request {
    /// Where it goes.
    pub backend: Backend,
    /// Instructions (empty: none).
    pub system: String,
    /// The message.
    pub user: String,
    /// The key, if the provider needs one.
    pub key: Option<Secret>,
    /// A private directory for curl's request body.
    pub scratch: PathBuf,
    /// Output budget.
    pub max_tokens: u32,
    /// Claude's effort (`low` for a connection test).
    pub effort: &'static str,
}

impl Request {
    /// A connection test: the fixed [`PING`], a small budget.
    pub fn ping(backend: Backend, key: Option<Secret>, scratch: PathBuf) -> Request {
        Request { backend, system: String::new(), user: PING.into(), key, scratch, max_tokens: 256, effort: "low" }
    }
}

/// One answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The text (only the text blocks).
    pub text: String,
    /// The model that answered.
    pub model: String,
    /// Something to know (cut off).
    pub note: Option<String>,
    /// What the request cost, when the provider says (OpenRouter's `usage.cost`): "$0.000034".
    pub cost: Option<String>,
}

fn fallbacks(r: &Request) -> bool {
    r.backend.kind == Kind::Anthropic && !r.backend.model.starts_with("claude-haiku")
}

/// The JSON body of a request (what is sent, byte for byte).
pub fn request_body(r: &Request) -> String {
    let mut messages = Vec::new();
    if r.backend.kind != Kind::Anthropic && !r.system.is_empty() {
        messages.push(json!({"role": "system", "content": r.system}));
    }
    messages.push(json!({"role": "user", "content": r.user}));
    let mut v = match r.backend.kind {
        Kind::Anthropic => json!({"model": r.backend.model, "max_tokens": r.max_tokens, "output_config": {"effort": r.effort}, "messages": messages}),
        Kind::OpenAi => json!({"model": r.backend.model, "messages": messages, "max_completion_tokens": r.max_tokens}),
        Kind::OpenAiCompatible | Kind::ClaudeCli => json!({"model": r.backend.model, "messages": messages, "max_tokens": r.max_tokens, "temperature": 0.2, "stream": false}),
    };
    if is_openrouter(&r.backend.base_url) {
        // the price of each request comes back in usage.cost
        v["usage"] = json!({"include": true});
    }
    if r.backend.kind == Kind::Anthropic {
        if !r.system.is_empty() {
            v["system"] = json!(r.system);
        }
        if fallbacks(r) {
            v["fallbacks"] = json!("default");
        }
    }
    v.to_string()
}

/// OpenRouter (`https://openrouter.ai/api/v1`): one key for many models, the price in every answer.
pub fn is_openrouter(base_url: &str) -> bool {
    base_url.starts_with("https://openrouter.ai/")
}

/// `usage.cost` (USD) as "$0.000034".
pub fn usage_cost(v: &Value) -> Option<String> {
    v.pointer("/usage/cost").and_then(Value::as_f64).map(|c| format!("${c:.6}"))
}

/// The answer of a Messages API call: the text blocks, after checking the stop reason.
pub fn parse_claude(body: &str) -> Result<Reply, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("the API answered something that is not JSON ({e})"))?;
    if let Some(e) = v.get("error") {
        return Err(format!("{}: {}", e["type"].as_str().unwrap_or("error"), e["message"].as_str().unwrap_or("no message")));
    }
    let stop = v["stop_reason"].as_str().unwrap_or("");
    if stop == "refusal" {
        let cat = v["stop_details"]["category"].as_str().map(|c| format!(" ({c})")).unwrap_or_default();
        return Err(format!("The model declined this request{cat}."));
    }
    let text: Vec<&str> = v["content"].as_array().map(|a| a.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect()).unwrap_or_default();
    let note = (stop == "max_tokens").then(|| "The answer was cut off at the output limit; the lines before the cut are shown.".to_string());
    Ok(Reply { text: text.join("\n"), model: v["model"].as_str().unwrap_or("?").to_string(), note, cost: None })
}

/// The answer of a Chat Completions call (OpenAI and compatible servers).
pub fn parse_chat(body: &str) -> Result<Reply, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("the server answered something that is not JSON ({e})"))?;
    if let Some(e) = v.get("error") {
        return Err(e["message"].as_str().or(e.as_str()).unwrap_or("error").to_string());
    }
    let text = v["choices"][0]["message"]["content"].as_str().ok_or("no answer text in the reply")?.to_string();
    let note = (v["choices"][0]["finish_reason"] == "length").then(|| "The answer was cut off at the output limit.".to_string());
    Ok(Reply { text, model: v["model"].as_str().unwrap_or("?").to_string(), note, cost: usage_cost(&v) })
}

/// Split an HTTP/1.1 response: status and body (chunked or with a length).
pub fn parse_http(buf: &[u8]) -> Result<(u16, String), String> {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n").ok_or("incomplete HTTP reply")?;
    let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
    let status = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse::<u16>().ok()).ok_or("bad HTTP status line")?;
    let mut body = buf[end + 4..].to_vec();
    let header = |name: &str| head.lines().find_map(|l| l.strip_prefix(name).map(|v| v.trim().to_string()));
    if header("transfer-encoding:").is_some_and(|v| v.contains("chunked")) {
        body = dechunk(&body)?;
    } else if let Some(n) = header("content-length:").and_then(|v| v.parse::<usize>().ok()) {
        body.truncate(n);
    }
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

fn dechunk(b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let rel = b[i..].windows(2).position(|w| w == b"\r\n").ok_or("truncated chunked body")?;
        let size = String::from_utf8_lossy(&b[i..i + rel]);
        let n = usize::from_str_radix(size.split(';').next().unwrap_or("").trim(), 16).map_err(|_| "bad chunk size")?;
        i += rel + 2;
        if n == 0 {
            return Ok(out);
        }
        if i + n > b.len() {
            return Err("truncated chunk".into());
        }
        out.extend_from_slice(&b[i..i + n]);
        i += n + 2;
        if i > b.len() {
            return Err("truncated chunked body".into());
        }
    }
}

/// One HTTP/1.1 request to a server on this machine. The address is always 127.0.0.1: no other host can be reached
/// through this function.
pub fn http_local(port: u16, method: &str, path: &str, headers: &[String], body: Option<&str>, timeout: Duration) -> Result<(u16, String), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_millis(500))).map_err(|e| format!("127.0.0.1:{port}: {e}"))?;
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(timeout));
    let body = body.unwrap_or("");
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: omarchyvr-app\r\nAccept: application/json\r\nContent-Type: application/json\r\n");
    for h in headers {
        req += h;
        req += "\r\n";
    }
    req += &format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let mut out = req.into_bytes();
    out.extend_from_slice(body.as_bytes());
    s.write_all(&out).map_err(|e| format!("127.0.0.1:{port}: {e}"))?;
    let mut buf = Vec::new();
    s.take(MAX_REPLY).read_to_end(&mut buf).map_err(|e| format!("127.0.0.1:{port}: {e}"))?;
    parse_http(&buf)
}

/// `http://127.0.0.1:PORT/path` or `http://localhost:PORT/path`: (port, path).
pub fn loopback_http(url: &str) -> Option<(u16, String)> {
    let rest = url.strip_prefix("http://")?;
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let (name, port) = host.rsplit_once(':').map(|(n, p)| (n, p.parse::<u16>().ok())).unwrap_or((host, Some(80)));
    (matches!(name, "127.0.0.1" | "localhost")).then_some(())?;
    Some((port?, path))
}

/// Look for local servers (GETs to 127.0.0.1 only, a short timeout).
pub fn probe_local() -> Vec<Backend> {
    let t = Duration::from_millis(1500);
    let mut v = Vec::new();
    let models = |port: u16| -> Option<Vec<String>> {
        let (code, body) = http_local(port, "GET", "/v1/models", &[], None, t).ok()?;
        let j: Value = serde_json::from_str(&body).ok().filter(|_| code == 200)?;
        Some(j["data"].as_array()?.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
    };
    let ollama = models(OLLAMA_PORT).or_else(|| {
        let (code, body) = http_local(OLLAMA_PORT, "GET", "/api/tags", &[], None, t).ok()?;
        let j: Value = serde_json::from_str(&body).ok().filter(|_| code == 200)?;
        Some(j["models"].as_array()?.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect())
    });
    for m in ollama.unwrap_or_default().into_iter().take(12) {
        v.push(Backend { provider: "ollama".into(), kind: Kind::OpenAiCompatible, model: m, base_url: ai::OLLAMA_BASE.into() });
    }
    for m in models(LLAMA_PORT).unwrap_or_default().into_iter().take(4) {
        v.push(Backend { provider: "llama-server".into(), kind: Kind::OpenAiCompatible, model: m, base_url: ai::LLAMA_SERVER_BASE.into() });
    }
    v
}

/// A key or token that can go into a header line: the token68 characters (letters, digits and `-._~+/=`).
pub fn key_ok(k: &Secret) -> bool {
    (8..=1024).contains(&k.len()) && k.expose().bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
}

/// The headers of a request (the key included: only for curl's stdin or a loopback socket).
fn headers(r: &Request) -> Result<Vec<String>, String> {
    let mut h = Vec::new();
    if let Some(k) = &r.key {
        if !key_ok(k) {
            return Err("The key has characters a key never has (letters, digits and -._~+/= only).".into());
        }
        match r.backend.kind {
            Kind::Anthropic => h.push(format!("x-api-key: {}", k.expose())),
            _ => h.push(format!("Authorization: Bearer {}", k.expose())),
        }
    } else if r.backend.kind.needs_key() {
        return Err(format!("{}: no API key", r.backend.provider));
    }
    if r.backend.kind == Kind::Anthropic {
        h.push(format!("anthropic-version: {ANTHROPIC_VERSION}"));
        if fallbacks(r) {
            h.push(format!("anthropic-beta: {FALLBACK_BETA}"));
        }
    }
    Ok(h)
}

/// curl's config for one call (read from its stdin).
pub fn curl_config(url: &str, headers: &[String], body_file: &Path) -> Result<String, String> {
    let f = body_file.display().to_string();
    let bad = |s: &str| s.contains(['"', '\\', '\n', '\r']);
    if bad(url) || bad(&f) || headers.iter().any(|h| bad(h)) {
        return Err("unusual characters in the request".into());
    }
    let proto = if url.starts_with("https://") {
        "=https"
    } else if url.starts_with("http://") {
        "=http"
    } else {
        return Err(format!("{url}: only http and https"));
    };
    let mut c = format!("url = \"{url}\"\n");
    for h in headers {
        c += &format!("header = \"{h}\"\n");
    }
    c += &format!(
        "header = \"content-type: application/json\"\ndata-binary = \"@{f}\"\nwrite-out = \"\\n%{{http_code}}\"\nsilent\nshow-error\nproto = \"{proto}\"\nconnect-timeout = 15\nmax-time = {}\n",
        SEND_TIMEOUT.as_secs()
    );
    Ok(c)
}

/// One call through curl. The body goes through a private temporary file (removed right after).
fn via_curl(r: &Request) -> Result<(u16, String), String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let hs = headers(r)?;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&r.scratch).map_err(|e| format!("{}: {e}", r.scratch.display()))?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let file = r.scratch.join(format!("request-{}-{nanos}.json", std::process::id()));
    let cfg = curl_config(&r.backend.endpoint(), &hs, &file)?;
    let write = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).and_then(|mut f| f.write_all(request_body(r).as_bytes()));
    let result = write.map_err(|e| format!("{}: {e}", file.display())).and_then(|_| {
        let mut child = std::process::Command::new("curl")
            .args(["--config", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("curl: {e} (install curl)"))?;
        if let Some(mut i) = child.stdin.take() {
            let _ = i.write_all(cfg.as_bytes());
        }
        child.wait_with_output().map_err(|e| format!("curl: {e}"))
    });
    let _ = std::fs::remove_file(&file);
    let out = result?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (body, code) = stdout.rsplit_once('\n').unwrap_or(("", stdout.as_ref()));
    match code.trim().parse::<u16>().unwrap_or(0) {
        0 => Err(format!("no answer: {}", String::from_utf8_lossy(&out.stderr).trim())),
        c => Ok((c, body.to_string())),
    }
}

/// Send a request (an explicit user action only).
pub fn send(r: &Request) -> Result<Reply, String> {
    let (code, body) = match loopback_http(&r.backend.endpoint()) {
        Some((port, path)) => http_local(port, "POST", &path, &headers(r)?, Some(&request_body(r)), SEND_TIMEOUT)?,
        None => via_curl(r)?,
    };
    let parse = |b: &str| if r.backend.kind == Kind::Anthropic { parse_claude(b) } else { parse_chat(b) };
    if code != 200 {
        let why = parse(&body).err().unwrap_or_else(|| body.chars().take(200).collect());
        return Err(format!("HTTP {code}: {why}"));
    }
    parse(&body)
}

/// How requests leave the app (tests count them; demo mode answers itself).
pub trait Transport: Send + Sync {
    /// Local servers (GETs to 127.0.0.1 only).
    fn probe(&self) -> Vec<Backend>;
    /// Send one request.
    fn send(&self, r: &Request) -> Result<Reply, String>;
}

/// The real transport.
pub struct Net;

impl Transport for Net {
    fn probe(&self) -> Vec<Backend> {
        probe_local()
    }

    fn send(&self, r: &Request) -> Result<Reply, String> {
        crate::cmd::allow_start("an AI request")?;
        send(r)
    }
}

/// Demo mode: fixed answers; nothing leaves the machine.
pub struct Demo {
    /// Autofix's answer.
    pub reply: String,
}

impl Transport for Demo {
    fn probe(&self) -> Vec<Backend> {
        vec![Backend { provider: "ollama".into(), kind: Kind::OpenAiCompatible, model: "qwen3:8b".into(), base_url: ai::OLLAMA_BASE.into() }]
    }

    fn send(&self, r: &Request) -> Result<Reply, String> {
        let text = if r.user == PING { "OK".to_string() } else { self.reply.clone() };
        Ok(Reply { text, model: format!("{} (demo)", r.backend.model), note: Some("Demo answer: nothing was sent anywhere.".into()), cost: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(kind: Kind, model: &str, base: &str, key: Option<&str>) -> Request {
        Request {
            backend: Backend { provider: "p".into(), kind, model: model.into(), base_url: base.into() },
            system: "S".into(),
            user: "U \"q\"\n".into(),
            key: key.map(Secret::new),
            scratch: PathBuf::new(),
            max_tokens: 100,
            effort: "high",
        }
    }

    #[test]
    fn request_shapes() {
        let c = req(Kind::Anthropic, "claude-opus-5-5", ai::ANTHROPIC_BASE, Some("sk-ant-api03-TEST-NOT-A-KEY"));
        let b: Value = serde_json::from_str(&request_body(&c)).unwrap();
        assert_eq!((b["model"].as_str(), b["system"].as_str(), b["fallbacks"].as_str()), (Some("claude-opus-5-5"), Some("S"), Some("default")));
        assert_eq!(b["messages"][0]["content"], "U \"q\"\n");
        assert!(!request_body(&c).contains("sk-ant"), "the key is never in the body");
        assert_eq!(c.backend.endpoint(), "https://api.anthropic.com/v1/messages");
        let h = headers(&c).unwrap();
        assert!(h.contains(&"anthropic-beta: server-side-fallback-2026-07-01".to_string()) && h[0].starts_with("x-api-key: "));
        // Haiku has no server-side fallback
        let haiku = req(Kind::Anthropic, "claude-haiku-5-5", ai::ANTHROPIC_BASE, Some("sk-ant-api03-TEST-NOT-A-KEY"));
        assert!(!request_body(&haiku).contains("fallbacks") && !headers(&haiku).unwrap().iter().any(|x| x.contains("beta")));
        let o = req(Kind::OpenAi, "gpt-x", ai::OPENAI_BASE, Some("sk-proj-TEST-NOT-A-KEY"));
        let b: Value = serde_json::from_str(&request_body(&o)).unwrap();
        assert_eq!((b["messages"][0]["role"].as_str(), b["max_completion_tokens"].as_u64()), (Some("system"), Some(100)));
        assert!(b.get("temperature").is_none(), "OpenAI reasoning models take no temperature");
        assert_eq!(headers(&o).unwrap(), vec!["Authorization: Bearer sk-proj-TEST-NOT-A-KEY".to_string()]);
        assert_eq!(o.backend.endpoint(), "https://api.openai.com/v1/chat/completions");
        let l = req(Kind::OpenAiCompatible, "qwen3:8b", ai::OLLAMA_BASE, None);
        assert_eq!(loopback_http(&l.backend.endpoint()), Some((11434, "/v1/chat/completions".into())));
        assert!(headers(&l).unwrap().is_empty());
        assert!(headers(&req(Kind::OpenAi, "m", ai::OPENAI_BASE, None)).is_err(), "OpenAI needs a key");
        assert!(headers(&req(Kind::OpenAi, "m", ai::OPENAI_BASE, Some("abc\"\nurl = x"))).is_err());
        assert_eq!(loopback_http("http://192.168.1.2:1234/v1"), None);
        assert_eq!(loopback_http("https://127.0.0.1/v1"), None);
    }

    #[test]
    fn openrouter_asks_for_the_price_and_the_test_shows_it() {
        let r = req(Kind::OpenAiCompatible, ai::OPENROUTER_VOICE_MODEL, ai::OPENROUTER_BASE, Some("sk-or-v1-abc123"));
        let b: Value = serde_json::from_str(&request_body(&r)).unwrap();
        assert_eq!(b["usage"]["include"], true);
        assert!(!request_body(&r).contains("sk-or"));
        assert!(!request_body(&req(Kind::OpenAiCompatible, "q", ai::OLLAMA_BASE, None)).contains("usage"));
        let a = parse_chat(r#"{"model":"x/y","choices":[{"message":{"content":"OK"}}],"usage":{"cost":0.0000254}}"#).unwrap();
        assert_eq!(a.cost.as_deref(), Some("$0.000025"));
        assert_eq!(parse_chat(r#"{"model":"m","choices":[{"message":{"content":"OK"}}]}"#).unwrap().cost, None);
    }

    #[test]
    fn answers() {
        let ok = r#"{"model":"claude-opus-5-5","stop_reason":"end_turn","content":[{"type":"thinking","thinking":""},{"type":"text","text":"EXPLAIN a"},{"type":"text","text":"FIX user.keepalive_enable"}]}"#;
        assert_eq!(parse_claude(ok).unwrap().text, "EXPLAIN a\nFIX user.keepalive_enable");
        let refused = r#"{"model":"x","stop_reason":"refusal","stop_details":{"category":"cyber"},"content":[]}"#;
        assert!(parse_claude(refused).unwrap_err().contains("declined this request (cyber)"));
        let err = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(parse_claude(err).unwrap_err(), "authentication_error: invalid x-api-key");
        assert_eq!(parse_chat(r#"{"model":"m","choices":[{"message":{"content":"OK"},"finish_reason":"stop"}]}"#).unwrap().text, "OK");
        assert!(parse_chat(r#"{"error":{"message":"bad model"}}"#).unwrap_err().contains("bad model"));
        // curl reads its config from stdin: the key is in a header line, never in argv
        let cfg = curl_config("https://api.anthropic.com/v1/messages", &["x-api-key: sk-ant-abc".into()], Path::new("/tmp/x/req.json")).unwrap();
        assert!(cfg.contains("header = \"x-api-key: sk-ant-abc\"\n") && cfg.contains("data-binary = \"@/tmp/x/req.json\"\n"));
        assert!(cfg.contains("proto = \"=https\"") && cfg.contains("write-out = \"\\n%{http_code}\""));
        assert!(curl_config("file:///etc/passwd", &[], Path::new("/x")).is_err());
    }

    #[test]
    fn local_servers_are_spoken_to_over_loopback() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            // read the whole request (headers and body) before answering
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            while !got.ends_with(b"{\"a\":1}") {
                let n = s.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            let body = r#"{"model":"m","choices":[{"message":{"content":"EXPLAIN fine"}}]}"#;
            let chunked = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n{:x}\r\n{}\r\n0\r\n\r\n", 10, &body[..10], body.len() - 10, &body[10..]);
            s.write_all(chunked.as_bytes()).unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        let (code, body) = http_local(port, "POST", "/v1/chat/completions", &["Authorization: Bearer t0k3n-abc".into()], Some("{\"a\":1}"), Duration::from_secs(5)).unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1:"), "{req}");
        assert!(req.contains("\r\nAuthorization: Bearer t0k3n-abc\r\n") && req.ends_with("\r\n\r\n{\"a\":1}"));
        assert_eq!(code, 200);
        assert_eq!(parse_chat(&body).unwrap().text, "EXPLAIN fine");
        assert_eq!(parse_http(b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\n\r\nnoXX").unwrap(), (404, "no".to_string()));
    }
}
