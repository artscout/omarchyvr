//! Autofix: ask a language model what is wrong, and get back only fixes from the catalogue.
//!
//! * (a) The provider: the one `~/.config/omarchyvr/ai.toml` names for Autofix (the AI tab: Claude, OpenAI, a local
//!   server, or any OpenAI-compatible one), with its key from the file, the environment, or kept for this session
//!   only. Every key the app knows is removed from the bundle by the redactor.
//! * (b) The exact text that would be sent (the redacted bundle, the prompt and the fix catalogue) with its size.
//!   Nothing leaves the machine before Send.
//! * (c) Send ([`crate::llm`]). The reply goes through `pimax_device::autofix::parse_reply`: EXPLAIN lines, FIX lines
//!   naming a catalogue fix with safe arguments (at most 5), everything else rejected. Each fix is approved or
//!   declined one at a time and runs like a Setup fix (root only through the confirmed pkexec dialog), logged to
//!   `fixes.log` with its undo.
//! * (d) Contribute. A local git branch with a test report and a PATCH.md; the push and the pull request are printed
//!   for the user to run.
//!
//! The app log gets one line per request (size, provider, model), never the content.
use crate::app::AppCore;
use crate::cmd::Cmd;
use crate::findings::CatalogueRun;
use crate::llm::{self, Backend, Reply, Request, Transport};
use crate::machine::Machine;
use crate::redact::Redactor;
use omarchyvr_config::ai::{Feature, Kind};
use pimax_device::autofix::{parse_reply, Proposal};
use pimax_device::Finding;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Output budget of one Autofix answer (Claude's thinking included).
pub const MAX_TOKENS: u32 = 16000;

/// Output budget of one Autofix answer from an OpenAI-style server.
pub const CHAT_MAX_TOKENS: u32 = 4096;

/// Session logs in the bundle and how many of their last lines.
pub const SESSION_LOGS: [(&str, usize); 6] = [("monado.log", 150), ("wayvr.log", 30), ("passthrough.log", 10), ("broker.log", 10), ("ctrltrack.log", 10), ("voice.log", 10)];

/// The instructions of every request.
pub const SYSTEM: &str = "You help someone run a Pimax VR headset on Linux with the open-source omarchyvr stack (Monado as the OpenXR runtime, WayVR for desktop windows in VR, a patched NVIDIA open kernel module). The user message holds the list of fixes this app can run and a redacted diagnostic bundle: check results, versions and the end of the session logs.\n\nAnswer only with lines of these two forms, nothing else:\nEXPLAIN <one sentence for the user>\nFIX <fix id> <name>=<value> ...\n\nStart with the most likely cause. Use at most 6 EXPLAIN lines and at most 5 FIX lines. Propose a FIX only when the bundle shows the problem it solves, and only with an id from the list, giving every argument it names as a plain path or word. If no listed fix applies, say in an EXPLAIN line what the person should check next. No markdown, no code blocks, no other commands.";

/// The demo answer: two fixes, an explanation, one line that is rejected.
pub fn demo_reply(monado_service: &Path) -> String {
    format!(
        "EXPLAIN This Monado build lost cap_sys_nice at its last relink, so the compositor thread cannot get realtime priority (frame drops while the GPU is busy).\n\
         FIX cpu.setcap_monado path={}\n\
         EXPLAIN The keepalive service is not running: the headset display turns off every few seconds without it.\n\
         FIX user.keepalive_enable\n\
         sudo rm -rf ~/.cache/omarchyvr\n",
        monado_service.display()
    )
}

/// What would be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// [`SYSTEM`].
    pub system: String,
    /// The catalogue and the redacted bundle.
    pub user: String,
}

/// Everything the bundle is made of (snapshots from the UI thread; files are read in the background).
#[derive(Debug, Clone)]
pub struct BundleIn {
    /// The findings.
    pub findings: Vec<Finding>,
    /// System lines.
    pub summary: Vec<(String, String)>,
    /// Session logs and how many lines of each.
    pub logs: Vec<(PathBuf, usize)>,
}

