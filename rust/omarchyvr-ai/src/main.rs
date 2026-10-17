//! `omarchyvr-ai`: install / serve / status / stop the local AI of Pimax voice control (see the crate docs, docs/AI.md).

use omarchyvr_ai::*;
use std::time::Duration;

const USAGE: &str = "omarchyvr-ai: the local AI for Pimax voice control (llama.cpp llama-server + Qwen3.5)

usage:
  omarchyvr-ai install [--runtime vulkan|cpu] [--no-gpu-model] [--no-cpu-model] [--dry-run]
      download the runtime and models into ~/.local/share/omarchyvr/ai (resumable, SHA-256 checked)
  omarchyvr-ai serve [--gpu-layers N|all] [--cpu] [--port P] [--ctx N] [--threads N] [--model FILE] [--foreground]
      start llama-server on 127.0.0.1 (nice 19, lowest best-effort I/O); --cpu = no GPU at all (the 4B model, 3/4 of the
      CPU threads; --model .../Qwen3.5-2B-Q4_K_M.gguf for the faster, less accurate 2B)
  omarchyvr-ai status [--json]
  omarchyvr-ai stop [--router|--asr|--all]
  omarchyvr-ai test [--port P]      one tiny chat request to the running server (prints the latency)
  omarchyvr-ai test --router        one classification by the two-stage router (prints the category and latency)
  omarchyvr-ai test --all           every check in order (router, action model, speech, agent model when it is on), one line each:
                                `check <router|action|speech|agent> <ok|fail|skip> <ms> <detail>` (the app's one Test button)

two stages (ai.toml voice_local = \"two-stage\"; docs/AI.md section 16):
  omarchyvr-ai install --two-stage [--router laya|qwen3.5-0.8b] [--action qwen3.5-4b|qwen3.5-9b] [--dry-run]
      the router (\"Lara\": Laya, Apache-2.0, a Python environment with PyTorch CPU) and the action model
      (default: the ones in ai.toml)
  omarchyvr-ai up [--cpu] [--wait S]   start what ai.toml asks for: the simple model, or the router on port 8081 plus
      the action model on port 8080, fitted to the free VRAM next to the session (Work / SteamVR / a game)
  omarchyvr-ai fit [--json]            what `up` would choose now (model, GPU or CPU, why)
  omarchyvr-ai down                    stop the router, the model and the VRAM watch
  omarchyvr-ai serve --slot router [--router ID] [--foreground]
  omarchyvr-ai watch                   (started by `up`) shrink the model off the GPU when a game needs the memory
  omarchyvr-ai tune-lara               tune Laya to the voice commands (Lara; about an hour, processor only)

Play keeps the graphics card for the game: `up` and `serve` refuse while a Play session (Monado started by
`omarchyvr vr start`, or SteamVR) runs; the local AI starts with Work. --force overrides (tests, by hand).

  omarchyvr-ai get ITEM...             download for the app's Work setup (docs/WORK-FIRST-RUN.md), reusing what is on disk
      (size + SHA-256; Lara: its pinned versions); ITEM: runtime | lara | qwen3.5-4b | qwen3.5-2b | qwen3.5-9b | qwen3.5-0.8b.
      One line per event on stdout: `get plan ID BYTES LABEL` (0 = already here), `get progress ID BYTES`, `get note TEXT`,
      `get done ID`, `get error ID TEXT`, `get cancelled ID`, `get all done`. SIGTERM / SIGINT cancel: the part already
      downloaded is kept for the next try (exit code 130).";

fn main() {
    // started from a WayVR button (through omarchyvr-voice ai-mode): the user's own XDG directories (pid files, models)
    omarchyvr_config::paths::restore_user_xdg();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("install") => cmd_install(&args[1..]),
        Some("serve") => cmd_serve(&args[1..]),
        Some("status") => cmd_status(&args[1..]),
        Some("stop") => cmd_stop(&args[1..]),
        Some("test") => cmd_test(&args[1..]),
        Some("up") => cmd_up(&args[1..]),
        Some("down") => cmd_down(),
        Some("get") => cmd_get(&args[1..]),
        Some("fit") => cmd_fit(&args[1..]),
        Some("watch") => twostage::watch(&Dirs::home()),
        Some("tune-lara") => {
            let dirs = Dirs::home();
            let _ = std::fs::create_dir_all(&dirs.state);
            let _ = std::fs::write(twostage::tune_pid(&dirs), std::process::id().to_string());
            let data = dirs.laya().join("lara-train.jsonl");
            let r = twostage::write_training_rows(&data, &mut |l| println!("{l}")).and_then(|()| twostage::tune_lara(&dirs, &data, &mut |l| println!("{l}")));
            let _ = std::fs::remove_file(twostage::tune_pid(&dirs));
            if r.is_ok() {
                // the running router switches to Lara
                if let Ok(cfg) = omarchyvr_config::ai::AiConfig::load() {
                    if cfg.voice_local == omarchyvr_config::ai::VoiceLocal::TwoStage && cfg.two_stage().router == "lara" && running_slot(&dirs, Slot::Router).is_some() {
                        let _ = twostage::up(&cfg, &dirs, false, Duration::from_secs(120), &mut |l| println!("{l}"));
                    }
                }
            }
            r
        }
        Some("-h" | "--help" | "help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(o) => Err(format!("unknown command {o:?}\n\n{USAGE}")),
    };
    if let Err(e) = r {
        eprintln!("omarchyvr-ai: {e}");
        std::process::exit(1);
    }
}

fn val<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<&'a str, String> {
    it.next().map(String::as_str).ok_or_else(|| format!("{flag} needs a value"))
}

fn cmd_install(args: &[String]) -> Result<(), String> {
    let mut plan = InstallPlan { runtime: RUNTIME_VULKAN, gpu_model: true, cpu_model: true };
    let mut dry = false;
    let (mut two, mut router, mut action) = (false, None::<String>, None::<String>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--two-stage" => two = true,
            "--router" => router = Some(val(&mut it, a)?.into()),
            "--action" => action = Some(val(&mut it, a)?.into()),
            "--runtime" => {
                plan.runtime = match val(&mut it, a)? {
                    "vulkan" => RUNTIME_VULKAN,
                    "cpu" => RUNTIME_CPU,
                    o => return Err(format!("--runtime {o}: vulkan or cpu")),
                }
            }
            "--no-gpu-model" => plan.gpu_model = false,
            "--no-cpu-model" => plan.cpu_model = false,
            "--dry-run" => dry = true,
            o => return Err(format!("install: unknown option {o}")),
        }
    }
    let dirs = Dirs::home();
    if two || router.is_some() || action.is_some() {
        let mut t = omarchyvr_config::ai::AiConfig::load().map(|c| c.two_stage()).unwrap_or_default();
        if let Some(r) = router {
            t.router = r;
        }
        if let Some(a) = action {
            t.action = a;
        }
        for id in [&t.router, &t.action] {
            let m = omarchyvr_config::ai::local_model(id).ok_or_else(|| format!("{id}: not a known model"))?;
            println!("  {} ({} MiB download, licence {}){}", m.label, m.download_mib, m.licence, if twostage::model_installed(&dirs, id) { ": installed" } else { "" });
        }
        if dry {
            return Ok(());
        }
        twostage::install_two_stage(&t, plan.runtime, &dirs, &mut |l| println!("{l}"))?;
        println!("done. Start it with: omarchyvr-ai up");
        return Ok(());
    }
    println!("install into {} ({} MiB):", dirs.data.display(), plan.bytes() >> 20);
    for a in plan.assets() {
        println!("  {}  {}\n      sha256 {}", a.label, a.url, a.sha256);
    }
    if dry {
        return Ok(());
    }
    install(&plan, &dirs, &mut |l| println!("{l}"))?;
    println!("done. Start it with: omarchyvr-ai serve   (CPU only: omarchyvr-ai serve --cpu)");
    Ok(())
}

fn cmd_serve(args: &[String]) -> Result<(), String> {
    let mut o = ServeOpts::default();
    let mut fg = false;
    let mut slot = Slot::Main;
    let mut router: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--gpu-layers" | "-ngl" => {
                o.gpu_layers = match val(&mut it, a)? {
                    "all" => None,
                    n => Some(n.parse().map_err(|_| format!("{a} {n}: a number or all"))?),
                }
            }
            "--cpu" => o.gpu_layers = Some(0),
            "--port" => o.port = val(&mut it, a)?.parse().map_err(|_| "bad --port")?,
            "--ctx" => o.ctx = val(&mut it, a)?.parse().map_err(|_| "bad --ctx")?,
            "--threads" => o.threads = val(&mut it, a)?.parse().map_err(|_| "bad --threads")?,
            "--model" => o.model = Some(val(&mut it, a)?.into()),
            "--foreground" => fg = true,
            "--slot" => {
                slot = match val(&mut it, a)? {
                    "router" => Slot::Router,
                    "main" => Slot::Main,
                    o => return Err(format!("--slot {o}: main or router")),
                }
            }
            "--router" => router = Some(val(&mut it, a)?.to_string()),
            "--force" => {}
            x => return Err(format!("serve: unknown option {x}")),
        }
    }
    play_guard(args)?;
    let dirs = Dirs::home();
    if slot == Slot::Router {
        let id = router.unwrap_or_else(|| omarchyvr_config::ai::AiConfig::load().map(|c| c.two_stage().router).unwrap_or_else(|_| "lara".into()));
        let p = twostage::serve_router(&dirs, &id, fg)?;
        println!("router pid {} on http://127.0.0.1:{} ({}); log {}", p.pid, p.port, p.model, dirs.slot_log(Slot::Router).display());
        return Ok(());
    }
    if !o.cpu() {
        eprintln!("omarchyvr-ai: GPU mode; budget about {GPU_BUDGET_MIB} MiB of VRAM (see docs/AI.md). Use --cpu to keep the GPU for VR only.");
    }
    let p = serve(&o, &dirs, fg)?;
    println!("llama-server pid {} on http://127.0.0.1:{}/v1 ({}, {}); log {}", p.pid, p.port, p.mode, p.model, dirs.server_log().display());
    // wait for the model to load (a few seconds), so callers know when it answers
    for _ in 0..120 {
        if !is_server(p.pid) {
            return Err(format!("llama-server exited; see {}", dirs.server_log().display()));
        }
        if http_get(p.port, "/health", Duration::from_millis(300)).is_ok_and(|(c, _)| c == 200) {
            println!("ready");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    println!("still loading; check `omarchyvr-ai status`");
    Ok(())
}

fn cmd_status(args: &[String]) -> Result<(), String> {
    let dirs = Dirs::home();
    let s = Status::get(&dirs);
    if args.iter().any(|a| a == "--json") {
        println!("{}", s.to_json());
        return Ok(());
    }
    println!("runtime: {}", s.runtime.as_ref().map(|p| p.display().to_string()).unwrap_or("not installed".into()));
    for (a, ok) in &s.models {
        println!("model:   {} {}", a.label, if *ok { "installed" } else { "not installed" });
    }
    match &s.running {
        Some(p) => println!(
            "server:  pid {} on http://127.0.0.1:{}/v1, {}, {}, {}{}",
            p.pid,
            p.port,
            p.mode,
            p.model,
            if s.ready { "ready" } else { "loading" },
            s.gpu_mib.map(|m| format!(", {m} MiB VRAM (budget {GPU_BUDGET_MIB})")).unwrap_or_default()
        ),
        None => println!("server:  stopped"),
    }
    match &s.router {
        Some(r) => println!("router:  pid {} on http://127.0.0.1:{}, {}, {}", r.pid, r.port, r.model, if s.router_ready { "ready" } else { "loading" }),
        None => println!("router:  stopped (two stages only){}", if s.lara { "; Lara tuned" } else if s.laya { "; Laya installed" } else { "" }),
    }
    match (&s.asr, &s.asr_missing) {
        (Some(a), _) => println!("speech:  whisper-server pid {} on port {}, {}{}", a.pid, a.port, if s.asr_ready { "ready" } else { "loading" }, s.asr_gpu_mib.map(|m| format!(", {m} MiB VRAM")).unwrap_or_default()),
        (None, Some(m)) => println!("speech:  stopped ({m})"),
        (None, None) => println!("speech:  stopped (starts with `up` when ai.toml's asr allows the graphics card)"),
    }
    if let (Some((free, total)), Some(sess)) = (s.vram, s.session) {
        println!("vram:    {free} MiB free of {total}, next to {}", sess.label());
    }
    Ok(())
}

fn cmd_stop(args: &[String]) -> Result<(), String> {
    let dirs = Dirs::home();
    if args.iter().any(|a| a == "--all") {
        return cmd_down();
    }
    let slot = if args.iter().any(|a| a == "--router") { Slot::Router } else if args.iter().any(|a| a == "--asr") { Slot::Asr } else { Slot::Main };
    match stop_slot(&dirs, slot)? {
        Some(pid) => println!("stopped the {} server (pid {pid})", slot.id()),
        None => println!("not running"),
    }
    Ok(())
}

fn cmd_up(args: &[String]) -> Result<(), String> {
    let cpu = args.iter().any(|a| a == "--cpu");
    let wait = match args.iter().position(|a| a == "--wait") {
        Some(i) => args.get(i + 1).and_then(|v| v.parse().ok()).ok_or("--wait SECONDS")?,
        None => 120,
    };
    let cfg = omarchyvr_config::ai::AiConfig::load()?;
    // nothing to load: `up` stops what runs (allowed in Play too: it only frees memory)
    if cfg.ai_servers_wanted() {
        play_guard(args)?;
    }
    twostage::up(&cfg, &Dirs::home(), cpu, Duration::from_secs(wait), &mut |l| println!("{l}"))
}

/// Play keeps the graphics card for the game: no AI server starts while a Play session runs (`--force` overrides).
fn play_guard(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--force") {
        return Ok(());
    }
    match twostage::play_running(std::path::Path::new("/proc"), &twostage::session_dir()) {
        Some(why) => Err(format!("{why}: the local AI is not started next to a game (it would take video memory from it); it starts with Work")),
        None => Ok(()),
    }
}

