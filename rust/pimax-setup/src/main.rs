//! `pimax-setup`: the Pimax app (window "Pimax").
//!
//! Without options it starts the supervisor, which runs the window in a second process and restarts it after a
//! crash (see `supervisor.rs`).
use pimax_config::Store;
use pimax_gui_core::app::AppCore;
use pimax_gui_core::machine::RealMachine;
use pimax_gui_core::paths::Paths;
use pimax_gui_core::preflight::Gate;
use pimax_gui_core::{crash, instance};
use pimax_setup::window::{self, Renderer};
use std::sync::Arc;

const USAGE: &str = "usage: pimax-setup [--demo] [--renderer wgpu|glow] [--no-supervisor]
       pimax-setup --self-test | --report | --screenshots DIR [--real] | --version
  (no option)        open the Pimax window: desk mode, setup checklist with fixes, tools, logs and history, wizard.
                     A small supervisor process runs the window and restarts it after a crash (crash reports in
                     ~/.local/state/pimax/crashes, the log in ~/.local/state/pimax/logs/pimax-setup.log)
  --demo             the window with built-in demo data; nothing is started, signalled or written
  --renderer R       glow (OpenGL, the default) or wgpu (Vulkan; builds with the `wgpu` feature, glow when it cannot start)
  --no-supervisor    run the window in this process (no restart after a crash)
  --self-test        collect everything once (read-only: /proc, /sys, journalctl -k, getcap, modinfo, pimax desk help),
                     print one summary line and exit 0; starts nothing
  --report           write a problem report (redacted .tar.gz, ~/.local/state/pimax/reports) and print its path
  --screenshots DIR  render every page from built-in demo data to DIR/*.png (CPU only, no window) and exit
      --real         ... from this machine's state instead (read-only; for bug reports)
  --version          print the version
environment: PIMAX_RENDERER, PIMAX_NO_SUPERVISOR=1, PIMAX_REPO (default ~/pimax-re), PIMAX_VR_SESSION_DIR,
             PIMAX_MONADO_BUILD, PIMAX_SYSFS_ROOT, PIMAX_PROC_ROOT";

fn sync_core() -> AppCore {
    let env = |k: &str| std::env::var(k).ok();
    let paths = Paths::from_env(&env);
    let opts = pimax_gui_core::paths::doctor_opts(&paths, &env);
    AppCore::new(
        Arc::new(RealMachine::from_env()),
        paths,
        opts,
        Arc::new(|k: &str| std::env::var(k).ok()),
        Store::from_env(&env),
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| args.iter().position(|a| a == f).map(|i| args.get(i + 1).cloned());
    crash::set_build_info(env!("PIMAX_GIT_HASH"), env!("PIMAX_BUILD_DATE"));
    if has("-h") || has("--help") {
        println!("{USAGE}");
        return;
    }
    if has("--version") || has("-V") {
        println!("pimax-setup {}", crash::version_line());
        return;
    }
    let known = [
        "--demo", "--renderer", "--no-supervisor", "--window", "--self-test", "--report", "--screenshots", "--real",
    ];
    let mut skip = false;
    for a in &args {
        if skip {
            skip = false;
            continue;
        }
        if a == "--renderer" || a == "--screenshots" {
            skip = true;
        }
        if !known.contains(&a.as_str()) {
            eprintln!("unknown argument {a}\n{USAGE}");
            std::process::exit(2);
        }
    }
    if let Some(dir) = value("--screenshots") {
        let Some(dir) = dir else {
            eprintln!("--screenshots needs a directory\n{USAGE}");
            std::process::exit(2);
        };
        let dir = std::path::Path::new(&dir);
        let r = if has("--real") { pimax_setup::shots::render_core(dir, sync_core()) } else { pimax_setup::shots::render_all(dir) };
        match r {
            Ok(files) => {
                for f in files {
                    println!("{}", f.display());
                }
                return;
            }
            Err(e) => {
                eprintln!("pimax-setup: screenshots failed: {e}");
                std::process::exit(1);
            }
        }
    }
    if has("--self-test") {
        let core = sync_core();
        let (ok, warn, fail) = pimax_gui_core::checklist::counts(&core.items);
        let gate = match core.preflight().gate {
            Gate::Ready => "ready".to_string(),
            Gate::NeedsConsent(r) => format!("needs consent ({})", r.join("; ")),
            Gate::Blocked(r) => format!("blocked ({})", r.join("; ")),
        };
        println!(
            "self-test ok: {} setup items ({ok} ok, {warn} warn, {fail} fail); desk start {gate}; monado {}; launcher {} (6dof {}, ctrl-track {})",
            core.items.len(),
            core.live.service.as_ref().map(|s| format!("running pid {}", s.pid)).unwrap_or_else(|| "stopped".into()),
            core.launcher_bin().display(),
            core.facts.caps.sixdof,
            core.facts.caps.ctrl_track,
        );
        for i in core.items.iter().filter(|i| i.bad()) {
            println!("  {:?} {}: {}: {}", i.level, i.id, i.title, i.detail);
        }
        return;
    }
    if has("--report") {
        crash::set_system(crash::collect_system(&|k| std::env::var(k).ok(), &[]));
        let mut core = sync_core();
        core.report_problem();
        match core.problem.result.take() {
            Some(Ok(p)) => println!("{}", p.display()),
            Some(Err(e)) => {
                eprintln!("pimax-setup: report failed: {e}");
                std::process::exit(1);
            }
            None => {
                eprintln!("pimax-setup: no report was written");
                std::process::exit(1);
            }
        }
        return;
    }
    let renderer = match value("--renderer") {
        Some(Some(r)) => match Renderer::parse(&r) {
            Some(r) => Some(r),
            None => {
                eprintln!("--renderer wants wgpu or glow, not {r}");
                std::process::exit(2);
            }
        },
        Some(None) => {
            eprintln!("--renderer needs a value\n{USAGE}");
            std::process::exit(2);
        }
        None => None,
    };
    let demo = has("--demo");
    if has("--window") {
        // the window process of the supervisor
        let r = renderer.or_else(|| std::env::var("PIMAX_RENDERER").ok().as_deref().and_then(Renderer::parse)).unwrap_or_else(Renderer::default_for_build);
        std::process::exit(window::run(window::Options { renderer: r, demo, supervised: std::env::var("PIMAX_SUPERVISED").is_ok_and(|v| v == "1") }));
    }
    if has("--no-supervisor") || std::env::var("PIMAX_NO_SUPERVISOR").is_ok_and(|v| v == "1") {
        let env = |k: &str| std::env::var(k).ok();
        let _lock = match instance::acquire(&instance::lock_path(&env)) {
            Ok(l) => Some(l),
            Err(Some(pid)) => {
                println!("Pimax is already running (pid {pid}).");
                return;
            }
            Err(None) => None,
        };
        let r = renderer.or_else(|| std::env::var("PIMAX_RENDERER").ok().as_deref().and_then(Renderer::parse)).unwrap_or_else(Renderer::default_for_build);
        std::process::exit(window::run(window::Options { renderer: r, demo, supervised: false }));
    }
    std::process::exit(pimax_setup::supervisor::run(demo, renderer));
}