/// The last `n` lines of a file through the machine (long lines shortened).
pub fn tail_lines(m: &dyn Machine, p: &Path, n: usize) -> Option<Vec<String>> {
    let len = m.meta(p)?.len;
    let want = len.min(96 * 1024);
    let bytes = m.read_range(p, len - want, want as usize)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if want < len && !lines.is_empty() {
        lines.remove(0);
    }
    let k = lines.len().saturating_sub(n);
    Some(lines[k..].iter().map(|l| if l.chars().count() > 400 { format!("{}...", l.chars().take(400).collect::<String>()) } else { l.to_string() }).collect())
}

/// The bundle text, redacted.
pub fn bundle_text(m: &dyn Machine, b: &BundleIn, r: &Redactor) -> String {
    let mut o = String::from("## System\n");
    for (k, v) in &b.summary {
        o += &format!("{k}: {v}\n");
    }
    let findings: Vec<Value> = b
        .findings
        .iter()
        .map(|f| {
            json!({
                "check": f.check,
                "area": f.area.id(),
                "severity": f.severity.id(),
                "title": f.title,
                "detail": f.detail,
                "fix": f.fix.as_ref().map(|x| json!({"id": x.id, "args": x.args.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>()})),
            })
        })
        .collect();
    o += "\n## Check results (JSON)\n";
    o += &serde_json::to_string_pretty(&findings).unwrap_or_default();
    o += "\n";
    for (p, n) in &b.logs {
        if let Some(lines) = tail_lines(m, p, *n).filter(|l| !l.is_empty()) {
            let name = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            o += &format!("\n## Last {} lines of {name}\n{}\n", lines.len(), lines.join("\n"));
        }
    }
    r.text(&o)
}

/// The user message: the catalogue, then the bundle.
pub fn user_text(bundle: &str) -> String {
    format!("{}\nDiagnostic bundle (redacted):\n\n{bundle}", pimax_device::autofix::catalogue_prompt())
}

/// A parsed answer and what the user decided per line.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The lines.
    pub proposals: Vec<Proposal>,
    /// Per line: approved (true), declined (false), open (None).
    pub decided: Vec<Option<bool>>,
    /// Who answered.
    pub model: String,
    /// Something to know.
    pub note: Option<String>,
    /// Seconds it took.
    pub secs: f32,
}

impl Answer {
    /// From a reply.
    pub fn new(r: Reply, took: Duration) -> Answer {
        let proposals = parse_reply(&r.text);
        Answer { decided: vec![None; proposals.len()], proposals, model: r.model, note: r.note, secs: took.as_secs_f32() }
    }
}

/// A prepared contribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contrib {
    /// `contrib/<date>-<id>`.
    pub branch: String,
    /// Where the report and PATCH.md were written.
    pub dir: PathBuf,
    /// The report's path inside the repository.
    pub report_rel: String,
    /// The commands that make the branch (shown first; run as a job on request).
    pub job: Vec<Cmd>,
    /// Lines for the user to share it (printed, never run).
    pub share: Vec<String>,
    /// The branch was made.
    pub made: bool,
}

/// A branch name for a contribution (`contrib/20261015-dream-air-1405`).
pub fn branch_name(stamp: &str, id: &str) -> String {
    let clean = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>();
    format!("contrib/{}-{}", clean(stamp), clean(id))
}

fn toml_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o += "\\\"",
            '\\' => o += "\\\\",
            '\n' | '\r' | '\t' => o.push(' '),
            c if c.is_control() => {}
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The test report (docs/ARCHITECTURE.md 6.2), redacted.
pub fn report_toml(date: &str, module: &str, host: &[(String, String)], findings: &[Finding], fixes: &[crate::fixlog::Entry], r: &Redactor) -> String {
    let mut o = format!("[report]\nversion = 1\ndate = {}\nmodule = {}\n", toml_str(date), toml_str(module));
    o += "\n[host]\n";
    for (k, v) in host {
        o += &format!("{} = {}\n", k.replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "_"), toml_str(v));
    }
    o += "\n[findings]\n";
    for f in findings {
        o += &format!("{} = {}\n", toml_str(&f.check), toml_str(&format!("{}: {}", f.severity.id(), f.title)));
    }
    for e in fixes.iter().rev() {
        o += &format!("\n[[result]]\nfeature = {}\nstatus = {}\nnote = {}\n", toml_str(&format!("fix {}", e.id)), toml_str(&e.outcome), toml_str(&e.when()));
    }
    r.text(&o)
}