/// SIGTERM / SIGINT during `get`: cancel the running download (its part stays for the next try).
static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_cancel(_: libc::c_int) {
    CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
}

fn cancelled() -> bool {
    CANCEL.load(std::sync::atomic::Ordering::SeqCst)
}

/// `omarchyvr-ai get ITEM...` (see the usage): the downloads of the app's Work setup, one event per line.
fn cmd_get(args: &[String]) -> Result<(), String> {
    use std::io::Write;
    // SAFETY: signal(2) with a handler that only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_cancel as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_cancel as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
    let items: Vec<&str> = args.iter().map(String::as_str).filter(|a| !a.starts_with("--")).collect();
    if items.is_empty() {
        return Err("get: name what to download (runtime, lara, qwen3.5-4b, ...)".into());
    }
    let dirs = Dirs::home();
    let out = |l: String| {
        println!("{l}");
        let _ = std::io::stdout().flush();
    };
    // the plan first, so the app can show every item with its size before anything starts
    for it in &items {
        let (bytes, label) = match *it {
            "runtime" => (if installed_server(&dirs).is_some() { 0 } else { RUNTIME_VULKAN.size }, RUNTIME_VULKAN.label.to_string()),
            "lara" => (if twostage::laya_installed(&dirs) { 0 } else { twostage::LAYA_DISK_MIB << 20 }, "Lara, the voice command router (Laya 421M, Apache-2.0, processor only)".to_string()),
            id => {
                let a = model_asset(id).ok_or_else(|| format!("get: {id} is not something this tool downloads"))?;
                (if twostage::model_installed(&dirs, id) { 0 } else { a.size }, a.label.to_string())
            }
        };
        out(format!("get plan {it} {bytes} {label}"));
    }
    for it in &items {
        let mut last = std::time::Instant::now();
        let mut tick = |bytes: u64| -> bool {
            if cancelled() {
                return false;
            }
            if last.elapsed() >= Duration::from_millis(500) {
                last = std::time::Instant::now();
                out(format!("get progress {it} {bytes}"));
            }
            true
        };
        let mut note = |l: &str| out(format!("get note {}", l.replace('\n', " ")));
        let r: Result<(), String> = match *it {
            "runtime" if installed_server(&dirs).is_some() => {
                note("llama.cpp runtime: already on this PC");
                Ok(())
            }
            "runtime" => fetch_with(&RUNTIME_VULKAN, &dirs.downloads().join(RUNTIME_VULKAN.file), &mut note, Some(&mut tick)).and_then(|tb| unpack_runtime(&RUNTIME_VULKAN, &tb, &dirs).map(|_| ())),
            "lara" => {
                let r = twostage::install_laya_with(&dirs, &mut note, Some(&mut tick));
                if r.is_ok() && !twostage::lara_installed(&dirs) {
                    // Laya as published answers at once; the tuning to the voice commands runs in the background
                    twostage::start_tuning(&dirs, &mut note);
                }
                r
            }
            id => {
                let a = model_asset(id).ok_or_else(|| format!("{id}: no download known"))?;
                fetch_with(&a, &dirs.model_path(&a), &mut note, Some(&mut tick)).map(|_| ())
            }
        };
        match r {
            Ok(()) => out(format!("get done {it}")),
            Err(e) if e == CANCELLED || cancelled() => {
                out(format!("get cancelled {it}"));
                std::process::exit(130);
            }
            Err(e) => {
                out(format!("get error {it} {}", e.replace('\n', " ")));
                return Err(e);
            }
        }
    }
    out("get all done".into());
    Ok(())
}

