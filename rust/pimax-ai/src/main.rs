//! `pimax-ai`: install / serve / status / stop the local AI of Pimax voice control (see the crate docs, docs/AI.md).

use pimax_ai::*;
use std::time::Duration;

const USAGE: &str = "pimax-ai: the local AI for Pimax voice control (llama.cpp llama-server + Qwen3.5)

usage:
  pimax-ai install [--runtime vulkan|cpu] [--no-gpu-model] [--no-cpu-model] [--dry-run]
      download the runtime and models into ~/.local/share/pimax/ai (resumable, SHA-256 checked)
  pimax-ai serve [--gpu-layers N|all] [--cpu] [--port P] [--ctx N] [--threads N] [--model FILE] [--foreground]
      start llama-server on 127.0.0.1 (nice 19, idle I/O); --cpu = no GPU at all (the 4B model, 3/4 of the
      CPU threads; --model .../Qwen3.5-2B-Q4_K_M.gguf for the faster, less accurate 2B)
  pimax-ai status [--json]
  pimax-ai stop
  pimax-ai test [--port P]      one tiny chat request to the running server (prints the latency)";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("install") => cmd_install(&args[1..]),
        Some("serve") => cmd_serve(&args[1..]),
        Some("status") => cmd_status(&args[1..]),
        Some("stop") => cmd_stop(),
        Some("test") => cmd_test(&args[1..]),
        Some("-h" | "--help" | "help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(o) => Err(format!("unknown command {o:?}\n\n{USAGE}")),
    };
    if let Err(e) = r {
        eprintln!("pimax-ai: {e}");
        std::process::exit(1);
    }
}

fn val<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<&'a str, String> {
    it.next().map(String::as_str).ok_or_else(|| format!("{flag} needs a value"))
}

fn cmd_install(args: &[String]) -> Result<(), String> {
    let mut plan = InstallPlan { runtime: RUNTIME_VULKAN, gpu_model: true, cpu_model: true };
    let mut dry = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
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
    println!("install into {} ({} MiB):", dirs.data.display(), plan.bytes() >> 20);
    for a in plan.assets() {
        println!("  {}  {}\n      sha256 {}", a.label, a.url, a.sha256);
    }
    if dry {
        return Ok(());
    }
    install(&plan, &dirs, &mut |l| println!("{l}"))?;
    println!("done. Start it with: pimax-ai serve   (CPU only: pimax-ai serve --cpu)");
    Ok(())
}

fn cmd_serve(args: &[String]) -> Result<(), String> {
    let mut o = ServeOpts::default();
    let mut fg = false;
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
            x => return Err(format!("serve: unknown option {x}")),
        }
    }
    let dirs = Dirs::home();
    if !o.cpu() {
        eprintln!("pimax-ai: GPU mode; budget about {GPU_BUDGET_MIB} MiB of VRAM (see docs/AI.md). Use --cpu to keep the GPU for VR only.");
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
    println!("still loading; check `pimax-ai status`");
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
    Ok(())
}

fn cmd_stop() -> Result<(), String> {
    match stop(&Dirs::home())? {
        Some(pid) => println!("stopped llama-server (pid {pid})"),
        None => println!("not running"),
    }
    Ok(())
}

fn cmd_test(args: &[String]) -> Result<(), String> {
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