/// PATCH.md: what changed, the evidence, the fixes.
pub fn patch_md(module: &str, findings: &[Finding], fixes: &[crate::fixlog::Entry], explain: &[String], report_rel: &str, r: &Redactor) -> String {
    let mut o = format!("# Test report: {module}\n\n## What changed\n\n- `{report_rel}`: the checks of this machine and the fixes that were applied.\n");
    o += "\n## Evidence\n\n";
    let bad: Vec<&Finding> = findings.iter().filter(|f| f.severity >= pimax_device::Severity::Warn).collect();
    if bad.is_empty() {
        o += "Every check passed.\n";
    }
    for f in bad {
        o += &format!("- {} `{}`: {} ({})\n", f.severity.id(), f.check, f.title, f.detail.lines().next().unwrap_or(""));
    }
    if !explain.is_empty() {
        o += "\n## What the Autofix answer said\n\n";
        for l in explain {
            o += &format!("- {l}\n");
        }
    }
    o += "\n## Fixes applied (fixes.log)\n\n```\n";
    for e in fixes.iter().rev() {
        o += &format!("{}\t{}\t{}\t{}\n", e.when(), e.id, e.outcome, e.commands.join(" && "));
    }
    o += "```\n\nA missing fix becomes a proposal: a new `Fix` entry in `pimax-device` (with its undo), never a command in prose.\n";
    r.text(&o)
}

/// The Autofix page's state.
pub struct AutofixView {
    /// What would be sent (None: being built).
    pub preview: Option<Preview>,
    /// The bundle is being built.
    pub building: bool,
    /// A request runs.
    pub sending: bool,
    /// The last answer.
    pub answer: Option<Result<Answer, String>>,
    /// The last request, for people (size, provider, time).
    pub last_request: String,
    /// Requests sent in this session.
    pub requests: usize,
    /// The prepared contribution.
    pub contrib: Option<Result<Contrib, String>>,
    /// Size of the request body for a backend (cached).
    pub(crate) sized: Option<(Backend, usize)>,
    /// How requests leave the app (shared with the AI tab).
    pub transport: Arc<dyn Transport>,
}

impl AutofixView {
    /// A fresh page with a transport.
    pub fn new(transport: Arc<dyn Transport>) -> AutofixView {
        AutofixView { preview: None, building: false, sending: false, answer: None, last_request: String::new(), requests: 0, contrib: None, sized: None, transport }
    }
}

/// Group thousands for people.
pub fn fmt_bytes(n: usize) -> String {
    if n >= 10_000 {
        format!("{:.1} KB", n as f64 / 1000.0)
    } else {
        format!("{n} bytes")
    }
}

impl AppCore {
    /// The provider Autofix uses (from the AI tab's settings), or why there is none.
    pub fn autofix_backend(&self) -> Result<Backend, String> {
        if !self.ai.loaded {
            return Err("Reading the AI settings...".into());
        }
        self.ai_resolve(Feature::Autofix).map(|r| Backend::from_resolved(&r))
    }

    /// Keep the page ready: the AI settings and the preview.
    pub fn want_autofix(&mut self) {
        self.want_ai();
        if self.autofix.preview.is_none() && !self.autofix.building && self.facts.at != 0 && self.ai.loaded {
            self.autofix_build();
        }
    }