fn cmd_down() -> Result<(), String> {
    let v = twostage::down(&Dirs::home())?;
    if v.is_empty() {
        println!("not running");
    }
    for (s, pid) in v {
        println!("stopped the {} server (pid {pid})", s.id());
    }
    Ok(())
}

fn cmd_fit(args: &[String]) -> Result<(), String> {
    let dirs = Dirs::home();
    let cfg = omarchyvr_config::ai::AiConfig::load()?;
    let session = twostage::detect_session(std::path::Path::new("/proc"));
    let own = running(&dirs).and_then(|r| gpu_mib(r.pid)).unwrap_or(0);
    let free = twostage::gpu_free_mib().map(|g| g.0 + own);
    let p = twostage::plan(&cfg, &dirs, false, free, session)?;
    let file = p.main_model.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    if args.iter().any(|a| a == "--json") {
        let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        println!("{{\"model\":{},\"gpu\":{},\"router\":{},\"why\":{},\"session\":{},\"free_mib\":{}}}", q(&file), q(&p.gpu.mode()), p.router.as_deref().map(q).unwrap_or("null".into()), q(&p.why), q(session.label()), free.map(|f| f.to_string()).unwrap_or("null".into()));
    } else {
        println!("{file}, {}: {}{}", p.gpu.mode(), p.why, p.router.map(|r| format!("; router {r}")).unwrap_or_default());
    }
    Ok(())
}