    /// System lines of the bundle (no file access).
    pub fn autofix_summary(&self) -> Vec<(String, String)> {
        let f = &self.facts;
        let m = self.onboarding_module().descriptor();
        let mut v = vec![
            ("app".to_string(), format!("omarchyvr-app {}", env!("CARGO_PKG_VERSION"))),
            ("kernel".to_string(), f.kernel_release.clone().unwrap_or_else(|| "?".into())),
            ("desktop".to_string(), f.compositor.clone().unwrap_or_else(|| "?".into())),
            ("gpu".to_string(), f.doctor("gpu").map(|c| c.detail.clone()).unwrap_or_else(|| "?".into())),
            ("merged_head_sync".to_string(), f.merged_head_sync.clone().unwrap_or_else(|| "not loaded".into())),
            ("headset module".to_string(), format!("{} ({}, {})", m.id, m.name, if m.status == pimax_device::Status::Real { "real" } else { "mock" })),
            ("headset connected".to_string(), f.headset.to_string()),
            ("eye tracker connected".to_string(), f.tobii.to_string()),
            ("gpu kernel events this boot".to_string(), f.kernel.iter().map(|e| e.summary()).collect::<Vec<_>>().join("; ")),
        ];
        for b in f.builds.iter().filter(|b| b.built()) {
            v.push((
                format!("monado {}", b.build.key),
                format!("branch {}, PIMAX_CAM {:?}, cap_sys_nice {:?}", b.branch.as_deref().unwrap_or("?"), b.pimax_cam(), b.sys_nice),
            ));
        }
        v.push((
            "session".to_string(),
            match &self.live.service {
                Some(s) => format!("running (pid {}, {} Hz, eye {}, 6dof {})", s.pid, s.hz.unwrap_or(0), s.eye, s.sixdof),
                None => "not running".into(),
            },
        ));
        v
    }

    /// Build what would be sent (in the background; every key the app knows is removed).
    pub fn autofix_build(&mut self) {
        self.autofix.building = true;
        self.autofix.sized = None;
        let m = self.machine.clone();
        let env = self.env.clone();
        let keys = self.ai_known_keys();
        let mut serials: Vec<String> = Vec::new();
        serials.extend(self.facts.headset_serial.clone());
        serials.extend(self.facts.tobii_serial.clone());
        serials.extend(self.profiles.serial.clone());
        let input = BundleIn {
            findings: self.facts.findings.clone().unwrap_or_default(),
            summary: self.autofix_summary(),
            logs: SESSION_LOGS.iter().map(|(f, n)| (self.paths.session_dir.join(f), *n)).collect(),
        };
        self.task(Some("autofix-bundle"), move || {
            let mut r = Redactor::from_system(&|k| env(k), &serials);
            for k in &keys {
                r = r.with_secret(k.expose());
            }
            let p = Preview { system: SYSTEM.to_string(), user: user_text(&bundle_text(m.as_ref(), &input, &r)) };
            Box::new(move |c: &mut AppCore| {
                c.autofix.building = false;
                c.autofix.preview = Some(p);
            })
        });
    }

    fn autofix_request(&self, b: Backend, p: Preview) -> Request {
        let key = self.ai_resolve(Feature::Autofix).ok().and_then(|r| r.api_key);
        let max_tokens = if b.kind == Kind::Anthropic { MAX_TOKENS } else { CHAT_MAX_TOKENS };
        Request { backend: b, system: p.system, user: p.user, key, scratch: self.paths.state_dir.join("autofix"), max_tokens, effort: "high" }
    }

    /// Bytes the request body would have with the provider in use.
    pub fn autofix_bytes(&mut self) -> Option<usize> {
        let b = self.autofix_backend().ok()?;
        let p = self.autofix.preview.clone()?;
        if let Some((sb, n)) = &self.autofix.sized {
            if *sb == b {
                return Some(*n);
            }
        }
        let mut r = self.autofix_request(b.clone(), p);
        r.key = None;
        let n = llm::request_body(&r).len();
        self.autofix.sized = Some((b, n));
        Some(n)
    }

    /// Why Send is not possible now (None: it is).
    pub fn autofix_blocker(&self) -> Option<String> {
        if self.autofix.sending {
            return Some("A request is running.".into());
        }
        if self.autofix.preview.is_none() {
            return Some("The bundle is being prepared.".into());
        }
        self.autofix_backend().err()
    }

    /// Send: the one place where the bundle leaves the app (to the provider in use).
    pub fn autofix_send(&mut self) {
        if let Some(b) = self.autofix_blocker() {
            self.message = b;
            return;
        }
        let (Ok(b), Some(p)) = (self.autofix_backend(), self.autofix.preview.clone()) else { return };
        let req = self.autofix_request(b.clone(), p);
        let bytes = llm::request_body(&req).len();
        self.autofix.requests += 1;
        // the log gets the size, the provider and the model, never the content
        log::info!("autofix request {}: {bytes} bytes to {} ({}, {})", self.autofix.requests, b.provider, b.kind.id(), b.model);
        self.autofix.last_request = format!("{} to {} at {} UTC", fmt_bytes(bytes), b.label(), crate::applog::now_utc().get(11..19).unwrap_or("?"));
        self.autofix.sending = true;
        self.autofix.answer = None;
        let t = self.autofix.transport.clone();
        let t0 = Instant::now();
        self.task(Some("autofix-send"), move || {
            let r = t.send(&req);
            match &r {
                Ok(x) => log::info!("autofix answer: {} bytes from {} in {:.1} s", x.text.len(), x.model, t0.elapsed().as_secs_f32()),
                Err(e) => log::warn!("autofix request failed: {}", e.lines().next().unwrap_or("")),
            }
            Box::new(move |c: &mut AppCore| {
                c.autofix.sending = false;
                c.autofix.answer = Some(r.map(|x| Answer::new(x, t0.elapsed())));
            })
        });
    }

    /// Approve (runs like a Setup fix: root only after the pkexec confirmation) or decline one proposed fix.
    pub fn autofix_decide(&mut self, i: usize, approve: bool) {
        let Some(Ok(a)) = self.autofix.answer.as_mut() else { return };
        let Some(Proposal::Fix(p)) = a.proposals.get(i).cloned() else { return };
        if a.decided[i].is_some() {
            return;
        }
        a.decided[i] = Some(approve);
        if approve {
            let title = p.fix.id.to_string();
            self.run_catalogue(CatalogueRun { planned: p, undo: false }, &title);
        }
    }