fn cmd_test(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--router") {
        let (ms, b) = test_router()?;
        println!("{ms} ms: {}", b.chars().take(400).collect::<String>());
        return Ok(());
    }
    if args.iter().any(|a| a == "--two-stage") {
        return test_two_stage();
    }
    if args.iter().any(|a| a == "--all") {
        return test_all();
    }
    let port = match args.iter().position(|a| a == "--port") {
        Some(i) => args.get(i + 1).and_then(|p| p.parse().ok()).ok_or("bad --port")?,
        None => running(&Dirs::home()).map(|p| p.port).unwrap_or(DEFAULT_PORT),
    };
    use std::io::{Read, Write};
    let body = format!(
        r#"{{"model":"{MODEL_ALIAS}","max_tokens":16,"temperature":0,"messages":[{{"role":"user","content":"Reply with the single word: ready"}}]}}"#
    );
    let t0 = std::time::Instant::now();
    let mut s = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(1)).map_err(|e| format!("port {port}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(60))).ok();
    write!(s, "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).map_err(|e| e.to_string())?;
    let mut out = String::new();
    s.read_to_string(&mut out).map_err(|e| e.to_string())?;
    let b = out.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    println!("{} ms: {}", t0.elapsed().as_millis(), b.chars().take(400).collect::<String>());
    Ok(())
}

/// The category word in a router answer.
fn router_category(body: &str) -> &'static str {
    // Laya: "choice":"system"; the LLM router: the same inside an escaped JSON string
    let body = body.replace("\\\"", "\"");
    ["screens", "apps", "tabs", "typing", "browser", "session", "passthrough", "recording", "captions", "system", "other"]
        .into_iter()
        .filter_map(|c| body.find(&format!("\"{c}\"")).map(|i| (i, c)))
        .min()
        .map(|x| x.1)
        .unwrap_or("?")
}

/// One check line of `test --all`: `check <id> <ok|fail|skip> <ms> <detail>`.
fn check_line(id: &str, state: &str, ms: u128, detail: &str) {
    println!("check {id} {state} {ms} {}", detail.replace('\n', " "));
}

/// A tiny chat request to a slot's server as one check line; false when it failed.
fn check_chat(id: &str, port: u16, what: &str) -> bool {
    let t0 = std::time::Instant::now();
    match chat_once(port) {
        Ok(b) if b.contains("\"choices\"") => {
            check_line(id, "ok", t0.elapsed().as_millis(), what);
            true
        }
        Ok(b) => {
            check_line(id, "fail", t0.elapsed().as_millis(), &format!("{what}: no answer ({})", b.chars().take(80).collect::<String>()));
            false
        }
        Err(e) => {
            check_line(id, "fail", t0.elapsed().as_millis(), &format!("{what}: {e}"));
            false
        }
    }
}

/// Every check in order, one line each (the app's one Test button). Fails (exit 1) when a check that should run failed.
fn test_all() -> Result<(), String> {
    let dirs = Dirs::home();
    let cfg = omarchyvr_config::ai::AiConfig::load().unwrap_or_default();
    let two = cfg.voice_local == omarchyvr_config::ai::VoiceLocal::TwoStage;
    let mut bad = 0;
    // 1. Lara, the router
    if running_slot(&dirs, Slot::Router).is_some() {
        match test_router() {
            Ok((ms, b)) => check_line("router", "ok", ms, &format!("Lara sorted a test command: {}", router_category(&b))),
            Err(e) => {
                bad += 1;
                check_line("router", "fail", 0, &e)
            }
        }
    } else if two {
        bad += 1;
        check_line("router", "fail", 0, "Lara is not running");
    } else {
        check_line("router", "skip", 0, "off (one stage)");
    }
    // 2. the action model
    match running(&dirs) {
        Some(r) => {
            let vram = gpu_mib(r.pid).map(|m| format!("{:.1} GB of video memory", m as f64 / 1024.0)).unwrap_or_else(|| "no video memory".into());
            let free = twostage::gpu_free_mib().map(|(f, _)| format!(", {:.1} GB free", f as f64 / 1024.0)).unwrap_or_default();
            if !check_chat("action", r.port, &format!("{} ({}); {vram}{free}", r.model.trim_end_matches(".gguf"), r.mode)) {
                bad += 1;
            }
        }
        None => {
            bad += 1;
            check_line("action", "fail", 0, "the model is not running");
        }
    }
    // 3. speech recognition (the large model answers /health; off is fine: the small one answers)
    match running_slot(&dirs, Slot::Asr) {
        Some(a) => {
            let t0 = std::time::Instant::now();
            match http_get(a.port, "/health", Duration::from_secs(2)) {
                Ok((200, _)) => check_line("speech", "ok", t0.elapsed().as_millis(), "large model ready"),
                Ok((c, _)) => {
                    bad += 1;
                    check_line("speech", "fail", t0.elapsed().as_millis(), &format!("large model answered {c} (still loading?)"))
                }
                Err(e) => {
                    bad += 1;
                    check_line("speech", "fail", t0.elapsed().as_millis(), &format!("large model: {e}"))
                }
            }
        }
        None => check_line("speech", "skip", 0, "large model off, the small one answers"),
    }
    // 4. the agent model, when it is on
    if cfg.two_stage().agent.is_some() {
        match running_slot(&dirs, Slot::Agent) {
            Some(a) => {
                if !check_chat("agent", a.port, a.model.trim_end_matches(".gguf")) {
                    bad += 1;
                }
            }
            None => check_line("agent", "skip", 0, "on, but not loaded now (agent mode uses the action model)"),
        }
    }
    if bad > 0 {
        Err(format!("{bad} check(s) failed"))
    } else {
        Ok(())
    }
}

/// Both stages, one line for the app: "two stages: router 312 ms: system; model 180 ms; VRAM 3.4 GB in use, 18.2 GB
/// free".
fn test_two_stage() -> Result<(), String> {
    let dirs = Dirs::home();
    let (rms, body) = test_router()?;
    // Laya: "choice":"system"; the LLM router: the same inside an escaped JSON string
    let body = body.replace("\\\"", "\"");
    let cat = ["screens", "apps", "tabs", "typing", "browser", "session", "passthrough", "recording", "captions", "system", "other"]
        .into_iter()
        .filter_map(|c| body.find(&format!("\"{c}\"")).map(|i| (i, c)))
        .min()
        .map(|x| x.1)
        .unwrap_or("?");
    let r = running(&dirs).ok_or("the model is not running: press Start")?;
    let t0 = std::time::Instant::now();
    chat_once(r.port)?;
    let mms = t0.elapsed().as_millis();
    let vram = gpu_mib(r.pid).map(|m| format!("{:.1} GB in use", m as f64 / 1024.0)).unwrap_or_else(|| "none (CPU)".into());
    let free = twostage::gpu_free_mib().map(|(f, _)| format!(", {:.1} GB free", f as f64 / 1024.0)).unwrap_or_default();
    println!("two stages: router {rms} ms: {cat}; model {mms} ms ({}, {}); VRAM {vram}{free}", r.model, r.mode);
    Ok(())
}

/// One tiny chat request to a port.
fn chat_once(port: u16) -> Result<String, String> {
    use std::io::{Read, Write};
    let body = format!(r#"{{"model":"{MODEL_ALIAS}","max_tokens":16,"temperature":0,"messages":[{{"role":"user","content":"Reply with the single word: ready"}}]}}"#);
    let mut s = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(1)).map_err(|e| format!("port {port}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(60))).ok();
    write!(s, "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).map_err(|e| e.to_string())?;
    let mut out = String::new();
    s.read_to_string(&mut out).map_err(|e| e.to_string())?;
    Ok(out.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

/// One classification by the router (Laya's `/v1/systemone` or the small LLM): the latency and the answer.
fn test_router() -> Result<(u128, String), String> {
    use std::io::{Read, Write};
    let dirs = Dirs::home();
    let r = running_slot(&dirs, Slot::Router).ok_or("Lara is not running: press Start")?;
    let (path, body) = if r.model.starts_with("laya") || r.model.starts_with("lara") {
        ("/v1/systemone", r#"{"state":{"heard":"make the music louder"},"questions":{"category":{"type":"choice","instructions":"What is this voice command about?","criteria":{"screens":"VR screens and windows","system":"volume, music, media","other":"anything else"}}}}"#.to_string())
    } else {
        ("/v1/chat/completions", format!(r#"{{"model":"{MODEL_ALIAS}","max_tokens":8,"temperature":0,"messages":[{{"role":"user","content":"One word, screens or system: make the music louder"}}]}}"#))
    };
    let t0 = std::time::Instant::now();
    let mut s = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], r.port).into(), Duration::from_secs(1)).map_err(|e| format!("port {}: {e}", r.port))?;
    s.set_read_timeout(Some(Duration::from_secs(60))).ok();
    write!(s, "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).map_err(|e| e.to_string())?;
    let mut out = String::new();
    s.read_to_string(&mut out).map_err(|e| e.to_string())?;
    let b = out.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    Ok((t0.elapsed().as_millis(), b.to_string()))
}