    /// Prepare a contribution: the report and PATCH.md in the state directory (redacted), and the commands that make
    /// the branch. Nothing runs yet.
    pub fn autofix_contribute(&mut self) {
        let stamp = crate::applog::now_utc();
        let date = stamp.get(..10).unwrap_or("").to_string();
        let compact = format!("{}-{}", date.replace('-', ""), stamp.get(11..16).unwrap_or("").replace(':', ""));
        let module = self.onboarding_module().id().to_string();
        let branch = branch_name(&compact, &module);
        let dir = self.paths.state_dir.join("contrib").join(branch.trim_start_matches("contrib/"));
        let report_rel = format!("reports/{date}-{module}.toml");
        let repo = self.paths.repo.clone();
        let findings = self.facts.findings.clone().unwrap_or_default();
        let explain: Vec<String> = match &self.autofix.answer {
            Some(Ok(a)) => a.proposals.iter().filter_map(|p| if let Proposal::Explain(t) = p { Some(t.clone()) } else { None }).collect(),
            _ => Vec::new(),
        };
        let host: Vec<(String, String)> = vec![
            ("kernel".into(), self.facts.kernel_release.clone().unwrap_or_default()),
            ("compositor".into(), self.facts.compositor.clone().unwrap_or_default()),
            ("gpu".into(), self.facts.doctor("gpu").map(|c| c.detail.clone()).unwrap_or_default()),
        ];
        let env = self.env.clone();
        let mut serials: Vec<String> = Vec::new();
        serials.extend(self.facts.headset_serial.clone());
        serials.extend(self.facts.tobii_serial.clone());
        let fixes_path = crate::fixlog::path(&self.paths.state_dir);
        let wt = dir.join("worktree");
        let s = |p: &Path| p.display().to_string();
        let title = format!("Test report: {module} ({date})");
        let job = vec![
            Cmd::new("git", &["-C", &s(&repo), "worktree", "add", "-b", &branch, &s(&wt), "HEAD"]),
            Cmd::new("install", &["-Dm644", &s(&dir.join("report.toml")), &s(&wt.join(&report_rel))]),
            Cmd::new("install", &["-Dm644", &s(&dir.join("PATCH.md")), &s(&wt.join("PATCH.md"))]),
            Cmd::new("git", &["-C", &s(&wt), "add", "--", &report_rel, "PATCH.md"]),
            Cmd::new("git", &["-C", &s(&wt), "commit", "-m", &title]),
            Cmd::new("git", &["-C", &s(&repo), "worktree", "remove", &s(&wt)]),
        ];
        let home = (self.env)("HOME").unwrap_or_default();
        let tilde = |p: String| if !home.is_empty() && p.starts_with(&home) { format!("~{}", &p[home.len()..]) } else { p };
        let share = vec![
            format!("git -C {} push -u origin {branch}", tilde(s(&repo))),
            format!("cd {} && gh pr create --head {branch} --title \"{title}\" --body-file {}", tilde(s(&repo)), tilde(s(&dir.join("PATCH.md")))),
            format!("git -C {} format-patch -1 {branch} -o ~/pimax-contrib", tilde(s(&repo))),
        ];
        let c = Contrib { branch, dir: dir.clone(), report_rel: report_rel.clone(), job, share, made: false };
        self.task(Some("autofix-contrib"), move || {
            let r = (|| {
                let fixes = crate::fixlog::read(&fixes_path);
                let red = Redactor::from_system(&|k| env(k), &serials);
                std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                std::fs::write(dir.join("report.toml"), report_toml(&date, &module, &host, &findings, &fixes, &red)).map_err(|e| e.to_string())?;
                std::fs::write(dir.join("PATCH.md"), patch_md(&module, &findings, &fixes, &explain, &report_rel, &red)).map_err(|e| e.to_string())?;
                Ok(c)
            })();
            Box::new(move |a: &mut AppCore| a.autofix.contrib = Some(r))
        });
    }

    /// Make the branch (a job: git worktree, copy, commit, remove the worktree; the checkout is not touched).
    pub fn autofix_make_branch(&mut self) {
        let Some(Ok(c)) = self.autofix.contrib.clone() else { return };
        let spec = crate::job::JobSpec {
            kind: crate::job::JobKind::Other("contrib".into()),
            label: format!("Make the branch {}", c.branch),
            steps: c.job.iter().cloned().map(crate::job::Step::Run).collect(),
        };
        if self.spawn(spec) {
            if let Some(Ok(c)) = self.autofix.contrib.as_mut() {
                c.made = true;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::llm::Request;
    use omarchyvr_config::ai::{AiConfig, Provider, Secret};
    use omarchyvr_config::Store;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) struct Counting {
        pub probes: AtomicUsize,
        pub sends: AtomicUsize,
        pub reply: String,
    }

    impl Transport for Counting {
        fn probe(&self) -> Vec<Backend> {
            self.probes.fetch_add(1, Ordering::SeqCst);
            vec![Backend { provider: "llama-server".into(), kind: Kind::OpenAiCompatible, model: "test.gguf".into(), base_url: omarchyvr_config::ai::LLAMA_SERVER_BASE.into() }]
        }
        fn send(&self, r: &Request) -> Result<Reply, String> {
            self.sends.fetch_add(1, Ordering::SeqCst);
            assert!(!r.user.contains("sk-ant-api03-TEST-NOT-A-KEY"), "a key reached the request");
            Ok(Reply { text: self.reply.clone(), model: "mock".into(), note: None, cost: None })
        }
    }

    fn demo_with(t: Arc<Counting>) -> AppCore {
        let d = std::env::temp_dir().join(format!("pimax-autofix-{}-{:?}", std::process::id(), std::thread::current().id()));
        let mut a = AppCore::demo(false, Store::new(d.join("cfg"), d.join("cache")));
        a.paths.state_dir = d.join("state");
        a.autofix = AutofixView::new(t);
        a
    }

    #[test]
    fn nothing_is_sent_before_send_and_fixes_wait_for_approval() {
        let svc = "/home/u/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service";
        let t = Arc::new(Counting {
            probes: AtomicUsize::new(0),
            sends: AtomicUsize::new(0),
            reply: format!("EXPLAIN cap lost\nFIX cpu.setcap_monado path={svc}\nFIX user.keepalive_enable\nrm -rf /\n"),
        });
        let mut a = demo_with(t.clone());
        // a provider with a key that must not show up anywhere
        let mut claude = Provider::new("claude", Kind::Anthropic);
        claude.api_key = Some(Secret::new("sk-ant-api03-TEST-NOT-A-KEY-0123456"));
        a.ai.config = AiConfig { default: Some("claude".into()), providers: vec![claude], ..Default::default() };
        a.ai.loaded = true;
        // everything the page does before Send
        for _ in 0..3 {
            a.want_autofix();
        }
        let p = a.autofix.preview.clone().expect("the bundle is built");
        assert!(p.user.contains("cpu.setcap_monado") && p.user.contains("## Check results"));
        assert!(!p.user.contains("/home/u/"), "the home directory is redacted");
        assert!(a.autofix_bytes().unwrap() > p.user.len());
        assert_eq!(t.sends.load(Ordering::SeqCst), 0, "nothing was sent without Send");
        // Send
        a.autofix_send();
        assert_eq!(t.sends.load(Ordering::SeqCst), 1);
        let ans = a.autofix.answer.clone().unwrap().unwrap();
        assert_eq!(ans.proposals.len(), 4);
        assert!(matches!(&ans.proposals[3], Proposal::Rejected { .. }));
        // a root fix opens the pkexec dialog; nothing ran
        a.autofix_decide(1, true);
        assert!(matches!(&a.pending, Some(crate::app::Pending::Root { catalogue: Some(_), .. })), "{:?}", a.pending);
        assert!(a.job.is_none());
        a.cancel_pending();
        // declining runs nothing; a second decision on the same line is ignored
        a.autofix_decide(2, false);
        a.autofix_decide(2, true);
        assert_eq!(a.autofix.answer.as_ref().unwrap().as_ref().unwrap().decided[2], Some(false));
        assert!(a.pending.is_none() && a.job.is_none());
        // no provider: no Send
        a.ai.config = AiConfig::default();
        assert!(a.autofix_blocker().unwrap().contains("no AI provider"));
        let _ = std::fs::remove_dir_all(a.paths.state_dir.parent().unwrap());
    }

    #[test]
    fn a_contribution_is_prepared_locally_and_its_commands_are_safe() {
        let t = Arc::new(Counting { probes: AtomicUsize::new(0), sends: AtomicUsize::new(0), reply: String::new() });
        let mut a = demo_with(t.clone());
        a.autofix_contribute();
        let c = a.autofix.contrib.clone().unwrap().unwrap();
        assert!(c.branch.starts_with("contrib/") && c.branch.ends_with("-dream-air"), "{}", c.branch);
        let report = std::fs::read_to_string(c.dir.join("report.toml")).unwrap();
        assert!(report.starts_with("[report]\nversion = 1\n"), "{report}");
        assert!(report.contains("[findings]"));
        assert!(!report.contains("/home/u/"), "{report}");
        assert!(std::fs::read_to_string(c.dir.join("PATCH.md")).unwrap().starts_with("# Test report: dream-air"));
        for cmd in &c.job {
            crate::safety::check(cmd).unwrap_or_else(|e| panic!("{}: {e}", cmd.display()));
        }
        assert!(c.share.iter().any(|l| l.starts_with("git -C ~/omarchyvr push -u origin contrib/")), "{:?}", c.share);
        // demo mode: the branch is not made
        a.autofix_make_branch();
        assert!(a.job.is_none());
        assert_eq!(t.sends.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(a.paths.state_dir.parent().unwrap());
    }
}
